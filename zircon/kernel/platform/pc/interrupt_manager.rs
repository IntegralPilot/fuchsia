// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::arch_rs::x86::apic::{
    ApicInterruptDeliveryMode, ApicInterruptDstMode, IO_APIC_IRQ_MASK, IO_APIC_IRQ_UNMASK,
};
use crate::arch_rs::x86::interrupts::{
    X86_INT_PLATFORM_BASE, X86_INT_PLATFORM_MAX, X86InterruptVector,
};
use crate::dev_interrupt::{
    InterruptHandler, InterruptPolarity, InterruptTriggerMode, MAX_MSI_IRQS, MsiBlock,
};
use bitmap::{Bitmap, FixedStorage, RawBitmapGeneric};
use core::sync::atomic::{AtomicBool, Ordering};
use ksync::{KMutex, RawSpinlock, guarded, lock};
use pin_init::{PinInit, pin_init, pin_init_array_from_fn};
use zx_status::Status;

/// The maximum number of interrupts in a block.
pub const MAX_IRQ_BLOCK_SIZE: u32 = MAX_MSI_IRQS as u32;

const BITMAP_WORDS: usize = X86InterruptVector::COUNT.div_ceil(usize::BITS as usize);

/// Bitmap representing the allocation status of the 256 interrupt vectors.
pub type Bitmap256 = RawBitmapGeneric<FixedStorage<BITMAP_WORDS>>;

/// Abstract representation of the hardware I/O APIC interface.
pub trait IoApic {
    /// Returns true if the vector is a valid interrupt vector number.
    fn is_valid_interrupt(vector: u32, flags: u32) -> bool;
    /// Fetches the x86 vector currently configured for the global IRQ.
    fn fetch_irq_vector(vector: u32) -> u8;
    /// Configures the routing of the global IRQ to the specified x86 vector.
    fn configure_irq_vector(global_irq: u32, x86_vector: u8);
    /// Configures detailed properties of the global IRQ routing.
    #[allow(clippy::too_many_arguments)]
    fn configure_irq(
        global_irq: u32,
        trig_mode: InterruptTriggerMode,
        polarity: InterruptPolarity,
        del_mode: ApicInterruptDeliveryMode,
        mask: bool,
        dst_mode: ApicInterruptDstMode,
        dst: u8,
        vector: u8,
    );
    /// Masks or unmasks the global IRQ routing.
    fn mask_irq(global_irq: u32, mask: bool);
    /// Fetches the current configuration of the global IRQ.
    fn fetch_irq_config(
        global_irq: u32,
    ) -> Result<(InterruptTriggerMode, InterruptPolarity), Status>;
}

/// Representation of a single entry in the interrupt table, including a
/// lock to ensure a consistent view of the entry.
#[guarded]
pub struct InterruptTableEntry {
    #[mutex]
    mu: KMutex<RawSpinlock>,

    #[guarded_by(mu)]
    handler: InterruptHandler,

    /// Indicates whether the handler is permanent/immutable and cannot be modified.
    permanent: AtomicBool,
}

