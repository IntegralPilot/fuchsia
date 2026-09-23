// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{AsAsyncDispatcherRef, AsyncDispatcher, AsyncDispatcherRef, GetAsyncDispatcher};
use libasync_sys::*;
use std::ptr::{NonNull, null};

/// A placeholder for the currently active dispatcher. Use
/// [`GetAsyncDispatcher::get_async_dispatcher`] to access it when needed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CurrentDispatcher;

impl CurrentDispatcher {
    /// Sets the currently active dispatcher for this thread.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the dispatcher they set will be alive until it is [`Self::unset`]
    /// or [`Self::set`] to another value. Using [`Self::with`] is a safer way to use this api as
    /// it will use the current execution stack to ensure that it is properly unset.
    pub unsafe fn set(dispatcher: &impl AsAsyncDispatcherRef) {
        let dispatcher = dispatcher.as_async_dispatcher_ref();
        // SAFETY: The caller promises that the dispatcher will live as long as it is set as the default.
        unsafe { async_set_default_dispatcher(dispatcher.0.as_ptr()) };
    }

    /// Unsets the currently active dispatcher whatever it is.
    pub fn unset() {
        // SAFETY: It is always safe to unset the current dispatcher, as the worst that can happen
        // is the set dispatcher is leaked.
        unsafe { async_set_default_dispatcher(null()) }
    }

    /// Runs the passed function with the current dispatcher set to the one passed in, and then
    /// re-sets the dispatcher to whatever it was before.
    pub fn with<R>(dispatcher: &impl AsAsyncDispatcherRef, f: impl FnOnce() -> R) -> R {
        let old = async_get_default_dispatcher();
        // SAFETY: We will reset the dispatcher below.
        unsafe { Self::set(dispatcher) };
        let ret = f();
        // SAFETY: The dispatcher being set was set prior to entering this function, so something
        // else is responsible for ensuring that it was valid before and after this function is
        // called. We are only returning to the status quo.
        unsafe { async_set_default_dispatcher(old) }
        ret
    }
}

impl GetAsyncDispatcher for CurrentDispatcher {
    fn try_get_async_dispatcher(&self) -> Option<AsyncDispatcher> {
        let dispatcher = NonNull::new(async_get_default_dispatcher().cast_mut())?;
        // SAFETY: NonNull::new will null-check that we have a current dispatcher, and the contract
        // of async-default is that the dispatcher set should be valid.
        Some(AsyncDispatcher::new(&unsafe { AsyncDispatcherRef::from_raw(dispatcher) }))
    }
}
