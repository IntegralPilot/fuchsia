// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::interrupt_manager::InterruptManager;
use super::pic::{pic_disable, pic_map};
use crate::arch_rs::x86::apic::{
    ApicInterruptDeliveryMode, ApicInterruptDstMode, GsiRange, IoApicDescriptor, IoApicIsaOverride,
};
use crate::arch_rs::x86::interrupts::{X86_INT_PLATFORM_BASE, X86_INT_PLATFORM_MAX};
use crate::arch_rs::x86::ioapic::{
    apic_io_configure_irq, apic_io_configure_irq_vector, apic_io_configure_isa_irq,
    apic_io_fetch_irq_config_safe, apic_io_fetch_irq_vector, apic_io_get_gsi_range,
    apic_io_init_safe, apic_io_is_valid_irq, apic_io_isa_to_global, apic_io_mask_irq,
};
use crate::dev_interrupt::{InterruptHandler, InterruptPolarity, InterruptTriggerMode, MsiBlock};
use crate::object::ResourceDispatcher;
use core::pin::Pin;
use lazy_init::LazyInit;
use zx_status::Status;
use zx_types::{ZX_RSRC_KIND_IRQ, zx_status_t};

struct RealIoApic;

impl super::interrupt_manager::IoApic for RealIoApic {
    fn is_valid_interrupt(vector: u32, flags: u32) -> bool {
        is_valid_interrupt(vector, flags)
    }
    fn fetch_irq_vector(vector: u32) -> u8 {
        apic_io_fetch_irq_vector(vector)
    }
    fn configure_irq_vector(global_irq: u32, x86_vector: u8) {
        apic_io_configure_irq_vector(global_irq, x86_vector);
    }
    fn configure_irq(
        global_irq: u32,
        trig_mode: InterruptTriggerMode,
        polarity: InterruptPolarity,
        del_mode: ApicInterruptDeliveryMode,
        mask: bool,
        dst_mode: ApicInterruptDstMode,
        dst: u8,
        vector: u8,
    ) {
        apic_io_configure_irq(
            global_irq, trig_mode, polarity, del_mode, mask, dst_mode, dst, vector,
        );
    }
    fn mask_irq(global_irq: u32, mask: bool) {
        apic_io_mask_irq(global_irq, mask);
    }
    fn fetch_irq_config(
        global_irq: u32,
    ) -> Result<(InterruptTriggerMode, InterruptPolarity), Status> {
        apic_io_fetch_irq_config_safe(global_irq)
    }
}

static INTERRUPT_MANAGER: LazyInit<InterruptManager<RealIoApic>> = LazyInit::uninit();

// Values from ioapic to cache for calls to interrupt_get_base_vector / interrupt_get_max_vector
static GSI_RANGE: LazyInit<GsiRange> = LazyInit::uninit();

unsafe extern "C" {
    fn apic_vm_init();
    fn apic_local_init();
    fn apic_issue_eoi();
    fn cpp_arch_ints_disabled() -> bool;
    fn apic_bsp_id() -> u8;
}

// Convert an ACPI entry into the format required by the platform's APIC code.
fn parse_isa_override(
    record: &acpi_lite::structures::AcpiMadtIntSourceOverrideEntry,
) -> IoApicIsaOverride {
    // 0 means ISA, ISOs are only ever for ISA IRQs.
    if record.bus != 0 {
        panic!("Invalid bus for IO APIC interrupt override.\n");
    }

    // "Conforms" below means conforms to the bus spec: edge triggered and active high.
    let flags = record.flags;
    let polarity = match flags & acpi_lite::structures::ACPI_MADT_FLAG_POLARITY_MASK {
        acpi_lite::structures::ACPI_MADT_FLAG_POLARITY_CONFORMS
        | acpi_lite::structures::ACPI_MADT_FLAG_POLARITY_HIGH => InterruptPolarity::High,
        acpi_lite::structures::ACPI_MADT_FLAG_POLARITY_LOW => InterruptPolarity::Low,
        p => panic!("Unknown IRQ polarity in override: {}\n", p),
    };

    let trigger_mode = match flags & acpi_lite::structures::ACPI_MADT_FLAG_TRIGGER_MASK {
        acpi_lite::structures::ACPI_MADT_FLAG_TRIGGER_CONFORMS
        | acpi_lite::structures::ACPI_MADT_FLAG_TRIGGER_EDGE => InterruptTriggerMode::Edge,
        acpi_lite::structures::ACPI_MADT_FLAG_TRIGGER_LEVEL => InterruptTriggerMode::Level,
        t => panic!("Unknown IRQ trigger in override: {}\n", t),
    };

    IoApicIsaOverride {
        isa_irq: record.source,
        remapped: true,
        tm: trigger_mode,
        pol: polarity,
        global_irq: record.global_sys_interrupt,
    }
}

