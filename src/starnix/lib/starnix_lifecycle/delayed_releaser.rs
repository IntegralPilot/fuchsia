// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_types::ownership::ReleaseGuard;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::ops::Deref;

pub trait ReleaserAction<T> {
    fn release(t: ReleaseGuard<T>);
}

/// Wrapper around `FileObject` that ensures that a unused `FileObject` is added to the current
/// delayed releasers to be released at the next release point.
pub struct ObjectReleaser<T, F: ReleaserAction<T>>(ManuallyDrop<ReleaseGuard<T>>, PhantomData<F>);

impl<T: Default, F: ReleaserAction<T>> Default for ObjectReleaser<T, F> {
    fn default() -> Self {
        Self::from(T::default())
    }
}

impl<T, F: ReleaserAction<T>> From<T> for ObjectReleaser<T, F> {
    fn from(object: T) -> Self {
        Self(ManuallyDrop::new(object.into()), Default::default())
    }
}

impl<T, F: ReleaserAction<T>> Drop for ObjectReleaser<T, F> {
    fn drop(&mut self) {
        // SAFETY
        // The `ManuallyDrop` is only ever extracted in this `drop` method, so it is guaranteed
        // that it still exists.
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let object = unsafe { ManuallyDrop::take(&mut self.0) };
        F::release(object);
    }
}

impl<T: std::fmt::Debug, F: ReleaserAction<T>> std::fmt::Debug for ObjectReleaser<T, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.deref().fmt(f)
    }
}

impl<T, F: ReleaserAction<T>> std::ops::Deref for ObjectReleaser<T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl<T, F: ReleaserAction<T>> std::borrow::Borrow<T> for ObjectReleaser<T, F> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<T, F: ReleaserAction<T>> std::convert::AsRef<T> for ObjectReleaser<T, F> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}
