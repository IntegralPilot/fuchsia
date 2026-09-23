// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common patterns for runtime configurable settings in netstack.

use core::ops::Deref;

/// Abstracts access to stack settings.
///
/// This trait is implemented by *bindings* to allow core access to
/// runtime-configurable behavior for the network stack.
///
/// Separate modules declare their own `T` for settings structures that can be
/// retrieved via this trait.
pub trait SettingsContext<T> {
    /// Borrows the current settings values.
    fn settings(&self) -> impl Deref<Target = T> + '_;
}

/// Common settings applied to buffer size configuration.
///
/// This type is a witness for a valid configuration where `min <= default <=
/// max`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BufferSizeSettings<T> {
    default: T,
    max: T,
    min: T,
}

impl<T: Copy + Ord> BufferSizeSettings<T> {
    /// Creates a new value with provided `min`, `default`, `max` sizes.
    ///
    /// Returns `None` if `min <= default <= max` is violated.
    pub fn new(min: T, default: T, max: T) -> Option<Self> {
        if min > default || max < default {
            return None;
        }
        Some(Self { default, max, min })
    }

    /// Returns the clamped value `val` to the configured maximum and minimum
    /// buffer sizes.
    pub fn clamp(&self, val: T) -> T {
        val.clamp(self.min, self.max)
    }
}

impl<T: Copy> BufferSizeSettings<T> {
    /// Returns the configured default buffer size.
    pub fn default(&self) -> T {
        self.default
    }

    /// Returns the configured maximum buffer size.
    pub fn max(&self) -> T {
        self.max
    }

    /// Returns the configured minimum buffer size.
    pub fn min(&self) -> T {
        self.min
    }

    /// Returns a tuple of the minimum and maximum buffer size, in this order.
    pub fn min_max(&self) -> (T, T) {
        (self.min, self.max)
    }
}

#[cfg(any(test, feature = "testutils"))]
pub(crate) mod testutil {
    use super::*;

    /// A marker trait that allows a type to implement [`SettingsContext`] that
    /// always yields a default value for the settings type.
    pub trait AlwaysDefaultsSettingsContext {}

    impl<T, O> SettingsContext<T> for O
    where
        T: Default + 'static,
        O: AlwaysDefaultsSettingsContext,
    {
        fn settings(&self) -> impl Deref<Target = T> + '_ {
            DefaultSettings(T::default())
        }
    }

    struct DefaultSettings<T>(T);

    impl<T> Deref for DefaultSettings<T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            let Self(s) = self;
            s
        }
    }
}