impl InterruptTableEntry {
    /// Pin-initializes the entry.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            mu <- KMutex::init(),
            handler: ksync::KCell::new(InterruptHandler::DEFAULT),
            permanent: AtomicBool::new(false),
        })
    }

    pub fn is_permanent(&self) -> bool {
        // Permanent handlers do not get modified once set, and are only set on startup, so we can use
        // relaxed loads.
        self.permanent.load(Ordering::Relaxed)
    }

    /// Returns true if the handler was present.  Must be called with
    /// interrupts disabled.
    pub fn invoke_if_present(&self) -> bool {
        if self.is_permanent() {
            // SAFETY: This violates `LockToken::new`'s safety precondition because we are
            // creating a token without actually holding the lock. This is safe here because
            // `permanent` is set to true using Acquire-Release semantics. Once `permanent` is
            // true, the handler becomes completely immutable, cannot be mutated, and will not
            // be freed. Therefore, concurrent reads of `self.handler` are safe without lock acquisition.
            unsafe {
                let token = ksync::LockToken::new();
                // SAFETY: As explained above, the handler is permanent/immutable, so reading it
                // via the token is safe and will not race with any writes.
                debug_assert!(self.handler.get(&token).present());
                self.handler.get(&token).invoke();
            }
            true
        } else {
            lock!(let guard = self.lock_mu_policy::<ksync::NoIrqSavePolicy>());
            let fields = guard.fields();
            if fields.handler.present() {
                fields.handler.invoke();
                true
            } else {
                false
            }
        }
    }

    /// Set the handler for this entry. If |handler| is nullptr,
    /// clear the entry. Makes no change and returns Err(ALREADY_BOUND) if
    /// |handler| is not nullptr and this entry already has a handler assigned.
    pub fn set_handler(
        &self,
        handler: InterruptHandler,
        make_permanent: bool,
    ) -> Result<(), Status> {
        lock!(let mut guard = self.lock_mu());
        let fields = guard.fields_mut();
        if self.is_permanent() {
            return Err(Status::ALREADY_BOUND);
        }

        if handler.present() && fields.handler.present() {
            return Err(Status::ALREADY_BOUND);
        }

        *fields.handler = handler;
        self.permanent.store(make_permanent, Ordering::Relaxed);
        Ok(())
    }

    /// Overwrites the handler unconditionally. Assumes not permanent.
    pub fn overwrite_handler(&self, handler: InterruptHandler) {
        lock!(let mut guard = self.lock_mu());
        let fields = guard.fields_mut();
        debug_assert!(!self.is_permanent());
        *fields.handler = handler;
    }
}

/// The interrupt manager coordinating global IRQ config and CPU vector dispatching.
#[guarded]
pub struct InterruptManager<I: IoApic> {
    /// This lock guards against concurrent access to the IOAPIC and handler allocation bitmap.
    #[mutex]
    mu: KMutex<RawSpinlock>,
    /// Handler table with one entry per CPU interrupt vector.
    #[pin]
    handler_table: [InterruptTableEntry; X86InterruptVector::COUNT],

    /// Bitmap to track what entries in handler_table_ are in use.
    #[guarded_by(mu)]
    handler_allocated: Bitmap256,
    _phantom: core::marker::PhantomData<I>,
}

// TODO: move this
unsafe extern "C" {
    fn apic_bsp_id() -> u8;
}

