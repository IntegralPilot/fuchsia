// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::platform_pc::acpi::global_acpi_lite_parser;
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ARCH_MMU_FLAG_UNCACHED_DEVICE,
};
use core::pin::Pin;
use debug::dprintf;
use ksync::{KMutex, RawSpinlock, guarded, lock};
use lazy_init::LazyInit;
use pin_init::pin_init;
use zx_status::Status;

#[cfg(console_enabled)]
use crate::console_rust::console::{CMD_AVAIL_ALWAYS, CmdArgs, static_command};

/// HPET Timer Register structure
#[repr(C)]
pub struct HpetTimerRegisters {
    pub conf_caps: u64,
    pub comparator_value: u64,
    pub fsb_int_route: u64,
    pub _reserved: [u8; 8],
}

/// HPET Registers structure
#[repr(C)]
pub struct HpetRegisters {
    pub general_caps: u64,
    pub _reserved0: [u8; 8],
    pub general_config: u64,
    pub _reserved1: [u8; 8],
    pub general_int_status: u64,
    pub _reserved2: [u8; 0xf0 - 0x28],
    pub main_counter_value: u64,
    pub _reserved3: [u8; 8],
}

impl HpetRegisters {
    /// Gets a mutable pointer to the `i`-th timer registers.
    ///
    /// # Safety
    /// The caller must ensure that `regs` is a valid pointer to `HpetRegisters`.
    pub fn timer_mut(regs: *mut HpetRegisters, i: usize) -> *mut HpetTimerRegisters {
        // SAFETY: The caller must ensure that `regs` is a valid pointer to HpetRegisters.
        unsafe {
            // Timers start directly after the HpetRegisters.
            let timers_base = regs.add(1).cast::<HpetTimerRegisters>();
            // Then index by the requested timer.
            timers_base.add(i)
        }
    }
}

// Static assertions to verify memory layout parity.
zr::static_assert!(core::mem::size_of::<HpetTimerRegisters>() == 32);
zr::static_assert!(core::mem::align_of::<HpetTimerRegisters>() == 8);
zr::static_assert!(core::mem::size_of::<HpetRegisters>() == 256);
zr::static_assert!(core::mem::align_of::<HpetRegisters>() == 8);

/// Driver state for the High Precision Event Timer.
#[guarded]
struct HpetState {
    present: bool,
    registers: *mut HpetRegisters,
    ticks_per_ms: u64,
    num_timers: u8,
    #[mutex]
    mu: KMutex<RawSpinlock>,
}

// SAFETY: HpetState contains a raw pointer to registers, but it is only initialized
// during boot and read-only afterwards, and register access is synchronized via `mu`.
unsafe impl Sync for HpetState {}

static HPET_STATE: LazyInit<HpetState> = LazyInit::uninit();

const MAX_PERIOD_IN_FS: u64 = 0x05F5E100;
/// Bit masks for the general_config register
const GEN_CONF_EN: u64 = 1;
/// Bit masks for the per-time conf_caps register
const TIMER_CONF_INT_EN: u64 = 1 << 2;

// FFI Declarations
unsafe extern "C" {
    fn cpp_hpet_set_ticks_to_clock_monotonic(n: u32, d: u32);
}

struct Hpet {
    registers: *mut HpetRegisters,
    ticks_per_ms: u64,
    num_timers: u8,
}

