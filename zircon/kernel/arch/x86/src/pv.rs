// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! Paravirtual functions, to execute some functions in a Hypervisor-specific way.
//! The paravirtual optimizations in this module are implemented by kvm/qemu.

use super::feature::{
    X86_FEATURE_KVM_PV_CLOCK_STABLE, X86VendorList, x86_feature_test, x86_get_vendor,
};
use super::platform_access::{MsrAccess, RealMsrAccess};
use super::registers::{X86_MSR_KVM_PV_EOI_EN, X86_MSR_KVM_PV_EOI_EN_ENABLE};
use crate::arch_rs as arch;
use crate::vm::{page_state, physmap, pmm, vm};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use kprint::kprintln;
use lazy_init::LazyInit;
use zx_status::Status;
use zx_types::zx_status_t;

const KVM_SYSTEM_TIME_MSR_OLD: u32 = 0x12;
const KVM_SYSTEM_TIME_MSR: u32 = 0x4b56_4d01;

const KVM_BOOT_TIME_OLD: u32 = 0x11;
const KVM_BOOT_TIME: u32 = 0x4b56_4d00;

const KVM_FEATURE_CLOCK_SOURCE_OLD: u32 = 1 << 0;
const KVM_FEATURE_CLOCK_SOURCE: u32 = 1 << 3;

const KVM_SYSTEM_TIME_STABLE: u8 = 1 << 0;

const SYSTEM_TIME_ENABLE: u64 = 1;

const PV_IPI_NUM: u32 = 10;

const SMP_MAX_CPUS: usize = zr::parse_usize(env!("SMP_MAX_CPUS")).expect("SMP_MAX_CPUS invalid");

// ABI used by Xen and KVM (https://www.kernel.org/doc/Documentation/virtual/kvm/msr.txt).

/// Structure for KVM paravirtual boot time.
///
/// With multiple VCPUs it is possible that one VCPU can try to read boot time
/// while we are updating it because another VCPU asked for the update. In this
/// case odd version value serves as an indicator for the guest that update is
/// in progress. Therefore we need to update version before we write anything
/// else and after, also we need to use proper memory barriers. The same logic
/// applies to system time version below, even though system time is per VCPU
/// others VCPUs still can access system times of other VCPUs (Linux however
/// never does that).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct pv_clock_boot_time {
    pub version: u32,
    pub seconds: u32,
    pub nseconds: u32,
}

zr::static_assert!(core::mem::size_of::<pv_clock_boot_time>() == 12);
zr::static_assert!(core::mem::align_of::<pv_clock_boot_time>() == 4);

/// Structure for KVM paravirtual system time.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct pv_clock_system_time {
    pub version: u32,
    pub pad0: u32,
    pub tsc_timestamp: u64,
    pub system_time: u64,
    pub tsc_mul: u32,
    pub tsc_shift: i8,
    pub flags: u8,
    pub pad1: [u8; 2],
}

zr::static_assert!(core::mem::size_of::<pv_clock_system_time>() == 32);
zr::static_assert!(core::mem::align_of::<pv_clock_system_time>() == 8);

#[derive(Copy, Clone, Debug)]
#[repr(transparent)]
struct PvClock<T>(ptr::NonNull<T>);

// SAFETY: PvClock represents a pointer to a mapped, hypervisor-accessible page.
unsafe impl<T: Send> Send for PvClock<T> {}
unsafe impl<T: Sync> Sync for PvClock<T> {}

impl<T> PvClock<T> {
    fn new(ptr: ptr::NonNull<T>) -> Self {
        Self(ptr)
    }

    fn as_ptr(&self) -> *mut T {
        self.0.as_ptr()
    }
}

static BOOT_TIME: LazyInit<PvClock<pv_clock_boot_time>> = LazyInit::uninit();
static SYSTEM_TIME: LazyInit<PvClock<pv_clock_system_time>> = LazyInit::uninit();

static PV_EOI: [PvEoi; SMP_MAX_CPUS] = [const { PvEoi::new() }; SMP_MAX_CPUS];

/// `PvEoi::state` must be contained within a single page. If its alignment is greater than or equal
/// to its size, then we know it's not straddling a page boundary.
type StateType = AtomicU64;
zr::static_assert!(core::mem::align_of::<StateType>() >= core::mem::size_of::<StateType>());

/// PvEoi provides optimized end-of-interrupt signaling for para-virtualized environments.
///
/// The initialization sequence of PvEoi instances is tricky. All PvEoi instances should be
/// initialized by the boot CPU prior to bringing the secondary CPUs online (see [`PvEoi::init_all`]).
#[derive(Debug)]
pub struct PvEoi {
    state: StateType,

    /// The physical address of `state`.
    state_paddr: AtomicUsize,

