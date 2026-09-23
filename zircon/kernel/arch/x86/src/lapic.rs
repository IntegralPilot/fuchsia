// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::apic::ApicInterruptDeliveryMode;
use super::registers::{X86_MSR_IA32_APIC_BASE, X86_MSR_IA32_TSC_DEADLINE};
use super::x86::{read_msr, read_msr32, write_msr};
use super::{feature, interrupts, pv};
use crate::arch_rs::InterruptDisableGuard;
#[cfg(console_enabled)]
use crate::console_rust::console::{CMD_AVAIL_NORMAL, CmdArgs, static_command};
use crate::kernel::types::{CPU_MASK_ALL, highest_cpu_set, lowest_cpu_set, mask_all_but_one};
use crate::vm::arch_vm_aspace::{
    ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE, ARCH_MMU_FLAG_UNCACHED_DEVICE,
};
use crate::vm::vm_aspace::VmAspace;
use debug::dprintf;
use kprint::kprint;
use libarch::x86::device_memory_barrier;
use zx_status::Status;

const LOCAL_TRACE: u32 = 0;

// We currently only implement support for the xAPIC

// Virtual address of the local APIC's MMIO registers
static mut APIC_VIRT_BASE: usize = 0;
static mut X2APIC_ENABLED: bool = false;

static mut BSP_APIC_ID: u8 = 0;
static mut BSP_APIC_ID_VALID: bool = false;

pub const INVALID_APIC_ID: u32 = 0xffff_ffff;
pub const APIC_PHYS_BASE: usize = 0xfee0_0000;
pub const IA32_APIC_BASE_BSP: u64 = 1 << 8;
pub const IA32_APIC_BASE_X2APIC_ENABLE: u64 = 1 << 10;
pub const IA32_APIC_BASE_XAPIC_ENABLE: u64 = 1 << 11;

// local apic registers
// set as an offset into the mmio region here
// x2APIC msr offsets are these >> 4
pub const LAPIC_REG_ID: usize = 0x020;
pub const LAPIC_REG_VERSION: usize = 0x030;
pub const LAPIC_REG_TASK_PRIORITY: usize = 0x080;
pub const LAPIC_REG_PROCESSOR_PRIORITY: usize = 0x0a0;
pub const LAPIC_REG_EOI: usize = 0x0b0;
pub const LAPIC_REG_LOGICAL_DST: usize = 0x0d0;
pub const LAPIC_REG_SPURIOUS_IRQ: usize = 0x0f0;
#[inline(always)]
pub const fn lapic_reg_in_service(x: usize) -> usize {
    0x100 + (x << 4)
}
#[inline(always)]
pub const fn lapic_reg_trigger_mode(x: usize) -> usize {
    0x180 + (x << 4)
}
#[inline(always)]
pub const fn lapic_reg_irq_request(x: usize) -> usize {
    0x200 + (x << 4)
}
pub const LAPIC_REG_ERROR_STATUS: usize = 0x280;
pub const LAPIC_REG_LVT_CMCI: usize = 0x2f0;
pub const LAPIC_REG_IRQ_CMD_LOW: usize = 0x300;
pub const LAPIC_REG_IRQ_CMD_HIGH: usize = 0x310;
pub const LAPIC_REG_LVT_TIMER: usize = 0x320;
pub const LAPIC_REG_LVT_THERMAL: usize = 0x330;
pub const LAPIC_REG_LVT_PERF: usize = 0x340;
pub const LAPIC_REG_LVT_LINT0: usize = 0x350;
pub const LAPIC_REG_LVT_LINT1: usize = 0x360;
pub const LAPIC_REG_LVT_ERROR: usize = 0x370;
pub const LAPIC_REG_INIT_COUNT: usize = 0x380;
pub const LAPIC_REG_CURRENT_COUNT: usize = 0x390;
pub const LAPIC_REG_DIVIDE_CONF: usize = 0x3e0;

pub const LAPIC_X2APIC_MSR_BASE: u32 = 0x800;
pub const LAPIC_X2APIC_MSR_ICR: u32 = 0x830;
pub const LAPIC_X2APIC_MSR_SELF_IPI: u32 = 0x83f;

// Spurious IRQ bitmasks
pub const SVR_APIC_ENABLE: u32 = 1 << 8;
#[inline(always)]
pub const fn svr_spurious_vector(x: u32) -> u32 {
    x
}

