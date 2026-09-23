// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use fbl::{Canary, RefPtr};
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_MSI, ZX_RIGHT_DUPLICATE, ZX_RIGHT_INSPECT, ZX_RIGHT_TRANSFER, zx_rights_t,
};

use super::KernelHandle;
use super::msi_allocation::MsiAllocation;
use super::msi_dispatcher_ffi::cpp_msi_dispatcher_create;

use object_constants_rs as object_constants;

/// Default rights assigned to an MsiDispatcher handle.
pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_INSPECT;

zr::static_assert_size_and_align!(
    MsiDispatcherState,
    object_constants::kMsiDispatcherStateSize,
    object_constants::kMsiDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_MSI_CREATE_COUNT, "dispatcher.msi.create", Sum);
define_kcounter!(DISPATCHER_MSI_DESTROY_COUNT, "dispatcher.msi.destroy", Sum);

/// Internal state storage for `MsiDispatcher`.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct MsiDispatcherState {
    /// Magic canary value validating structural integrity.
    canary: Canary<{ fbl::magic(b"MSIA") }>,

    /// Reference to the underlying C++ `MsiAllocation` object (not protected by lock).
    msi_alloc: RefPtr<MsiAllocation>,

    /// Mutex guarding state operations.
    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl MsiDispatcherState {
    /// Initializes the `MsiDispatcherState`.
    pub fn init(
        _dispatcher: *const MsiDispatcher,
        msi_alloc: RefPtr<MsiAllocation>,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_MSI_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            msi_alloc,
            lock <- KMutex::init(),
        })
    }

    /// Returns a reference to the underlying `MsiAllocation` ref pointer.
    pub fn msi_allocation(&self) -> &RefPtr<MsiAllocation> {
        &self.msi_alloc
    }
}

#[pinned_drop]
impl PinnedDrop for MsiDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_MSI_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct MsiDispatcher,
    MsiDispatcherState,
    ZX_OBJ_TYPE_MSI,
    object_constants::kMsiDispatcherStateOffset
);

impl MsiDispatcher {
    /// Returns default rights for an MsiDispatcher handle.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new MsiDispatcher wrapping an MsiAllocation and returns its handle and rights.
    pub fn create(
        msi_alloc: RefPtr<MsiAllocation>,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // SAFETY: `msi_alloc` is transferred to C++ (`cpp_msi_dispatcher_create` takes ownership
        // of the raw pointer), and `cpp_msi_dispatcher_create` initializes `handle` on success.
        let handle = unsafe {
            KernelHandle::create(|out| {
                cpp_msi_dispatcher_create(RefPtr::into_raw(msi_alloc).cast_mut(), out)
            })
        }?;
        Ok((handle, DEFAULT_RIGHTS))
    }

    /// Returns `zx_info_msi_t` for this MSI dispatcher.
    pub fn get_info(&self) -> zx_types::zx_info_msi_t {
        self.msi_allocation().get_info()
    }

    /// Returns a reference to the underlying `MsiAllocation` ref pointer.
    pub fn msi_allocation(&self) -> &RefPtr<MsiAllocation> {
        self.state().msi_allocation()
    }
}

#[cfg(ktest)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_default_rights() {
        assert_eq!(
            MsiDispatcher::default_rights(),
            ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_INSPECT
        );
    }
}