fn probe_hpet() -> Option<Hpet> {
    // Look up the HPET table.
    let hpet_desc = acpi_lite::get_table_by_type::<acpi_lite::structures::AcpiHpetTable>(
        global_acpi_lite_parser(),
    )?;

    let hpet_address = hpet_desc.address.address;

    // Ensure the HPET table uses MMIO.
    if hpet_desc.address.address_space_id != acpi_lite::structures::ACPI_ADDR_SPACE_MEMORY {
        dprintf!(INFO, "HPET unsupported: require MMIO-based HPET.\n");
        return None;
    }

    let mut regs_addr: *mut HpetRegisters = core::ptr::null_mut();
    // SAFETY: FFI call with correct pointers is safe.
    let res = unsafe {
        crate::vm::vm_aspace::VmAspace::kernel_aspace().alloc_physical(
            c"hpet",
            page::SIZE,
            &mut regs_addr as *mut *mut HpetRegisters as *mut *mut core::ffi::c_void,
            page::SHIFT as u8,
            hpet_address.into(),
            0,
            ARCH_MMU_FLAG_UNCACHED_DEVICE | ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
        )
    };
    if res.is_err() {
        return None;
    }

    let cleanup_and_fail = || {
        // SAFETY: Free the allocated region on failure.
        unsafe {
            let _ = crate::vm::vm_aspace::VmAspace::kernel_aspace().free_region(regs_addr as usize);
        }
        None
    };

    // SAFETY: regs_addr is validly mapped MMIO.
    let general_caps =
        unsafe { core::ptr::read_volatile(core::ptr::addr_of!((*regs_addr).general_caps)) };
    let has_64bit_count = (general_caps & (1 << 13)) != 0;
    let tick_period_in_fs = general_caps >> 32;

    if tick_period_in_fs == 0 || tick_period_in_fs > MAX_PERIOD_IN_FS {
        return cleanup_and_fail();
    }

    // We only support HPETs that are 64-bit and have at least two timers.
    let num_timers_val = (((general_caps >> 8) & 0x1f) + 1) as u8;
    if !has_64bit_count || num_timers_val < 2 {
        return cleanup_and_fail();
    }

    // Make sure all timers have interrupts disabled.
    for i in 0..num_timers_val {
        // SAFETY: i is verified to be within [0, num_timers_val).
        unsafe {
            let timer_regs = HpetRegisters::timer_mut(regs_addr, i as usize);
            let conf_caps = core::ptr::read_volatile(core::ptr::addr_of!((*timer_regs).conf_caps));
            core::ptr::write_volatile(
                core::ptr::addr_of_mut!((*timer_regs).conf_caps),
                conf_caps & !TIMER_CONF_INT_EN,
            );
        }
    }

    // Figure out the nominal ratio of clock monotonic ticks (nsec) to HPET ticks.
    // This is the scaling factor when converting from HPET to clock monotonic.
    // Unfortunately, the HPET's rate is reported by the registers as a nominal
    // period (in femtosecond) instead of a nominal frequency (in Hz, or even
    // mHz).
    //
    // In the real world, the HPET is most likely running at the bus-issue rate
    // for the motherboard (24MHz, 100MHz, etc) or the CPU issue rate (2.4GHz, 4.0
    // GHZ, etc), meaning that the nominal period reported is off by some fraction
    // of a femtosecond because the nominal frequency of the counter does not
    // perfectly divide the value 10^15.
    //
    // For example, when the actual nominal HPET rate is 24MHz, the value which
    // will be reported by the register is 41,666,667 instead of the more precise
    // 41,666,666 + 2/3 (the actual nominal ratio).
    //
    // So: when computing the HPET -> clock monotonic ticks ratio, assume that the
    // underlying period actually comes from a clock expressed as an integer
    // number of Hz, and try to reconstruct that frequency from the reported
    // period by dividing and rounding up instead of rounding down.
    const VAL10E15: u64 = 1_000_000_000_000_000;
    let hpet_nominal_frequency = VAL10E15.div_ceil(tick_period_in_fs);

    let mut n: u64 = 1_000_000_000;
    let mut d: u64 = hpet_nominal_frequency;
    affine::Ratio::reduce_u64(&mut n, &mut d);

    // If ratio between HPET's rate and clock monotonic's rate cannot be stored
    // in a 32 bit integer, then we cannot use HPET as our reference timer.
    if n > u32::MAX as u64 || d > u32::MAX as u64 {
        dprintf!(
            INFO,
            "HPET to clock monotonic rate ratio ({}/{}) cannot be stored as a 32 bit ratio! Ignoring HPET\n",
            n,
            d,
        );
        return cleanup_and_fail();
    }

    // SAFETY: FFI call to set global ratio in timer.cc.
    unsafe {
        cpp_hpet_set_ticks_to_clock_monotonic(n as u32, d as u32);
    }

    let ticks_per_ms = hpet_nominal_frequency / 1000;

    dprintf!(
        INFO,
        "HPET: detected at {:#x} ticks per ms {} num timers {}\n",
        hpet_address,
        ticks_per_ms,
        num_timers_val,
    );

    Some(Hpet { registers: regs_addr, ticks_per_ms, num_timers: num_timers_val })
}

