// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon event objects.

use crate::{NullableHandle, ok};

/// An object representing a Zircon
/// [event object](https://fuchsia.dev/fuchsia-src/concepts/objects/event.md).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Event(NullableHandle);
impl_handle_based!(Event);

impl Event {
    /// Create an event object, an object which is signalable but nothing else. Wraps the
    /// [zx_event_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/event_create.md)
    /// syscall.
    ///
    /// # Panics
    ///
    /// If the kernel reports no memory available or the process' job policy denies event creation.
    pub fn create() -> Self {
        let mut out = 0;
        let opts = 0;
        let status = unsafe { crate::sys::zx_event_create(opts, &mut out) };
        ok(status)
            .expect("event creation always succeeds except with OOM or when job policy denies it");
        unsafe { Self::from(NullableHandle::from_raw(out)) }
    }
}
