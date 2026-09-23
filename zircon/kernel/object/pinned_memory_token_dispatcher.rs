// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use core::mem::MaybeUninit;
use debug::ltrace_entry;
use fbl::{Canary, RefPtr};
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{ZX_OBJ_TYPE_PMT, ZX_RIGHT_INSPECT, zx_rights_t};

use super::KernelHandle;
use super::bus_transaction_initiator_dispatcher::BusTransactionInitiatorDispatcher;
use super::pinned_memory_token_dispatcher_ffi::cpp_pinned_memory_token_dispatcher_create;
use super::pmt::Pmt;
pub use super::pmt::{QueryAddressResult, dev_vaddr_t};
use crate::vm::pinned_vm_object::PinnedVmObject;

use object_constants_rs as object_constants;

const LOCAL_TRACE: u32 = 0;

/// Default rights assigned to a newly created PinnedMemoryTokenDispatcher handle.
pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_INSPECT;

zr::static_assert_size_and_align!(
    PinnedMemoryTokenDispatcherState,
    object_constants::kPinnedMemoryTokenDispatcherStateSize,
    object_constants::kPinnedMemoryTokenDispatcherStateAlign,
);

define_kcounter!(
    DISPATCHER_PINNED_MEMORY_TOKEN_CREATE_COUNT,
    "dispatcher.pinned_memory_token.create",
    Sum
);
define_kcounter!(
    DISPATCHER_PINNED_MEMORY_TOKEN_DESTROY_COUNT,
    "dispatcher.pinned_memory_token.destroy",
    Sum
);

/// Internal state storage for `PinnedMemoryTokenDispatcher`.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct PinnedMemoryTokenDispatcherState {
    /// Magic canary value validating structural integrity.
    canary: Canary<{ fbl::magic(b"PIMT") }>,

    /// Reference to the containing BTI dispatcher.
    bti: RefPtr<BusTransactionInitiatorDispatcher>,

    /// Reference to the underlying IOMMU PMT object.
    #[guarded_by(lock)]
    pmt: Option<RefPtr<Pmt>>,

    /// Mutex guarding state operations.
    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl PinnedMemoryTokenDispatcherState {
    /// Initializes the `PinnedMemoryTokenDispatcherState`.
    pub fn init(
        _dispatcher: *const PinnedMemoryTokenDispatcher,
        bti: RefPtr<BusTransactionInitiatorDispatcher>,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_PINNED_MEMORY_TOKEN_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            bti,
            pmt: None.into(),
            lock <- KMutex::init(),
        })
    }

    /// Stores the PMT reference into state.
    pub fn set_pmt(&self, pmt: RefPtr<Pmt>) {
        ksync::lock!(let mut guard = self.lock_lock());
        *guard.as_mut().fields_mut().pmt = Some(pmt);
    }
}

#[pinned_drop]
impl PinnedDrop for PinnedMemoryTokenDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_PINNED_MEMORY_TOKEN_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    /// Dispatcher representing a token of memory pinned by a Bus Transaction Initiator.
    pub struct PinnedMemoryTokenDispatcher,
    PinnedMemoryTokenDispatcherState,
    ZX_OBJ_TYPE_PMT,
    object_constants::kPinnedMemoryTokenDispatcherStateOffset
);

impl PinnedMemoryTokenDispatcher {
    /// Returns the default rights for a PinnedMemoryTokenDispatcher handle.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new `PinnedMemoryTokenDispatcher` representing pinned pages in `pinned_vmo`.
    ///
    /// Sets the permissions of `pinned_vmo`'s pinned range to `perms` on behalf of `bti`.
    /// `perms` should be flags suitable for the `Iommu::map()` interface.
    pub fn create(
        bti: RefPtr<BusTransactionInitiatorDispatcher>,
        mut pinned_vmo: PinnedVmObject,
        perms: u32,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        ltrace_entry!();
        debug_assert!(page::is_aligned(pinned_vmo.offset() as usize));
        debug_assert!(page::is_aligned(pinned_vmo.size() as usize));
        debug_assert!(pinned_vmo.vmo().is_some());

        // Note: Do not move our BTI reference into the PMT dispatcher we are
        // creating. Instead, give the new PMT dispatcher its own reference. We
        // still need to hold onto our reference for now so that we can access the
        // underlying driver BTI instance to perform the map operation.
        let bti_driver = bti.bti().clone();
        let bti_raw = RefPtr::into_raw(bti);
        let mut handle_out = MaybeUninit::<KernelHandle<Self>>::uninit();
        // SAFETY: `bti_raw` is a valid pointer carrying an acquired refcount, and
        // `handle_out` points to valid uninitialized memory for `KernelHandle<Self>`.
        let status = unsafe {
            cpp_pinned_memory_token_dispatcher_create(bti_raw as *mut _, &raw mut handle_out)
        };
        Status::ok(status)?;
        // SAFETY: `cpp_pinned_memory_token_dispatcher_create` initialized `handle_out`.
        let handle = unsafe { handle_out.assume_init() };

        // TODO(b/502262026): It feels like whether or not we require a contiguous
        // mapping should come as a flag from user mode, not as an expectation based
        // on whether or not the underlying VMO is actually physically contiguous.
        let contiguous_vmo = pinned_vmo.vmo().expect("pinned_vmo must have a vmo").is_contiguous();
        let pmt = bti_driver.map(&mut pinned_vmo, perms, contiguous_vmo)?;

        handle.dispatcher().state().set_pmt(pmt);

        Ok((handle, DEFAULT_RIGHTS))
    }

    /// Unpins and unmaps the memory which was managed by this PMT.
    pub fn unpin(&self) {
        ksync::lock!(let guard = self.state().lock_lock());
        let pmt = guard.fields().pmt.as_ref().expect("PMT must be initialized");
        pmt.release_pinned_memory();
    }

    /// Query the pinned and mapped VMO for a region specified by offset/size.
    pub fn query_address(&self, offset: u64, size: usize) -> Result<QueryAddressResult, Status> {
        ksync::lock!(let guard = self.state().lock_lock());
        let pmt = guard.fields().pmt.as_ref().expect("PMT must be initialized");
        pmt.query_address(offset, size)
    }

    /// Returns the number of bytes pinned by the PMT.
    pub fn size(&self) -> u64 {
        ksync::lock!(let guard = self.state().lock_lock());
        guard.fields().pmt.as_ref().expect("PMT must be initialized").size()
    }

    /// Callback invoked when all handles to this dispatcher are closed.
    pub fn on_zero_handles(&self) {
        ksync::lock!(let guard = self.state().lock_lock());
        // We may not have a driver level PMT if we never fully constructed
        // successfully. In this case, we will be dropping the kernel handle to the
        // PMT Dispatcher without ever having assigned an iommu::Pmt to it.
        if let Some(pmt) = guard.fields().pmt.as_ref() {
            pmt.on_dispatcher_zero_handles();
        }
    }
}
