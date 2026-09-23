// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Deref;

/// A flexible FIDL response.
#[derive(Clone, Debug)]
pub struct Flexible<T>(pub T);

impl<T> Deref for Flexible<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> AsRef<T> for Flexible<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}
