// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;

use selinux_policy_derive::{Parse, Serialize, Validate};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned, little_endian as le};

use super::NewPolicy;
use super::error::{ParseError, SerializeError, ValidateError};
use super::metadata::PolicyVersion;
use super::traits::{Parse, Serialize, Validate};

/// Cursor used to parse elements from the binary policy data.
#[derive(Debug)]
pub struct PolicyCursor<'a> {
    data: &'a [u8],
    offset: usize,
    policy_version: PolicyVersion,
    types_count: u32,
}

impl<'a> PolicyCursor<'a> {
    /// Creates a new [`PolicyCursor`] wrapping the supplied `data`.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0, policy_version: PolicyVersion::V30, types_count: 0 }
    }

    /// Sets the SELinux policy database version on this cursor.
    pub fn set_policy_version(&mut self, version: PolicyVersion) {
        self.policy_version = version;
    }

    /// Returns the SELinux policy database version recorded on this cursor.
    pub fn policy_version(&self) -> PolicyVersion {
        self.policy_version
    }

    /// Sets the number of types recorded on this cursor.
    pub fn set_types_count(&mut self, count: u32) {
        self.types_count = count;
    }

    /// Returns the number of types recorded on this cursor.
    pub fn types_count(&self) -> u32 {
        self.types_count
    }

    /// Returns the current offset of the cursor.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Parses a type `T` that implements [`Parse`] from the cursor.
    pub fn parse<T: Parse>(&mut self) -> Result<T, ParseError> {
        T::parse(self)
    }

    /// Parses `count` contiguous elements of type `T` from the cursor.
    pub(super) fn parse_elements<T: Parse>(
        &mut self,
        count: usize,
    ) -> Result<Box<[T]>, ParseError> {
        let mut elements = Vec::with_capacity(count);
        for _ in 0..count {
            elements.push(self.parse()?);
        }
        Ok(elements.into_boxed_slice())
    }

    /// Reads a generic sized, zerocopyable type directly from the cursor.
    fn read<T: FromBytes + KnownLayout + Immutable + Unaligned>(
        &mut self,
    ) -> Result<T, ParseError> {
        let size = std::mem::size_of::<T>();
        let bytes = self.read_bytes(size)?;
        // Safe because T is FromBytes, Immutable, Unaligned and bytes is exactly `size` bytes.
        let value = T::read_from_bytes(bytes).map_err(|_| ParseError::MissingData {
            type_name: std::any::type_name::<T>(),
            type_size: size,
            num_bytes: bytes.len(),
        })?;
        Ok(value)
    }

    /// Reads a byte slice of `count` bytes from the cursor.
    pub(super) fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], ParseError> {
        if self.offset + count > self.data.len() {
            return Err(ParseError::MissingData {
                type_name: "bytes",
                type_size: count,
                num_bytes: self.data.len() - self.offset,
            });
        }
        let bytes = &self.data[self.offset..self.offset + count];
        self.offset += count;
        Ok(bytes)
    }
}

/// Writer used to serialize elements into a byte vector for a specific policy version.
#[derive(Debug)]
pub struct PolicyWriter<'a> {
    version: PolicyVersion,
    writer: &'a mut Vec<u8>,
}

impl<'a> PolicyWriter<'a> {
    /// Creates a new [`PolicyWriter`] wrapping `writer` for the specified `version`.
    pub fn new(version: PolicyVersion, writer: &'a mut Vec<u8>) -> Self {
        Self { version, writer }
    }

    /// Returns target [`PolicyVersion`].
    pub fn version(&self) -> PolicyVersion {
        self.version
    }

    /// Appends raw bytes into destination writer.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.writer.extend_from_slice(bytes);
    }
}

impl Parse for u8 {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        cursor.read::<u8>()
    }
}

impl Serialize for u8 {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        writer.write_bytes(&[*self]);
        Ok(())
    }
}

impl Validate for u8 {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

impl Parse for u16 {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let val: le::U16 = cursor.read::<le::U16>()?;
        Ok(val.get())
    }
}

impl Serialize for u16 {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let val = le::U16::new(*self);
        writer.write_bytes(zerocopy::IntoBytes::as_bytes(&val));
        Ok(())
    }
}

impl Validate for u16 {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

impl Parse for u32 {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let val: le::U32 = cursor.read::<le::U32>()?;
        Ok(val.get())
    }
}

