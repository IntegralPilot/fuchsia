// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// The `Merge` trait allows merging two structs.
pub trait Merge<Other = Self> {
    /// Returns a copy of the original struct where the values of all fields set in `other`
    /// replace the matching fields in the copy of `self`.
    fn merge(&self, other: Other) -> Self;
}
