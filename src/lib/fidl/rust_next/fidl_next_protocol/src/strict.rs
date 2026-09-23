// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

/// A strict FIDL response.
#[derive(Clone, Debug)]
pub struct Strict<T>(pub T);

impl<T> Deref for Strict<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> AsRef<T> for Strict<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}
