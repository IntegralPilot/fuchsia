// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Attributes may contain a single value, a list of NLAs, or a list of lists of
//! NLAs. In some of these case, nl80211 does not provide a constant value to
//! use as the NLA `kind`. The standard is instead to fill the field with a
//! 1-based index. `to_nested_nlas` and `to_nested_values` are helpers to easily
//! add these indices.

use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::Nla;
use netlink_packet_utils::Emitable;
use std::mem::size_of_val;

pub(crate) struct NestedNla<'a, T> {
    index: u16,
    nlas: &'a [T],
}

impl<T: Nla> Nla for NestedNla<'_, T> {
    fn value_len(&self) -> usize {
        self.nlas.buffer_len()
    }

    fn kind(&self) -> u16 {
        self.index
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        self.nlas.emit(buffer)
    }
}

pub(crate) fn to_nested_nlas<T>(list: &[Vec<T>]) -> Vec<NestedNla<'_, T>> {
    list.iter().enumerate().map(|(idx, nlas)| NestedNla { index: idx as u16 + 1, nlas }).collect()
}

// We need a separate struct to handle types that are not NLAs, since the upstream crate
// could later define more types (e.g. u8) as NLAs and create a conflict.
// This seems unlikely, but the compiler complains regardless.

pub(crate) struct NestedValue<'a, T> {
    index: u16,
    value: &'a T,
}

impl<T: NlaValue> Nla for NestedValue<'_, T> {
    fn value_len(&self) -> usize {
        self.value.value_len()
    }

    fn kind(&self) -> u16 {
        self.index
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        self.value.emit_value(buffer)
    }
}

pub(crate) fn to_nested_values<T>(list: &[T]) -> Vec<NestedValue<'_, T>> {
    list.iter()
        .enumerate()
        .map(|(idx, value)| NestedValue { index: idx as u16 + 1, value })
        .collect()
}

pub(crate) trait NlaValue {
    fn value_len(&self) -> usize;
    fn emit_value(&self, buffer: &mut [u8]);
}

impl NlaValue for u32 {
    fn value_len(&self) -> usize {
        size_of_val(self)
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        NativeEndian::write_u32(buffer, *self);
    }
}

impl NlaValue for Vec<u8> {
    fn value_len(&self) -> usize {
        self.len()
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        buffer[..self.len()].copy_from_slice(&self[..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestNla(u32);

    impl Nla for TestNla {
        fn value_len(&self) -> usize {
            4
        }

        fn kind(&self) -> u16 {
            0xaabb
        }

        fn emit_value(&self, buffer: &mut [u8]) {
            NativeEndian::write_u32(buffer, self.0);
        }
    }

    #[test]
    fn test_nested_nla() {
        let nlas = vec![vec![TestNla(1)], vec![TestNla(2)]];
        let nested = to_nested_nlas(&nlas);
        let mut buffer = vec![0; nested.as_slice().buffer_len()];
        nested.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            12, 0, // length
            1, 0, // kind (idx)
            8, 0, // length
            0xbb, 0xaa, // kind
            1, 0, 0, 0, // value
            12, 0, // length
            2, 0, // kind (idx)
            8, 0, // length
            0xbb, 0xaa, // kind
            2, 0, 0, 0, // value
        ];
        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn test_nested_u32() {
        let nlas = vec![2, 4, 8];
        let nested = to_nested_values(&nlas);
        let mut buffer = vec![0; nested.as_slice().buffer_len()];
        nested.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            8, 0, // length
            1, 0, // kind (idx)
            2, 0, 0, 0, // entry
            8, 0, // length
            2, 0, // kind (idx)
            4, 0, 0, 0, // entry
            8, 0, // length
            3, 0, // kind (idx)
            8, 0, 0, 0, // entry
        ];
        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn test_nested_vec_u8() {
        let nlas = vec![vec![1, 2, 3]];
        let nested = to_nested_values(&nlas);
        let mut buffer = vec![0; nested.as_slice().buffer_len()];
        nested.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            7, 0, // length
            1, 0, // kind (idx)
            1, 2, 3, // entries
            0, // padding (NLAs are 4-byte aligned)
        ];
        assert_eq!(buffer, expected_buffer);
    }
}
