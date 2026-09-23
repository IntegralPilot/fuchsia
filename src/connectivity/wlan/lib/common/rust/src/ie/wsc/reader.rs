// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{AttributeHeader, Id};
use crate::buffer_reader::BufferReader;
use std::mem::size_of;
use zerocopy::SplitByteSlice;

pub struct Reader<B>(BufferReader<B>);

impl<B: SplitByteSlice> Reader<B> {
    pub fn new(bytes: B) -> Self {
        Reader(BufferReader::new(bytes))
    }
}

impl<B: SplitByteSlice> Iterator for Reader<B> {
    type Item = (Id, B);

    fn next(&mut self) -> Option<Self::Item> {
        let header = self.0.peek::<AttributeHeader>()?;
        let body_len = header.body_len.get() as usize;
        if self.0.bytes_remaining() < size_of::<AttributeHeader>() + body_len {
            None
        } else {
            // Unwraps are OK because we checked the length above
            let header = self.0.read::<AttributeHeader>().unwrap();
            let body = self.0.read_bytes(body_len).unwrap();
            Some((header.id, body))
        }
    }
}
