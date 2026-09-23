// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/pdev/interrupt/interrupt.cc

#[cfg(console_enabled)]
pub mod console;
#[cfg(not(console_enabled))]
use debug as _;

use crate::arch_rs::Iframe;
use crate::kernel::mp::MpIpi;
use crate::kernel::types::cpu_mask_t;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_init::LazyInit;
use pin_init as _;
#[cfg(ktest)]
use unittest as _;
use zx_status::Status;

// NOTE: Keep constants, structures, and layout definitions in sync with the C++ header:
// zircon/kernel/dev/pdev/interrupt/include/pdev/interrupt.h

pub use crate::dev_interrupt::{
    InterruptHandler, InterruptPolarity, InterruptTriggerMode, InterruptVector, MAX_INTERRUPTS,
    MsiBlock,
};

pub struct IntHandlerStruct {
    handler: InterruptHandler,
    permanent: AtomicBool,
}

// SAFETY: Concurrent mutation of `handler` is guarded by
// `PdevInterruptManager`'s lock. Lock-free reads are only performed when
// `permanent` is true, which is set during early boot and remains
// immutable for the remainder of the kernel's lifetime.
unsafe impl Sync for IntHandlerStruct {}

#[ksync::guarded]
pub struct PdevInterruptManager {
    #[guarded_by(lock)]
    table: [IntHandlerStruct; MAX_INTERRUPTS],

    #[mutex]
    lock: ksync::KMutex<ksync::RawSpinlock>,
}

// SAFETY: Internal table synchronization is managed via `lock` for dynamic
// registrations and atomic/immutable invariants for permanent handlers.
unsafe impl Sync for PdevInterruptManager {}

impl PdevInterruptManager {
    /// Returns a reference to an `IntHandlerStruct` slot without acquiring the lock.
    ///
    /// # Safety
    ///
    /// `vector` must be `< MAX_INTERRUPTS`. The caller must only access lock-free atomic fields
    /// (such as `permanent` or immutable `handler` once `permanent` is set to `true`).
    #[inline]
    pub unsafe fn slot_unchecked(&self, vector: usize) -> &IntHandlerStruct {
        let table_ptr = core::ptr::addr_of!(self.table) as *const IntHandlerStruct;
        // SAFETY: `self.table` is `repr(transparent)` around `UnsafeCell<[IntHandlerStruct; MAX_INTERRUPTS]>`.
        // `vector` is guaranteed to be < MAX_INTERRUPTS.
        unsafe { &*table_ptr.add(vector) }
    }
}

static PDEV_INTERRUPTS: LazyInit<PdevInterruptManager> = LazyInit::uninit();

#[repr(C)]
pub struct PdevInterruptOps {
    pub mask: extern "C" fn(vector: InterruptVector) -> Result<(), Status>,
    pub unmask: extern "C" fn(vector: InterruptVector) -> Result<(), Status>,
    pub deactivate: extern "C" fn(vector: InterruptVector) -> Result<(), Status>,
    pub configure: extern "C" fn(
        vector: InterruptVector,
        tm: InterruptTriggerMode,
        pol: InterruptPolarity,
    ) -> Result<(), Status>,
    pub get_config: extern "C" fn(
        vector: InterruptVector,
        tm: *mut InterruptTriggerMode,
        pol: *mut InterruptPolarity,
    ) -> Result<(), Status>,
    pub set_affinity: extern "C" fn(vector: InterruptVector, mask: u32) -> Result<(), Status>,
    pub is_valid: extern "C" fn(vector: InterruptVector, flags: u32) -> bool,
    pub get_base_vector: extern "C" fn() -> InterruptVector,
    pub get_max_vector: extern "C" fn() -> InterruptVector,
    pub remap: extern "C" fn(vector: InterruptVector) -> InterruptVector,
    pub send_ipi: extern "C" fn(target: cpu_mask_t, ipi: MpIpi) -> Result<(), Status>,
    pub init_percpu_early: extern "C" fn(),
    pub init_percpu: extern "C" fn(),
    pub handle_irq: extern "C" fn(frame: *mut Iframe),
    pub shutdown: extern "C" fn(),
    pub shutdown_cpu: extern "C" fn(),
    pub suspend_cpu: extern "C" fn() -> Result<(), Status>,
    pub resume_cpu: extern "C" fn() -> Result<(), Status>,
    pub msi_is_supported: extern "C" fn() -> bool,
    pub msi_supports_masking: extern "C" fn() -> bool,
    pub msi_mask_unmask: extern "C" fn(block: *const MsiBlock, msi_id: u32, mask: bool),
    pub msi_alloc_block: extern "C" fn(
        requested_irqs: u32,
        can_target_64bit: bool,
        is_msix: bool,
        out_block: *mut MsiBlock,
    ) -> Result<(), Status>,
    pub msi_free_block: extern "C" fn(block: *mut MsiBlock),
    pub msi_register_handler:
        extern "C" fn(block: *mut MsiBlock, msi_id: u32, handler: InterruptHandler),
    pub get_status: Option<
        extern "C" fn(
            vector: InterruptVector,
            out_pending: *mut bool,
            out_enabled: *mut bool,
        ) -> Result<(), Status>,
    >,
}