    enabled: AtomicBool,
}

impl Default for PvEoi {
    fn default() -> Self {
        Self::new()
    }
}

impl PvEoi {
    pub const fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
            state_paddr: AtomicUsize::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    /// Initialize all PvEoi instances.
    ///
    /// Must be called from a context in which blocking is allowed.
    pub fn init_all() {
        for pv in &PV_EOI {
            pv.init();
        }
    }

    /// Initialize this PvEoi instance.
    ///
    /// Must be called from a context in which blocking is allowed.
    pub fn init(&self) {
        // SAFETY: FFI to check if blocking is disallowed.
        debug_assert!(!arch::blocking_disallowed());
        debug_assert!(!self.enabled.load(Ordering::Relaxed));
        debug_assert!(self.state_paddr.load(Ordering::Relaxed) == 0);

        let va = (&self.state as *const AtomicU64).cast::<core::ffi::c_void>();
        let paddr = vm::vaddr_to_paddr(va).0;
        debug_assert!(paddr != 0);
        debug_assert!(paddr.is_multiple_of(core::mem::align_of::<AtomicU64>()));

        self.state_paddr.store(paddr, Ordering::Relaxed);
    }

    /// Get the current CPU's PvEoi instance.
    pub fn get() -> &'static Self {
        let cpu = arch::curr_cpu_num() as usize;
        debug_assert!(cpu < SMP_MAX_CPUS);
        &PV_EOI[cpu]
    }

    /// Enable PV_EOI for the current CPU. After it is enabled, callers may use `eoi()` rather than
    /// access a local APIC register if desired.
    ///
    /// Once enabled this PvEoi object must be disabled prior to destruction.
    ///
    /// It is an error to enable a PvEoi object more than once over its lifetime.
    pub fn enable<M: MsrAccess + ?Sized>(&self, msr: &mut M) {
        // It is critical that this method does not block as it may be called early during boot, prior to
        // the calling CPU being marked active.
        debug_assert!(!self.enabled.load(Ordering::Relaxed));
        let paddr = self.state_paddr.load(Ordering::Relaxed);
        debug_assert!(paddr != 0);

        msr.write_msr(X86_MSR_KVM_PV_EOI_EN, (paddr as u64) | X86_MSR_KVM_PV_EOI_EN_ENABLE);
        self.enabled.store(true, Ordering::Release);
    }

    /// Disable PV_EOI for the current CPU.
    pub fn disable<M: MsrAccess + ?Sized>(&self, msr: &mut M) {
        // It is critical that this method does not block as it may be called when the current CPU is
        // being shutdown.

        // Mark as disabled before writing to the MSR; otherwise an interrupt appearing in the window
        // between the two could fail to EOI via the legacy mechanism.
        self.enabled.store(false, Ordering::Release);
        msr.write_msr(X86_MSR_KVM_PV_EOI_EN, 0);
    }

    /// Attempt to acknowledge and signal an end-of-interrupt (EOI) for the current CPU via a
    /// paravirtual interface. If a fast acknowledge was not available, the function returns
    /// false and the caller must signal an EOI via the legacy mechanism.
    pub fn eoi(&self) -> bool {
        if !self.enabled.load(Ordering::Relaxed) {
            return false;
        }

        let old_val = self.state.swap(0, Ordering::Relaxed);
        old_val != 0
    }

    /// Returns true if PV_EOI is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// For testing purposes only: sets the physical address directly.
    #[cfg(ktest)]
    pub fn set_state_paddr_for_test(&self, paddr: usize) {
        self.state_paddr.store(paddr, Ordering::Relaxed);
    }

    /// For testing purposes only: gets direct reference to state atomic.
    #[cfg(ktest)]
    pub fn state_for_test(&self) -> &AtomicU64 {
        &self.state
    }
}

impl Drop for PvEoi {
    fn drop(&mut self) {
        debug_assert!(!self.is_enabled());
    }
}

