// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for mapping deref guards like mutex or RCU guards.

use core::marker::PhantomData;
use core::ops::Deref;

/// A helper type that provides mapping `Deref` implementations.
pub struct MapDeref<S, T, F> {
    storage: S,
    map: F,
    _marker: PhantomData<T>,
}

impl<S, T, F> MapDeref<S, T, F> {
    /// Retrieves the inner storage, undoing any mapping.
    pub fn into_inner(self) -> S {
        self.storage
    }
}

impl<S, T, F> Deref for MapDeref<S, T, F>
where
    S: Deref,
    F: Fn(&S::Target) -> &T,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        let Self { storage, map, _marker } = self;
        map(&*storage)
    }
}

/// A trait providing an easy way to instantiate [`MapDeref`].
pub trait MapDerefExt: Deref + Sized {
    /// Maps this value from the current `Deref::Target` to `T`.
    fn map_deref<F: Fn(&Self::Target) -> &T, T>(self, f: F) -> MapDeref<Self, T, F>;
}

impl<S> MapDerefExt for S
where
    S: Deref + Sized,
{
    fn map_deref<F: Fn(&Self::Target) -> &T, T>(self, map: F) -> MapDeref<Self, T, F> {
        MapDeref { storage: self, map, _marker: PhantomData::<T>::default() }
    }
}