impl<I: IoApic> InterruptManager<I> {
    /// Returns a pin-initializer for the InterruptManager.
    pub fn new() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            handler_allocated: ksync::KCell::new(Bitmap256::new(bitmap::FixedStorage::new())),
            mu <- KMutex::init(),
            handler_table <- pin_init_array_from_fn(|_i| InterruptTableEntry::init()),
            _phantom: core::marker::PhantomData,
        })
    }

    /// Initializes the allocation bitmap.
    pub fn init(&self) -> Result<(), Status> {
        lock!(let mut inner = self.lock_mu());
        inner.fields_mut().handler_allocated.reset(X86InterruptVector::COUNT)?;
        Ok(())
    }

    #[cfg(ktest)]
    pub fn reset(&self) {
        lock!(let mut inner = self.lock_mu());
        let fields = inner.fields_mut();
        // Fixed storage can never fail.
        let _ = fields.handler_allocated.reset(X86InterruptVector::COUNT);
        for entry in &self.handler_table {
            entry.overwrite_handler(InterruptHandler::DEFAULT);
            entry.permanent.store(false, Ordering::Relaxed);
        }
    }

    /// Masks the specified global interrupt.
    pub fn mask_interrupt(&self, global_irq: u32) -> Result<(), Status> {
        I::mask_irq(global_irq, IO_APIC_IRQ_MASK);
        Ok(())
    }

    /// Unmasks the specified global interrupt.
    pub fn unmask_interrupt(&self, global_irq: u32) -> Result<(), Status> {
        I::mask_irq(global_irq, IO_APIC_IRQ_UNMASK);
        Ok(())
    }

    /// Configures the polarity and trigger mode for the global interrupt.
    pub fn configure_interrupt(
        &self,
        global_irq: u32,
        tm: InterruptTriggerMode,
        pol: InterruptPolarity,
    ) -> Result<(), Status> {
        lock!(self.lock_mu());
        let x86_vector = I::fetch_irq_vector(global_irq);
        if (X86_INT_PLATFORM_BASE.0..=X86_INT_PLATFORM_MAX.0).contains(&x86_vector) {
            return Err(Status::ALREADY_BOUND);
        }
        // SAFETY: `platform_apic_bsp_id` is safe to call at any point after APIC initialization
        // to obtain the bootstrap processor's APIC ID.
        let bsp_id = unsafe { apic_bsp_id() };
        I::configure_irq(
            global_irq,
            tm,
            pol,
            ApicInterruptDeliveryMode::Fixed,
            IO_APIC_IRQ_MASK,
            ApicInterruptDstMode::Physical,
            bsp_id,
            0,
        );
        Ok(())
    }

    /// Gets the polarity and trigger mode for the global interrupt.
    pub fn get_interrupt_config(
        &self,
        global_irq: u32,
    ) -> Result<(InterruptTriggerMode, InterruptPolarity), Status> {
        lock!(self.lock_mu());
        I::fetch_irq_config(global_irq)
    }

    /// Returns true if the handler was present.  Must be called with
    /// interrupts disabled.
    pub fn invoke_x86_vector(&self, x86_vector: u8) -> bool {
        self.handler_table[x86_vector as usize].invoke_if_present()
    }

    /// Register a handler for an external interrupt.
    /// |global_irq| is a "global IRQ" number used by the IOAPIC module.
    ///
    /// If |handler| is nullptr, |arg| is ignored and the specified |vector| has
    /// its current handler removed.
    ///
    /// If |handler| is not nullptr and no handler is currently installed for
    /// |vector|, |handler| will be installed and will be invoked whenever that interrupt fires.
    ///
    /// If |handler| is not nullptr and a handler is already installed, this will
    /// return ZX_ERR_ALREADY_BOUND.
    ///
    /// If no more CPU interrupt vectors are available, returns
    /// ZX_ERR_NO_RESOURCES.
    pub fn register_interrupt_handler(
        &self,
        global_irq: u32,
        handler: InterruptHandler,
        permanent: bool,
    ) -> Result<(), Status> {
        if !I::is_valid_interrupt(global_irq, 0) {
            return Err(Status::INVALID_ARGS);
        }

        lock!(let mut inner = self.lock_mu());
        let fields = inner.fields_mut();

        // Fetch the x86 vector currently configured for this global irq.  Force
        // its value to zero if it is currently invalid.
        let mut x86_vector = I::fetch_irq_vector(global_irq);
        if !(X86_INT_PLATFORM_BASE.0..=X86_INT_PLATFORM_MAX.0).contains(&x86_vector) {
            x86_vector = 0;
        }

        if x86_vector == 0 && !handler.present() {
            return Ok(());
        }

        // If the vector already exists make sure it's not permanent and that we're allowed to modify it.
        if x86_vector != 0 && self.handler_table[x86_vector as usize].is_permanent() {
            return Err(Status::ALREADY_BOUND);
        }

        if x86_vector != 0 && !handler.present() {
            // If the x86 vector is valid, and we are unregistering the handler,
            // return the x86 vector to the pool.
            self.free_handler(fields.handler_allocated, x86_vector as usize, 1);
        } else if x86_vector == 0 && handler.present() {
            // If the x86 vector is invalid, and we are registering a handler,
            // attempt to get a new x86 vector from the pool.
            let range_start = self.alloc_handler(fields.handler_allocated, 1).inspect_err(|_| {
                // Right now, there is not much we can do if the allocation fails.  In
                // debug builds, we ASSERT that everything went well.  In release
                // builds, we log a message and then silently ignore the request to
                // register a new handler.
                debug::tracef!(
                    "Failed to allocate x86 IRQ vector for global IRQ ({}) when registering new handler {:p}\n",
                    global_irq,
                    handler.r#fn.unwrap(),);
            })?;

            debug_assert!(
                (range_start >= X86_INT_PLATFORM_BASE.0 as usize)
                    && (range_start <= X86_INT_PLATFORM_MAX.0 as usize)
            );

            x86_vector = range_start as u8;
        }

        debug_assert!(x86_vector != 0);

        let handler_set = handler.present();

        // Update the handler table and register the x86 vector with the io_apic.
        // On error then RegisterInterruptHandler() was called on the
        // same vector twice to set the handler without clearing the handler
        // in-between.
        self.handler_table[x86_vector as usize].set_handler(handler, permanent)?;
        I::configure_irq_vector(global_irq, if handler_set { x86_vector } else { 0 });
        Ok(())
    }

    /// Allocates an MSI block.
    pub fn msi_alloc_block(
        &self,
        requested_irqs: u32,
        _can_target_64bit: bool,
        _is_msix: bool,
    ) -> Result<MsiBlock, Status> {
        if requested_irqs == 0 || requested_irqs > MAX_MSI_IRQS as u32 {
            return Err(Status::INVALID_ARGS);
        }

        let alloc_size = requested_irqs.next_power_of_two() as usize;

        let vec = {
            lock!(let mut guard = self.lock_mu());
            self.alloc_handler(guard.fields_mut().handler_allocated, alloc_size)?
        };

        // Compute the target address.
        // See section 10.11.1 of the Intel 64 and IA-32 Architectures Software
        // Developer's Manual Volume 3A.
        //
        // TODO(johngro) : don't just bind this block to the Local APIC of the
        // processor which is active when calling msi_alloc_block.  Instead,
        // there should either be a system policy (like, always send to any
        // processor, or just processor 0, or something), or the decision of
        // which CPUs to bind to should be left to the caller.
        let mut tgt_addr = 0xFEE00000; // base addr
        // SAFETY: `platform_apic_bsp_id` is safe to call at any point after APIC initialization
        // to obtain the bootstrap processor's APIC ID.
        let bsp_id = unsafe { apic_bsp_id() }; // Dest ID == the BSP APIC ID
        tgt_addr |= (bsp_id as u64) << 12;
        tgt_addr |= 0x08; // Redir hint == 1
        tgt_addr &= !0x04; // Dest Mode == Physical

        // Compute the target data.
        // See section 10.11.2 of the Intel 64 and IA-32 Architectures Software
        // Developer's Manual Volume 3A.
        //
        // delivery mode == 0 (fixed)
        // trigger mode  == 0 (edge)
        // vector == start of block range
        debug_assert!(vec & !0xFF == 0);
        debug_assert!(vec & (alloc_size - 1) == 0);
        let tgt_data = vec as u32;

        Ok(MsiBlock {
            is_32bit: false,
            base_irq_id: vec as u32,
            num_irq: alloc_size as u32,
            tgt_addr,
            tgt_data,
            allocated: true,
        })
    }

    /// Frees the allocated MSI block.
    pub fn msi_free_block(&self, block: &mut MsiBlock) {
        debug_assert!(block.allocated);
        {
            lock!(let mut inner = self.lock_mu());
            let fields = inner.fields_mut();
            fields
                .handler_allocated
                .clear(block.base_irq_id as usize, (block.base_irq_id + block.num_irq) as usize)
                .expect("clear failed");
        }
        // SAFETY: `MsiBlock` is a plain-data struct with simple fields (u32, u64, bool) and
        // does not contain any references or drop-sensitive types. Zeroing it is safe to reset its state.
        *block = unsafe { core::mem::zeroed() };
    }

    /// Registers the handler for MSI.
    pub fn msi_register_handler(&self, block: &MsiBlock, msi_id: u32, handler: InterruptHandler) {
        debug_assert!(block.allocated);
        debug_assert!(msi_id < block.num_irq);

        let x86_vector = msi_id + block.base_irq_id;
        debug_assert!(
            x86_vector >= X86_INT_PLATFORM_BASE.0 as u32
                && x86_vector <= X86_INT_PLATFORM_MAX.0 as u32
        );

        self.handler_table[x86_vector as usize].overwrite_handler(handler);
    }

    fn free_handler(&self, handler_allocated: &mut Bitmap256, base: usize, count: usize) {
        handler_allocated.clear(base, base + count).expect("clear failed");
    }
    fn alloc_handler(
        &self,
        handler_allocated: &mut Bitmap256,
        count: usize,
    ) -> Result<usize, Status> {
        debug_assert!(count.is_power_of_two());
        // This is the anchor of our search. We always start at the beginning.
        let mut bitoff: usize = X86_INT_PLATFORM_BASE.0 as usize;

        loop {
            // Round the start of our search up to count (which is also our alignment).
            // Find will return an error if bitoff has exceeded the end of the range.
            bitoff = bitoff.next_multiple_of(count);
            // Bail early if we get any kind of error.
            bitoff = handler_allocated.find(
                false,
                bitoff,
                X86_INT_PLATFORM_MAX.0 as usize + 1,
                count,
            )?;
            if bitoff.is_multiple_of(count) {
                break;
            }
        }
        // Loop only exits if we found a valid range.
        handler_allocated.set(bitoff, bitoff + count).expect("Failed to set free range");
        Ok(bitoff)
    }
}