/// Initializes the HPET driver by locating the ACPI table, mapping the MMIO registers,
/// disabling interrupts on all HPET timers, and setting up the ticks-to-monotonic ratio.
///
/// # Safety
/// This function must be called exactly once during early boot (at LK_INIT).
fn platform_hpet_init(_level: init::LkInitLevel) {
    let (present, registers, ticks_per_ms, num_timers) = match probe_hpet() {
        Some(hpet) => (true, hpet.registers, hpet.ticks_per_ms, hpet.num_timers),
        None => (false, core::ptr::null_mut(), 0, 0),
    };

    // SAFETY: Called once during boot, serialized with respect to any other access.
    let _ = unsafe {
        Pin::static_ref(&HPET_STATE).init_pin(pin_init!(HpetState {
            present,
            registers,
            ticks_per_ms,
            num_timers,
            mu <- KMutex::init(),
        }))
    };
}

/// Returns the current main counter value of the HPET.
/// If HPET is not present, returns 0.
///
/// # Safety
/// This is safe to call from any context after driver initialization. It performs volatile
/// MMIO reads from mapped HPET registers.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_get_value() -> u64 {
    let state = &*HPET_STATE;
    if !state.present {
        return 0;
    }
    let regs = state.registers;
    if regs.is_null() {
        return 0;
    }
    // SAFETY: regs is valid if not null, and we perform volatile reads.
    unsafe {
        let v = core::ptr::read_volatile(core::ptr::addr_of!((*regs).main_counter_value));
        let v2 = core::ptr::read_volatile(core::ptr::addr_of!((*regs).main_counter_value));
        // Even though the specification says it should not be necessary to read
        // multiple times, we have observed that QEMU converts the 64-bit
        // memory access in to two 32-bit accesses, resulting in bad reads. QEMU
        // reads the low 32-bits first, so the result is a large jump when it
        // wraps 32 bits.  To work around this, we return the lesser of two reads.
        core::cmp::min(v, v2)
    }
}

/// Sets the main counter value of the HPET to `v`.
/// Returns `Status::BAD_STATE` if the HPET is enabled or not present.
///
/// # Safety
/// Safe to call after driver initialization. Acquires the global HPET lock and performs
/// volatile MMIO writes.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_set_value(v: u64) -> zx_types::zx_status_t {
    let state = &*HPET_STATE;
    if !state.present {
        return Status::BAD_STATE.into_raw();
    }
    let regs = state.registers;
    if regs.is_null() {
        return Status::BAD_STATE.into_raw();
    }

    lock!(state.lock_mu_policy::<ksync::NoIrqSavePolicy>());

    // SAFETY: regs is valid and we hold the lock.
    unsafe {
        let general_config = core::ptr::read_volatile(core::ptr::addr_of!((*regs).general_config));
        if (general_config & GEN_CONF_EN) != 0 {
            return Status::BAD_STATE.into_raw();
        }
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*regs).main_counter_value), v);
    }
    zx_status::sys::ZX_OK
}

/// Returns true if the HPET is present and initialized.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_is_present() -> bool {
    HPET_STATE.present
}

/// Enables the HPET main counter.
///
/// # Safety
/// Safe to call after driver initialization. Acquires the global HPET lock and performs
/// volatile MMIO writes.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_enable() {
    debug_assert!(hpet_is_present());
    let state = &*HPET_STATE;
    let regs = state.registers;
    if regs.is_null() {
        return;
    }
    lock!(state.lock_mu_policy::<ksync::NoIrqSavePolicy>());
    // SAFETY: regs is valid and we hold the lock.
    unsafe {
        let general_config = core::ptr::read_volatile(core::ptr::addr_of!((*regs).general_config));
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!((*regs).general_config),
            general_config | GEN_CONF_EN,
        );
    }
}

/// Disables the HPET main counter.
///
/// # Safety
/// Safe to call after driver initialization. Acquires the global HPET lock and performs
/// volatile MMIO writes.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_disable() {
    debug_assert!(hpet_is_present());
    let state = &*HPET_STATE;
    let regs = state.registers;
    if regs.is_null() {
        return;
    }
    lock!(state.lock_mu_policy::<ksync::NoIrqSavePolicy>());
    // SAFETY: regs is valid and we hold the lock.
    unsafe {
        let general_config = core::ptr::read_volatile(core::ptr::addr_of!((*regs).general_config));
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!((*regs).general_config),
            general_config & !GEN_CONF_EN,
        );
    }
}

/// Busy-waits for approximately `ms` milliseconds using the HPET main counter.
///
/// # Safety
/// Safe to call after driver initialization. Performs volatile MMIO reads.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_wait_ms(ms: u16) {
    let state = &*HPET_STATE;
    let regs = state.registers;
    if regs.is_null() {
        return;
    }
    // SAFETY: regs is valid.
    unsafe {
        let init_timer_value =
            core::ptr::read_volatile(core::ptr::addr_of!((*regs).main_counter_value));
        let target = (ms as u64) * state.ticks_per_ms;
        while core::ptr::read_volatile(core::ptr::addr_of!((*regs).main_counter_value))
            .wrapping_sub(init_timer_value)
            <= target
        {
            core::hint::spin_loop();
        }
    }
}