impl Serialize for u32 {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let val = le::U32::new(*self);
        writer.write_bytes(zerocopy::IntoBytes::as_bytes(&val));
        Ok(())
    }
}

impl Validate for u32 {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

/// Container representing a `u32` count followed by that many raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, Validate)]
pub struct ByteArray {
    data: Box<[u8]>,
}

impl Deref for ByteArray {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl AsRef<[u8]> for ByteArray {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Parse for ByteArray {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let count = u32::parse(cursor)? as usize;
        let bytes = cursor.read_bytes(count)?;
        Ok(Self { data: bytes.to_vec().into_boxed_slice() })
    }
}

impl Serialize for ByteArray {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let count = self.data.len() as u32;
        count.serialize(writer)?;
        writer.write_bytes(&self.data);
        Ok(())
    }
}

impl Parse for u64 {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let val: le::U64 = cursor.read::<le::U64>()?;
        Ok(val.get())
    }
}

impl Serialize for u64 {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let val = le::U64::new(*self);
        writer.write_bytes(zerocopy::IntoBytes::as_bytes(&val));
        Ok(())
    }
}

impl Validate for u64 {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

impl<T: Parse, const N: usize> Parse for [T; N] {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let mut items = Vec::with_capacity(N);
        for _ in 0..N {
            items.push(T::parse(cursor)?);
        }
        Ok(items.try_into().ok().expect("pushed N items"))
    }
}

impl<T: Serialize, const N: usize> Serialize for [T; N] {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        for item in self {
            item.serialize(writer)?;
        }
        Ok(())
    }
}

impl<T: Validate, const N: usize> Validate for [T; N] {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        for item in self {
            item.validate(policy)?;
        }
        Ok(())
    }
}

impl<T: Serialize> Serialize for Box<[T]> {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        for item in self.iter() {
            item.serialize(writer)?;
        }
        Ok(())
    }
}

impl<T: Validate> Validate for Box<[T]> {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        for item in self.iter() {
            item.validate(policy)?;
        }
        Ok(())
    }
}

impl<T> Parse for PhantomData<T> {
    fn parse(_cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        Ok(Self)
    }
}

impl<T> Serialize for PhantomData<T> {
    fn serialize(&self, _writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        Ok(())
    }
}

impl<T> Validate for PhantomData<T> {
    fn validate(&self, _policy: &NewPolicy) -> Result<(), ValidateError> {
        Ok(())
    }
}

/// Standard policy array container, storing elements in a heap-allocated slice.
/// Prefixed by a 32-bit count field in the binary policy format.
#[derive(Debug, Clone, PartialEq, Eq, Validate)]
pub struct Array<T> {
    elements: Box<[T]>,
}

impl<T> Default for Array<T> {
    fn default() -> Self {
        Self { elements: Box::new([]) }
    }
}

impl<T> From<Vec<T>> for Array<T> {
    fn from(v: Vec<T>) -> Self {
        Self { elements: v.into_boxed_slice() }
    }
}

impl<T> Deref for Array<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.elements
    }
}

impl<T> AsRef<[T]> for Array<T> {
    fn as_ref(&self) -> &[T] {
        &self.elements
    }
}

impl<T: Parse> Parse for Array<T> {
    fn parse(cursor: &mut PolicyCursor<'_>) -> Result<Self, ParseError> {
        let count = u32::parse(cursor)? as usize;
        let elements = cursor.parse_elements(count)?;
        Ok(Self { elements })
    }
}

impl<T: Serialize> Serialize for Array<T> {
    fn serialize(&self, writer: &mut PolicyWriter<'_>) -> Result<(), SerializeError> {
        let count = self.elements.len() as u32;
        count.serialize(writer)?;
        self.elements.serialize(writer)
    }
}

/// Symbol list header and associated array of items.
///
/// In the SELinux binary policy database, each symbol table is prefixed by a header
/// containing a `primary_names_count` field, followed by the array of items (which itself
/// is prefixed by an `items_count` field).
#[derive(Debug, Parse, Serialize)]
pub struct SymbolArray<T> {
    /// Number of primary names in this array (excluding aliases).
    /// Included in the policy to allow allocation of index structures to be optimized.
    primary_names_count: u32,
    items: Array<T>,
}