/// Initialize the para-virtualized clock.
///
/// This function should only be called by CPU 0.
pub fn pv_clock_init() -> Result<(), Status> {
    let (page, pa) = pmm::alloc_page(pmm::ALLOC_FLAG_ANY)?;
    let va = physmap::paddr_to_physmap(pa);

    unsafe { page.set_state(page_state::VmPageState(page_state::bindings::vm_page_state::WIRED)) };

    // SAFETY: Page has just been allocated and has no references.
    unsafe {
        arch::ops::zero_page(va);
    }

    // SAFETY: The zero address is not in the physmap.
    let boot_time_ptr =
        unsafe { ptr::NonNull::new_unchecked(ptr::with_exposed_provenance_mut(va.0)) };

    // SAFETY: Called once during boot, serialized with respect to any other access.
    unsafe { BOOT_TIME.init(PvClock::new(boot_time_ptr)) };
    // SAFETY: Writing pa to KVM boot time MSR informs the hypervisor of the boot time page.
    unsafe { super::x86::write_msr(KVM_BOOT_TIME, pa.0 as u64) };

    let (page, pa) = pmm::alloc_page(pmm::ALLOC_FLAG_ANY)?;
    let va = physmap::paddr_to_physmap(pa);

    unsafe { page.set_state(page_state::VmPageState(page_state::bindings::vm_page_state::WIRED)) };

    // SAFETY: Page has just been allocated and has no references.
    unsafe {
        arch::ops::zero_page(va);
    }

    // SAFETY: The zero address is not in the physmap.
    let system_time_ptr =
        unsafe { ptr::NonNull::new_unchecked(ptr::with_exposed_provenance_mut(va.0)) };

    // SAFETY: Called once during boot, serialized with respect to any other access.
    unsafe { SYSTEM_TIME.init(PvClock::new(system_time_ptr)) };

    // Note: We're setting up one, system-wide PV clock rather than per-CPU system
    // clocks. This is OK because
    //   - the PV clock is only used if it's stable
    //   - we assume invariant TSC if the clock is stable
    //   - we don't read from the clock's tsc_timestamp; we use rdtsc directly
    // SAFETY: Writing pa | SYSTEM_TIME_ENABLE enables the PV system time clock.
    unsafe { super::x86::write_msr(KVM_SYSTEM_TIME_MSR, (pa.0 as u64) | SYSTEM_TIME_ENABLE) };

    Ok(())
}

/// Shuts down the para-virtualized clock.
///
/// This function should only be called by CPU 0.
#[unsafe(no_mangle)]
pub extern "C" fn pv_clock_shutdown() {
    debug_assert!(arch::curr_cpu_num() == 0);

    // Tell our hypervisor to stop updating the clock.
    // SAFETY: Writing 0 to KVM system time MSR stops the hypervisor clock update.
    unsafe { super::x86::write_msr(KVM_SYSTEM_TIME_MSR, 0) };
}

/// Checks if the para-virtualized clock is stable.
#[unsafe(no_mangle)]
pub extern "C" fn pv_clock_is_stable() -> bool {
    let system_time_ptr = SYSTEM_TIME.as_ptr();

    // SAFETY: system_time_ptr points to the valid hypervisor-mapped page.
    let flags = unsafe { ptr::read_volatile(ptr::addr_of!((*system_time_ptr).flags)) };

    let is_stable =
        (flags & KVM_SYSTEM_TIME_STABLE) != 0 || x86_feature_test(X86_FEATURE_KVM_PV_CLOCK_STABLE);

    if is_stable {
        kprintln!("pv_clock: Clocksource is stable");
    } else {
        kprintln!("pv_clock: Clocksource is not stable");
    }

    is_stable
}

/// Computes the TSC frequency in Hz from KVM's `tsc_mul` and `tsc_shift` values.
pub fn calculate_tsc_freq(tsc_mul: u32, tsc_shift: i8) -> u64 {
    let mut tsc_khz = (1_000_000u64) << 32;
    tsc_khz /= tsc_mul as u64;
    if tsc_shift > 0 {
        tsc_khz >>= tsc_shift as u32;
    } else {
        tsc_khz <<= (-tsc_shift) as u32;
    }
    tsc_khz * 1000
}

/// Fetches the TSC frequency via the para-virtualized clock interface.
#[unsafe(no_mangle)]
pub extern "C" fn pv_clock_get_tsc_freq() -> u64 {
    kprintln!("pv_clock: Fetching TSC frequency");

    let system_time_ptr = SYSTEM_TIME.as_ptr();

    // SAFETY: system_time_ptr is non-null and valid.
    let version_ptr = unsafe { ptr::addr_of!((*system_time_ptr).version) };
    // SAFETY: system_time_ptr is non-null and valid.
    let tsc_mul_ptr = unsafe { ptr::addr_of!((*system_time_ptr).tsc_mul) };
    // SAFETY: system_time_ptr is non-null and valid.
    let tsc_shift_ptr = unsafe { ptr::addr_of!((*system_time_ptr).tsc_shift) };

    let mut tsc_mul: u32;
    let mut tsc_shift: i8;

    loop {
        // SAFETY: Reading version from volatile system_time page.
        let pre_version = unsafe { ptr::read_volatile(version_ptr) };
        if (pre_version & 1) != 0 {
            core::hint::spin_loop();
            continue;
        }
        // SAFETY: Reading tsc_mul and tsc_shift from volatile system_time page.
        unsafe {
            tsc_mul = ptr::read_volatile(tsc_mul_ptr);
            tsc_shift = ptr::read_volatile(tsc_shift_ptr);
        }
        // SAFETY: Reading version from volatile system_time page.
        let post_version = unsafe { ptr::read_volatile(version_ptr) };
        if pre_version == post_version {
            break;
        }
    }

    calculate_tsc_freq(tsc_mul, tsc_shift)
}

