// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::apic::{
    ApicInterruptDeliveryMode, ApicInterruptDstMode, GsiRange, IoApicDescriptor, IoApicIsaOverride,
    NUM_ISA_IRQS,
};
use crate::arch_rs::x86::interrupts::{X86_INT_PLATFORM_BASE, X86_INT_PLATFORM_MAX};
#[cfg(console_enabled)]
use crate::console_rust::console::{CMD_AVAIL_NORMAL, CmdArgs, static_command};
use crate::dev_interrupt::{InterruptPolarity, InterruptTriggerMode};
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ARCH_MMU_FLAG_UNCACHED_DEVICE,
};
use crate::vm::vm_aspace::VmAspace;
#[allow(unused_imports)]
use core::ffi::c_int;
use core::fmt::Write;
use core::pin::Pin;
use debug::ltrace::KernelConsoleWriter;
use debug::ltracef;
use fbl::Array;
use kalloc::Box;
use ksync::{KMutex, RawSpinlock, guarded, lock};
use lazy_init::LazyInit;
use zx_status::Status;
use zx_types::zx_status_t;

const LOCAL_TRACE: u32 = 0;

// IO APIC register offsets
const IO_APIC_REG_VER: u8 = 0x01;

// The minimum address space required past the base address
const IO_APIC_WINDOW_SIZE: usize = 0x44;
// The minimum version that supported the EOIR
const IO_APIC_EOIR_MIN_VERSION: u8 = 0x20;

// Reg select offsets in MMIO bank
const IO_APIC_REGSEL: regio::Offset<u32, regio::RwSafe> = regio::Offset::new(0x00);
const IO_APIC_WIN: regio::Offset<u32, regio::RwSafe> = regio::Offset::new(0x10);
const IO_APIC_EOIR: regio::Offset<u32, regio::RwSafe> = regio::Offset::new(0x40);

// Technically this can be larger, but the spec as of the 100-Series doesn't
// guarantee where the additional redirections will be.
pub const IO_APIC_NUM_REDIRECTIONS: usize = 120;

const IO_APIC_RTE_MASKED: u64 = 1 << 16;
const IO_APIC_RTE_MASK: u64 = 0xff;

#[derive(Debug)]
/// Struct for tracking all we need to know about each IO APIC
pub struct IoApic {
    pub desc: IoApicDescriptor,
    /// Virtual address of the base of this IOAPIC's MMIO
    pub vaddr: usize,
    pub version: u8,
    /// The index of the last redirection entry
    pub max_redirection_entry: u8,
    /// Pre-allocated space for suspend/resume bookkeeping
    pub saved_rtes: core::cell::UnsafeCell<[u64; IO_APIC_NUM_REDIRECTIONS]>,
}

// SAFETY: `IoApic` is safe to send and share across threads. The interior mutability of `saved_rtes`
// is only accessed during suspend/resume when interrupts are disabled and there is no concurrent access.
unsafe impl Send for IoApic {}
unsafe impl Sync for IoApic {}

unsafe fn bank_for_ioapic(vaddr: usize) -> regio::MmioBank<u32, regio::RwSafe> {
    // SAFETY: The virtual address `vaddr` points to a valid mapped MMIO window allocated
    // by the kernel address space during initialization.
    let ptr = unsafe { regio::MmioPtr::<u32, regio::RwSafe>::new(vaddr as *mut u32) };
    regio::MmioBank::new(ptr, IO_APIC_WINDOW_SIZE)
}

impl IoApic {
    fn bank(&self) -> regio::MmioBank<u32, regio::RwSafe> {
        // SAFETY: The virtual address `vaddr` points to a valid mapped MMIO window allocated
        // by the kernel address space during initialization.
        unsafe { bank_for_ioapic(self.vaddr) }
    }
}

#[guarded]
pub struct IoApicState {
    /// Track all IO APICs in the system
    pub io_apics: fbl::Array<IoApic>,
    /// The first 16 global IRQs are identity mapped to the legacy ISA IRQs unless
    /// we are told otherwise.  This tracks the actual mapping.
    pub isa_overrides: [IoApicIsaOverride; NUM_ISA_IRQS],

