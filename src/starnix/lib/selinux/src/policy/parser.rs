// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::error::ParseError;

use std::fmt::Debug;
use std::sync::Arc;
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

pub type PolicyData = Arc<[u8]>;
pub type PolicyOffset = u32;

#[derive(Clone, Debug, PartialEq)]
pub struct PolicyCursor<'a> {
    data: &'a PolicyData,
    offset: PolicyOffset,
}

impl<'a> PolicyCursor<'a> {
    /// Returns a new [`PolicyCursor`] that wraps `data` in a [`Cursor`] for parsing.
    pub fn new(data: &'a PolicyData) -> Self {
        Self { data, offset: 0 }
    }

    /// Returns a new [`PolicyCursor`] that wraps `data` in a [`Cursor`] for parsing at `offset`.
    pub fn new_at(data: &'a PolicyData, offset: PolicyOffset) -> Self {
        Self { data, offset }
    }

    /// Returns an `P` as the parsed output of the next bytes in the underlying [`Cursor`] data.
    pub fn parse<P: Clone + Debug + FromBytes + KnownLayout + Immutable + PartialEq + Unaligned>(
        mut self,
    ) -> Result<(P, Self), ParseError> {
        let remaining_slice = &(self.data.as_ref()[self.offset as usize..]);
        let (output, _) =
            P::read_from_prefix(remaining_slice).map_err(|_| ParseError::MissingData {
                type_name: std::any::type_name::<P>(),
                type_size: std::mem::size_of::<P>(),
                num_bytes: self.data.len() - self.offset as usize,
            })?;
        self.offset += std::mem::size_of::<P>() as PolicyOffset;
        Ok((output, self))
    }

    pub fn offset(&self) -> PolicyOffset {
        self.offset
    }

    pub fn data(&self) -> &'a PolicyData {
        &self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocopy::little_endian as le;

    #[derive(Clone, Debug, KnownLayout, FromBytes, Immutable, PartialEq, Unaligned)]
    #[repr(C, packed)]
    struct SomeNumbers {
        a: u8,
        b: le::U32,
        c: le::U16,
        d: u8,
    }

    #[test]
    fn entire_vector() {
        let bytes: Vec<u8> = (0..8).collect();
        let data: PolicyData = Arc::from(bytes);

        let tail = PolicyCursor::new(&data);
        let (some_numbers, tail) = tail.parse::<SomeNumbers>().expect("some numbers");

        assert_eq!(0, some_numbers.a);
        assert_eq!((1 << 0) + (2 << 8) + (3 << 16) + (4 << 24), some_numbers.b.get());
        assert_eq!((5 << 0) + (6 << 8), some_numbers.c.get());
        assert_eq!(7, some_numbers.d);
        assert_eq!(8, tail.offset());
        assert_eq!(8, tail.data().len());
    }

    #[test]
    fn range_within_vector() {
        let bytes: Vec<u8> = (0..40).collect();
        let data: PolicyData = Arc::from(bytes);

        let tail = PolicyCursor::new_at(&data, 8);
        let (first_some_numbers, tail) = tail.parse::<SomeNumbers>().expect("some numbers");
        let (second_some_numbers, tail) = tail.parse::<SomeNumbers>().expect("some numbers");
        let (third_some_numbers, tail) = tail.parse::<SomeNumbers>().expect("some numbers");

        assert_eq!(8, first_some_numbers.a);
        assert_eq!((9 << 0) + (10 << 8) + (11 << 16) + (12 << 24), first_some_numbers.b.get());
        assert_eq!((13 << 0) + (14 << 8), first_some_numbers.c.get());
        assert_eq!(15, first_some_numbers.d);
        assert_eq!(16, second_some_numbers.a);
        assert_eq!((17 << 0) + (18 << 8) + (19 << 16) + (20 << 24), second_some_numbers.b.get());
        assert_eq!((21 << 0) + (22 << 8), second_some_numbers.c.get());
        assert_eq!(23, second_some_numbers.d);
        assert_eq!(24, third_some_numbers.a);
        assert_eq!((25 << 0) + (26 << 8) + (27 << 16) + (28 << 24), third_some_numbers.b.get());
        assert_eq!((29 << 0) + (30 << 8), third_some_numbers.c.get());
        assert_eq!(31, third_some_numbers.d);
        assert_eq!(32, tail.offset());
        assert_eq!(40, tail.data().len());
    }
}
