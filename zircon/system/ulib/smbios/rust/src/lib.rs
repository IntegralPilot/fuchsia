// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe Rust interface for parsing SMBIOS tables and structures, matching `smbios.h` / `smbios.cc`.

#![no_std]

mod entry_point;
mod string_table;
mod structures;
#[cfg(test)]
mod tests;

pub use entry_point::{
    EntryPoint, EntryPoint2_1, EntryPoint3_0, EntryPointType, EntryPointVersion, SMBIOS2_ANCHOR,
    SMBIOS2_INTERMEDIATE_ANCHOR, SMBIOS3_ANCHOR, SpecVersion, compute_checksum,
};
pub use string_table::StringTable;
pub use structures::{
    BaseboardInformationStruct, BiosInformationStruct2_0, BiosInformationStruct2_4, Header,
    StructType, SystemInformationStruct2_0, SystemInformationStruct2_1, SystemInformationStruct2_4,
};