    #[mutex]
    lock: KMutex<RawSpinlock>,
}

// SAFETY: `IoApicState` is safe to send and share across threads because the internal `io_apics`
// and `isa_overrides` fields are constant after initialization (accessed concurrently without a lock),
// and MMIO register accesses are protected by the global spinlock `lock`.
unsafe impl Send for IoApicState {}
unsafe impl Sync for IoApicState {}

static IO_APIC_STATE: LazyInit<IoApicState> = LazyInit::uninit();

unsafe extern "C" {
    fn root_resource_filter_add_deny_region(paddr: usize, len: usize, kind: u32);
    fn cpp_arch_ints_disabled() -> bool;
}

fn io_apic_reg_rte(idx: u8) -> u8 {
    0x10 + 2 * idx
}

fn io_apic_ver_max_redir_entry(v: u32) -> u8 {
    ((v >> 16) & 0xff) as u8
}

fn io_apic_ver_version(v: u32) -> u8 {
    (v & 0xff) as u8
}

fn io_apic_rte_dst(v: u8) -> u64 {
    (v as u64) << 56
}

fn io_apic_rte_trigger_mode(tm: InterruptTriggerMode) -> u64 {
    (tm as u64) << 15
}

fn io_apic_rte_polarity(p: InterruptPolarity) -> u64 {
    (p as u64) << 13
}

fn io_apic_rte_dst_mode(dm: ApicInterruptDstMode) -> u64 {
    (dm as u64) << 11
}

fn io_apic_rte_delivery_mode(dm: ApicInterruptDeliveryMode) -> u64 {
    ((dm as u64) & 0x7) << 8
}

fn io_apic_rte_vector(x: u8) -> u64 {
    (x as u64) & 0xff
}

fn io_apic_rte_get_polarity(r: u64) -> InterruptPolarity {
    if ((r >> 13) & 0x1) != 0 { InterruptPolarity::Low } else { InterruptPolarity::High }
}

fn io_apic_rte_get_trigger_mode(r: u64) -> InterruptTriggerMode {
    if ((r >> 15) & 0x1) != 0 { InterruptTriggerMode::Level } else { InterruptTriggerMode::Edge }
}

fn io_apic_rte_get_vector(r: u64) -> u8 {
    (r & 0xFF) as u8
}

fn read_reg(bank: &regio::MmioBank<u32, regio::RwSafe>, reg: u8) -> u32 {
    // SAFETY: Offset IO_APIC_REGSEL and IO_APIC_WIN are within bank bounds and aligned.
    unsafe {
        bank.at(IO_APIC_REGSEL).write(reg as u32);
        bank.at(IO_APIC_WIN).read()
    }
}

fn write_reg(bank: &regio::MmioBank<u32, regio::RwSafe>, reg: u8, val: u32) {
    // SAFETY: Offset IO_APIC_REGSEL and IO_APIC_WIN are within bank bounds and aligned.
    unsafe {
        bank.at(IO_APIC_REGSEL).write(reg as u32);
        bank.at(IO_APIC_WIN).write(val);
    }
}

fn read_redirection_entry(
    bank: &regio::MmioBank<u32, regio::RwSafe>,
    io_apic: &IoApic,
    global_irq: u32,
) -> u64 {
    assert!(global_irq >= io_apic.desc.global_irq_base);
    let offset = global_irq - io_apic.desc.global_irq_base;
    assert!(offset <= io_apic.max_redirection_entry as u32);

    let reg_id = io_apic_reg_rte(offset as u8);
    let mut result: u64 = 0;
    result |= read_reg(bank, reg_id) as u64;
    result |= (read_reg(bank, reg_id + 1) as u64) << 32;
    result
}

fn write_redirection_entry(
    bank: &regio::MmioBank<u32, regio::RwSafe>,
    io_apic: &IoApic,
    global_irq: u32,
    value: u64,
) {
    assert!(global_irq >= io_apic.desc.global_irq_base);
    let offset = global_irq - io_apic.desc.global_irq_base;
    assert!(offset <= io_apic.max_redirection_entry as u32);

    let reg_id = io_apic_reg_rte(offset as u8);
    write_reg(bank, reg_id, value as u32);
    write_reg(bank, reg_id + 1, (value >> 32) as u32);
}