static INTR_OPS: LazyInit<&'static PdevInterruptOps> = LazyInit::uninit();

extern "C" fn default_mask(_: InterruptVector) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_unmask(_: InterruptVector) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_deactivate(_: InterruptVector) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_configure(
    _: InterruptVector,
    _: InterruptTriggerMode,
    _: InterruptPolarity,
) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_get_config(
    _: InterruptVector,
    _: *mut InterruptTriggerMode,
    _: *mut InterruptPolarity,
) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_set_affinity(_: InterruptVector, _: u32) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_is_valid(_: InterruptVector, _: u32) -> bool {
    false
}
extern "C" fn default_get_base_vector() -> InterruptVector {
    InterruptVector(0)
}
extern "C" fn default_get_max_vector() -> InterruptVector {
    InterruptVector(0)
}
extern "C" fn default_remap(_: InterruptVector) -> InterruptVector {
    InterruptVector(0)
}
extern "C" fn default_send_ipi(_: cpu_mask_t, _: MpIpi) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_init_percpu_early() {}
extern "C" fn default_init_percpu() {}
extern "C" fn default_handle_irq(_: *mut Iframe) {}
extern "C" fn default_shutdown() {}
extern "C" fn default_shutdown_cpu() {}
extern "C" fn default_suspend_cpu() -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_resume_cpu() -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_msi_is_supported() -> bool {
    false
}
extern "C" fn default_msi_supports_masking() -> bool {
    false
}
extern "C" fn default_msi_mask_unmask(_: *const MsiBlock, _: u32, _: bool) {}
extern "C" fn default_msi_alloc_block(
    _: u32,
    _: bool,
    _: bool,
    _: *mut MsiBlock,
) -> Result<(), Status> {
    Err(Status::NOT_SUPPORTED)
}
extern "C" fn default_msi_free_block(_: *mut MsiBlock) {}

extern "C" fn default_msi_register_handler(_: *mut MsiBlock, _: u32, _: InterruptHandler) {}

// By default most of these are empty stubs and the particular interrupt controller must override
// all of them.
static DEFAULT_OPS: PdevInterruptOps = PdevInterruptOps {
    mask: default_mask,
    unmask: default_unmask,
    deactivate: default_deactivate,
    configure: default_configure,
    get_config: default_get_config,
    set_affinity: default_set_affinity,
    is_valid: default_is_valid,
    get_base_vector: default_get_base_vector,
    get_max_vector: default_get_max_vector,
    remap: default_remap,
    send_ipi: default_send_ipi,
    init_percpu_early: default_init_percpu_early,
    init_percpu: default_init_percpu,
    handle_irq: default_handle_irq,
    shutdown: default_shutdown,
    shutdown_cpu: default_shutdown_cpu,
    suspend_cpu: default_suspend_cpu,
    resume_cpu: default_resume_cpu,
    msi_is_supported: default_msi_is_supported,
    msi_supports_masking: default_msi_supports_masking,
    msi_mask_unmask: default_msi_mask_unmask,
    msi_alloc_block: default_msi_alloc_block,
    msi_free_block: default_msi_free_block,
    msi_register_handler: default_msi_register_handler,
    get_status: None,
};

