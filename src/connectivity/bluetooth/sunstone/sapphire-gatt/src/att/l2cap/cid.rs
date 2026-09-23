// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::num::NonZero;

/// L2CAP Channel identifier
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct Cid(NonZero<u16>);

impl Cid {
    /// Constructs a CID from the raw value.
    ///
    /// If the provided `cid` is 0, then this returns None
    pub const fn new(cid: u16) -> Option<Self> {
        match NonZero::new(cid) {
            Some(cid) => Some(Self(cid)),
            None => None,
        }
    }
}

/// Represents a fixed CID as defined by the Bluetooth Core specification.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct FixedCid(Cid);

impl FixedCid {
    /// Returns the underlying CID
    pub const fn cid(&self) -> Cid {
        self.0
    }
}

/// Fixed CIDs as specified in Core Spec v6.0, Vol 3, Part A, Table 2.[1, 2, 3]
impl FixedCid {
    /// CID for the LE Attribute Protocol channel
    pub const ATTRIBUTE_PROTOCOL: Self = Self(Cid::new(0x0004).expect("Fixed value is non-zero"));
}