/// PC Interrupt tests.
#[cfg(ktest)]
#[unittest::suite]
#[allow(unused_imports)]
mod pc_interrupt_tests {
    use core::hint;
    use core::pin::Pin;
    use core::sync::atomic::{AtomicU8, Ordering};

    use super::{InterruptManager, IoApic};
    use crate::arch_rs::x86::interrupts::{
        X86_INT_PLATFORM_BASE, X86_INT_PLATFORM_MAX, X86InterruptVector,
    };
    use crate::dev_interrupt::{InterruptHandler, InterruptPolarity, InterruptTriggerMode};
    use lazy_init::LazyInit;
    use unittest::{assert_ok, expect_eq, expect_gt, expect_ne, expect_true, unwrap_ok};
    use zx_status::Status;

    #[derive(Clone, Copy)]
    struct FakeEntry {
        x86_vector: u8,
        trig_mode: InterruptTriggerMode,
        polarity: InterruptPolarity,
    }

    impl Default for FakeEntry {
        fn default() -> Self {
            Self {
                x86_vector: 0,
                trig_mode: InterruptTriggerMode::Edge,
                polarity: InterruptPolarity::High,
            }
        }
    }

    const K_IRQ_COUNT: usize = X86InterruptVector::COUNT + 1;

    static mut FAKE_STATE: [FakeEntry; K_IRQ_COUNT] = [FakeEntry {
        x86_vector: 0,
        trig_mode: InterruptTriggerMode::Edge,
        polarity: InterruptPolarity::High,
    }; K_IRQ_COUNT];