/// Registers the platform interrupt operations.
///
/// # Safety
///
/// `ops` must point to a valid, static `PdevInterruptOps` structure that remains
/// valid for the lifetime of the kernel.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdev_register_interrupts(ops: *const PdevInterruptOps) {
    // SAFETY: Called once during boot, serialized with respect to any other access.
    let _ = unsafe {
        Pin::static_ref(&PDEV_INTERRUPTS).init_pin(pin_init::pin_init!(PdevInterruptManager {
            table: [
                const {
                    IntHandlerStruct {
                        handler: InterruptHandler::DEFAULT,
                        permanent: AtomicBool::new(false),
                    }
                };
                MAX_INTERRUPTS
            ].into(),
            lock <- ksync::KSpinlock::init(),
        }))
    };
    // SAFETY: Called once during boot, serialized with respect to any other access.
    unsafe {
        INTR_OPS.init(&*ops);
    }
}

/// Invokes the interrupt handler for the vector if it is present and permanent.
///
/// # Safety
///
/// `vector` must be a valid interrupt vector index.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pdev_invoke_int_if_present(vector: InterruptVector) -> bool {
    // SAFETY: vector is guaranteed by caller to be a valid interrupt vector index (< MAX_INTERRUPTS).
    let slot = unsafe { PDEV_INTERRUPTS.slot_unchecked(vector.0 as usize) };
    // Use a relaxed load as permanent handlers are never modified once set, and they are only set in
    // startup code, so there is nothing to race with.
    if slot.permanent.load(Ordering::Relaxed) {
        // Once permanent is set to true we know that handler is immutable and so it is safe
        // to read without holding the lock.
        slot.handler.invoke();
        return true;
    }

    ksync::lock!(let guard = PDEV_INTERRUPTS.lock_lock());
    let slot = &guard.fields().table[vector.0 as usize];
    if slot.handler.present() {
        slot.handler.invoke();
        true
    } else {
        false
    }
}

/// Registers an interrupt handler for the specified vector.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
unsafe fn register_int_handler_common(
    vector: InterruptVector,
    handler: InterruptHandler,
    permanent: bool,
) -> Result<(), Status> {
    if !(INTR_OPS.is_valid)(vector, 0) {
        return Err(Status::INVALID_ARGS);
    }

    ksync::lock!(let mut guard = PDEV_INTERRUPTS.lock_lock());
    let slot = &mut guard.as_mut().fields_mut().table[vector.0 as usize];
    if (handler.present() && slot.handler.present()) || slot.permanent.load(Ordering::Relaxed) {
        return Err(Status::ALREADY_BOUND);
    }

    slot.handler = handler;
    slot.permanent.store(permanent, Ordering::Relaxed);

    Ok(())
}

/// Registers an interrupt handler for the specified vector.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
#[unsafe(no_mangle)]
unsafe extern "C" fn register_int_handler(
    vector: InterruptVector,
    handler: InterruptHandler,
) -> Result<(), Status> {
    unsafe { register_int_handler_common(vector, handler, false) }
}

/// Registers a permanent interrupt handler for the specified vector.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_permanent_int_handler(
    vector: InterruptVector,
    handler: InterruptHandler,
) -> Result<(), Status> {
    unsafe { register_int_handler_common(vector, handler, true) }
}

/// Checks if an interrupt is registered
///
/// # Safety
///
/// The vector must be within valid range, and HANDLER_TABLE must be initialized.
pub fn is_interrupt_registered(vector: u32) -> bool {
    if vector as usize >= MAX_INTERRUPTS {
        return false;
    }
    ksync::lock!(let guard = PDEV_INTERRUPTS.lock_lock());
    let slot = &guard.fields().table[vector as usize];
    slot.handler.present()
}