impl<T> Deref for SymbolArray<T> {
    type Target = [T];
    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl<T: Validate> Validate for SymbolArray<T> {
    fn validate(&self, policy: &NewPolicy) -> Result<(), ValidateError> {
        self.items.validate(policy)?;
        if self.primary_names_count > self.items.len() as u32 {
            return Err(ValidateError::InvalidPrimaryNamesCount {
                expected_at_most: self.items.len() as u32,
                found: self.primary_names_count,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_policy::metadata::PolicyVersion;

    #[test]
    fn test_policy_cursor_parse_u32() {
        let data = [1, 0, 0, 0, 2, 0, 0, 0];
        let mut cursor = PolicyCursor::new(&data);

        let val1 = cursor.parse::<u32>().unwrap();
        assert_eq!(val1, 1);
        assert_eq!(cursor.offset, 4);

        let val2 = cursor.parse::<u32>().unwrap();
        assert_eq!(val2, 2);
        assert_eq!(cursor.offset, 8);
    }

    #[test]
    fn test_policy_cursor_parse_u64() {
        let data = [1, 0, 0, 0, 0, 0, 0, 0];
        let mut cursor = PolicyCursor::new(&data);
        let val = cursor.parse::<u64>().unwrap();
        assert_eq!(val, 1);
        assert_eq!(cursor.offset, 8);
    }

    #[test]
    fn test_policy_cursor_parse_elements() {
        let data = [1, 0, 0, 0, 2, 0, 0, 0];
        let mut cursor = PolicyCursor::new(&data);

        let elements = cursor.parse_elements::<u32>(2).unwrap();
        assert_eq!(elements.as_ref(), &[1, 2]);
        assert_eq!(cursor.offset, 8);
    }

    #[test]
    fn test_policy_cursor_missing_data_u32() {
        let data = [1, 0];
        let mut cursor = PolicyCursor::new(&data);

        let err = cursor.parse::<u32>().unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingData { type_name: "bytes", type_size: 4, num_bytes: 2 }
        ));
    }

    #[test]
    fn test_u32_serialize() {
        let val = 42u32;
        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        val.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, [42, 0, 0, 0]);
    }

    #[test]
    fn test_u64_serialize() {
        let val = 42u64;
        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        val.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, [42, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn test_policy_cursor_byte_array() {
        let data = [4, 0, 0, 0, 5, 6, 7, 8];
        let mut cursor = PolicyCursor::new(&data);

        let array = cursor.parse::<ByteArray>().unwrap();
        assert_eq!(array.as_ref(), &[5, 6, 7, 8]);
        assert_eq!(cursor.offset, 8);
    }

    #[test]
    fn test_policy_cursor_byte_array_missing_data() {
        let data = [5, 0, 0, 0, 6, 7, 8];
        let mut cursor = PolicyCursor::new(&data);

        let err = cursor.parse::<ByteArray>().unwrap_err();
        assert!(matches!(
            err,
            ParseError::MissingData { type_name: "bytes", type_size: 5, num_bytes: 3 }
        ));
    }

    #[test]
    fn test_byte_array_serialize() {
        let array = ByteArray { data: vec![5, 6, 7, 8].into_boxed_slice() };
        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        array.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, [4, 0, 0, 0, 5, 6, 7, 8]);
    }

    #[test]
    fn test_array_parse_and_serialize() {
        let data = [2, 0, 0, 0, 10, 0, 0, 0, 20, 0, 0, 0];
        let mut cursor = PolicyCursor::new(&data);

        let array = cursor.parse::<Array<u32>>().unwrap();
        assert_eq!(array.as_ref(), &[10, 20]);
        assert_eq!(cursor.offset, 12);

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        array.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, data);
    }

    #[test]
    fn test_symbol_array_parse_and_serialize() {
        // primary_names_count = 5, array count = 2, elements = [10, 20]
        let data = [5, 0, 0, 0, 2, 0, 0, 0, 10, 0, 0, 0, 20, 0, 0, 0];
        let mut cursor = PolicyCursor::new(&data);

        let sym_array = cursor.parse::<SymbolArray<u32>>().unwrap();
        assert_eq!(sym_array.primary_names_count, 5);
        assert_eq!(sym_array.as_ref(), &[10, 20]);
        assert_eq!(cursor.offset, 16);

        let mut writer = Vec::new();
        let mut policy_writer = PolicyWriter::new(PolicyVersion::V33, &mut writer);
        sym_array.serialize(&mut policy_writer).unwrap();
        assert_eq!(writer, data);
    }
}