/// Returns the nominal frequency of the HPET in ticks per millisecond.
#[unsafe(no_mangle)]
pub extern "C" fn hpet_ticks_per_ms() -> u64 {
    HPET_STATE.ticks_per_ms
}

#[cfg(console_enabled)]
fn cmd_show_hpet_regs() -> i32 {
    if !hpet_is_present() {
        dprintf!(ALWAYS, "HPET is not present.\n");
        return -1;
    }
    let state = &*HPET_STATE;
    let regs = state.registers;
    if regs.is_null() {
        dprintf!(ALWAYS, "HPET registers are NULL.\n");
        return -1;
    }

    let dump = |reg_val: u64, high_bit: u32, low_bit: u32, name: &str| {
        let mask = (1u64 << (high_bit - low_bit + 1)) - 1;
        let val = (reg_val >> low_bit) & mask;
        dprintf!(ALWAYS, "{:>16} : {:#x} ({})\n", name, val, val);
    };

    // SAFETY: regs is validly mapped MMIO.
    unsafe {
        dprintf!(ALWAYS, "HPET registers are mapped at {:?}\n", regs);
        let general_caps = core::ptr::read_volatile(core::ptr::addr_of!((*regs).general_caps));
        dump(general_caps, 63, 0, "CAPS (all)");
        dump(general_caps, 63, 32, "CLK_PERIOD");
        dump(general_caps, 31, 16, "VENDOR_ID");
        dump(general_caps, 15, 15, "LEG_RT_CAP");
        dump(general_caps, 13, 13, "COUNT_SIZE_CAP");
        dump(general_caps, 12, 8, "NUM_TIM_CAP");
        dump(general_caps, 7, 0, "REV_ID");
        dprintf!(ALWAYS, "\n");

        let general_config = core::ptr::read_volatile(core::ptr::addr_of!((*regs).general_config));
        dump(general_config, 63, 0, "CONFIG (all)");
        dump(general_config, 1, 1, "LEG_RT_CNF");
        dump(general_config, 0, 0, "ENABLE_CNF");
        dprintf!(ALWAYS, "\n");

        let general_int_status =
            core::ptr::read_volatile(core::ptr::addr_of!((*regs).general_int_status));
        dump(general_int_status, 63, 0, "INT_STS (all)");
        dprintf!(ALWAYS, "\n");

        let main_counter_value =
            core::ptr::read_volatile(core::ptr::addr_of!((*regs).main_counter_value));
        dump(main_counter_value, 63, 0, "COUNT");
    }

    0
}

#[cfg(console_enabled)]
unsafe extern "C" fn cmd_hpet(argc: i32, argv: *const CmdArgs, _flags: u32) -> i32 {
    // SAFETY: The console framework guarantees `argv` is valid and contains `argc` elements.
    let args = unsafe { core::slice::from_raw_parts(argv, argc as usize) };

    let usage = || -> i32 {
        // SAFETY: The console framework guarantees `args[0].arg_str` is a valid null-terminated C string.
        let arg0 = unsafe { core::ffi::CStr::from_ptr(args[0].arg_str) }
            .to_str()
            .unwrap_or("<invalid UTF-8>");
        dprintf!(ALWAYS, "Usage:\n");
        dprintf!(ALWAYS, "{} regs : show the HPET registers\n", arg0);
        -1
    };

    if argc < 2 {
        return usage();
    }

    // SAFETY: The console framework guarantees `args[1].arg_str` is a valid null-terminated C string.
    let cmd = unsafe { core::ffi::CStr::from_ptr(args[1].arg_str) };
    if cmd.to_bytes() == b"regs" {
        cmd_show_hpet_regs()
    } else {
        let arg1 = cmd.to_str().unwrap_or("<invalid UTF-8>");
        dprintf!(ALWAYS, "Unrecognized command \"{}\".\n", arg1);
        usage()
    }
}

#[cfg(console_enabled)]
static_command!(CMD_HPET, c"hpet".as_ptr(), c"HPET commands".as_ptr(), cmd_hpet, CMD_AVAIL_ALWAYS);

init::lk_init_hook!(hpet, platform_hpet_init, init::LkInitLevel(init::LK_INIT_LEVEL_VM.0 + 2));
