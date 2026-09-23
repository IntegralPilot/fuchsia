// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;

/// An optimization hint about whether the conversion from `T` to `U` is equivalent to copying the
/// raw bytes of `T`.
pub struct CopyOptimization<T: ?Sized, U: ?Sized>(bool, PhantomData<(*mut T, *mut U)>);

impl<T: ?Sized, U: ?Sized> CopyOptimization<T, U> {
    /// Returns a `CopyOptimization` hint with the optimization enabled.
    ///
    /// # Safety
    ///
    /// `T` and `U` must be the same size and must not have any uninit bytes (e.g. padding).
    pub const unsafe fn enable() -> Self {
        Self(true, PhantomData)
    }

    /// Returns a `CopyOptimization` hint with the optimization enabled if `value` is `true`.
    ///
    /// # Safety
    ///
    /// `T` and `U` must be the same size and must not have any uninit bytes (e.g. padding) if
    /// `value` is `true`.
    pub const unsafe fn enable_if(value: bool) -> Self {
        Self(value, PhantomData)
    }

    /// Returns a `CopyOptimization` hint with the optimization disabled.
    pub const fn disable() -> Self {
        Self(false, PhantomData)
    }

    /// Returns whether the optimization is enabled.
    pub const fn is_enabled(&self) -> bool {
        self.0
    }

    /// Infers whether the conversion from `[T; N]` to `[U; N]` is copy-optimizable based on the
    /// conversion from `T` to `U`.
    pub const fn infer_array<const N: usize>(&self) -> CopyOptimization<[T; N], [U; N]>
    where
        T: Sized,
        U: Sized,
    {
        // SAFETY: If `T` and `U` are copy-optimizable, then `[T; N]` and `[U; N]` are also
        // copy-optimizable.
        unsafe { CopyOptimization::enable_if(self.is_enabled()) }
    }

    /// Infers whether the conversion from `[T]` to `[U]` is copy-optimizable based on the
    /// conversion from `T` to `U`.
    pub const fn infer_slice(&self) -> CopyOptimization<[T], [U]>
    where
        T: Sized,
        U: Sized,
    {
        // SAFETY: If `T` and `U` are copy-optimizable, then `[T]` and `[U]` are also
        // copy-optimizable.
        unsafe { CopyOptimization::enable_if(self.is_enabled()) }
    }
}

impl<T: ?Sized> CopyOptimization<T, T> {
    /// Returns an enabled `CopyOptimization`, as copy optimization is always enabled from a type to
    /// itself.
    pub const fn identity() -> Self {
        // SAFETY: A type is always copy-optimizable to itself.
        unsafe { Self::enable() }
    }
}
