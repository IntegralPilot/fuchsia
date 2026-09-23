// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utility struct for constructing a shutdown observer compatible with rust callbacks.

use fdf_sys::*;

use core::ptr::NonNull;

use crate::dispatcher::{DriverDispatcherRef, ShutdownObserverFn};

/// A shutdown observer for [`fdf_dispatcher_create`] that can call any kind of callback instead of
/// just a C-compatible function when a dispatcher is shutdown.
///
/// # Safety
///
/// This object relies on a specific layout to allow it to be cast between a
/// `*mut fdf_dispatcher_shutdown_observer` and a `*mut ShutdownObserver`. To that end,
/// it is important that this struct stay both `#[repr(C)]` and that `observer` be its first member.
#[repr(C)]
pub struct ShutdownObserver {
    observer: fdf_dispatcher_shutdown_observer,
    shutdown_fn: Box<dyn ShutdownObserverFn>,
}

impl ShutdownObserver {
    /// Creates a new [`ShutdownObserver`] with `f` as the callback to run when a dispatcher
    /// finishes shutting down.
    pub fn new<F: ShutdownObserverFn>(f: F) -> Self {
        let shutdown_fn = Box::new(f);
        Self {
            observer: fdf_dispatcher_shutdown_observer { handler: Some(Self::handler) },
            shutdown_fn,
        }
    }

    /// Turns this object into a stable pointer suitable for passing to [`fdf_dispatcher_create`]
    /// by wrapping it in a [`Box`] and leaking it to be reconstituded by [`Self::handler`] when
    /// the dispatcher is shut down.
    pub fn into_ptr(self) -> *mut fdf_dispatcher_shutdown_observer {
        // Note: this relies on the assumption that `self.observer` is at the beginning of the
        // struct.
        Box::leak(Box::new(self)) as *mut _ as *mut _
    }

    /// The callback that is registered with the dispatcher that will be called when the dispatcher
    /// is shut down.
    ///
    /// # Safety
    ///
    /// This function should only ever be called by the driver runtime at dispatcher shutdown
    /// time, must only ever be called once for any given [`ShutdownObserver`] object, and
    /// that [`ShutdownObserver`] object must have previously been made into a pointer by
    /// [`Self::into_ptr`].
    unsafe extern "C" fn handler(
        dispatcher: *mut fdf_dispatcher_t,
        observer: *mut fdf_dispatcher_shutdown_observer_t,
    ) {
        // SAFETY: The driver framework promises to only call this function once, so we can
        // safely take ownership of the [`Box`] and deallocate it when this function ends.
        let observer = unsafe { Box::from_raw(observer as *mut ShutdownObserver) };
        // SAFETY: `dispatcher` is the dispatcher being shut down, so it can't be non-null.
        let dispatcher_ref =
            unsafe { DriverDispatcherRef::from_raw(NonNull::new_unchecked(dispatcher)) };
        (observer.shutdown_fn)(dispatcher_ref);
        // SAFETY: we only shutdown the dispatcher when the dispatcher is dropped, and we only ever
        // instantiate one owned copy of `Dispatcher` for a given dispatcher.
        unsafe { fdf_dispatcher_destroy(dispatcher) };
    }
}
