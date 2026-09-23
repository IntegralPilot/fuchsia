// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Bindings for the core of the fuchsia driver framework C API
#![deny(unsafe_op_in_unsafe_fn, missing_docs)]

pub mod dispatcher;
pub mod handle;
pub mod shutdown_observer;

/// Gets the inner pointer of a dispatcher ref
pub fn dispatcher_ptr<'a>(
    dispatcher: &'a dispatcher::DriverDispatcherRef<'a>,
) -> &'a core::ptr::NonNull<fdf_sys::fdf_dispatcher_t> {
    &dispatcher.0
}

/// Overrides the current dispatcher used by [`dispatcher::CurrentDispatcher::on_dispatcher`] while
/// the callback is being called.
pub fn override_current_dispatcher<R>(
    dispatcher: dispatcher::DriverDispatcherRef<'_>,
    f: impl FnOnce() -> R,
) -> R {
    dispatcher::OVERRIDE_DISPATCHER.with(|global| {
        let previous = global.replace(Some(dispatcher.0));
        let res = f();
        global.replace(previous);
        res
    })
}
