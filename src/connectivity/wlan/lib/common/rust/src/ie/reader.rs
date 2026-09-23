// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Header, Id, IeType};
use crate::buffer_reader::BufferReader;
use std::mem::size_of;
use std::ops::Range;
use zerocopy::SplitByteSlice;

// TODO(https://fxbug.dev/42164332): Should probably remove Reader in favor of
// IeSummaryIter everywhere.
pub struct Reader<B>(BufferReader<B>);

impl<B: SplitByteSlice> Reader<B> {
    pub fn new(bytes: B) -> Self {
        Reader(BufferReader::new(bytes))
    }
}

impl<B: SplitByteSlice> Iterator for Reader<B> {
    type Item = (Id, B);

    fn next(&mut self) -> Option<Self::Item> {
        let header = self.0.peek::<Header>()?;
        let body_len = header.body_len as usize;
        if self.0.bytes_remaining() < size_of::<Header>() + body_len {
            None
        } else {
            // Unwraps are OK because we checked the length above
            let header = self.0.read::<Header>().unwrap();
            let body = self.0.read_bytes(body_len).unwrap();
            Some((header.id, body))
        }
    }
}

/// An iterator that takes in a chain of IEs and produces summary for each IE.
/// The summary is a tuple consisting of:
/// - The IeType
/// - The range of the rest of the IE:
///   - If the IeType is basic, this range is the IE body
///   - If the IeType is vendor, this range is the IE body without the first six bytes that
///     identify the particular vendor IE
///   - If the IeType is extended, this range is the IE body without the first byte that identifies
///     the extension ID
pub struct IeSummaryIter<B>(BufferReader<B>);

impl<B: SplitByteSlice> IeSummaryIter<B> {
    pub fn new(bytes: B) -> Self {
        Self(BufferReader::new(bytes))
    }
}

impl<B: SplitByteSlice> Iterator for IeSummaryIter<B> {
    type Item = (IeType, Range<usize>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let header = self.0.peek::<Header>()?;
            let body_len = header.body_len as usize;

            // There are not enough bytes left, return None.
            if self.0.bytes_remaining() < size_of::<Header>() + body_len {
                return None;
            }

            // Unwraps are OK because we checked the length above.
            let header = self.0.read::<Header>().unwrap();
            let start_idx = self.0.bytes_read();
            let body = self.0.read_bytes(body_len).unwrap();
            let ie_type = match header.id {
                Id::VENDOR_SPECIFIC => IeType::new_vendor(&body[..]),
                Id::EXTENSION => {
                    if body.len() >= 1 {
                        Some(IeType::new_extended(body[0]))
                    } else {
                        None
                    }
                }
                _ => Some(IeType::new_basic(header.id)),
            };
            // If IE type is valid, return the IE block. Otherwise, skip to the next one.
            match ie_type {
                Some(ie_type) => {
                    return Some((ie_type, start_idx + ie_type.extra_len()..start_idx + body_len));
                }
                None => (),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn empty() {
        assert_eq!(None, Reader::new(&[][..]).next());
    }

    #[test]
    pub fn less_than_header() {
        assert_eq!(None, Reader::new(&[0][..]).next());
    }

    #[test]
    pub fn body_too_short() {
        assert_eq!(None, Reader::new(&[0, 2, 10][..]).next());
    }

    #[test]
    pub fn empty_body() {
        let elems: Vec<_> = Reader::new(&[0, 0][..]).collect();
        assert_eq!(&[(Id::SSID, &[][..])], &elems[..]);
    }

    #[test]
    pub fn two_elements() {
        let bytes = vec![0, 2, 10, 20, 1, 3, 11, 22, 33];
        let elems: Vec<_> = Reader::new(&bytes[..]).collect();
        assert_eq!(
            &[(Id::SSID, &[10, 20][..]), (Id::SUPPORTED_RATES, &[11, 22, 33][..])],
            &elems[..]
        );
    }

    #[test]
    pub fn ie_summary_iter() {
        let bytes = vec![
            0, 2, 10, 20, // IE with no extension ID
            1, 0, // Empty IE
            0xdd, 0x05, 0x50, 0x6f, 0x9a, 0x1c, 0xaa, // Vendor IE with known header
            0xdd, 0x09, 0x00, 0x03, 0x7f, 0x01, 0x01, 0x00, 0x00, 0xff, 0x7f, // Vendor IE
            255, 2, 5, 1, // IE with extension ID
        ];
        let elems: Vec<_> = IeSummaryIter::new(&bytes[..]).collect();
        let expected = &[
            (IeType::new_basic(Id::SSID), 2..4),
            (IeType::new_basic(Id::SUPPORTED_RATES), 6..6),
            (IeType::new_vendor4([0x50, 0x6f, 0x9a, 0x1c]), 12..13),
            (IeType::new_vendor6([0x00, 0x03, 0x7f, 0x01, 0x01, 0x00]), 21..24),
            (IeType::new_extended(5), 27..28),
        ];
        assert_eq!(&elems[..], expected);
    }

    #[test]
    pub fn ie_summary_iter_skip_invalid_ies() {
        let bytes = vec![
            0, 2, 10, 20, // IE with no extension ID
            1, 0, // Empty IE
            0xdd, 0x05, 0x00, 0x03, 0x7f, 0x01, 0x01, // Not long enough for vendor IE
            0xdd, 0x09, 0x00, 0x03, 0x7f, 0x01, 0x01, 0x00, 0x00, 0xff, 0x7f, // Vendor IE
            255, 0, // Not long enough for IE with extension ID
            255, 2, 5, 1, // IE with extension ID
            2, 2, 1, // Not enough trailing bytes
        ];
        let elems: Vec<_> = IeSummaryIter::new(&bytes[..]).collect();
        let expected = &[
            (IeType::new_basic(Id::SSID), 2..4),
            (IeType::new_basic(Id::SUPPORTED_RATES), 6..6),
            (IeType::new_vendor6([0x00, 0x03, 0x7f, 0x01, 0x01, 0x00]), 21..24),
            (IeType::new_extended(5), 29..30),
        ];
        assert_eq!(&elems[..], expected);
    }
}
