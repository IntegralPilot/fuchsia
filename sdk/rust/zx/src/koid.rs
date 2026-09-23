// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::sys::zx_koid_t;

/// The unique id assigned by kernel to the object referenced by a handle.
///
/// # Layout
///
/// This type is guaranteed to have the same layout and bit patterns as `zx_koid_t`.
#[derive(
    Debug,
    Copy,
    Clone,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Hash,
    zerocopy::FromBytes,
    zerocopy::KnownLayout,
    zerocopy::Immutable,
)]
#[repr(transparent)]
pub struct Koid(zx_koid_t);

impl Koid {
    pub const fn from_raw(raw: zx_koid_t) -> Koid {
        Koid(raw)
    }

    pub const fn raw_koid(&self) -> zx_koid_t {
        self.0
    }
}