// Interrupt Command bitmasks
#[inline(always)]
pub const fn icr_vector(x: u32) -> u32 {
    x
}
pub const ICR_DELIVERY_PENDING: u32 = 1 << 12;
pub const ICR_LEVEL_ASSERT: u32 = 1 << 14;
#[inline(always)]
pub const fn icr_dst(x: u32) -> u32 {
    x << 24
}
pub const ICR_DST_BROADCAST: u32 = icr_dst(0xff);
#[inline(always)]
pub const fn icr_delivery_mode(x: u32) -> u32 {
    x << 8
}
#[inline(always)]
pub const fn icr_dst_shorthand(x: u32) -> u32 {
    x << 18
}
pub const ICR_DST_SELF: u32 = icr_dst_shorthand(1);
pub const ICR_DST_ALL: u32 = icr_dst_shorthand(2);
pub const ICR_DST_ALL_MINUS_SELF: u32 = icr_dst_shorthand(3);

#[inline(always)]
pub const fn x2_icr_dst(x: u64) -> u64 {
    x << 32
}
pub const X2_ICR_BROADCAST: u64 = 0xffff_ffff_u64 << 32;

// Common LVT bitmasks
#[inline(always)]
pub const fn lvt_vector(x: u32) -> u32 {
    x
}
#[inline(always)]
pub const fn lvt_delivery_mode(x: u32) -> u32 {
    x << 8
}
pub const LVT_DELIVERY_PENDING: u32 = 1 << 12;

// LVT Timer bitmasks
pub const LVT_TIMER_VECTOR_MASK: u32 = 0x0000_00ff;
pub const LVT_TIMER_MODE_MASK: u32 = 0x0006_0000;
pub const LVT_TIMER_MODE_ONESHOT: u32 = 0 << 17;
pub const LVT_TIMER_MODE_PERIODIC: u32 = 1 << 17;
pub const LVT_TIMER_MODE_TSC_DEADLINE: u32 = 2 << 17;
pub const LVT_TIMER_MODE_RESERVED: u32 = 3 << 17;
pub const LVT_MASKED: u32 = 1 << 16;

unsafe extern "C" {
    fn root_resource_filter_add_deny_region(paddr: usize, len: usize, kind: u32);
    fn x86_set_local_apic_id(apic_id: u32);
    fn cpp_x86_percpu_get_apic_id(cpu_num: u32) -> u32;
    fn cpp_x86_curr_percpu_get_apic_id() -> u32;
    fn platform_handle_apic_timer_tick();
}

fn lapic_reg_read(offset: usize) -> u32 {
    // SAFETY: X2APIC_ENABLED is initialized during local APIC init on BSP.
    if unsafe { X2APIC_ENABLED } {
        // SAFETY: Reading valid LAPIC MSR in x2APIC mode.
        unsafe { read_msr32(LAPIC_X2APIC_MSR_BASE + (offset >> 4) as u32) }
    } else {
        // SAFETY: APIC_VIRT_BASE is mapped and offset is within the 4KB page.
        unsafe {
            let ptr = core::ptr::with_exposed_provenance::<u32>(APIC_VIRT_BASE + offset);
            core::ptr::read_volatile(ptr)
        }
    }
}

fn lapic_reg_write(offset: usize, val: u32) {
    // SAFETY: X2APIC_ENABLED is initialized during local APIC init on BSP.
    if unsafe { X2APIC_ENABLED } {
        // SAFETY: Writing valid LAPIC MSR in x2APIC mode.
        unsafe { write_msr(LAPIC_X2APIC_MSR_BASE + (offset >> 4) as u32, val as u64) };
    } else {
        // SAFETY: APIC_VIRT_BASE is mapped and offset is within the 4KB page.
        unsafe {
            let ptr = core::ptr::with_exposed_provenance_mut::<u32>(APIC_VIRT_BASE + offset);
            core::ptr::write_volatile(ptr, val);
        }
    }
}

fn lapic_reg_or(offset: usize, bits: u32) {
    lapic_reg_write(offset, lapic_reg_read(offset) | bits);
}

fn lapic_reg_and(offset: usize, bits: u32) {
    lapic_reg_write(offset, lapic_reg_read(offset) & bits);
}

