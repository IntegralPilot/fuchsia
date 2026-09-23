// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers for writing futures against the C-based async API

use std::sync::Arc;

/// Implements the basic pattern of a two-sided struct used by a system api
/// that calls a callback (which uses the `C` generic) and a rust abstraction
/// over that API to provide either rust callbacks or futures, where the two
/// sides need to coordinate.
///
/// # Safety
///
/// This object relies on its layout being such that a pointer to its first
/// member is also a pointer to the struct as a whole, so that references to
/// it can be safely converted back and forth.
#[repr(C)]
pub struct CallbackSharedState<Base, Inner> {
    /// The "base" type is the one that the C api expects to be at the beginning
    /// of the struct it's given.
    base: Base,
    /// The "inner" type is the one that is used by the Rust wrappers to manage
    /// state and resources.
    inner: Inner,
}

impl<B, I> CallbackSharedState<B, I> {
    /// Creates a new shared state object in an [`Arc`] that can be easily manipulated
    /// into the pointer types needed for interaction with a C API.
    ///
    /// After calling this, the `base` value will be the one given to the C API and
    /// will not be available to rust code without going through the pointer. The
    /// idea is that while this object is alive, the C code owns that data. The `inner`
    /// object can be accessed on this through the implementation of [`std::ops::Deref`].
    pub fn new(base: B, inner: I) -> Arc<Self> {
        Arc::new(Self { base, inner })
    }

    /// Transforms this reference to the CallbackSharedState into a pointer to the first
    /// element of the struct that can be passed to the C API. Every call to
    /// [`Self::make_raw_ptr`] must be paired with a corresponding call to either
    /// [`Self::from_raw_ptr`] or [`Self::release_raw_ptr`] or the object will leak.
    ///
    /// Note that this returns a mutable pointer because that's usually what the
    /// C API wants. The expectation is that the C API can have mutable access
    /// to its own state object, but once we've done this we will not access
    /// it from rust anymore until/unless we've reclaimed ownership of the
    /// struct somehow.
    pub fn make_raw_ptr(this: Arc<Self>) -> *mut B {
        Arc::into_raw(this) as *mut B
    }

    /// Gets a raw pointer to the first element of this struct that can be passed to a C
    /// API without affecting the reference count of the underlying [`Arc`].
    ///
    /// Note that this returns a mutable pointer because that's usually what the
    /// C API wants. The expectation is that the C API can have mutable access
    /// to its own state object, but once we've done this we will not access
    /// it from rust anymore until/unless we've reclaimed ownership of the
    /// struct somehow.
    pub fn as_raw_ptr(this: &Arc<Self>) -> *mut B {
        Arc::as_ptr(this) as *mut B
    }

    /// Converts the given pointer to the base type back to a fully owned
    /// [`Arc`] to the [`CallbackSharedState`].
    ///
    /// This should be used when reclaiming
    /// the state object after the callback has been called or synchronously
    /// cancelled.
    ///
    /// # Safety
    ///
    /// This must only ever be called up to once for every [`Self::make_raw_ptr`]
    /// that has been called. See the safety comments for [`Arc::from_raw`] for
    /// more information.
    pub unsafe fn from_raw_ptr(this: *mut B) -> Arc<Self> {
        // SAFETY: The caller promises that this is a balanced use of this function.
        unsafe { Arc::from_raw(this as *const Self) }
    }

    /// Releases the given pointer to the base type by decrementing the
    /// reference count on the original Arc.
    ///
    /// This should be used when the callback has been called or synchronously
    /// cancelled, but there's no need to access the base data.
    ///
    /// # Safety
    ///
    /// This must only ever be called up to once for every [`Self::make_raw_ptr`]
    /// that has been called. See the safety comments for [`Arc::decrement_strong_count`] for
    /// more information.
    pub unsafe fn release_raw_ptr(this: *mut B) {
        // SAFETY: The caller promises that this is a balanced use of this function.
        unsafe { Arc::decrement_strong_count(this as *const Self) }
    }
}

impl<B, I> std::ops::Deref for CallbackSharedState<B, I> {
    type Target = I;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