/// Initializes the APIC platform and registers the interrupt manager.
fn platform_init_apic(_level: init::LkInitLevel) {
    pic_map(0x20, 0x28);
    pic_disable();

    let parser = super::acpi::global_acpi_lite_parser();

    // Enumerate the IO APICs
    let mut descriptors = fbl::Vector::<IoApicDescriptor>::new();
    let status = acpi_lite::enumerate_io_apics(parser, |descriptor| {
        descriptors
            .push_back(IoApicDescriptor {
                apic_id: descriptor.io_apic_id,
                global_irq_base: descriptor.global_system_interrupt_base,
                paddr: descriptor.io_apic_address as usize,
            })
            .map_err(|_| Status::NO_MEMORY)
    });
    if status.is_err() {
        panic!("Could not get IO APIC details: {:?}", status);
    }

    // Enumerate interrupt source overrides.
    let mut overrides = fbl::Vector::<IoApicIsaOverride>::new();
    let status = acpi_lite::enumerate_io_apic_isa_overrides(parser, |isa_override| {
        overrides.push_back(parse_isa_override(isa_override)).map_err(|_| Status::NO_MEMORY)
    });
    if status.is_err() {
        panic!("Could not get interrupt source overrides: {:?}", status);
    }

    // SAFETY: These functions initialize the virtual memory mappings and local APIC registers
    // on the bootstrap processor. This must be done before configuring any interrupt routing.
    unsafe {
        apic_vm_init();
        apic_local_init();
    }

    // Call ioapic init
    apic_io_init_safe(&descriptors, &overrides);

    let range = apic_io_get_gsi_range();
    // SAFETY: Called once during boot, serialized with respect to any other access.
    unsafe {
        GSI_RANGE.init(range);
    }

    // SAFETY: `cpp_arch_ints_disabled` returns if interrupts are disabled for the current cpu,
    // which is safe to call at any time.
    assert!(unsafe { cpp_arch_ints_disabled() });

    // Initialize the delivery modes/targets for the ISA interrupts
    // SAFETY: `apic_bsp_id` is safe to call to retrieve the bootstrap processor APIC ID
    // once the APIC vm is initialized.
    let bsp_apic_id = unsafe { apic_bsp_id() };
    for irq in 0..8 {
        if irq != 2 {
            // Explicitly skip mapping the PIC2 interrupt, since it is actually
            // just used internally on the PICs for daisy chaining.  QEMU remaps
            // ISA IRQ 0 to global IRQ 2, but does not remap ISA IRQ 2 off of
            // global IRQ 2, so skipping this mapping also prevents a collision
            // with the PIT IRQ.
            apic_io_configure_isa_irq(
                irq,
                ApicInterruptDeliveryMode::Fixed,
                true, // mask
                ApicInterruptDstMode::Physical,
                bsp_apic_id,
                0,
            );
        }
        apic_io_configure_isa_irq(
            irq + 8,
            ApicInterruptDeliveryMode::Fixed,
            true, // mask
            ApicInterruptDstMode::Physical,
            bsp_apic_id,
            0,
        );
    }

    // Initialize the global INTERRUPT_MANAGER
    // SAFETY: Called once during boot, serialized with respect to any other access.
    let _ = unsafe {
        Pin::static_ref(&INTERRUPT_MANAGER).init_pin(InterruptManager::<RealIoApic>::new())
    };

    let status = (*INTERRUPT_MANAGER).init();
    if status.is_err() {
        panic!("InterruptManager init failed: {:?}", status);
    }

    let status = ResourceDispatcher::initialize_allocator(
        ZX_RSRC_KIND_IRQ,
        interrupt_get_base_vector() as u64,
        interrupt_get_max_vector() as usize,
    );
    assert!(status.is_ok());
}

