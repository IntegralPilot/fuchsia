// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Relocatable absolute-offset pointers and AST buffer wrapper for zero-copy serialized data
//! structures.
//!
//! This module provides [`Ptr`], [`Slice`], [`BStr`], and [`Buffer`] which act as safe, relocatable
//! pointers within a contiguous memory buffer. Unlike standard Rust references or pointers which
//! store absolute 64-bit virtual memory addresses, these structures store a 32-bit unsigned integer
//! offset (`u32`) representing the byte distance from the start of the containing [`Buffer`]
//! to the target data.
//!
//! # Benefits
//! 1. **Relocatability**: The entire memory buffer containing these pointers can be copied, moved
//!    in memory, or serialized to disk/network and loaded at a different address, and all internal
//!    offsets remain valid without needing "pointer patching" or translation.
//! 2. **Zero-copy Deserialization**: A serialized buffer can be cast directly to the root structure
//!    (using `zerocopy`) and traversed immediately.
//! 3. **Space Savings**: On 64-bit systems, offsets use 4 bytes (or 8 bytes for slices) instead of
//!    8 bytes (or 16 bytes for slices).
//! 4. **100% Safe Traversal**: Unlike traditional relative pointers that compute target addresses
//!    by raw pointer arithmetic from `&self`, dereferencing a [`Ptr`], [`Slice`], or [`BStr`]
//!    requires passing the containing [`Buffer`]. This allows safe Rust slice bounds-checking to
//!    verify that the target memory is within the bounds of the AST buffer before converting to a
//!    reference.

use std::marker::PhantomData;
use zerocopy::FromBytes;

/// Sentinel offset value representing a null or invalid pointer.
const NULL_OFFSET: u32 = u32::MAX;

/// A wrapper around a byte slice representing a serialized AST buffer.
///
/// By pairing a relocatable pointer ([`Ptr`], [`Slice`], or [`BStr`]) with an `Buffer`,
/// code can safely obtain references to serialized AST structures.
#[derive(
    zerocopy::FromBytes,
    zerocopy::IntoBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Eq,
    PartialEq,
    Hash,
)]
#[repr(C)]
pub struct Buffer([u8]);

impl Buffer {
    /// Wraps a raw byte slice as a `Buffer`.
    pub fn from_bytes(bytes: &[u8]) -> &Self {
        Self::ref_from_bytes(bytes).unwrap()
    }

    /// Wraps a mutable raw byte slice as a `Buffer`.
    pub fn from_bytes_mut(bytes: &mut [u8]) -> &mut Self {
        Self::mut_from_bytes(bytes).unwrap()
    }