fn resolve_global_irq_no_panic(irq: u32) -> Option<&'static IoApic> {
    for apic in IO_APIC_STATE.io_apics.iter() {
        let start = apic.desc.global_irq_base;
        let end = start + apic.max_redirection_entry as u32;
        if start <= irq && irq <= end {
            return Some(apic);
        }
    }
    None
}

fn resolve_global_irq(irq: u32) -> &'static IoApic {
    if let Some(io_apic) = resolve_global_irq_no_panic(irq) {
        io_apic
    } else {
        // Treat this as fatal, since dealing with an unmapped IRQ is a bug.
        panic!("Could not resolve global IRQ: {}\n", irq);
    }
}

pub fn apic_io_init_safe(io_apic_descs: &[IoApicDescriptor], overrides: &[IoApicIsaOverride]) {
    let mut box_uninit = Box::<[IoApic]>::try_new_uninit_slice(io_apic_descs.len())
        .expect("Failed to allocate io_apics array");

    // Allocate windows to their control pages
    for (i, desc) in io_apic_descs.iter().enumerate() {
        let paddr = desc.paddr;
        let mut vaddr: *mut core::ffi::c_void = core::ptr::null_mut();
        let paddr_page_base = paddr & !page::MASK;

        // An IO APIC cannot cross a page boundary.
        assert!(paddr + IO_APIC_WINDOW_SIZE <= paddr_page_base + page::SIZE);

        // Check if a previous IO APIC shared the same page as this one so we can re-use the mapping.
        let mut found_existing = false;
        for j in 0..i {
            if (unsafe { box_uninit[j].assume_init_ref() }.desc.paddr & !page::MASK)
                == paddr_page_base
            {
                // The vaddr stored in the io_apics is to the MMIO base, round it back down to the
                // page base.
                let existing_page_base =
                    unsafe { box_uninit[j].assume_init_ref() }.vaddr & !page::MASK;
                vaddr = existing_page_base as *mut core::ffi::c_void;
                found_existing = true;
                break;
            }
        }

        // If we did not find a previous mapping, create one.
        if !found_existing {
            // Make sure that user mode cannot ever gain access to these registers using
            // zx_vmo_alloc_physical and the root resource.
            // SAFETY: `paddr_page_base` is the physical base page of the IO APIC MMIO register region,
            // which is valid for physical access.
            unsafe {
                root_resource_filter_add_deny_region(
                    paddr_page_base,
                    page::SIZE,
                    zx_types::ZX_RSRC_KIND_MMIO,
                );
            }

            let kernel_aspace = VmAspace::kernel_aspace();

            // SAFETY: The physical range is valid, aligned, and dedicated to the IO APIC.
            unsafe {
                kernel_aspace
                    .alloc_physical(
                        c"ioapic",
                        page::SIZE,
                        &mut vaddr,
                        page::SHIFT as u8,
                        crate::kernel::types::PAddr(paddr_page_base),
                        0,
                        ARCH_MMU_FLAG_PERM_READ
                            | ARCH_MMU_FLAG_PERM_WRITE
                            | ARCH_MMU_FLAG_UNCACHED_DEVICE,
                    )
                    .expect("Failed to allocate physical memory for ioapic");
            }
        }

        // Offset from the base of the mapped in page to the actual IO APIC base.
        let final_vaddr = (vaddr as usize) + (paddr - paddr_page_base);

        let bank = unsafe { bank_for_ioapic(final_vaddr) };
        let ver = read_reg(&bank, IO_APIC_REG_VER);
        let version = io_apic_ver_version(ver);
        let mut max_redirection_entry = io_apic_ver_max_redir_entry(ver);

        ltracef!(
            "Found an IO APIC at phys {:#x}, virt {:#x}: ver {:08x}\n",
            paddr,
            final_vaddr,
            ver
        );

        if max_redirection_entry > (IO_APIC_NUM_REDIRECTIONS - 1) as u8 {
            ltracef!("IO APIC supports more redirections than kernel: {:08x}\n", ver);
            max_redirection_entry = (IO_APIC_NUM_REDIRECTIONS - 1) as u8;
        }

        box_uninit[i].write(IoApic {
            desc: desc.clone(),
            vaddr: final_vaddr,
            version,
            max_redirection_entry,
            saved_rtes: core::cell::UnsafeCell::new([0; IO_APIC_NUM_REDIRECTIONS]),
        });

        // Cleanout the redirection entries.
        for j in 0..=max_redirection_entry {
            let global_irq = (j as u32) + desc.global_irq_base;
            write_redirection_entry(
                &bank,
                unsafe { box_uninit[i].assume_init_ref() },
                global_irq,
                IO_APIC_RTE_MASKED,
            );
        }
    }

    // SAFETY: All elements in `box_uninit` have been initialized.
    let io_apics_slice = unsafe { box_uninit.assume_init() };
    let mut isa_overrides = [const {
        IoApicIsaOverride {
            isa_irq: 0,
            remapped: false,
            tm: InterruptTriggerMode::Edge,
            pol: InterruptPolarity::High,
            global_irq: 0,
        }
    }; NUM_ISA_IRQS];

    // Process ISA IRQ overrides.
    for o in overrides {
        let isa_irq = o.isa_irq as usize;
        assert!(isa_irq < NUM_ISA_IRQS);
        isa_overrides[isa_irq] = o.clone();
        ltracef!("ISA IRQ override for ISA IRQ {}, mapping to {}\n", isa_irq, o.global_irq);
    }

    let io_apics = Array::from_box(io_apics_slice);

    // SAFETY: Called once during boot, serialized with respect to any other access.
    let _ = unsafe {
        Pin::static_ref(&IO_APIC_STATE).init_pin(pin_init::pin_init!(IoApicState {
            io_apics: io_apics,
            isa_overrides: isa_overrides,
            lock <- KMutex::init(),
        }))
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn apic_io_init(
    io_apic_descs: *const IoApicDescriptor,
    num_io_apic_descs: usize,
    overrides: *const IoApicIsaOverride,
    num_overrides: usize,
) {
    // SAFETY: The caller guarantees `io_apic_descs` points to `num_io_apic_descs` initialized
    // descriptors whenever `num_io_apic_descs > 0`.
    let io_apic_descs = unsafe { zr::slice_from_raw_parts(io_apic_descs, num_io_apic_descs) };
    // SAFETY: The caller guarantees `overrides` points to `num_overrides` initialized entries
    // whenever `num_overrides > 0`.
    let overrides = unsafe { zr::slice_from_raw_parts(overrides, num_overrides) };
    apic_io_init_safe(io_apic_descs, overrides);
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_is_valid_irq(global_irq: u32) -> bool {
    resolve_global_irq_no_panic(global_irq).is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_mask_irq(global_irq: u32, mask: bool) {
    let io_apic = resolve_global_irq(global_irq);
    lock!(let _guard = IO_APIC_STATE.lock_lock());
    let bank = io_apic.bank();
    let mut reg = read_redirection_entry(&bank, io_apic, global_irq);
    if mask {
        reg |= IO_APIC_RTE_MASKED;
    } else {
        // If we are unmasking, we had better have been assigned a valid vector.
        debug_assert!(
            (io_apic_rte_get_vector(reg) >= X86_INT_PLATFORM_BASE.0)
                && (io_apic_rte_get_vector(reg) <= X86_INT_PLATFORM_MAX.0)
        );
        reg &= !IO_APIC_RTE_MASKED;
    }
    write_redirection_entry(&bank, io_apic, global_irq, reg);
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_configure_irq(
    global_irq: u32,
    trig_mode: InterruptTriggerMode,
    polarity: InterruptPolarity,
    del_mode: ApicInterruptDeliveryMode,
    mask: bool,
    dst_mode: ApicInterruptDstMode,
    dst: u8,
    vector: u8,
) {
    let io_apic = resolve_global_irq(global_irq);

    lock!(IO_APIC_STATE.lock_lock());

    let mut mask = mask;

    // If we are configuring an invalid vector, for the IRQ to be masked.
    if (del_mode == ApicInterruptDeliveryMode::Fixed
        || del_mode == ApicInterruptDeliveryMode::LowestPri)
        && !(X86_INT_PLATFORM_BASE.0..=X86_INT_PLATFORM_MAX.0).contains(&vector)
    {
        mask = true;
    }

    let mut reg: u64 = 0;
    reg |= io_apic_rte_trigger_mode(trig_mode);
    reg |= io_apic_rte_polarity(polarity);
    reg |= io_apic_rte_delivery_mode(del_mode);
    reg |= io_apic_rte_dst_mode(dst_mode);
    reg |= io_apic_rte_dst(dst);
    reg |= io_apic_rte_vector(vector);
    if mask {
        reg |= IO_APIC_RTE_MASKED;
    }

    let bank = io_apic.bank();
    write_redirection_entry(&bank, io_apic, global_irq, reg);
}

pub fn apic_io_fetch_irq_config_safe(
    global_irq: u32,
) -> Result<(InterruptTriggerMode, InterruptPolarity), Status> {
    let io_apic = resolve_global_irq_no_panic(global_irq).ok_or(Status::INVALID_ARGS)?;

    lock!(let _guard = IO_APIC_STATE.lock_lock());
    let bank = io_apic.bank();
    let reg = read_redirection_entry(&bank, io_apic, global_irq);

    Ok((io_apic_rte_get_trigger_mode(reg), io_apic_rte_get_polarity(reg)))
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_fetch_irq_config(
    global_irq: u32,
    trig_mode: *mut InterruptTriggerMode,
    polarity: *mut InterruptPolarity,
) -> zx_status_t {
    match apic_io_fetch_irq_config_safe(global_irq) {
        Ok((trig, pol)) => {
            if !trig_mode.is_null() {
                // SAFETY: `trig_mode` is verified to be non-null and points to valid memory.
                unsafe {
                    *trig_mode = trig;
                }
            }
            if !polarity.is_null() {
                // SAFETY: `polarity` is verified to be non-null and points to valid memory.
                unsafe {
                    *polarity = pol;
                }
            }
            zx_status::sys::ZX_OK
        }
        Err(e) => e.into_raw(),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_configure_irq_vector(global_irq: u32, vector: u8) {
    let io_apic = resolve_global_irq(global_irq);

    lock!(let _guard = IO_APIC_STATE.lock_lock());
    let bank = io_apic.bank();
    let mut reg = read_redirection_entry(&bank, io_apic, global_irq);

    // If we are configuring an invalid vector, automatically mask the IRQ.
    if (io_apic_rte_get_vector(reg) < X86_INT_PLATFORM_BASE.0)
        || (io_apic_rte_get_vector(reg) > X86_INT_PLATFORM_MAX.0)
    {
        reg |= IO_APIC_RTE_MASKED;
    }

    reg &= !IO_APIC_RTE_MASK;
    reg |= io_apic_rte_vector(vector);
    write_redirection_entry(&bank, io_apic, global_irq, reg);
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_fetch_irq_vector(global_irq: u32) -> u8 {
    let io_apic = resolve_global_irq(global_irq);

    lock!(let _guard = IO_APIC_STATE.lock_lock());
    let bank = io_apic.bank();
    let reg = read_redirection_entry(&bank, io_apic, global_irq);
    io_apic_rte_get_vector(reg)
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_mask_isa_irq(isa_irq: u8, mask: bool) {
    assert!(isa_irq < NUM_ISA_IRQS as u8);

    let mut global_irq = isa_irq as u32;
    if IO_APIC_STATE.isa_overrides[isa_irq as usize].remapped {
        global_irq = IO_APIC_STATE.isa_overrides[isa_irq as usize].global_irq;
    }
    apic_io_mask_irq(global_irq, mask);
}

/// For ISA configuration, we don't need to specify the trigger mode
/// and polarity since we initialize these to match the ISA bus or
/// any overrides we've been told about.
#[unsafe(no_mangle)]
pub extern "C" fn apic_io_configure_isa_irq(
    isa_irq: u8,
    del_mode: ApicInterruptDeliveryMode,
    mask: bool,
    dst_mode: ApicInterruptDstMode,
    dst: u8,
    vector: u8,
) {
    assert!(isa_irq < NUM_ISA_IRQS as u8);

    let mut global_irq = isa_irq as u32;
    let mut trig_mode = InterruptTriggerMode::Edge;
    let mut polarity = InterruptPolarity::High;
    if IO_APIC_STATE.isa_overrides[isa_irq as usize].remapped {
        global_irq = IO_APIC_STATE.isa_overrides[isa_irq as usize].global_irq;
        trig_mode = IO_APIC_STATE.isa_overrides[isa_irq as usize].tm;
        polarity = IO_APIC_STATE.isa_overrides[isa_irq as usize].pol;
    }

    apic_io_configure_irq(global_irq, trig_mode, polarity, del_mode, mask, dst_mode, dst, vector);
}

// To correctly use this function, we need to do some work first.
// 1) We need to check for EOI-broadcast suppression support in the local APIC
//    version register.
// 2) We need to check that the IOAPIC is new enough to support the EOI
// 3) We need to enable suppression in the spurious interrupt register.
// 4) Call this function after calling apic_issue_eoi() (or maybe modify
//    apic_issue_eoi() to call this automatically).
//
// In the mean time, IO APIC EOIs are automatically issued via broadcast to
// all IO APICs whenever the local APIC receives an EOI for a level-triggered
// interrupt.
#[unsafe(no_mangle)]
pub extern "C" fn apic_io_issue_eoi(global_irq: u32, vec: u8) {
    let io_apic = resolve_global_irq(global_irq);

    assert!(io_apic.version >= IO_APIC_EOIR_MIN_VERSION);
    let bank = io_apic.bank();
    lock!(let _guard = IO_APIC_STATE.lock_lock());
    // SAFETY: Writing to the write-only IO_APIC_EOIR register is safe for a mapped window.
    unsafe {
        bank.at(IO_APIC_EOIR).write(vec as u32);
    }
}

// Convert a legacy ISA IRQ number into a global IRQ number.
#[unsafe(no_mangle)]
pub extern "C" fn apic_io_isa_to_global(isa_irq: u8) -> u32 {
    // It is a programming bug for this to be invoked with an invalid value.
    assert!(isa_irq < NUM_ISA_IRQS as u8);

    if IO_APIC_STATE.isa_overrides[isa_irq as usize].remapped {
        IO_APIC_STATE.isa_overrides[isa_irq as usize].global_irq
    } else {
        isa_irq as u32
    }
}

/// Returns the [min, max) range representing the (assumed) contiguous range of
/// global system interrupts provided to us by ACPI.
#[unsafe(no_mangle)]
pub extern "C" fn apic_io_get_gsi_range() -> GsiRange {
    assert!(!IO_APIC_STATE.io_apics.is_empty());

    let mut range = GsiRange { start: u32::MAX, end: 0 };

    // If we could be certain the MADT tables were always in increasing order this
    // could be a constant time operation, but since no such guarantee exists we
    // need to walk the descriptors.
    for apic in IO_APIC_STATE.io_apics.iter() {
        range.start = core::cmp::min(range.start, apic.desc.global_irq_base);
        // max_redirection_entry is the offset of the last entry, not the number of entries.
        range.end = core::cmp::max(
            range.end,
            apic.desc.global_irq_base + apic.max_redirection_entry as u32 + 1,
        );
    }

    range
}

// These functions must be invoked with interrupts disabled.  They save/restore the
// current redirection table entries to/from memory.  They are intended for use
// with suspend-to-RAM.
#[unsafe(no_mangle)]
pub extern "C" fn apic_io_save() {
    // SAFETY: FFI call to check if interrupts are disabled is safe.
    unsafe {
        assert!(cpp_arch_ints_disabled());
    }

    lock!(IO_APIC_STATE.lock_lock());
    for apic in IO_APIC_STATE.io_apics.iter() {
        let bank = apic.bank();
        for j in 0..=apic.max_redirection_entry {
            let global_irq = apic.desc.global_irq_base + j as u32;
            let reg = read_redirection_entry(&bank, apic, global_irq);
            // SAFETY: This runs with interrupts disabled on the boot CPU during suspend/resume,
            // so there is no concurrent access to `saved_rtes`.
            unsafe {
                (*apic.saved_rtes.get())[j as usize] = reg;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_restore() {
    // SAFETY: FFI call to check if interrupts are disabled is safe.
    unsafe {
        assert!(cpp_arch_ints_disabled());
    }

    lock!(IO_APIC_STATE.lock_lock());
    for apic in IO_APIC_STATE.io_apics.iter() {
        let bank = apic.bank();
        for j in 0..=apic.max_redirection_entry {
            let global_irq = apic.desc.global_irq_base + j as u32;
            // SAFETY: This runs with interrupts disabled on the boot CPU during suspend/resume,
            // so there is no concurrent access to `saved_rtes`.
            let reg = unsafe { (*apic.saved_rtes.get())[j as usize] };
            write_redirection_entry(&bank, apic, global_irq, reg);
        }
    }
}

fn apic_io_debug_nolock() {
    let mut w = KernelConsoleWriter;
    for (i, apic) in IO_APIC_STATE.io_apics.iter().enumerate() {
        let _ = writeln!(w, "IO APIC idx {}:", i);
        let _ = writeln!(w, "  id: {:08x}", apic.desc.apic_id);
        let _ = writeln!(w, "  version: {:08x}", apic.version);
        let _ = writeln!(w, "  entries: {:08x}", apic.max_redirection_entry as u32 + 1);

        let bank = apic.bank();
        for j in 0..=apic.max_redirection_entry {
            let global_irq = apic.desc.global_irq_base + j as u32;
            let reg = read_redirection_entry(&bank, apic, global_irq);

            let dest_mode_str = if (reg & (1 << 11)) != 0 { "l" } else { "p" };
            let dest = (reg >> 56) as u8;
            let masked_str = if (reg & IO_APIC_RTE_MASKED) != 0 { "masked" } else { "unmasked" };
            let trig_mode_str = io_apic_rte_get_trigger_mode(reg).string();
            let polarity_str = io_apic_rte_get_polarity(reg).string();
            let delivery_mode = ((reg >> 8) & 0x7) as u8;
            let vector = reg as u8;
            let pending_str = if (reg & (1 << 12)) != 0 { "pending" } else { "" };
            let rirr_str = if (reg & (1 << 14)) != 0 { "RIRR" } else { "" };

            let _ = writeln!(
                w,
                "    {:4}: dst: {} {:02x}, {}, {}, {}, dm {:x}, vec {:2x}, {} {}",
                global_irq,
                dest_mode_str,
                dest,
                masked_str,
                trig_mode_str,
                polarity_str,
                delivery_mode,
                vector,
                pending_str,
                rirr_str
            );
        }
    }
    let _ = writeln!(w, "ISA Overrides:");
    for o in IO_APIC_STATE.isa_overrides.iter() {
        if o.remapped {
            let _ = writeln!(w, "  isa_irq {} global_irq {}", o.isa_irq, o.global_irq);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_io_debug() {
    lock!(IO_APIC_STATE.lock_lock());
    apic_io_debug_nolock();
}

#[cfg(console_enabled)]
unsafe extern "C" fn cmd_ioapic(_argc: c_int, _argv: *const CmdArgs, _flags: u32) -> c_int {
    apic_io_debug();
    zx_status::sys::ZX_OK
}

#[cfg(console_enabled)]
static_command!(
    CMD_IO_DEBUG,
    c"ioapic".as_ptr(),
    c"Print IO APIC descriptor information".as_ptr(),
    cmd_ioapic,
    CMD_AVAIL_NORMAL
);