#[unsafe(no_mangle)]
pub extern "C" fn is_x2apic_enabled() -> bool {
    // SAFETY: Reads whether x2APIC mode is enabled.
    unsafe { X2APIC_ENABLED }
}

/// This function must be called once on the kernel address space
#[unsafe(no_mangle)]
pub extern "C" fn apic_vm_init() {
    // only memory map the aperture if we're using the legacy mmio interface
    if !is_x2apic_enabled() {
        // SAFETY: APIC_VIRT_BASE is verified unallocated.
        unsafe {
            assert!(APIC_VIRT_BASE == 0);
        }
        // Create a mapping for the page of MMIO registers
        let kernel_aspace = VmAspace::kernel_aspace();
        let mut vaddr: *mut core::ffi::c_void = core::ptr::null_mut();
        // SAFETY: Allocating physical LAPIC management page in kernel aspace.
        let res = unsafe {
            kernel_aspace.alloc_physical(
                c"lapic",
                page::SIZE,
                &mut vaddr,
                page::SHIFT as u8,
                crate::kernel::types::PAddr(APIC_PHYS_BASE),
                0,
                ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE | ARCH_MMU_FLAG_UNCACHED_DEVICE,
            )
        };
        if let Err(res) = res {
            panic!("Could not allocate APIC management page: {:?}\n", res);
        }
        // SAFETY: Storing mapped virtual address.
        unsafe {
            APIC_VIRT_BASE = vaddr as usize;
            assert!(APIC_VIRT_BASE != 0);
        }
    }

    // Whether we chose to map the old MMIO region or not, make sure we put the
    // registers on the system-wide MMIO deny list.
    // SAFETY: Calling C FFI to add APIC physical base to root resource deny list.
    unsafe {
        root_resource_filter_add_deny_region(
            APIC_PHYS_BASE,
            page::SIZE,
            zx_types::ZX_RSRC_KIND_MMIO,
        );
    }
}

