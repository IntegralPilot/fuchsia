// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::channel::oneshot::Sender;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicPtr, Ordering};

pub struct AtomicBox<T>(AtomicPtr<T>);

impl<T> AtomicBox<T> {
    pub fn new(x: T) -> Self {
        Self(AtomicPtr::new(Box::into_raw(Box::new(x))))
    }

    pub fn take(&self) -> Option<Box<T>> {
        let ptr = self.0.swap(null_mut(), Ordering::SeqCst);
        (!ptr.is_null()).then(|| {
            // SAFETY: This pointer is non-null, AtomicPtr::swap ensures it's the only copy,
            // and it must have been allocated by the global allocator.
            unsafe { Box::from_raw(ptr) }
        })
    }
}

impl<T> AtomicBox<Sender<T>> {
    pub fn send(&self, t: T) -> Option<Result<(), T>> {
        self.take().map(|s| s.send(t))
    }
}

impl<T> Drop for AtomicBox<T> {
    fn drop(&mut self) {
        self.take();
    }
}
