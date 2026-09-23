// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::user_copy::UserOutPtr;
use fbl::Canary;
use ksync::{KMutex, RawCriticalMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_SAMPLER, ZX_RIGHT_DUPLICATE, ZX_RIGHT_INSPECT, ZX_RIGHT_TRANSFER, zx_rights_t,
    zx_sampler_config_t,
};

use super::KernelHandle;
use super::sampler_dispatcher_ffi::cpp_sampler_dispatcher_create;

use object_constants_rs as object_constants;

/// Default rights assigned to a newly created SamplerDispatcher handle.
pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE | ZX_RIGHT_INSPECT;

zr::static_assert_size_and_align!(
    SamplerDispatcherState,
    object_constants::kSamplerDispatcherStateSize,
    object_constants::kSamplerDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_SAMPLER_CREATE_COUNT, "dispatcher.sampler.create", Sum);
define_kcounter!(DISPATCHER_SAMPLER_DESTROY_COUNT, "dispatcher.sampler.destroy", Sum);

/// Internal state storage for `SamplerDispatcher`.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct SamplerDispatcherState {
    canary: Canary<{ fbl::magic(b"SAMP") }>,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl SamplerDispatcherState {
    /// Initializes the `SamplerDispatcherState`.
    pub fn init(
        _dispatcher: *const SamplerDispatcher,
    ) -> impl PinInit<Self, core::convert::Infallible> {
        DISPATCHER_SAMPLER_CREATE_COUNT.add(1);
        pin_init!(Self {
            canary: Canary::new(),
            lock <- KMutex::init(),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for SamplerDispatcherState {
    fn drop(self: core::pin::Pin<&mut Self>) {
        DISPATCHER_SAMPLER_DESTROY_COUNT.add(1);
    }
}

crate::object::dispatcher::impl_dispatcher_facade_with_state!(
    pub struct SamplerDispatcher,
    SamplerDispatcherState,
    ZX_OBJ_TYPE_SAMPLER,
    object_constants::kSamplerDispatcherStateOffset
);

impl SamplerDispatcher {
    /// Returns the default rights for a SamplerDispatcher handle.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new `SamplerDispatcher` with the given configuration.
    pub fn create(
        config: &zx_sampler_config_t,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let rights = Self::default_rights();
        // SAFETY: `config` is a valid reference and `cpp_sampler_dispatcher_create` initializes
        // `handle` on success.
        let handle =
            unsafe { KernelHandle::create(|out| cpp_sampler_dispatcher_create(config, out)) }?;
        Ok((handle, rights))
    }

    /// Starts sampling session for this sampler.
    pub fn start(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid SamplerDispatcher pointer.
        let status = unsafe {
            super::sampler_dispatcher_ffi::cpp_sampler_dispatcher_start(self as *const _)
        };
        Status::ok(status)
    }

    /// Stops sampling session for this sampler.
    pub fn stop(&self) -> Result<(), Status> {
        // SAFETY: `self` is a valid SamplerDispatcher pointer.
        let status =
            unsafe { super::sampler_dispatcher_ffi::cpp_sampler_dispatcher_stop(self as *const _) };
        Status::ok(status)
    }

    /// Read out the data contained in the sampler buffers into `ptr` return the number of bytes
    /// written. The Sampling state must be Stopped before calling this function.
    ///
    /// `len` _must_ be at least equal to the total size of the sampler buffers, which can be
    /// queried by passing a null `ptr`. In this case, no data will be written and the return value
    /// will be the required minimum size of the buffer to write to.
    pub fn read_user(&self, ptr: UserOutPtr<u8>, len: usize) -> (Result<(), Status>, usize) {
        let mut actual = 0;
        // SAFETY: `self` is valid, `ptr` is a valid UserOutPtr buffer of `len` bytes, and
        // `actual` is a local out pointer.
        let status = unsafe {
            super::sampler_dispatcher_ffi::cpp_sampler_dispatcher_read_user(
                self as *const _,
                ptr.as_ptr().cast(),
                len,
                &mut actual,
            )
        };
        (Status::ok(status), actual)
    }
}