/// Send para-virtualized IPI.
///
/// # Arguments
/// * `mask_low` - Low part of CPU mask.
/// * `mask_high` - High part of CPU mask.
/// * `start_id` - APIC ID that the CPU mask starts at.
/// * `icr` - APIC ICR value.
///
/// Returns the number of CPUs that the IPI was delivered to, or an error value.
#[unsafe(no_mangle)]
pub extern "C" fn pv_ipi(mask_low: u64, mask_high: u64, start_id: u64, icr: u64) -> i32 {
    let vendor = x86_get_vendor();
    match vendor {
        X86VendorList::Intel => {
            let ret: i32;
            // SAFETY: Executes hypervisor hypercall vmcall for Intel CPUs.
            // Note: `rbx` is reserved by LLVM, so we use `xchg` to temporarily load `mask_low` into
            // `rbx` and restore the original `rbx` after the hypercall.
            unsafe {
                core::arch::asm!(
                    "xchg rbx, {mask_low}",
                    "vmcall",
                    "xchg rbx, {mask_low}",
                    mask_low = inout(reg) mask_low => _,
                    inout("eax") PV_IPI_NUM => ret,
                    in("rcx") mask_high,
                    in("rdx") start_id,
                    in("rsi") icr,
                    options(nostack),
                );
            }
            ret
        }
        X86VendorList::Amd => {
            let ret: i32;
            // SAFETY: Executes hypervisor hypercall vmmcall for AMD CPUs.
            // Note: `rbx` is reserved by LLVM, so we use `xchg` to temporarily load `mask_low` into
            // `rbx` and restore the original `rbx` after the hypercall.
            unsafe {
                core::arch::asm!(
                    "xchg rbx, {mask_low}",
                    "vmmcall",
                    "xchg rbx, {mask_low}",
                    mask_low = inout(reg) mask_low => _,
                    inout("eax") PV_IPI_NUM => ret,
                    in("rcx") mask_high,
                    in("rdx") start_id,
                    in("rsi") icr,
                    options(nostack),
                );
            }
            ret
        }
        _ => panic!("PANIC_UNIMPLEMENTED: unsupported x86 vendor for pv_ipi"),
    }
}

// FFI exports to C++

#[unsafe(no_mangle)]
pub extern "C" fn rust_pv_clock_init() -> zx_status_t {
    Status::result_into_raw(pv_clock_init())
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_pveoi_init_all() {
    PvEoi::init_all();
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_pveoi_enable_real_msr() {
    let mut msr_access = RealMsrAccess {};
    PvEoi::get().enable(&mut msr_access);
}

#[unsafe(no_mangle)]
pub extern "C" fn rust_pveoi_eoi() -> bool {
    PvEoi::get().eoi()
}

/// PV EOI tests.
#[cfg(ktest)]
#[unittest::suite(name = "pv_tests")]
#[allow(unused_imports)]
mod pv_tests {
    use crate::arch_rs::x86::fake_msr_access::{FakeMsr, FakeMsrAccess};

    /// PvEoi
    #[test]
    fn test_pveoi() {
        // Can't signal if not enabled
        {
            let pv = PvEoi::new();
            pv.set_state_paddr_for_test(0x1000);
            assert!(!pv.eoi());
        }

        // Enable, signal, then disable
        {
            let mut msr = FakeMsrAccess {
                msrs: [
                    FakeMsr { index: X86_MSR_KVM_PV_EOI_EN, value: 0 },
                    FakeMsr::default(),
                    FakeMsr::default(),
                    FakeMsr::default(),
                ],
                no_writes: false,
            };

            let pv = PvEoi::new();
            pv.set_state_paddr_for_test(0x2000);
            pv.enable(&mut msr);

            assert!(pv.is_enabled());
            assert_eq!(msr.msrs[0].value, 0x2000 | X86_MSR_KVM_PV_EOI_EN_ENABLE);

            // Fast acknowledge not pending
            assert!(!pv.eoi());

            // Set state to 1 (pending)
            pv.state_for_test().store(1, Ordering::Relaxed);
            assert!(pv.eoi());
            assert_eq!(pv.state_for_test().load(Ordering::Relaxed), 0);

            pv.disable(&mut msr);
            assert!(!pv.is_enabled());
            assert_eq!(msr.msrs[0].value, 0);
        }
    }
}
