// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Pretty printing utilities.

#![no_std]

pub mod hexdump;
pub mod sizes;

pub use hexdump::{
    hexdump_very_ex_raw, hexdump_very_ex_rs, hexdump8_very_ex_raw, hexdump8_very_ex_rs,
};
pub use sizes::{
    FormattedBytes, MAX_FORMAT_SIZE_LEN, SizeUnit, format_size_fixed_rs, format_size_rs,
    parse_size_bytes,
};