/// Handles a platform interrupt from an interrupt vector.
///
/// # Safety
/// This function is called directly from the assembly interrupt entry point.
/// `frame` must be a valid pointer to the interrupt frame containing the CPU registers,
/// and interrupts must be disabled on the current CPU when this is called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn platform_irq(frame: *const crate::arch_rs::x86::Iframe) {
    crate::kernel::stats::inc_interrupts();

    // SAFETY: `frame` is a valid pointer to an interrupt register state frame.
    let x86_vector = unsafe { (*frame).vector };
    debug_assert!(
        x86_vector >= X86_INT_PLATFORM_BASE.0 as u64 && x86_vector <= X86_INT_PLATFORM_MAX.0 as u64
    );

    INTERRUPT_MANAGER.invoke_x86_vector(x86_vector as u8);

    // SAFETY: Issuing an EOI is required to acknowledge the interrupt at the APIC level
    // and is safe to call inside the interrupt handler.
    unsafe {
        apic_issue_eoi();
    }
}

/// Registers the handler for the specified vector.
#[unsafe(no_mangle)]
pub extern "C" fn register_int_handler(
    vector: u32,
    handler: InterruptHandler,
) -> Result<(), Status> {
    INTERRUPT_MANAGER.register_interrupt_handler(vector, handler, false)
}

/// Registers a permanent handler for the specified vector.
#[unsafe(no_mangle)]
pub extern "C" fn register_permanent_int_handler(
    vector: u32,
    handler: InterruptHandler,
) -> Result<(), Status> {
    INTERRUPT_MANAGER.register_interrupt_handler(vector, handler, true)
}

/// Registers the MSI handler.
#[unsafe(no_mangle)]
pub extern "C" fn msi_register_handler(block: &MsiBlock, msi_id: u32, handler: InterruptHandler) {
    INTERRUPT_MANAGER.msi_register_handler(block, msi_id, handler);
}

/// Masks the specified interrupt vector.
#[unsafe(no_mangle)]
pub extern "C" fn mask_interrupt(vector: u32) -> zx_status_t {
    Status::result_into_raw(INTERRUPT_MANAGER.mask_interrupt(vector))
}

/// Unmasks the specified interrupt vector.
#[unsafe(no_mangle)]
pub extern "C" fn unmask_interrupt(vector: u32) -> zx_status_t {
    Status::result_into_raw(INTERRUPT_MANAGER.unmask_interrupt(vector))
}

/// Configures the trigger mode and polarity of the specified interrupt vector.
#[unsafe(no_mangle)]
pub extern "C" fn configure_interrupt(
    vector: u32,
    tm: InterruptTriggerMode,
    pol: InterruptPolarity,
) -> zx_status_t {
    Status::result_into_raw(INTERRUPT_MANAGER.configure_interrupt(vector, tm, pol))
}

/// Retrieves the configuration of the specified interrupt vector.
#[unsafe(no_mangle)]
pub extern "C" fn get_interrupt_config(
    vector: u32,
    tm: *mut InterruptTriggerMode,
    pol: *mut InterruptPolarity,
) -> zx_status_t {
    match INTERRUPT_MANAGER.get_interrupt_config(vector) {
        Ok((t, p)) => {
            if !tm.is_null() {
                // SAFETY: `tm` is checked to be non-null and is a valid pointer provided by the caller
                // to write the interrupt trigger mode.
                unsafe {
                    *tm = t;
                }
            }
            if !pol.is_null() {
                // SAFETY: `pol` is checked to be non-null and is a valid pointer provided by the caller
                // to write the interrupt polarity.
                unsafe {
                    *pol = p;
                }
            }
            zx_status::sys::ZX_OK
        }
        Err(e) => e.into_raw(),
    }
}

