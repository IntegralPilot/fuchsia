// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;

/// This structure represents counters for packets and bytes.
///
/// Functional equivalent of [`otsys::otPacketsAndBytes`](crate::otsys::otPacketsAndBytes).
#[derive(Debug, Default, Clone)]
#[repr(transparent)]
pub struct PacketsAndBytes(pub otPacketsAndBytes);

impl_ot_castable!(PacketsAndBytes, otPacketsAndBytes);

impl PacketsAndBytes {
    /// The number of packets.
    pub fn packets(&self) -> u64 {
        self.0.mPackets
    }

    /// The number of bytes.
    pub fn bytes(&self) -> u64 {
        self.0.mBytes
    }
}