    /// Returns the underlying byte slice.
    pub const fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the underlying mutable byte slice.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }

    /// Retrieves a shared reference to `T` pointed to by `ptr`.
    pub fn get_ref<T>(&self, ptr: Ptr<T>) -> &T
    where
        T: zerocopy::FromBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        assert!(!ptr.is_null(), "attempted to dereference null Ptr in Buffer");
        let start = ptr.offset();
        let size = std::mem::size_of::<T>();
        let end = start.checked_add(size).expect("pointer address overflow");
        let slice = &self.0[start..end];
        match T::ref_from_bytes(slice) {
            Ok(v) => v,
            Err(e) => panic!(
                "invalid AST get_ref in Buffer (offset {}, size {}, align {}, ptr align {}): {:?}",
                start,
                size,
                std::mem::align_of::<T>(),
                slice.as_ptr() as usize % std::mem::align_of::<T>(),
                e
            ),
        }
    }

    /// Retrieves a mutable reference to `T` pointed to by `ptr`.
    pub fn get_mut<T>(&mut self, ptr: Ptr<T>) -> &mut T
    where
        T: zerocopy::FromBytes + zerocopy::IntoBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        assert!(!ptr.is_null(), "attempted to dereference null Ptr in Buffer");
        let start = ptr.offset();
        let size = std::mem::size_of::<T>();
        let end = start.checked_add(size).expect("pointer address overflow");
        let slice = &mut self.0[start..end];
        match T::mut_from_bytes(slice) {
            Ok(v) => v,
            Err(e) => panic!(
                "invalid AST get_mut in Buffer (offset {}, size {}, align {}): {:?}",
                start,
                size,
                std::mem::align_of::<T>(),
                e
            ),
        }
    }

    /// Retrieves a shared reference to a slice `[T]` pointed to by `slice`.
    pub fn get_slice<T>(&self, slice: Slice<T>) -> &[T]
    where
        T: zerocopy::FromBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        if slice.is_empty() {
            return &[];
        }
        let start = slice.offset();
        let count = slice.len();
        let size = count.checked_mul(std::mem::size_of::<T>()).expect("slice size overflow");
        let end = start.checked_add(size).expect("slice address overflow");
        let bytes = &self.0[start..end];
        let (res, rem) = <[T]>::ref_from_prefix_with_elems(bytes, count)
            .expect("invalid AST slice reference in Buffer");
        assert!(rem.is_empty());
        res
    }

    /// Retrieves a mutable reference to a slice `[T]` pointed to by `slice`.
    pub fn get_slice_mut<T>(&mut self, slice: Slice<T>) -> &mut [T]
    where
        T: zerocopy::FromBytes + zerocopy::IntoBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        if slice.is_empty() {
            return &mut [];
        }
        let start = slice.offset();
        let count = slice.len();
        let size = count.checked_mul(std::mem::size_of::<T>()).expect("slice size overflow");
        let end = start.checked_add(size).expect("slice address overflow");
        let bytes = &mut self.0[start..end];
        let (res, rem) = <[T]>::mut_from_prefix_with_elems(bytes, count)
            .expect("invalid AST slice reference in Buffer");
        assert!(rem.is_empty());
        res
    }
}

impl std::ops::Deref for Buffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl std::ops::DerefMut for Buffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl AsRef<Buffer> for [u8] {
    fn as_ref(&self) -> &Buffer {
        Buffer::from_bytes(self)
    }
}

impl AsMut<Buffer> for [u8] {
    fn as_mut(&mut self) -> &mut Buffer {
        Buffer::from_bytes_mut(self)
    }
}

impl std::borrow::Borrow<Buffer> for Vec<u8> {
    fn borrow(&self) -> &Buffer {
        Buffer::from_bytes(self.as_slice())
    }
}

impl std::borrow::Borrow<Buffer> for [u8] {
    fn borrow(&self) -> &Buffer {
        Buffer::from_bytes(self)
    }
}

impl ToOwned for Buffer {
    type Owned = Vec<u8>;
    fn to_owned(&self) -> Self::Owned {
        self.0.to_owned()
    }
}

/// A relocatable pointer to a single value of type `T` within an [`Buffer`].
///
/// Stores a 32-bit unsigned byte offset from the start of the buffer to the target.
#[derive(
    zerocopy::IntoBytes, zerocopy::FromBytes, zerocopy::Immutable, zerocopy::KnownLayout, Debug,
)]
#[repr(transparent)]
pub struct Ptr<T> {
    offset: u32,
    _phantom: PhantomData<T>,
}

// Note: We implement Copy, Clone, PartialEq, Eq, and Hash manually for Ptr<T> and Slice<T>
// rather than deriving them. Deriving these traits on a generic struct adds implicit trait bounds
// on `T` (e.g., `where T: Copy`, `where T: Hash`), which would prevent pointer copying, comparison,
// or hashing whenever target types (such as Command or unsized slices) lack those traits.
impl<T> Copy for Ptr<T> {}
impl<T> Clone for Ptr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PartialEq for Ptr<T> {
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset
    }
}
impl<T> Eq for Ptr<T> {}
impl<T> std::hash::Hash for Ptr<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.offset.hash(state);
    }
}

impl<T> Ptr<T> {
    /// Creates a new pointer with the given buffer byte offset.
    pub const fn new(offset: usize) -> Self {
        Self { offset: offset as u32, _phantom: PhantomData }
    }