/// On x64 these methods return the base and max Global System Interrupts as
/// defined by ACPI and described by MADT tables.
/// ACPI Spec 6.1, section 5.2.12 & section 5.2.13.
#[unsafe(no_mangle)]
pub extern "C" fn interrupt_get_base_vector() -> u32 {
    GSI_RANGE.start
}

/// Returns the maximum global IRQ vector.
#[unsafe(no_mangle)]
pub extern "C" fn interrupt_get_max_vector() -> u32 {
    GSI_RANGE.end
}

/// Returns true if the vector is a valid interrupt vector.
#[unsafe(no_mangle)]
pub extern "C" fn is_valid_interrupt(vector: u32, _flags: u32) -> bool {
    apic_io_is_valid_irq(vector)
}

/// Remaps the ISA vector to a global system interrupt vector.
#[unsafe(no_mangle)]
pub extern "C" fn remap_interrupt(vector: u32) -> u32 {
    if vector >= crate::arch_rs::x86::apic::NUM_ISA_IRQS as u32 {
        return vector;
    }
    apic_io_isa_to_global(vector as u8)
}

/// Disables legacy interrupts globally.
#[unsafe(no_mangle)]
pub extern "C" fn shutdown_interrupts() {
    pic_disable();
}

/// Suspends interrupts on the current CPU. Returns Status::NOT_SUPPORTED on PC.
#[unsafe(no_mangle)]
pub extern "C" fn suspend_interrupts_curr_cpu() -> zx_status_t {
    Status::NOT_SUPPORTED.into_raw()
}

/// Resumes interrupts on the current CPU. Returns Status::NOT_SUPPORTED on PC.
#[unsafe(no_mangle)]
pub extern "C" fn resume_interrupts_curr_cpu() -> zx_status_t {
    Status::NOT_SUPPORTED.into_raw()
}

/// Returns true if MSI is supported.
#[unsafe(no_mangle)]
pub extern "C" fn msi_is_supported() -> bool {
    true
}

/// Returns true if MSI masking is supported (always false on PC).
#[unsafe(no_mangle)]
pub extern "C" fn msi_supports_masking() -> bool {
    false
}

/// Masks or unmasks MSI (always panics on PC since masking is unsupported).
#[unsafe(no_mangle)]
pub extern "C" fn msi_mask_unmask(_block: *const MsiBlock, _msi_id: u32, _mask: bool) {
    panic!("MSI masking not supported on x64");
}

/// Allocates an MSI block.
#[unsafe(no_mangle)]
pub extern "C" fn msi_alloc_block(
    requested_irqs: u32,
    can_target_64bit: bool,
    is_msix: bool,
    out_block: *mut MsiBlock,
) -> zx_status_t {
    let out_block = match unsafe { out_block.as_mut() }.ok_or(Status::INVALID_ARGS) {
        Ok(block) => block,
        Err(e) => {
            return e.into_raw();
        }
    };

    if out_block.allocated {
        return Status::INVALID_ARGS.into_raw();
    }
    match INTERRUPT_MANAGER.msi_alloc_block(requested_irqs, can_target_64bit, is_msix) {
        Ok(block) => {
            *out_block = block;
            zx_status::sys::ZX_OK
        }
        Err(e) => e.into_raw(),
    }
}

/// Frees an allocated MSI block.
#[unsafe(no_mangle)]
pub extern "C" fn msi_free_block(block: &mut MsiBlock) {
    INTERRUPT_MANAGER.msi_free_block(block);
}

/// Shutdown interrupts for the calling CPU.
///
/// Should be called before powering off the calling CPU.
#[unsafe(no_mangle)]
pub extern "C" fn shutdown_interrupts_curr_cpu() {
    if crate::arch_rs::x86::feature::x86_hypervisor_has_pv_eoi() {
        let mut msr_access = crate::arch_rs::x86::platform_access::RealMsrAccess {};
        crate::arch_rs::x86::pv::PvEoi::get().disable(&mut msr_access);
    }

    // TODO(maniscalco): Walk interrupt redirection entries and make sure nothing targets this CPU.
}

init::lk_init_hook!(apic, platform_init_apic, init::LkInitLevel(init::LK_INIT_LEVEL_VM.0 + 2));
