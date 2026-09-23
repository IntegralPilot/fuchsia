// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// A Bluetooth Company ID, which is assigned by the Bluetotoh SIG.
/// Company identifiers are unique numbers assigned by the Bluetooth SIG to
/// member companies requesting one. Referenced in the Bluetooth Core Spec in
/// many commands and formats. See the Assigned Number Document for a reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompanyId(u16);

impl CompanyId {
    pub fn to_le_bytes(&self) -> [u8; 2] {
        self.0.to_le_bytes()
    }
}

impl From<u16> for CompanyId {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<CompanyId> for u16 {
    fn from(value: CompanyId) -> Self {
        value.0
    }
}

impl core::fmt::Display for CompanyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Company (ID {:02x})", self.0)
    }
}
