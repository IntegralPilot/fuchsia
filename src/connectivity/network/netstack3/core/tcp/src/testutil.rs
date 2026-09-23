// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ops::Range;

use netstack3_base::{SackBlock, SackBlocks, SeqNum};

/// Creates a [`SackBlocks`] from the sequence number ranges represented as
/// `u32`s.
pub(crate) fn sack_blocks(iter: impl IntoIterator<Item = Range<u32>>) -> SackBlocks {
    iter.into_iter()
        .map(|Range { start, end }| {
            SackBlock::try_new(SeqNum::new(start), SeqNum::new(end)).unwrap()
        })
        .collect()
}