    struct TestIoApic;

    impl IoApic for TestIoApic {
        fn is_valid_interrupt(vector: u32, _flags: u32) -> bool {
            vector < K_IRQ_COUNT as u32
        }
        fn fetch_irq_vector(vector: u32) -> u8 {
            assert!(vector < K_IRQ_COUNT as u32);
            // SAFETY: The fake test state is accessed only on a single test execution thread.
            unsafe { FAKE_STATE[vector as usize].x86_vector }
        }
        fn configure_irq_vector(global_irq: u32, x86_vector: u8) {
            assert!(global_irq < K_IRQ_COUNT as u32);
            // SAFETY: The fake test state is accessed only on a single test execution thread.
            unsafe {
                FAKE_STATE[global_irq as usize].x86_vector = x86_vector;
            }
        }
        fn configure_irq(
            global_irq: u32,
            trig_mode: InterruptTriggerMode,
            polarity: InterruptPolarity,
            _del_mode: ApicInterruptDeliveryMode,
            _mask: bool,
            _dst_mode: ApicInterruptDstMode,
            _dst: u8,
            vector: u8,
        ) {
            assert!(global_irq < K_IRQ_COUNT as u32);
            // SAFETY: The fake test state is accessed only on a single test execution thread.
            unsafe {
                FAKE_STATE[global_irq as usize].x86_vector = vector;
                FAKE_STATE[global_irq as usize].trig_mode = trig_mode;
                FAKE_STATE[global_irq as usize].polarity = polarity;
            }
        }
        fn mask_irq(global_irq: u32, _mask: bool) {
            assert!(global_irq < K_IRQ_COUNT as u32);
        }
        fn fetch_irq_config(
            global_irq: u32,
        ) -> Result<(InterruptTriggerMode, InterruptPolarity), Status> {
            assert!(global_irq < K_IRQ_COUNT as u32);
            // SAFETY: The fake test state is accessed only on a single test execution thread.
            unsafe {
                Ok((
                    FAKE_STATE[global_irq as usize].trig_mode,
                    FAKE_STATE[global_irq as usize].polarity,
                ))
            }
        }
    }