    /// Creates a null pointer (offset NULL_OFFSET).
    pub const fn null() -> Self {
        Self { offset: NULL_OFFSET, _phantom: PhantomData }
    }

    /// Sets the byte offset of the pointer.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset as u32;
    }

    /// Clears the pointer to null.
    pub fn clear(&mut self) {
        self.offset = NULL_OFFSET;
    }

    /// Returns `true` if this pointer is null (offset is NULL_OFFSET).
    pub const fn is_null(&self) -> bool {
        self.offset == NULL_OFFSET
    }

    /// Returns the byte offset as a `usize`.
    ///
    /// For null pointers (`is_null()`), this returns `u32::MAX as usize` (the sentinel value for
    /// null or invalid offsets).
    const fn offset(&self) -> usize {
        self.offset as usize
    }

    /// Returns the byte offset as a `usize`.
    pub const fn to_usize(&self) -> usize {
        self.offset as usize
    }

    /// Casts this pointer to a pointer of another type `U`.
    pub const fn cast<U>(self) -> Ptr<U> {
        Ptr::new(self.offset())
    }

    /// Returns a pointer offset by `count` elements of type `T`.
    pub const fn add(self, count: usize) -> Self {
        let bytes = count.checked_mul(std::mem::size_of::<T>()).expect("pointer offset overflow");
        let new_offset = self.offset().checked_add(bytes).expect("pointer address overflow");
        Self::new(new_offset)
    }

    /// Obtains a shared reference to the target `T` in the provided buffer.
    pub fn as_ref<'a>(&self, buf: &'a Buffer) -> &'a T
    where
        T: zerocopy::FromBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        buf.get_ref(*self)
    }

    /// Obtains an optional shared reference to `T`, returning `None` if the pointer is null.
    pub fn get<'a>(&self, buf: &'a Buffer) -> Option<&'a T>
    where
        T: zerocopy::FromBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        if self.is_null() { None } else { Some(buf.get_ref(*self)) }
    }
}

impl<T> Default for Ptr<T> {
    fn default() -> Self {
        Self::null()
    }
}

impl<T> std::fmt::Display for Ptr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null() { write!(f, "null") } else { write!(f, "ptr@{}", self.offset()) }
    }
}

/// Layout of a slice pointer containing offset and length.
#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Hash,
)]
#[repr(C)]
pub struct SliceData {
    offset: u32,
    len: u32,
}

/// A relocatable pointer to a slice of type `[T]` within an [`Buffer`].
///
/// Stores a 32-bit unsigned byte offset from the start of the buffer to the first element
/// and a 32-bit length.
#[derive(
    zerocopy::IntoBytes, zerocopy::FromBytes, zerocopy::Immutable, zerocopy::KnownLayout, Debug,
)]
#[repr(transparent)]
pub struct Slice<T> {
    data: SliceData,
    _phantom: PhantomData<T>,
}

// Note: Implemented manually to avoid implicit trait bounds on T (see note on Ptr<T> above).
impl<T> Copy for Slice<T> {}
impl<T> Clone for Slice<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PartialEq for Slice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}
impl<T> Eq for Slice<T> {}
impl<T> std::hash::Hash for Slice<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.data.hash(state);
    }
}

impl<T> Slice<T> {
    /// Creates a new slice pointer with the given byte offset and element length.
    pub const fn new(offset: usize, len: usize) -> Self {
        Self { data: SliceData { offset: offset as u32, len: len as u32 }, _phantom: PhantomData }
    }

    /// Creates an empty slice pointer.
    pub const fn empty() -> Self {
        Self::new(NULL_OFFSET as usize, 0)
    }

    /// Sets the slice byte offset and element length.
    pub fn set_offset(&mut self, offset: usize, len: usize) {
        if len == 0 {
            self.data.offset = NULL_OFFSET;
            self.data.len = 0;
        } else {
            self.data.offset = offset as u32;
            self.data.len = len as u32;
        }
    }