/// Queries the status of an interrupt vector.
pub fn query_interrupt_status(vector: u32) -> (Option<bool>, Option<bool>) {
    let ops = *INTR_OPS;
    let mut pending = false;
    let mut enabled = false;
    if let Some(get_status) = ops.get_status
        && get_status(InterruptVector(vector), &mut pending, &mut enabled).is_ok()
    {
        return (Some(pending), Some(enabled));
    }

    (None, None)
}

/// Queries the configuration of an interrupt vector.
pub fn query_interrupt_config(
    vector: u32,
) -> (Option<InterruptTriggerMode>, Option<InterruptPolarity>) {
    let ops = *INTR_OPS;
    let mut tm = InterruptTriggerMode::Edge;
    let mut pol = InterruptPolarity::High;
    if (ops.get_config)(InterruptVector(vector), &mut tm, &mut pol).is_ok() {
        return (Some(tm), Some(pol));
    }

    (None, None)
}

/// Masks the specified interrupt vector.
///
/// # Safety
///
/// The global interrupt ops must be registered, and vector must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mask_interrupt(vector: InterruptVector) -> Result<(), Status> {
    (INTR_OPS.mask)(vector)
}

/// Unmasks the specified interrupt vector.
///
/// # Safety
///
/// The global interrupt ops must be registered, and vector must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unmask_interrupt(vector: InterruptVector) -> Result<(), Status> {
    (INTR_OPS.unmask)(vector)
}

/// Deactivates the specified interrupt vector.
///
/// # Safety
///
/// The global interrupt ops must be registered, and vector must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn deactivate_interrupt(vector: InterruptVector) -> Result<(), Status> {
    (INTR_OPS.deactivate)(vector)
}

/// Configures the specified interrupt vector trigger mode and polarity.
///
/// # Safety
///
/// The global interrupt ops must be registered, and vector must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn configure_interrupt(
    vector: InterruptVector,
    tm: InterruptTriggerMode,
    pol: InterruptPolarity,
) -> Result<(), Status> {
    (INTR_OPS.configure)(vector, tm, pol)
}

/// Gets the specified interrupt vector configuration.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
/// - `tm` and `pol` must be valid, non-null pointers to enum memory.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn get_interrupt_config(
    vector: InterruptVector,
    tm: *mut InterruptTriggerMode,
    pol: *mut InterruptPolarity,
) -> Result<(), Status> {
    (INTR_OPS.get_config)(vector, tm, pol)
}

/// Sets the interrupt affinity mask for the specified vector.
///
/// # Safety
///
/// The global interrupt ops must be registered, and vector must be valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn set_interrupt_affinity(
    vector: InterruptVector,
    mask: u32,
) -> Result<(), Status> {
    (INTR_OPS.set_affinity)(vector, mask)
}

/// Gets the base interrupt vector.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn interrupt_get_base_vector() -> InterruptVector {
    (INTR_OPS.get_base_vector)()
}

/// Gets the maximum interrupt vector.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn interrupt_get_max_vector() -> InterruptVector {
    (INTR_OPS.get_max_vector)()
}

/// Checks if the given interrupt vector is valid.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn is_valid_interrupt(vector: InterruptVector, flags: u32) -> bool {
    (INTR_OPS.is_valid)(vector, flags)
}

/// Remaps the specified interrupt vector.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn remap_interrupt(vector: InterruptVector) -> InterruptVector {
    (INTR_OPS.remap)(vector)
}

/// Sends an IPI to the target CPU.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn interrupt_send_ipi(target: u32, ipi: MpIpi) -> Result<(), Status> {
    (INTR_OPS.send_ipi)(target, ipi)
}

/// Initializes interrupts for the current CPU early in boot.
///
/// # Safety
///
/// The global interrupt ops must be registered.
fn interrupt_init_percpu_early(_level: init::LkInitLevel) {
    (INTR_OPS.init_percpu_early)()
}