    fn reset_fake_state() {
        // SAFETY: The fake test state is accessed only on a single test execution thread.
        unsafe {
            let ptr = core::ptr::addr_of_mut!(FAKE_STATE) as *mut FakeEntry;
            for i in 0..K_IRQ_COUNT {
                *ptr.add(i) = FakeEntry::default();
            }
        }
    }

    extern "C" fn dummy_handler_fn(_cookie: *mut core::ffi::c_void) {}

    fn create_dummy_handler() -> InterruptHandler {
        InterruptHandler { cookie: core::ptr::null_mut(), r#fn: Some(dummy_handler_fn) }
    }

    static TEST_IM: LazyInit<InterruptManager<TestIoApic>> = LazyInit::uninit();

    fn get_test_im() -> &'static InterruptManager<TestIoApic> {
        #[repr(u8)]
        enum InitState {
            Uninitialized = 0,
            Initializing = 1,
            Initialized = 2,
        }

        static INIT_STATE: AtomicU8 = AtomicU8::new(InitState::Uninitialized as u8);

        match INIT_STATE.compare_exchange(
            InitState::Uninitialized as u8,
            InitState::Initializing as u8,
            Ordering::Acquire,
            // Acquire on failure so that that the write to TEST_IM by the
            // initializing thread is visible to this one.
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // SAFETY: Only the thread that transitioned from Uninitialized executes this.
                unsafe {
                    let _ =
                        Pin::static_ref(&TEST_IM).init_pin(InterruptManager::<TestIoApic>::new());
                }
                INIT_STATE.store(InitState::Initialized as u8, Ordering::Release);
            }
            Err(s) if s == InitState::Initializing as u8 => {
                // Spin until the initializing thread completes.
                while INIT_STATE.load(Ordering::Acquire) != InitState::Initialized as u8 {
                    hint::spin_loop();
                }
            }
            _ => {}
        }