    /// Returns `true` if this slice has length 0.
    pub const fn is_empty(&self) -> bool {
        self.data.len == 0
    }

    /// Returns the number of elements in the slice.
    pub const fn len(&self) -> usize {
        self.data.len as usize
    }

    /// Returns the starting byte offset of the slice.
    ///
    /// For zero-sized slices (`len() == 0`), this returns `u32::MAX as usize` (the sentinel value
    /// for null or invalid offsets).
    const fn offset(&self) -> usize {
        self.data.offset as usize
    }

    /// Casts this slice pointer to a pointer to its first element.
    pub const fn cast(self) -> Ptr<T> {
        Ptr::new(self.offset())
    }

    /// Returns a pointer to the `i`-th element of this slice.
    pub const fn at(&self, index: usize) -> Ptr<T> {
        let bytes =
            index.checked_mul(std::mem::size_of::<T>()).expect("slice index offset overflow");
        let new_offset = self.offset().checked_add(bytes).expect("slice address overflow");
        Ptr::new(new_offset)
    }

    /// Obtains a shared slice reference to the target `[T]` in the provided buffer.
    pub fn as_slice<'a>(&self, buf: &'a Buffer) -> &'a [T]
    where
        T: zerocopy::FromBytes + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        buf.get_slice(*self)
    }
}

impl<T> Default for Slice<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T> std::fmt::Display for Slice<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() && self.offset() == NULL_OFFSET as usize {
            write!(f, "empty")
        } else {
            write!(f, "slice(offset={}, len={})", self.offset(), self.len())
        }
    }
}

/// A relocatable pointer to a byte string slice (`[u8]`) within an [`Buffer`].
#[derive(
    zerocopy::IntoBytes,
    zerocopy::FromBytes,
    zerocopy::Immutable,
    zerocopy::KnownLayout,
    Debug,
    Clone,
    Copy,
    Eq,
    PartialEq,
    Hash,
)]
#[repr(transparent)]
pub struct BStr {
    slice: Slice<u8>,
}

impl BStr {
    /// Creates a new byte string pointer with the given byte offset and length.
    pub const fn new(offset: usize, len: usize) -> Self {
        Self { slice: Slice::new(offset, len) }
    }

    /// Creates an empty byte string pointer.
    pub const fn empty() -> Self {
        Self { slice: Slice::empty() }
    }
}

impl Default for BStr {
    fn default() -> Self {
        Self::empty()
    }
}

impl BStr {
    /// Sets the byte string byte offset and length.
    pub fn set_offset(&mut self, offset: usize, len: usize) {
        self.slice.set_offset(offset, len);
    }

    /// Returns `true` if this byte string has length 0.
    pub const fn is_empty(&self) -> bool {
        self.slice.is_empty()
    }

    /// Returns the length of the byte string in bytes.
    pub const fn len(&self) -> usize {
        self.slice.len()
    }

    /// Returns the starting byte offset of the byte string.
    ///
    /// For zero-length strings (`len() == 0`), this returns `u32::MAX as usize` (the sentinel value
    /// for null or invalid offsets).
    const fn offset(&self) -> usize {
        self.slice.offset()
    }

    /// Obtains a shared byte slice in the provided buffer.
    pub fn as_slice<'a>(&self, buf: &'a Buffer) -> &'a [u8] {
        self.slice.as_slice(buf)
    }

    /// Obtains a reference to a `bstr::BStr` in the provided buffer.
    pub fn as_bstr<'a>(&self, buf: &'a Buffer) -> &'a bstr::BStr {
        bstr::BStr::new(self.as_slice(buf))
    }

    /// Converts the byte string into an owned `bstr::BString`.
    pub fn to_bstring(&self, buf: &Buffer) -> bstr::BString {
        bstr::BString::from(self.as_bstr(buf))
    }
}

impl std::fmt::Display for BStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() && self.offset() == NULL_OFFSET as usize {
            write!(f, "empty")
        } else {
            write!(f, "bstr(offset={}, len={})", self.offset(), self.len())
        }
    }
}