/// Initializes interrupts for the current CPU.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn interrupt_init_percpu() {
    (INTR_OPS.init_percpu)()
}

/// Handles a platform IRQ.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
/// - `frame` must point to a valid interrupt frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn platform_irq(frame: *mut Iframe) {
    (INTR_OPS.handle_irq)(frame)
}

/// Shuts down all interrupts.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shutdown_interrupts() {
    (INTR_OPS.shutdown)()
}

/// Shuts down interrupts for the current CPU.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn shutdown_interrupts_curr_cpu() {
    (INTR_OPS.shutdown_cpu)()
}

/// Suspends interrupts for the current CPU.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn suspend_interrupts_curr_cpu() -> Result<(), Status> {
    (INTR_OPS.suspend_cpu)()
}

/// Resumes interrupts for the current CPU.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn resume_interrupts_curr_cpu() -> Result<(), Status> {
    (INTR_OPS.resume_cpu)()
}

/// Checks if MSI is supported.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn msi_is_supported() -> bool {
    (INTR_OPS.msi_is_supported)()
}

/// Checks if MSI supports masking.
///
/// # Safety
///
/// The global interrupt ops must be registered.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn msi_supports_masking() -> bool {
    (INTR_OPS.msi_supports_masking)()
}

/// Masks or unmasks the specified MSI interrupt.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
/// - `block` must point to a valid, initialized `MsiBlock`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn msi_mask_unmask(block: *const MsiBlock, msi_id: u32, mask: bool) {
    (INTR_OPS.msi_mask_unmask)(block, msi_id, mask)
}

/// Allocates a block of MSIs.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
/// - `out_block` must point to a valid, writable `MsiBlock` memory slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn msi_alloc_block(
    requested_irqs: u32,
    can_target_64bit: bool,
    is_msix: bool,
    out_block: *mut MsiBlock,
) -> Result<(), Status> {
    (INTR_OPS.msi_alloc_block)(requested_irqs, can_target_64bit, is_msix, out_block)
}

/// Frees the specified block of MSIs.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
/// - `block` must point to a valid, previously allocated `MsiBlock` that needs to be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn msi_free_block(block: *mut MsiBlock) {
    (INTR_OPS.msi_free_block)(block)
}

/// Registers an interrupt handler for the specified MSI.
///
/// # Safety
///
/// - The global interrupt ops must be registered.
/// - `block` must point to a valid, initialized `MsiBlock`.
/// - `handler` must point to a valid interrupt handler function or be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn msi_register_handler(
    block: *mut MsiBlock,
    msi_id: u32,
    handler: InterruptHandler,
) {
    (INTR_OPS.msi_register_handler)(block, msi_id, handler)
}

// SAFETY: `PdevInterruptOps` is a static dispatch table of function pointers
// whose operations are thread-safe once registered during boot.
unsafe impl Sync for PdevInterruptOps {}

/// PDEV interrupt layer kernel tests.
#[cfg(ktest)]
#[unittest::suite(name = "interrupts")]
mod tests {
    #[allow(unused_imports)]
    use super::*;
    use unittest::{assert_err, assert_false};

    /// Test default ops dispatch table fallback behavior.
    #[test]
    fn test_pdev_default_ops_fallback() {
        assert_err!((DEFAULT_OPS.mask)(InterruptVector(0)), Status::NOT_SUPPORTED);
        assert_err!((DEFAULT_OPS.unmask)(InterruptVector(0)), Status::NOT_SUPPORTED);
    }

    /// Test unregistered interrupt vector state query.
    #[test]
    fn test_pdev_unregistered_interrupt_state() {
        assert_false!(is_interrupt_registered(999));
    }
}

init::lk_init_hook_flags!(
    interrupt_init_percpu_early,
    interrupt_init_percpu_early,
    init::LK_INIT_LEVEL_PLATFORM_EARLY,
    init::LkInitFlags::SecondaryCpus
);
