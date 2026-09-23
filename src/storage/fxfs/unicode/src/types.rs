// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
pub use std::ops::Range;

#[derive(Debug)]
#[allow(dead_code)] // This is included in two targets. It is exercised in lib, but not in 'gen'.
pub struct CCCEntry {
    /// The exclusive, lower 16-bits of end of the range for a given entry.
    /// e.g. If 0xfff0..0xffff maps to 222 (say), then this would contain 0xfffe but not 0xffff
    pub low_plane_end_ix: u16,
    /// The size of the range. e.g. For range 2..6, this would be 4.
    pub range_len: u8,
    /// The CCC for this range. Note that we don't store CCC=0.
    pub ccc: u8,
}
impl CCCEntry {
    #[allow(dead_code)]
    pub fn range(&self) -> Range<u16> {
        (self.low_plane_end_ix - self.range_len as u16)..self.low_plane_end_ix
    }
}

#[derive(Debug)]
#[allow(dead_code)] // This is included in two targets. It is exercised in lib, but not in 'gen'.
pub struct MappingOffset {
    /// The exclusive, lower 16-bits of a codepoint in a code plane.
    pub low_plane_ix: u16,
    /// A mapping offset (e.g. into a string table)
    pub offset: u16,
}
