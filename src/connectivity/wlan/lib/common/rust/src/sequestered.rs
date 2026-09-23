// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Data isolation.

// TODO(https://fxbug.dev/42067751): This type should not derive `PartialEq`. Though it only allows trivial
//                         observation (i.e., "is this black box the same as that black box?"),
//                         that is observation nonetheless and client code could foreseeably branch
//                         based on this query. It is implemented to ease the implementation of
//                         tests, where assertions of equality are ubiquitous.
/// Isolates data such that it is inaccessible without explicitly releasing it.
///
/// Sequestered data cannot be directly read nor written while contained (though it can be
/// trivially replaced). This is useful for data that must be "ferried" through a system but should
/// not generally be examined nor inspected, in particular when inspection of the data would
/// otherwise seem innocuous but implicitly violates a design contract or introduces an unwanted
/// data dependency.
///
/// **This type cannot completely prevent reads and writes.** Rather, it makes reads and writes
/// very explicit and more obvious in order to avoid mistakes in data APIs.
///
/// Sequestering data is trivial and is done via the core `From` and `Into` traits. Releasing data
/// is intentionally more explicit and requires the use of fully-qualified syntax that names the
/// `Sequestered` type.
///
/// As sequestered data is considered a "black box", `Sequestered` only implements the `Clone` and
/// `Debug` traits (so long as the type `T` provides implementations). Note that `Copy` is not
/// implemented, because releasing data would not be strictly affine and data could be implicitly
/// copied out of fields via `release`. This is not only implicit, but can be counterintuitive in
/// some contexts, because `release` moves a copy of the `Sequestered`.
///
/// `Sequestered` also implements `PartialEq` largely for testing. See https://fxbug.dev/42067751.
#[derive(Clone, Debug, PartialEq)]
#[repr(transparent)]
pub struct Sequestered<T>(T);

impl<T> Sequestered<T> {
    /// Releases the sequestered data.
    ///
    /// Releasing should be performed sparingly, carefully, and typically at API boundaries where
    /// there is no longer a need to prevent reads of the data. Releases, which are explicit,
    /// should be given extra scrutiny, somewhat like `unsafe` code.
    ///
    /// This function does not use a receiver and so requires fully-qualified syntax in order to
    /// make releases more explicit and obvious.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // Sequester data.
    /// let text: Sequestered<&'static str> = "lorem ipsum".into();
    /// // Release data. The fully-qualified syntax is required.
    /// let text = Sequestered::release(text);
    /// ```
    #[inline(always)]
    pub fn release(sequestered: Self) -> T {
        sequestered.0
    }
}

impl<T> From<T> for Sequestered<T> {
    #[inline(always)]
    fn from(inner: T) -> Self {
        Sequestered(inner)
    }
}