        &TEST_IM
    }

    /// Test registering and unregistering an interrupt handler.
    #[test]
    fn test_register_interrupt_handler() {
        reset_fake_state();
        let im = get_test_im();
        im.reset();
        let _ = im.init();

        let k_irq1 = 1;
        let handler1 = create_dummy_handler();

        // Register
        assert_ok!(im.register_interrupt_handler(k_irq1, handler1, false));
        expect_ne!(TestIoApic::fetch_irq_vector(k_irq1), 0);

        // Unregister
        assert_ok!(im.register_interrupt_handler(k_irq1, InterruptHandler::DEFAULT, false));
        expect_eq!(TestIoApic::fetch_irq_vector(k_irq1), 0);
    }

    /// Test registering an interrupt handler twice.
    #[test]
    fn test_register_interrupt_handler_twice() {
        reset_fake_state();
        let im = get_test_im();
        im.reset();
        let _ = im.init();

        let k_irq = 1;
        let handler1 = create_dummy_handler();
        let handler2 = create_dummy_handler();

        assert_ok!(im.register_interrupt_handler(k_irq, handler1, false));
        let irq_x86_vector = TestIoApic::fetch_irq_vector(k_irq);

        // Register again should fail
        let res = im.register_interrupt_handler(k_irq, handler2, false);
        expect_true!(res == Err(Status::ALREADY_BOUND));
        expect_eq!(irq_x86_vector, TestIoApic::fetch_irq_vector(k_irq));

        // Unregister
        assert_ok!(im.register_interrupt_handler(k_irq, InterruptHandler::DEFAULT, false));
        expect_eq!(TestIoApic::fetch_irq_vector(k_irq), 0);
    }

    /// Test unregistering a handler that was not registered.
    #[test]
    fn test_unregister_interrupt_handler_not_registered() {
        reset_fake_state();
        let im = get_test_im();
        im.reset();
        let _ = im.init();

        let k_irq1 = 1;
        assert_ok!(im.register_interrupt_handler(k_irq1, InterruptHandler::DEFAULT, false));
    }

    /// Test registering too many handlers.
    #[test]
    fn test_register_interrupt_handler_too_many() {
        reset_fake_state();
        let im = get_test_im();
        im.reset();
        let _ = im.init();

        let num_cpu_vectors = X86_INT_PLATFORM_MAX.0 - X86_INT_PLATFORM_BASE.0 + 1;

        for i in 0..num_cpu_vectors {
            let handler = create_dummy_handler();
            assert_ok!(im.register_interrupt_handler(i as u32, handler, false));
        }

        // Try to allocate one more
        let handler = create_dummy_handler();
        let res = im.register_interrupt_handler(num_cpu_vectors as u32, handler, false);
        expect_true!(res == Err(Status::NO_RESOURCES));

        // Clean up
        for i in 0..num_cpu_vectors {
            assert_ok!(im.register_interrupt_handler(i as u32, InterruptHandler::DEFAULT, false));
        }
    }

    /// Test alignment of vector allocation.
    #[test]
    fn test_handler_allocation_alignment() {
        let im = get_test_im();
        im.reset();
        let _ = im.init();

        // Allocation in new IM should succeed and be correctly aligned.
        let mut block = unwrap_ok!(im.msi_alloc_block(32, false, false));
        expect_eq!(block.base_irq_id % 32, 0);
        im.msi_free_block(&mut block);

        let platform_base = X86_INT_PLATFORM_BASE.0 as usize;

        // Set a high bit such that our allocation just won't fit.
        {
            lock!(let mut inner = im.lock_mu());
            let fields = inner.fields_mut();
            fields.handler_allocated.set(platform_base + 31, platform_base + 32).unwrap();
        }
        block = unwrap_ok!(im.msi_alloc_block(32, false, false));
        expect_gt!(block.base_irq_id, (platform_base + 31) as u32);
        expect_eq!(block.base_irq_id % 32, 0);
        im.msi_free_block(&mut block);
        {
            lock!(let mut inner = im.lock_mu());
            let fields = inner.fields_mut();
            fields.handler_allocated.clear(platform_base + 31, platform_base + 32).unwrap();
        }

        // Set a low bit ensuring that allocation happens on the next roundup up block.
        {
            lock!(let mut inner = im.lock_mu());
            let fields = inner.fields_mut();
            fields.handler_allocated.set(platform_base, platform_base + 1).unwrap();
        }
        block = unwrap_ok!(im.msi_alloc_block(32, false, false));
        expect_eq!(block.base_irq_id % 32, 0);
        im.msi_free_block(&mut block);
        {
            lock!(let mut inner = im.lock_mu());
            let fields = inner.fields_mut();
            fields.handler_allocated.clear(platform_base, platform_base + 1).unwrap();
        }

        // Set two bits such that the distance between them is greater than our desired allocation
        // but such that the only valid alignment requires an allocation in an even higher block.
        {
            lock!(let mut inner = im.lock_mu());
            let fields = inner.fields_mut();
            fields.handler_allocated.set(platform_base, platform_base + 1).unwrap();
            fields.handler_allocated.set(platform_base + 34, platform_base + 35).unwrap();
        }
        block = unwrap_ok!(im.msi_alloc_block(32, false, false));
        expect_gt!(block.base_irq_id, (platform_base + 34) as u32);
        expect_eq!(block.base_irq_id % 32, 0);
        im.msi_free_block(&mut block);
        {
            lock!(let mut inner = im.lock_mu());
            let fields = inner.fields_mut();
            fields.handler_allocated.clear(platform_base, platform_base + 1).unwrap();
            fields.handler_allocated.clear(platform_base + 34, platform_base + 35).unwrap();
        }
    }
}