/// Initializes the current processor's local APIC.  Should be called after
/// apic_vm_init has been called.
#[unsafe(no_mangle)]
pub extern "C" fn apic_local_init() {
    debug_assert!(crate::arch_rs::ints_disabled());

    // SAFETY: Reading IA32_APIC_BASE MSR is valid on x86 CPUs.
    let mut v = unsafe { read_msr(X86_MSR_IA32_APIC_BASE) };

    // if were the boot processor, test and cache x2apic ability
    if (v & IA32_APIC_BASE_BSP) != 0 && feature::x86_feature_test(feature::X86_FEATURE_X2APIC) {
        dprintf!(SPEW, "x2APIC enabled\n");
        // SAFETY: Updating X2APIC_ENABLED during single-core boot.
        unsafe {
            X2APIC_ENABLED = true;
        }
    }

    // Enter xAPIC or x2APIC mode and set the base address
    v |= IA32_APIC_BASE_XAPIC_ENABLE;
    v |= if is_x2apic_enabled() { IA32_APIC_BASE_X2APIC_ENABLE } else { 0 };
    // SAFETY: Writing updated IA32_APIC_BASE MSR configures APIC mode.
    unsafe { write_msr(X86_MSR_IA32_APIC_BASE, v) };

    // If this is the bootstrap processor, we should record our APIC ID now
    // that we know it.
    if (v & IA32_APIC_BASE_BSP) != 0 {
        let id = apic_local_id();

        // SAFETY: Updating BSP_APIC_ID and BSP_APIC_ID_VALID during single-core boot.
        unsafe {
            BSP_APIC_ID = id;
            BSP_APIC_ID_VALID = true;
            x86_set_local_apic_id(id as u32);
        }
    }

    // Specify the spurious interrupt vector and enable the local APIC
    let svr = svr_spurious_vector(interrupts::X86_INT_APIC_SPURIOUS.0 as u32) | SVR_APIC_ENABLE;
    lapic_reg_write(LAPIC_REG_SPURIOUS_IRQ, svr);

    apic_error_init();
    apic_timer_init();
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_local_id() -> u8 {
    let mut id = lapic_reg_read(LAPIC_REG_ID);

    // legacy apic stores the id in the top 8 bits of the register
    // SAFETY: Checking X2APIC_ENABLED.
    if unsafe { !X2APIC_ENABLED } {
        id >>= 24;
    }

    // we can only deal with 8 bit apic ids right now
    debug_assert!(id < 256);

    id as u8
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_bsp_id() -> u8 {
    // SAFETY: Checking BSP_APIC_ID_VALID before reading BSP_APIC_ID.
    unsafe {
        debug_assert!(BSP_APIC_ID_VALID);
        BSP_APIC_ID
    }
}

#[inline(always)]
fn apic_wait_for_ipi_send() {
    while (lapic_reg_read(LAPIC_REG_IRQ_CMD_LOW) & ICR_DELIVERY_PENDING) != 0 {
        core::hint::spin_loop();
    }
}

// We only support physical destination modes for now

#[unsafe(no_mangle)]
pub extern "C" fn apic_send_ipi(vector: u8, dst_apic_id: u32, dm: ApicInterruptDeliveryMode) {
    // we only support 8 bit apic ids
    debug_assert!(dst_apic_id < (u8::MAX as u32));

    let request = ICR_LEVEL_ASSERT | icr_delivery_mode(dm as u32) | icr_vector(vector as u32);
    if feature::x86_hypervisor_has_pv_ipi() {
        let ret = pv::pv_ipi(1, 0, dst_apic_id as u64, request as u64);
        debug_assert!(ret >= 0);
        return;
    }

    // SAFETY: Checking X2APIC_ENABLED.
    if unsafe { X2APIC_ENABLED } {
        // SAFETY: Writing x2APIC ICR MSR.
        unsafe {
            write_msr(LAPIC_X2APIC_MSR_ICR, x2_icr_dst(dst_apic_id as u64) | (request as u64))
        };
        return;
    }

    {
        let _irqd = InterruptDisableGuard::new();

        lapic_reg_write(LAPIC_REG_IRQ_CMD_HIGH, icr_dst(dst_apic_id));
        lapic_reg_write(LAPIC_REG_IRQ_CMD_LOW, request);
        apic_wait_for_ipi_send();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_send_self_ipi(vector: u8, dm: ApicInterruptDeliveryMode) {
    let mut request = ICR_LEVEL_ASSERT | icr_delivery_mode(dm as u32) | icr_vector(vector as u32);
    if feature::x86_hypervisor_has_pv_ipi() {
        // SAFETY: Retrieves APIC ID of current CPU.
        let apic_id = unsafe { cpp_x86_curr_percpu_get_apic_id() };
        let ret = pv::pv_ipi(1, 0, apic_id as u64, request as u64);
        debug_assert!(ret >= 0);
        return;
    }

    request |= ICR_DST_SELF;
    // SAFETY: Checking X2APIC_ENABLED.
    if unsafe { X2APIC_ENABLED } {
        // special register for triggering self ipis
        // SAFETY: Writing x2APIC SELF_IPI MSR.
        unsafe { write_msr(LAPIC_X2APIC_MSR_SELF_IPI, vector as u64) };
        return;
    }

    {
        let _irqd = InterruptDisableGuard::new();

        lapic_reg_write(LAPIC_REG_IRQ_CMD_LOW, request);
        apic_wait_for_ipi_send();
    }
}

fn pv_mask_ipi(mask: u32, request: u32) {
    // |mask_size| represents the number of bits in a CPU mask. As each CPU mask
    // is a uint64_t, there are ofcourse 64 bits in a CPU mask.
    const MASK_SIZE: u64 = 64;
    // We only handle 8 bit APICs so there are 2^8 possible CPUs.
    const NUM_MASKS: usize = 256 / (MASK_SIZE as usize);
    zr::static_assert!(NUM_MASKS.is_multiple_of(2));
    let mut masks = [0u64; NUM_MASKS];

    let num_cpus = core::cmp::min(crate::arch_rs::max_num_cpus(), highest_cpu_set(mask) + 1);
    for cpu_id in lowest_cpu_set(mask)..num_cpus {
        if ((mask >> cpu_id) & 1) != 0 {
            // SAFETY: FFI to get APIC ID for cpu_id.
            let apic_id = unsafe { cpp_x86_percpu_get_apic_id(cpu_id) };
            if apic_id != INVALID_APIC_ID {
                let apic_id = apic_id as u64;
                masks[(apic_id / MASK_SIZE) as usize] |= 1u64 << (apic_id % MASK_SIZE);
            }
        }
    }

    for i in (0..masks.len()).step_by(2) {
        if masks[i] != 0 || masks[i + 1] != 0 {
            let ret = pv::pv_ipi(masks[i], masks[i + 1], (i as u64) * MASK_SIZE, request as u64);
            debug_assert!(ret >= 0);
        }
    }
}

// Broadcast to everyone including self
#[unsafe(no_mangle)]
pub extern "C" fn apic_send_broadcast_self_ipi(vector: u8, dm: ApicInterruptDeliveryMode) {
    let mut request = ICR_LEVEL_ASSERT | icr_delivery_mode(dm as u32) | icr_vector(vector as u32);
    if feature::x86_hypervisor_has_pv_ipi() {
        pv_mask_ipi(CPU_MASK_ALL, request);
        return;
    }

    request |= ICR_DST_ALL;
    // SAFETY: Checking X2APIC_ENABLED.
    if unsafe { X2APIC_ENABLED } {
        // SAFETY: Writing x2APIC ICR MSR with broadcast destination.
        unsafe { write_msr(LAPIC_X2APIC_MSR_ICR, X2_ICR_BROADCAST | (request as u64)) };
        return;
    }

    {
        let _irqd = InterruptDisableGuard::new();
        lapic_reg_write(LAPIC_REG_IRQ_CMD_HIGH, ICR_DST_BROADCAST);
        lapic_reg_write(LAPIC_REG_IRQ_CMD_LOW, request);
        apic_wait_for_ipi_send();
    }
}

// Broadcast to everyone excluding self
#[unsafe(no_mangle)]
pub extern "C" fn apic_send_broadcast_ipi(vector: u8, dm: ApicInterruptDeliveryMode) {
    let mut request = ICR_LEVEL_ASSERT | icr_delivery_mode(dm as u32) | icr_vector(vector as u32);
    if feature::x86_hypervisor_has_pv_ipi() {
        let mask = mask_all_but_one(crate::arch_rs::curr_cpu_num());
        pv_mask_ipi(mask, request);
        return;
    }

    request |= ICR_DST_ALL_MINUS_SELF;
    // SAFETY: Checking X2APIC_ENABLED.
    if unsafe { X2APIC_ENABLED } {
        // SAFETY: Writing x2APIC ICR MSR with broadcast destination excluding self.
        unsafe { write_msr(LAPIC_X2APIC_MSR_ICR, X2_ICR_BROADCAST | (request as u64)) };
        return;
    }

    {
        let _irqd = InterruptDisableGuard::new();

        lapic_reg_write(LAPIC_REG_IRQ_CMD_HIGH, ICR_DST_BROADCAST);
        lapic_reg_write(LAPIC_REG_IRQ_CMD_LOW, request);
        apic_wait_for_ipi_send();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_send_mask_ipi(vector: u8, mask: u32, dm: ApicInterruptDeliveryMode) {
    debug_assert!((crate::arch_rs::max_num_cpus() as usize) <= core::mem::size_of::<u32>() * 8);
    if feature::x86_hypervisor_has_pv_ipi() {
        let request = ICR_LEVEL_ASSERT | icr_delivery_mode(dm as u32) | icr_vector(vector as u32);
        pv_mask_ipi(mask, request);
        return;
    }

    let num_cpus = core::cmp::min(crate::arch_rs::max_num_cpus(), highest_cpu_set(mask) + 1);
    for cpu_id in lowest_cpu_set(mask)..num_cpus {
        if ((mask >> cpu_id) & 1) != 0 {
            // SAFETY: FFI to get APIC ID for cpu_id.
            let apic_id = unsafe { cpp_x86_percpu_get_apic_id(cpu_id) };
            if apic_id != INVALID_APIC_ID {
                apic_send_ipi(vector, apic_id, dm);
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_issue_eoi() {
    if pv::PvEoi::get().eoi() {
        return;
    }
    // Write 0 to the EOI address to issue an EOI
    lapic_reg_write(LAPIC_REG_EOI, 0);
}

// If this function returns an error, timer state will not have
// been changed.
fn apic_timer_set_divide_value(v: u8) -> Result<(), Status> {
    let new_value = match v {
        1 => 0xb,
        2 => 0x0,
        4 => 0x1,
        8 => 0x2,
        16 => 0x3,
        32 => 0x8,
        64 => 0x9,
        128 => 0xa,
        _ => return Err(Status::INVALID_ARGS),
    };
    lapic_reg_write(LAPIC_REG_DIVIDE_CONF, new_value);
    Ok(())
}

fn apic_timer_init() {
    lapic_reg_write(
        LAPIC_REG_LVT_TIMER,
        lvt_vector(interrupts::X86_INT_APIC_TIMER.0 as u32) | LVT_MASKED,
    );
    if feature::x86_feature_test(feature::X86_FEATURE_TSC_DEADLINE) {
        apic_timer_tsc_deadline_init();
    }
}

// Invoked on each CPU to enable the TSC Deadline timer.
#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_tsc_deadline_init() {
    debug_assert!(feature::x86_feature_test(feature::X86_FEATURE_TSC_DEADLINE));
    lapic_reg_write(
        LAPIC_REG_LVT_TIMER,
        lvt_vector(interrupts::X86_INT_APIC_TIMER.0 as u32) | LVT_TIMER_MODE_TSC_DEADLINE,
    );
    // Intel recommends using an MFENCE to ensure the LVT_TIMER_ADDR write
    // takes before the write_msr(), since writes to this MSR are ignored if the
    // time mode is not DEADLINE.
    device_memory_barrier();
}

// Racy; primarily useful for calibrating the timer.
#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_current_count() -> u32 {
    lapic_reg_read(LAPIC_REG_CURRENT_COUNT)
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_mask() {
    let _irqd = InterruptDisableGuard::new();

    lapic_reg_or(LAPIC_REG_LVT_TIMER, LVT_MASKED);
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_unmask() {
    let _irqd = InterruptDisableGuard::new();

    lapic_reg_and(LAPIC_REG_LVT_TIMER, !LVT_MASKED);
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_stop() {
    let _irqd = InterruptDisableGuard::new();

    lapic_reg_write(LAPIC_REG_INIT_COUNT, 0);
    if feature::x86_feature_test(feature::X86_FEATURE_TSC_DEADLINE) {
        // SAFETY: Writing 0 to IA32_TSC_DEADLINE MSR disables deadline timer.
        unsafe { write_msr(X86_MSR_IA32_TSC_DEADLINE, 0) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_set_oneshot(
    count: u32,
    divisor: u8,
    masked: bool,
) -> Result<(), Status> {
    let mut timer_config =
        lvt_vector(interrupts::X86_INT_APIC_TIMER.0 as u32) | LVT_TIMER_MODE_ONESHOT;
    if masked {
        timer_config |= LVT_MASKED;
    }

    let _irqd = InterruptDisableGuard::new();

    apic_timer_set_divide_value(divisor)?;
    lapic_reg_write(LAPIC_REG_LVT_TIMER, timer_config);
    lapic_reg_write(LAPIC_REG_INIT_COUNT, count);
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_set_tsc_deadline(deadline: u64) {
    debug_assert!(feature::x86_feature_test(feature::X86_FEATURE_TSC_DEADLINE));
    debug_assert!(crate::arch_rs::ints_disabled());
    // SAFETY: Writing deadline timestamp to IA32_TSC_DEADLINE MSR.
    unsafe { write_msr(X86_MSR_IA32_TSC_DEADLINE, deadline) };
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_timer_interrupt_handler() {
    // SAFETY: Calling C FFI to handle APIC timer tick.
    unsafe { platform_handle_apic_timer_tick() };
}

fn apic_error_init() {
    lapic_reg_write(LAPIC_REG_LVT_ERROR, lvt_vector(interrupts::X86_INT_APIC_ERROR.0 as u32));
    // Re-arm the error interrupt triggering mechanism
    lapic_reg_write(LAPIC_REG_ERROR_STATUS, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_error_interrupt_handler() {
    debug_assert!(crate::arch_rs::ints_disabled());

    // This write doesn't effect the subsequent read, but is required prior to
    // reading.
    lapic_reg_write(LAPIC_REG_ERROR_STATUS, 0);
    panic!("APIC error detected: {}\n", lapic_reg_read(LAPIC_REG_ERROR_STATUS));
}

#[cfg(console_enabled)]
unsafe extern "C" fn cmd_apic(
    argc: core::ffi::c_int,
    argv: *const CmdArgs,
    _flags: u32,
) -> core::ffi::c_int {
    // SAFETY: The console framework guarantees argv points to argc elements.
    let args = unsafe { core::slice::from_raw_parts(argv, argc as usize) };

    let usage = || -> core::ffi::c_int {
        let arg0 = if argc > 0 && !args[0].arg_str.is_null() {
            // SAFETY: arg_str is a null-terminated C string provided by console framework.
            unsafe { core::ffi::CStr::from_ptr(args[0].arg_str) }.to_str().unwrap_or("apic")
        } else {
            "apic"
        };
        kprint!("usage:\n");
        kprint!("{:s} dump io\n", arg0);
        kprint!("{:s} dump local\n", arg0);
        kprint!("{:s} broadcast <vec>\n", arg0);
        kprint!("{:s} self <vec>\n", arg0);
        Status::INTERNAL.into_raw()
    };

    if argc < 2 || args[1].arg_str.is_null() {
        kprint!("not enough arguments\n");
        return usage();
    }

    // SAFETY: arg_str is a null-terminated C string provided by console framework.
    let subcmd = unsafe { core::ffi::CStr::from_ptr(args[1].arg_str) }.to_bytes();

    if subcmd == b"broadcast" {
        if argc < 3 {
            kprint!("not enough arguments\n");
            return usage();
        }
        let vec = args[2].arg_uint as u8;
        apic_send_broadcast_ipi(vec, ApicInterruptDeliveryMode::Fixed);
        kprint!("irr: {:x}\n", lapic_reg_read(lapic_reg_irq_request((vec / 32) as usize)));
        kprint!("isr: {:x}\n", lapic_reg_read(lapic_reg_in_service((vec / 32) as usize)));
        kprint!("icr: {:x}\n", lapic_reg_read(LAPIC_REG_IRQ_CMD_LOW));
    } else if subcmd == b"self" {
        if argc < 3 {
            kprint!("not enough arguments\n");
            return usage();
        }
        let vec = args[2].arg_uint as u8;
        apic_send_self_ipi(vec, ApicInterruptDeliveryMode::Fixed);
        kprint!("irr: {:x}\n", lapic_reg_read(lapic_reg_irq_request((vec / 32) as usize)));
        kprint!("isr: {:x}\n", lapic_reg_read(lapic_reg_in_service((vec / 32) as usize)));
        kprint!("icr: {:x}\n", lapic_reg_read(LAPIC_REG_IRQ_CMD_LOW));
    } else if subcmd == b"dump" {
        if argc < 3 || args[2].arg_str.is_null() {
            kprint!("not enough arguments\n");
            return usage();
        }
        // SAFETY: arg_str is a null-terminated C string provided by console framework.
        let dump_target = unsafe { core::ffi::CStr::from_ptr(args[2].arg_str) }.to_bytes();
        if dump_target == b"local" {
            kprint!("Caution: this is only for one CPU\n");
            apic_local_debug();
        } else if dump_target == b"io" {
            super::ioapic::apic_io_debug();
        } else {
            kprint!("unknown subcommand\n");
            return usage();
        }
    } else {
        kprint!("unknown command\n");
        return usage();
    }

    zx_status::sys::ZX_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn apic_local_debug() {
    let _irqd = InterruptDisableGuard::new();

    kprint!("apic {:02x}:\n", apic_local_id());
    kprint!("  version: {:08x}:\n", lapic_reg_read(LAPIC_REG_VERSION));
    kprint!("  logical_dst: {:08x}\n", lapic_reg_read(LAPIC_REG_LOGICAL_DST));
    kprint!("  spurious_irq: {:08x}\n", lapic_reg_read(LAPIC_REG_SPURIOUS_IRQ));
    kprint!("  tpr: {:02x}\n", lapic_reg_read(LAPIC_REG_TASK_PRIORITY) as u8);
    kprint!("  ppr: {:02x}\n", lapic_reg_read(LAPIC_REG_PROCESSOR_PRIORITY) as u8);
    for i in 0..8 {
        kprint!("  irr {}: {:08x}\n", i, lapic_reg_read(lapic_reg_irq_request(i)));
    }
    for i in 0..8 {
        kprint!("  isr {}: {:08x}\n", i, lapic_reg_read(lapic_reg_in_service(i)));
    }
}

#[cfg(console_enabled)]
static_command!(CMD_APIC, c"apic".as_ptr(), c"apic commands".as_ptr(), cmd_apic, CMD_AVAIL_NORMAL);
