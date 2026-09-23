// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{AsHandleRef, Handle, HandleBased, HandleRef};

/// Represents a copy of a handle that is owned elsewhere. I.e. dropping this
/// value won't result in the handle being closed.
pub struct Unowned<T: HandleBased>(T);

impl<T: HandleBased> Unowned<T> {
    /// Convert an `Unowned<Handle>` into an `Unowned<T>`
    pub fn from_unowned_handle(src: Unowned<Handle>) -> Unowned<T> {
        Unowned(T::from_handle(Handle { id: src.0.id, client: src.0.client.clone() }))
    }

    /// Get an `Unowned` copy of a given handle.
    pub fn from_handle(src: &Handle) -> Unowned<T> {
        Unowned::<T>::from_unowned_handle(Unowned::<Handle>::from(src))
    }
}

impl<T: HandleBased> Drop for Unowned<T> {
    fn drop(&mut self) {
        self.0.invalidate()
    }
}

impl<T: AsHandleRef + HandleBased> AsHandleRef for Unowned<T> {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.0.as_handle_ref()
    }

    fn object_type() -> fidl::ObjectType {
        T::object_type()
    }
}

impl<T: HandleBased> std::ops::Deref for Unowned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: HandleBased> From<&T> for Unowned<T> {
    fn from(src: &T) -> Unowned<T> {
        let handle = src.as_handle_ref();
        let handle = Handle { id: handle.id, client: handle.client.clone() };
        let inner = T::from_handle(handle);
        Unowned(inner)
    }
}
