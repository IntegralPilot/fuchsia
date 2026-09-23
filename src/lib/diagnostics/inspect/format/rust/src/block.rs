// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for writing VMO blocks in a type-safe way.

use crate::bitfields::{HeaderFields, PayloadFields};
use crate::block_index::BlockIndex;
use crate::block_type::BlockType;
use crate::container::{ReadBytes, WriteBytes};
use crate::error::Error;
use crate::{constants, utils};
use byteorder::{ByteOrder, LittleEndian};
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use std::cmp::min;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{Ordering, fence};

pub use diagnostics_hierarchy::ArrayFormat;

/// Disposition of a Link value.
#[derive(Clone, Debug, PartialEq, Eq, FromPrimitive)]
#[repr(u8)]
pub enum LinkNodeDisposition {
    Child = 0,
    Inline = 1,
}

/// Format in which the property will be read.
#[derive(Debug, PartialEq, Eq, FromPrimitive)]
#[repr(u8)]
pub enum PropertyFormat {
    String = 0,
    Bytes = 1,
    StringReference = 2,
}

/// Points to an index in the VMO and reads it according to the bytes in it.
#[derive(Debug, Clone)]
pub struct Block<T, Kind> {
    pub(crate) index: BlockIndex,
    pub(crate) container: T,
    _phantom: PhantomData<Kind>,
}

impl<T: Deref<Target = Q>, Q, K> Block<T, K> {
    /// Creates a new block.
    #[inline]
    pub(crate) fn new(container: T, index: BlockIndex) -> Self {
        Block { container, index, _phantom: PhantomData }
    }
}

pub trait BlockAccessorExt: ReadBytes + Sized {
    #[inline]
    fn maybe_block_at<K: BlockKind>(&self, index: BlockIndex) -> Option<Block<&Self, K>> {
        let block = Block::new(self, index);
        let block_type = HeaderFields::block_type(&block);
        if block_type != K::block_type() as u8 {
            return None;
        }
        Some(block)
    }

    #[cfg_attr(debug_assertions, track_caller)]
    #[inline]
    fn block_at_unchecked<K: BlockKind>(&self, index: BlockIndex) -> Block<&Self, K> {
        Block::new(self, index)
        // TODO(https://fxbug.dev/392965471): bring back when we have BlockIndex<K>.
        // debug_assert_eq!(this.block_type(), Some(K::block_type()));
    }

    #[inline]
    fn block_at(&self, index: BlockIndex) -> Block<&Self, Unknown> {
        Block::new(self, index)
    }
}

pub trait BlockAccessorMutExt: WriteBytes + ReadBytes + Sized {
    #[inline]
    fn maybe_block_at_mut<K: BlockKind>(
        &mut self,
        index: BlockIndex,
    ) -> Option<Block<&mut Self, K>> {
        let block = Block::new(self, index);
        let block_type = HeaderFields::block_type(&block);
        if block_type != K::block_type() as u8 {
            return None;
        }
        Some(block)
    }

    #[inline]
    fn block_at_unchecked_mut<K: BlockKind>(&mut self, index: BlockIndex) -> Block<&mut Self, K> {
        Block::new(self, index)
    }

    #[inline]
    fn block_at_mut(&mut self, index: BlockIndex) -> Block<&mut Self, Unknown> {
        Block::new(self, index)
    }
}

impl<T> BlockAccessorExt for T where T: ReadBytes {}
impl<T> BlockAccessorMutExt for T where T: WriteBytes + ReadBytes {}

mod private {
    pub trait Sealed {}
}

pub trait BlockKind: private::Sealed {
    fn block_type() -> BlockType;
}

macro_rules! block_kind {
    ([$(($name:ident, $block_type:ident)),*]) => {
        $(
            #[derive(Copy, Clone, Debug)]
            pub struct $name;
            impl BlockKind for $name {
                fn block_type() -> BlockType {
                    BlockType::$block_type
                }
            }
            impl private::Sealed for $name {}
        )*
    }
}

block_kind!([
    (Header, Header),
    (Double, DoubleValue),
    (Int, IntValue),
    (Uint, UintValue),
    (Bool, BoolValue),
    (Buffer, BufferValue),
    (Extent, Extent),
    (StringRef, StringReference),
    (Link, LinkValue),
    (Node, NodeValue),
    (Free, Free),
    (Tombstone, Tombstone),
    (Reserved, Reserved),
    (Name, Name)
]);

#[derive(Copy, Clone, Debug)]
pub struct Unknown;
impl private::Sealed for Unknown {}
impl BlockKind for Unknown {
    #[track_caller]
    fn block_type() -> BlockType {
        panic!("implementation must not call into Unknown::block_type()")
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Array<T>(PhantomData<T>);
impl<T> private::Sealed for Array<T> {}
impl<T: ArraySlotKind> BlockKind for Array<T> {
    fn block_type() -> BlockType {
        BlockType::ArrayValue
    }
}

pub trait ValueBlockKind: BlockKind {}

macro_rules! impl_value_block {
    ([$($name:ident),*]) => {
        $(
            impl ValueBlockKind for $name {}
        )*
    }
}

impl_value_block!([Node, Int, Uint, Double, Buffer, Link, Bool]);
impl<T: ArraySlotKind> ValueBlockKind for Array<T> {}

pub trait ArraySlotKind: BlockKind {
    fn array_entry_type_size() -> usize;
}

impl ArraySlotKind for Double {
    fn array_entry_type_size() -> usize {
        std::mem::size_of::<f64>()
    }
}

impl ArraySlotKind for Int {
    fn array_entry_type_size() -> usize {
        std::mem::size_of::<i64>()
    }
}

impl ArraySlotKind for Uint {
    fn array_entry_type_size() -> usize {
        std::mem::size_of::<u64>()
    }
}

impl ArraySlotKind for StringRef {
    fn array_entry_type_size() -> usize {
        std::mem::size_of::<u32>()
    }
}

impl ArraySlotKind for Unknown {
    #[track_caller]
    fn array_entry_type_size() -> usize {
        panic!("Implementation must not get the size of Unknown");
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes, K: BlockKind> Block<T, K> {
    /// Returns index of the block in the vmo.
    pub fn index(&self) -> BlockIndex {
        self.index
    }

    /// Returns the order of the block.
    pub fn order(&self) -> u8 {
        HeaderFields::order(self)
    }

    /// Returns the type of a block.
    pub fn block_type(&self) -> Option<BlockType> {
        let block_type = self.block_type_raw();
        BlockType::from_u8(block_type)
    }

    /// Returns the type of a block.
    pub fn block_type_raw(&self) -> u8 {
        HeaderFields::block_type(self)
    }

    /// Get the offset of the payload in the container.
    pub(crate) fn payload_offset(&self) -> usize {
        self.index.offset() + constants::HEADER_SIZE_BYTES
    }

    /// Get the offset of the header in the container.
    pub(crate) fn header_offset(&self) -> usize {
        self.index.offset()
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Unknown> {
    pub fn cast<K: BlockKind>(self) -> Option<Block<T, K>> {
        let block_type = HeaderFields::block_type(&self);
        if block_type != K::block_type() as u8 {
            return None;
        }
        Some(Block { container: self.container, index: self.index, _phantom: PhantomData })
    }

    pub fn cast_unchecked<K: BlockKind>(self) -> Block<T, K> {
        Block { container: self.container, index: self.index, _phantom: PhantomData }
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Array<Unknown>> {
    pub fn cast_array<K: ArraySlotKind>(self) -> Option<Block<T, Array<K>>> {
        let entry_type = self.entry_type()?;
        if entry_type != K::block_type() {
            return None;
        }
        Some(Block { container: self.container, index: self.index, _phantom: PhantomData })
    }

    pub fn cast_array_unchecked<K: BlockKind>(self) -> Block<T, Array<K>> {
        Block { container: self.container, index: self.index, _phantom: PhantomData }
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Header> {
    /// Returns the magic number in a HEADER block.
    pub fn magic_number(&self) -> u32 {
        HeaderFields::header_magic(self)
    }

    /// Returns the version of a HEADER block.
    pub fn version(&self) -> u32 {
        HeaderFields::header_version(self)
    }

    /// Returns the generation count of a HEADER block.
    pub fn generation_count(&self) -> u64 {
        PayloadFields::value(self)
    }

    /// Returns the size of the part of the VMO that is currently allocated. The size
    /// is saved in a field in the HEADER block.
    pub fn vmo_size(&self) -> Result<Option<u32>, Error> {
        if self.order() != constants::HEADER_ORDER {
            return Ok(None);
        }
        let offset = (self.index + 1).offset();
        let value = self.container.get_value(offset).ok_or(Error::InvalidOffset(offset))?;
        Ok(Some(value))
    }

    /// True if the header is locked, false otherwise.
    pub fn is_locked(&self) -> bool {
        PayloadFields::value(self) & 1 == 1
    }

    /// Check if the HEADER block is locked (when generation count is odd).
    /// NOTE: this should only be used for testing.
    #[doc(hidden)]
    #[cfg_attr(debug_assertions, track_caller)]
    pub fn check_locked(&self, value: bool) {
        if cfg!(any(debug_assertions, test)) {
            let generation_count = PayloadFields::value(self);
            if (generation_count & 1 == 1) != value {
                panic!("Expected lock state: {value}")
            }
        }
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Double> {
    /// Gets the double value of a DOUBLE_VALUE block.
    pub fn value(&self) -> f64 {
        f64::from_bits(PayloadFields::value(self))
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Int> {
    /// Gets the value of an INT_VALUE block.
    pub fn value(&self) -> i64 {
        i64::from_le_bytes(PayloadFields::value(self).to_le_bytes())
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Uint> {
    /// Gets the unsigned value of a UINT_VALUE block.
    pub fn value(&self) -> u64 {
        PayloadFields::value(self)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Bool> {
    /// Gets the bool values of a BOOL_VALUE block.
    pub fn value(&self) -> bool {
        PayloadFields::value(self) != 0
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Buffer> {
    /// Gets the index of the EXTENT of the PROPERTY block.
    pub fn extent_index(&self) -> BlockIndex {
        PayloadFields::property_extent_index(self).into()
    }

    /// Gets the total length of a PROPERTY or STRING_REFERERENCE block.
    pub fn total_length(&self) -> usize {
        PayloadFields::property_total_length(self) as usize
    }

    /// Gets the flags of a PROPERTY block.
    pub fn format(&self) -> Option<PropertyFormat> {
        let raw_format = PayloadFields::property_flags(self);
        PropertyFormat::from_u8(raw_format)
    }

    /// Gets the flags of a PROPERTY block.
    pub fn format_raw(&self) -> u8 {
        PayloadFields::property_flags(self)
    }
}

impl<'a, T: Deref<Target = Q>, Q: ReadBytes + 'a> Block<T, StringRef> {
    /// Gets the total length of a PROPERTY or STRING_REFERERENCE block.
    pub fn total_length(&self) -> usize {
        PayloadFields::property_total_length(self) as usize
    }

    /// Returns the next EXTENT in an EXTENT chain.
    pub fn next_extent(&self) -> BlockIndex {
        HeaderFields::extent_next_index(self).into()
    }

    /// Returns the current reference count of a string reference.
    pub fn reference_count(&self) -> u32 {
        HeaderFields::string_reference_count(self)
    }

    /// Read the inline portion of a STRING_REFERENCE
    pub fn inline_data(&'a self) -> Result<&'a [u8], Error> {
        let max_len_inlined = utils::payload_size_for_order(self.order())
            - constants::STRING_REFERENCE_TOTAL_LENGTH_BYTES;
        let length = self.total_length();
        let offset = self.payload_offset() + constants::STRING_REFERENCE_TOTAL_LENGTH_BYTES;
        let bytes = self
            .container
            .get_slice_at(offset, min(length, max_len_inlined))
            .ok_or(Error::InvalidOffset(offset))?;
        Ok(bytes)
    }
}

impl<'a, T: Deref<Target = Q>, Q: ReadBytes + 'a> Block<T, Extent> {
    /// Returns the next EXTENT in an EXTENT chain.
    pub fn next_extent(&self) -> BlockIndex {
        HeaderFields::extent_next_index(self).into()
    }

    /// Returns the payload bytes value of an EXTENT block.
    pub fn contents(&'a self) -> Result<&'a [u8], Error> {
        let length = utils::payload_size_for_order(self.order());
        let offset = self.payload_offset();
        self.container.get_slice_at(offset, length).ok_or(Error::InvalidOffset(offset))
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes, S: ArraySlotKind> Block<T, Array<S>> {
    /// Gets the format of an ARRAY_VALUE block.
    pub fn format(&self) -> Option<ArrayFormat> {
        let raw_flags = PayloadFields::array_flags(self);
        ArrayFormat::from_u8(raw_flags)
    }

    /// Gets the number of slots in an ARRAY_VALUE block.
    pub fn slots(&self) -> usize {
        PayloadFields::array_slots_count(self) as usize
    }

    /// Get the array capacity size for the given |order|.
    pub fn capacity(&self) -> Option<usize> {
        self.entry_type_size().map(|size| array_capacity(size, self.order()))
    }

    /// Gets the type of each slot in an ARRAY_VALUE block.
    pub fn entry_type(&self) -> Option<BlockType> {
        let array_type_raw = PayloadFields::array_entry_type(self);
        BlockType::from_u8(array_type_raw).filter(|array_type| array_type.is_valid_for_array())
    }

    /// Gets the type of each slot in an ARRAY_VALUE block.
    pub fn entry_type_raw(&self) -> u8 {
        PayloadFields::array_entry_type(self)
    }

    pub fn entry_type_size(&self) -> Option<usize> {
        self.entry_type().and_then(|entry_type| match entry_type {
            BlockType::IntValue => Some(Int::array_entry_type_size()),
            BlockType::UintValue => Some(Uint::array_entry_type_size()),
            BlockType::DoubleValue => Some(Double::array_entry_type_size()),
            BlockType::StringReference => Some(StringRef::array_entry_type_size()),
            _ => None,
        })
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Array<StringRef>> {
    pub fn get_string_index_at(&self, slot_index: usize) -> Option<BlockIndex> {
        if slot_index >= self.slots() {
            return None;
        }
        let offset = (self.index + 1).offset() + slot_index * StringRef::array_entry_type_size();
        self.container.get_value(offset).map(BlockIndex::new)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Array<Int>> {
    /// Gets the value of an int ARRAY_VALUE slot.
    pub fn get(&self, slot_index: usize) -> Option<i64> {
        if slot_index >= self.slots() {
            return None;
        }
        let offset = (self.index + 1).offset() + slot_index * 8;
        self.container.get_value(offset)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Array<Double>> {
    /// Gets the value of a double ARRAY_VALUE slot.
    pub fn get(&self, slot_index: usize) -> Option<f64> {
        if slot_index >= self.slots() {
            return None;
        }
        let offset = (self.index + 1).offset() + slot_index * 8;
        self.container.get_value(offset)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Array<Uint>> {
    /// Gets the value of a uint ARRAY_VALUE slot.
    pub fn get(&self, slot_index: usize) -> Option<u64> {
        if slot_index >= self.slots() {
            return None;
        }
        let offset = (self.index + 1).offset() + slot_index * 8;
        self.container.get_value(offset)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Link> {
    /// Gets the index of the content of this LINK_VALUE block.
    pub fn content_index(&self) -> BlockIndex {
        PayloadFields::content_index(self).into()
    }

    /// Gets the node disposition of a LINK_VALUE block.
    pub fn link_node_disposition(&self) -> Option<LinkNodeDisposition> {
        let flag = PayloadFields::disposition_flags(self);
        LinkNodeDisposition::from_u8(flag)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes, K: ValueBlockKind> Block<T, K> {
    /// Gets the NAME block index of a *_VALUE block.
    pub fn name_index(&self) -> BlockIndex {
        HeaderFields::value_name_index(self).into()
    }

    /// Get the parent block index of a *_VALUE block.
    pub fn parent_index(&self) -> BlockIndex {
        HeaderFields::value_parent_index(self).into()
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Node> {
    /// Get the child count of a NODE_VALUE block.
    pub fn child_count(&self) -> u64 {
        PayloadFields::value(self)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Tombstone> {
    /// Get the child count of a TOMBSTONE block.
    pub fn child_count(&self) -> u64 {
        PayloadFields::value(self)
    }
}

impl<T: Deref<Target = Q>, Q: ReadBytes> Block<T, Free> {
    /// Get next free block
    pub fn free_next_index(&self) -> BlockIndex {
        HeaderFields::free_next_index(self).into()
    }
}

impl<'a, T: Deref<Target = Q>, Q: ReadBytes + 'a> Block<T, Name> {
    /// Get the length of the name of a NAME block
    pub fn length(&self) -> usize {
        HeaderFields::name_length(self).into()
    }

    /// Returns the contents of a NAME block.
    pub fn contents(&'a self) -> Result<&'a str, Error> {
        let length = self.length();
        let offset = self.payload_offset();
        let bytes =
            self.container.get_slice_at(offset, length).ok_or(Error::InvalidOffset(offset))?;
        std::str::from_utf8(bytes).map_err(|_| Error::NameNotUtf8)
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes, K: BlockKind>
    Block<T, K>
{
    /// Converts a block to a FREE block
    pub fn become_free(mut self, next: BlockIndex) -> Block<T, Free> {
        HeaderFields::set_free_reserved(&mut self, 0);
        HeaderFields::set_block_type(&mut self, BlockType::Free as u8);
        HeaderFields::set_free_next_index(&mut self, *next);
        HeaderFields::set_free_empty(&mut self, 0);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes, K: BlockKind>
    Block<T, K>
{
    /// Set the order of the block.
    pub fn set_order(&mut self, order: u8) -> Result<(), Error> {
        if order >= constants::NUM_ORDERS {
            return Err(Error::InvalidBlockOrder(order));
        }
        HeaderFields::set_order(self, order);
        Ok(())
    }

    /// Write |bytes| to the payload section of the block in the container.
    fn write_payload_from_bytes(&mut self, bytes: &[u8]) {
        let offset = self.payload_offset();
        self.container.copy_from_slice_at(offset, bytes);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Free> {
    /// Initializes an empty free block.
    pub fn free(
        container: T,
        index: BlockIndex,
        order: u8,
        next_free: BlockIndex,
    ) -> Result<Block<T, Free>, Error> {
        if order >= constants::NUM_ORDERS {
            return Err(Error::InvalidBlockOrder(order));
        }
        let mut block = Block::new(container, index);
        HeaderFields::set_value(&mut block, 0);
        HeaderFields::set_order(&mut block, order);
        HeaderFields::set_block_type(&mut block, BlockType::Free as u8);
        HeaderFields::set_free_next_index(&mut block, *next_free);
        Ok(block)
    }

    /// Converts a FREE block to a RESERVED block
    pub fn become_reserved(mut self) -> Block<T, Reserved> {
        HeaderFields::set_block_type(&mut self, BlockType::Reserved as u8);
        HeaderFields::set_reserved_empty(&mut self, 0);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Set the next free block.
    pub fn set_free_next_index(&mut self, next_free: BlockIndex) {
        HeaderFields::set_free_next_index(self, *next_free);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Reserved> {
    /// Initializes a HEADER block.
    pub fn become_header(mut self, size: usize) -> Result<Block<T, Header>, Error> {
        self.index = BlockIndex::HEADER;
        HeaderFields::set_order(&mut self, constants::HEADER_ORDER);
        HeaderFields::set_block_type(&mut self, BlockType::Header as u8);
        HeaderFields::set_header_magic(&mut self, constants::HEADER_MAGIC_NUMBER);
        HeaderFields::set_header_version(&mut self, constants::HEADER_VERSION_NUMBER);
        PayloadFields::set_value(&mut self, 0);
        let mut this =
            Block { index: self.index, container: self.container, _phantom: PhantomData };
        // Safety: a valid `size` is smaller than a u32
        this.set_vmo_size(size.try_into().unwrap())?;
        Ok(this)
    }

    /// Converts a block to an EXTENT block.
    pub fn become_extent(mut self, next_extent_index: BlockIndex) -> Block<T, Extent> {
        HeaderFields::set_block_type(&mut self, BlockType::Extent as u8);
        HeaderFields::set_extent_next_index(&mut self, *next_extent_index);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Converts a RESERVED block into a DOUBLE_VALUE block.
    pub fn become_double_value(
        mut self,
        value: f64,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) -> Block<T, Double> {
        self.write_value_header(BlockType::DoubleValue, name_index, parent_index);
        let mut this =
            Block { index: self.index, container: self.container, _phantom: PhantomData::<Double> };
        this.set(value);
        this
    }

    /// Converts a RESERVED block into an INT_VALUE block.
    pub fn become_int_value(
        mut self,
        value: i64,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) -> Block<T, Int> {
        self.write_value_header(BlockType::IntValue, name_index, parent_index);
        let mut this =
            Block { index: self.index, container: self.container, _phantom: PhantomData::<Int> };
        this.set(value);
        this
    }

    /// Converts a RESERVED block into a UINT_VALUE block.
    pub fn become_uint_value(
        mut self,
        value: u64,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) -> Block<T, Uint> {
        self.write_value_header(BlockType::UintValue, name_index, parent_index);
        let mut this =
            Block { index: self.index, container: self.container, _phantom: PhantomData::<Uint> };
        this.set(value);
        this
    }

    /// Converts a RESERVED block into a BOOL_VALUE block.
    pub fn become_bool_value(
        mut self,
        value: bool,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) -> Block<T, Bool> {
        self.write_value_header(BlockType::BoolValue, name_index, parent_index);
        let mut this =
            Block { index: self.index, container: self.container, _phantom: PhantomData::<Bool> };
        this.set(value);
        this
    }

    /// Initializes a NODE_VALUE block.
    pub fn become_node(
        mut self,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) -> Block<T, Node> {
        self.write_value_header(BlockType::NodeValue, name_index, parent_index);
        PayloadFields::set_value(&mut self, 0);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Converts a *_VALUE block into a BUFFER_VALUE block.
    pub fn become_property(
        mut self,
        name_index: BlockIndex,
        parent_index: BlockIndex,
        format: PropertyFormat,
    ) -> Block<T, Buffer> {
        self.write_value_header(BlockType::BufferValue, name_index, parent_index);
        PayloadFields::set_value(&mut self, 0);
        PayloadFields::set_property_flags(&mut self, format as u8);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Initializes a STRING_REFERENCE block. Everything is set except for
    /// the payload string and total length.
    pub fn become_string_reference(mut self) -> Block<T, StringRef> {
        HeaderFields::set_block_type(&mut self, BlockType::StringReference as u8);
        HeaderFields::set_extent_next_index(&mut self, *BlockIndex::EMPTY);
        HeaderFields::set_string_reference_count(&mut self, 0);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Creates a NAME block.
    pub fn become_name(mut self, name: &str) -> Block<T, Name> {
        let max_len = utils::payload_size_for_order(self.order());
        let valid_len = name.floor_char_boundary(max_len);
        let bytes = &name.as_bytes()[..valid_len];
        HeaderFields::set_block_type(&mut self, BlockType::Name as u8);
        // Safety: name length must fit in 12 bytes.
        HeaderFields::set_name_length(&mut self, u16::from_usize(bytes.len()).unwrap());
        self.write_payload_from_bytes(bytes);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Creates a LINK block.
    pub fn become_link(
        mut self,
        name_index: BlockIndex,
        parent_index: BlockIndex,
        content_index: BlockIndex,
        disposition_flags: LinkNodeDisposition,
    ) -> Block<T, Link> {
        self.write_value_header(BlockType::LinkValue, name_index, parent_index);
        PayloadFields::set_value(&mut self, 0);
        PayloadFields::set_content_index(&mut self, *content_index);
        PayloadFields::set_disposition_flags(&mut self, disposition_flags as u8);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Converts a block to an *_ARRAY_VALUE block
    pub fn become_array_value<S: ArraySlotKind>(
        mut self,
        slots: usize,
        format: ArrayFormat,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) -> Result<Block<T, Array<S>>, Error> {
        if S::block_type() == BlockType::StringReference && format != ArrayFormat::Default {
            return Err(Error::InvalidArrayType(self.index));
        }
        let order = self.order();
        let max_capacity = max_array_capacity::<S>(order);

        if slots > max_capacity {
            return Err(Error::array_capacity_exceeded(slots, order, max_capacity));
        }
        self.write_value_header(BlockType::ArrayValue, name_index, parent_index);
        PayloadFields::set_value(&mut self, 0);
        PayloadFields::set_array_entry_type(&mut self, S::block_type() as u8);
        PayloadFields::set_array_flags(&mut self, format as u8);
        PayloadFields::set_array_slots_count(&mut self, slots as u8);
        let mut this =
            Block { index: self.index, container: self.container, _phantom: PhantomData };
        this.clear(0);
        Ok(this)
    }

    /// Initializes a *_VALUE block header.
    #[cfg_attr(debug_assertions, track_caller)]
    fn write_value_header(
        &mut self,
        block_type: BlockType,
        name_index: BlockIndex,
        parent_index: BlockIndex,
    ) {
        debug_assert!(block_type.is_any_value(), "Unexpected block: {block_type}");
        HeaderFields::set_block_type(self, block_type as u8);
        HeaderFields::set_value_name_index(self, *name_index);
        HeaderFields::set_value_parent_index(self, *parent_index);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Header> {
    /// Allows to set the magic value of the header.
    /// NOTE: this should only be used for testing.
    #[doc(hidden)]
    pub fn set_magic(&mut self, value: u32) {
        HeaderFields::set_header_magic(self, value);
    }

    /// Set the size of the part of the VMO that is currently allocated. The size is saved in
    /// a field in the HEADER block.
    pub fn set_vmo_size(&mut self, size: u32) -> Result<(), Error> {
        if self.order() != constants::HEADER_ORDER {
            return Ok(());
        }
        self.container.set_value((self.index + 1).offset(), size)
    }

    /// Freeze the HEADER, indicating a VMO is frozen.
    pub fn freeze(&mut self) -> u64 {
        let value = PayloadFields::value(self);
        PayloadFields::set_value(self, constants::VMO_FROZEN);
        value
    }

    /// Thaw the HEADER, indicating a VMO is Live again.
    pub fn thaw(&mut self, generation: u64) {
        PayloadFields::set_value(self, generation)
    }

    /// Lock a HEADER block
    pub fn lock(&mut self) {
        self.check_locked(false);
        self.increment_generation_count();
        fence(Ordering::Acquire);
    }

    /// Unlock a HEADER block
    pub fn unlock(&mut self) {
        self.check_locked(true);
        fence(Ordering::Release);
        self.increment_generation_count();
    }

    /// Increment generation counter in a HEADER block for locking/unlocking
    fn increment_generation_count(&mut self) {
        let value = PayloadFields::value(self);
        // TODO(miguelfrde): perform this in place using overflowing add.
        let new_value = value.wrapping_add(1);
        PayloadFields::set_value(self, new_value);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Node> {
    /// Initializes a TOMBSTONE block.
    pub fn become_tombstone(mut self) -> Block<T, Tombstone> {
        HeaderFields::set_block_type(&mut self, BlockType::Tombstone as u8);
        HeaderFields::set_tombstone_empty(&mut self, 0);
        Block { index: self.index, container: self.container, _phantom: PhantomData }
    }

    /// Set the child count of a NODE_VALUE block.
    pub fn set_child_count(&mut self, count: u64) {
        PayloadFields::set_value(self, count);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes, S: ArraySlotKind>
    Block<T, Array<S>>
{
    /// Sets all values of the array to zero starting on `start_slot_index` (inclusive).
    pub fn clear(&mut self, start_slot_index: usize) {
        let array_slots = self.slots() - start_slot_index;
        // TODO(https://fxbug.dev/392965471): this can come back when we improve the BlockIndex<K>
        // stuff.
        // let type_size = S::array_entry_type_size();
        let type_size = self.entry_type_size().unwrap();
        let offset = (self.index + 1).offset() + start_slot_index * type_size;
        if let Some(slice) = self.container.get_slice_mut_at(offset, array_slots * type_size) {
            slice.fill(0);
        }
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes>
    Block<T, Array<StringRef>>
{
    /// Sets the value of a string ARRAY_VALUE block.
    pub fn set_string_slot(&mut self, slot_index: usize, string_index: BlockIndex) {
        if slot_index >= self.slots() {
            return;
        }
        // 0 is used as special value; the reader won't dereference it
        let type_size = StringRef::array_entry_type_size();
        let _ = self
            .container
            .set_value((self.index + 1).offset() + slot_index * type_size, *string_index);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Array<Int>> {
    /// Sets the value of an int ARRAY_VALUE block.
    pub fn set(&mut self, slot_index: usize, value: i64) {
        if slot_index >= self.slots() {
            return;
        }
        let type_size = Int::array_entry_type_size();
        let _ = self.container.set_value((self.index + 1).offset() + slot_index * type_size, value);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes>
    Block<T, Array<Double>>
{
    /// Sets the value of a double ARRAY_VALUE block.
    pub fn set(&mut self, slot_index: usize, value: f64) {
        if slot_index >= self.slots() {
            return;
        }
        let type_size = Double::array_entry_type_size();
        let _ = self.container.set_value((self.index + 1).offset() + slot_index * type_size, value);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Array<Uint>> {
    /// Sets the value of a uint ARRAY_VALUE block.
    pub fn set(&mut self, slot_index: usize, value: u64) {
        if slot_index >= self.slots() {
            return;
        }
        let type_size = Uint::array_entry_type_size();
        let _ = self.container.set_value((self.index + 1).offset() + slot_index * type_size, value);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Extent> {
    /// Sets the index of the next EXTENT in the chain.
    pub fn set_next_index(&mut self, next_extent_index: BlockIndex) {
        HeaderFields::set_extent_next_index(self, *next_extent_index);
    }

    /// Set the payload of an EXTENT block. The number of bytes written will be returned.
    pub fn set_contents(&mut self, value: &[u8]) -> usize {
        let order = self.order();
        let max_bytes = utils::payload_size_for_order(order);
        let mut bytes = value;
        if bytes.len() > max_bytes {
            bytes = &bytes[..min(bytes.len(), max_bytes)];
        }
        self.write_payload_from_bytes(bytes);
        bytes.len()
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, StringRef> {
    /// Sets the index of the next EXTENT in the chain.
    pub fn set_next_index(&mut self, next_extent_index: BlockIndex) {
        HeaderFields::set_extent_next_index(self, *next_extent_index);
    }

    /// Sets the total length of a BUFFER_VALUE or STRING_REFERENCE block.
    pub fn set_total_length(&mut self, length: u32) {
        PayloadFields::set_property_total_length(self, length);
    }

    /// Increment the reference count by 1.
    pub fn increment_ref_count(&mut self) -> Result<(), Error> {
        let cur = HeaderFields::string_reference_count(self);
        if cur < constants::MAX_REFERENCE_COUNT {
            HeaderFields::set_string_reference_count(self, cur + 1);
        }
        Ok(())
    }

    /// Decrement the reference count by 1.
    pub fn decrement_ref_count(&mut self) -> Result<(), Error> {
        let cur = HeaderFields::string_reference_count(self);
        if cur < constants::MAX_REFERENCE_COUNT {
            let new_count = cur.checked_sub(1).ok_or(Error::InvalidReferenceCount)?;
            HeaderFields::set_string_reference_count(self, new_count);
        }
        Ok(())
    }

    /// Write the portion of the string that fits into the STRING_REFERENCE block,
    /// as well as write the total length of value to the block.
    /// Returns the number of bytes written.
    pub fn write_inline(&mut self, value: &[u8]) -> usize {
        let payload_offset = self.payload_offset();
        self.set_total_length(value.len() as u32);
        let max_len = utils::payload_size_for_order(self.order())
            - constants::STRING_REFERENCE_TOTAL_LENGTH_BYTES;
        // we do not care about splitting multibyte UTF-8 characters, because the rest
        // of the split will go in an extent and be joined together at read time.
        let bytes = min(value.len(), max_len);
        let to_inline = &value[..bytes];
        self.container.copy_from_slice_at(
            payload_offset + constants::STRING_REFERENCE_TOTAL_LENGTH_BYTES,
            to_inline,
        );
        bytes
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Double> {
    /// Sets the value of a DOUBLE_VALUE block.
    pub fn set(&mut self, value: f64) {
        PayloadFields::set_value(self, value.to_bits());
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Int> {
    /// Sets the value of a INT_VALUE block.
    pub fn set(&mut self, value: i64) {
        PayloadFields::set_value(self, LittleEndian::read_u64(&value.to_le_bytes()));
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Uint> {
    /// Sets the value of a UINT_VALUE block.
    pub fn set(&mut self, value: u64) {
        PayloadFields::set_value(self, value);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Bool> {
    /// Sets the value of a BOOL_VALUE block.
    pub fn set(&mut self, value: bool) {
        PayloadFields::set_value(self, value as u64);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Buffer> {
    /// Sets the total length of a BUFFER_VALUE block.
    pub fn set_total_length(&mut self, length: u32) {
        PayloadFields::set_property_total_length(self, length);
    }

    /// Sets the index of the EXTENT of a BUFFER_VALUE block.
    pub fn set_extent_index(&mut self, index: BlockIndex) {
        // TODO(https://fxbug.dev/392965471): this should probably take a reference to a
        // Block<Extent> to guarantee the index is valid.
        PayloadFields::set_property_extent_index(self, *index);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes> Block<T, Tombstone> {
    /// Set the child count of a NODE_VALUE block.
    pub fn set_child_count(&mut self, count: u64) {
        PayloadFields::set_value(self, count);
    }
}

impl<T: Deref<Target = Q> + DerefMut<Target = Q>, Q: WriteBytes + ReadBytes, K: ValueBlockKind>
    Block<T, K>
{
    pub fn set_parent(&mut self, new_parent_index: BlockIndex) {
        HeaderFields::set_value_parent_index(self, *new_parent_index);
    }
}

/// Get the array capacity size for the given |order|.
fn max_array_capacity<S: ArraySlotKind>(order: u8) -> usize {
    array_capacity(S::array_entry_type_size(), order)
}

fn array_capacity(slot_size: usize, order: u8) -> usize {
    (utils::order_to_size(order)
        - constants::HEADER_SIZE_BYTES
        - constants::ARRAY_PAYLOAD_METADATA_SIZE_BYTES)
        / slot_size
}

pub mod testing {
    use super::*;

    pub fn override_header<T: WriteBytes + ReadBytes, K: BlockKind>(
        block: &mut Block<&mut T, K>,
        value: u64,
    ) {
        let _ = block.container.set_value(block.header_offset(), value);
    }

    pub fn override_payload<T: WriteBytes + ReadBytes, K: BlockKind>(
        block: &mut Block<&mut T, K>,
        value: u64,
    ) {
        let _ = block.container.set_value(block.payload_offset(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Container, CopyBytes};

    macro_rules! assert_8_bytes {
        ($container:ident, $offset:expr, $expected:expr) => {
            let slice = $container.get_slice_at($offset, 8).unwrap();
            assert_eq!(slice, &$expected);
        };
    }

    #[fuchsia::test]
    fn test_new_free() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        assert!(Block::free(&mut container, 3.into(), constants::NUM_ORDERS, 1.into()).is_err());

        let res = Block::free(&mut container, BlockIndex::EMPTY, 3, 1.into());
        assert!(res.is_ok());
        let block = res.unwrap();
        assert_eq!(*block.index(), 0);
        assert_eq!(block.order(), 3);
        assert_eq!(*block.free_next_index(), 1);
        assert_eq!(block.block_type(), Some(BlockType::Free));
        assert_8_bytes!(container, 0, [0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_set_order() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut block = Block::free(&mut container, BlockIndex::EMPTY, 1, 1.into()).unwrap();
        assert!(block.set_order(3).is_ok());
        assert_eq!(block.order(), 3);
    }

    #[fuchsia::test]
    fn test_become_reserved() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = Block::free(&mut container, BlockIndex::EMPTY, 1, 2.into()).unwrap();
        let block = block.become_reserved();
        assert_eq!(block.block_type(), Some(BlockType::Reserved));
        assert_8_bytes!(container, 0, [0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_become_string_reference() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_string_reference();
        assert_eq!(block.block_type(), Some(BlockType::StringReference));
        assert_eq!(*block.next_extent(), 0);
        assert_eq!(block.reference_count(), 0);
        assert_eq!(block.total_length(), 0);
        assert_eq!(block.inline_data().unwrap(), Vec::<u8>::new());
        assert_8_bytes!(container, 0, [0x01, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_inline_string_reference() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut block = get_reserved(&mut container);
        block.set_order(0).unwrap();
        let mut block = block.become_string_reference();

        assert_eq!(block.write_inline("ab".as_bytes()), 2);
        assert_eq!(block.reference_count(), 0);
        assert_eq!(block.total_length(), 2);
        assert_eq!(block.order(), 0);
        assert_eq!(*block.next_extent(), 0);
        assert_eq!(block.inline_data().unwrap(), "ab".as_bytes());

        assert_eq!(block.write_inline("abcd".as_bytes()), 4);
        assert_eq!(block.reference_count(), 0);
        assert_eq!(block.total_length(), 4);
        assert_eq!(block.order(), 0);
        assert_eq!(*block.next_extent(), 0);
        assert_eq!(block.inline_data().unwrap(), "abcd".as_bytes());

        assert_eq!(
            block.write_inline("abcdefghijklmnopqrstuvwxyz".as_bytes()),
            4 // with order == 0, only 4 bytes will be inlined
        );
        assert_eq!(block.reference_count(), 0);
        assert_eq!(block.total_length(), 26);
        assert_eq!(block.order(), 0);
        assert_eq!(*block.next_extent(), 0);
        assert_eq!(block.inline_data().unwrap(), "abcd".as_bytes());

        assert_eq!(block.write_inline("abcdef".as_bytes()), 4);
        assert_eq!(block.reference_count(), 0);
        assert_eq!(block.total_length(), 6);
        assert_eq!(block.order(), 0);
        assert_eq!(*block.next_extent(), 0);
        assert_eq!(block.inline_data().unwrap(), "abcd".as_bytes());
    }

    #[fuchsia::test]
    fn test_string_reference_count() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut block = get_reserved(&mut container);
        block.set_order(0).unwrap();
        let mut block = block.become_string_reference();
        assert_eq!(block.reference_count(), 0);

        assert!(block.increment_ref_count().is_ok());
        assert_eq!(block.reference_count(), 1);

        assert!(block.decrement_ref_count().is_ok());
        assert_eq!(block.reference_count(), 0);

        assert!(block.decrement_ref_count().is_err());
        assert_eq!(block.reference_count(), 0);

        HeaderFields::set_string_reference_count(&mut block, constants::MAX_REFERENCE_COUNT);
        assert_eq!(block.reference_count(), constants::MAX_REFERENCE_COUNT);

        assert!(block.increment_ref_count().is_ok());
        assert_eq!(block.reference_count(), constants::MAX_REFERENCE_COUNT);

        assert!(block.decrement_ref_count().is_ok());
        assert_eq!(block.reference_count(), constants::MAX_REFERENCE_COUNT);
    }

    #[fuchsia::test]
    fn test_become_header() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block =
            get_reserved(&mut container).become_header(constants::MIN_ORDER_SIZE * 2).unwrap();
        assert_eq!(block.block_type(), Some(BlockType::Header));
        assert_eq!(*block.index(), 0);
        assert_eq!(block.order(), constants::HEADER_ORDER);
        assert_eq!(block.magic_number(), constants::HEADER_MAGIC_NUMBER);
        assert_eq!(block.version(), constants::HEADER_VERSION_NUMBER);
        assert_eq!(block.vmo_size().unwrap().unwrap() as usize, constants::MIN_ORDER_SIZE * 2);
        assert_8_bytes!(container, 0, [0x01, 0x02, 0x02, 0x00, 0x49, 0x4e, 0x53, 0x50]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_header_without_size() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block =
            get_reserved(&mut container).become_header(constants::MIN_ORDER_SIZE * 2).unwrap();
        assert_eq!(block.order(), constants::HEADER_ORDER);
        assert!(block.vmo_size().unwrap().is_some());

        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        // Should be possible to handle headers without size.
        let mut block = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
        assert!(block.set_order(0).is_ok());
        assert_eq!(block.vmo_size().unwrap(), None);
        // Set vmo size should have no effect.
        assert!(block.set_vmo_size(123456789).is_ok());
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_freeze_thaw_header() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block =
            get_reserved(&mut container).become_header(constants::MIN_ORDER_SIZE * 2).unwrap();
        assert_eq!(block.block_type(), Some(BlockType::Header));
        assert_eq!(*block.index(), 0);
        assert_eq!(block.order(), constants::HEADER_ORDER);
        assert_eq!(block.magic_number(), constants::HEADER_MAGIC_NUMBER);
        assert_eq!(block.version(), constants::HEADER_VERSION_NUMBER);
        assert_8_bytes!(container, 0, [0x01, 0x02, 0x02, 0x00, 0x49, 0x4e, 0x53, 0x50]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let old = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER).freeze();
        assert_8_bytes!(container, 8, [0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER).thaw(old);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x020, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_unaligned_buffer_access() {
        #[repr(C, packed)]
        struct Unaligned(u8, [u8; 32]);

        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let _ = get_reserved(&mut container).become_header(constants::MIN_ORDER_SIZE * 2).unwrap();

        let mut unaligned = Unaligned(0, [0; 32]);
        assert_eq!(unaligned.0, 0);
        unaligned.1.copy_from_slice(container.get_slice_at(0, 32).unwrap());

        let block = unaligned.1.block_at_unchecked::<Header>(BlockIndex::EMPTY);
        assert_eq!(block.magic_number(), constants::HEADER_MAGIC_NUMBER);
        assert_eq!(block.version(), constants::HEADER_VERSION_NUMBER);
        assert_eq!(block.generation_count(), 0);
    }

    #[fuchsia::test]
    fn test_set_value_invalid_offset() {
        let mut buffer = [0u8; 16];
        assert_eq!(buffer.set_value(16, 42u32), Err(Error::InvalidOffset(16)));
        assert_eq!(buffer.set_value(14, 42u32), Err(Error::InvalidOffset(14)));
        assert_eq!(buffer.set_value(0, 42u32), Ok(()));
        assert_eq!(buffer.get_value::<u32>(0), Some(42));
    }

    #[fuchsia::test]
    #[should_panic]
    fn test_cant_unlock_locked_header() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let mut block = get_header(&mut container, constants::MIN_ORDER_SIZE * 2);
        // Can't unlock unlocked header.
        block.unlock();
    }

    #[fuchsia::test]
    #[should_panic]
    fn test_cant_lock_locked_header() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let mut block = get_header(&mut container, constants::MIN_ORDER_SIZE * 2);
        block.lock();
        // Can't lock locked header.
        let mut block = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
        block.lock();
    }

    #[fuchsia::test]
    #[should_panic]
    fn test_header_overflow() {
        // set payload bytes to max u64 value. Ensure we cannot lock
        // and after unlocking, the value is zero.
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        container.set_value(8, u64::MAX).unwrap();
        let mut block = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
        block.lock();
    }

    #[fuchsia::test]
    fn test_lock_unlock_header() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let mut block = get_header(&mut container, constants::MIN_ORDER_SIZE * 2);
        block.lock();
        assert!(block.is_locked());
        assert_eq!(block.generation_count(), 1);
        let header_bytes: [u8; 8] = [0x01, 0x02, 0x02, 0x00, 0x49, 0x4e, 0x53, 0x50];
        assert_8_bytes!(container, 0, header_bytes[..]);
        assert_8_bytes!(container, 8, [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let mut block = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
        block.unlock();
        assert!(!block.is_locked());
        assert_eq!(block.generation_count(), 2);
        assert_8_bytes!(container, 0, header_bytes[..]);
        assert_8_bytes!(container, 8, [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        // Test overflow: Ensure that after unlocking, the value is zero.
        container.set_value(8, u64::MAX).unwrap();
        let mut block = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
        block.unlock();
        assert_eq!(block.generation_count(), 0);
        assert_8_bytes!(container, 0, header_bytes[..]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_header_vmo_size() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let mut block = get_header(&mut container, constants::MIN_ORDER_SIZE * 2);
        assert!(block.set_vmo_size(constants::DEFAULT_VMO_SIZE_BYTES.try_into().unwrap()).is_ok());
        assert_8_bytes!(container, 0, [0x01, 0x02, 0x02, 0x00, 0x49, 0x4e, 0x53, 0x50]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 16, [0x00, 0x00, 0x4, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 24, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let block = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
        assert_eq!(block.vmo_size().unwrap().unwrap() as usize, constants::DEFAULT_VMO_SIZE_BYTES);
    }

    #[fuchsia::test]
    fn test_become_tombstone() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut block = get_reserved(&mut container).become_node(2.into(), 3.into());
        block.set_child_count(4);
        let block = block.become_tombstone();
        assert_eq!(block.block_type(), Some(BlockType::Tombstone));
        assert_eq!(block.child_count(), 4);
        assert_8_bytes!(container, 0, [0x01, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_child_count() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let _ = get_reserved(&mut container).become_node(2.into(), 3.into());
        assert_8_bytes!(container, 0, [0x01, 0x03, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let mut block = container.block_at_unchecked_mut::<Node>(BlockIndex::EMPTY);
        block.set_child_count(4);
        assert_eq!(block.child_count(), 4);
        assert_8_bytes!(container, 0, [0x01, 0x03, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_free() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut block = Block::free(&mut container, BlockIndex::EMPTY, 1, 1.into()).unwrap();
        block.set_free_next_index(3.into());
        assert_eq!(*block.free_next_index(), 3);
        assert_8_bytes!(container, 0, [0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_extent() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block = get_reserved(&mut container).become_extent(3.into());
        assert_eq!(block.block_type(), Some(BlockType::Extent));
        assert_eq!(*block.next_extent(), 3);
        assert_8_bytes!(container, 0, [0x01, 0x08, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let mut block = container.block_at_unchecked_mut::<Extent>(BlockIndex::EMPTY);
        assert_eq!(block.set_contents("test-rust-inspect".as_bytes()), 17);
        assert_eq!(
            String::from_utf8(block.contents().unwrap().to_vec()).unwrap(),
            "test-rust-inspect\0\0\0\0\0\0\0"
        );
        let slice = container.get_slice_at(8, 17).unwrap();
        assert_eq!(slice, "test-rust-inspect".as_bytes());
        let slice = container.get_slice_at(25, 7).unwrap();
        assert_eq!(slice, &[0, 0, 0, 0, 0, 0, 0]);

        let mut block = container.block_at_unchecked_mut::<Extent>(BlockIndex::EMPTY);
        block.set_next_index(4.into());
        assert_eq!(*block.next_extent(), 4);
    }

    #[fuchsia::test]
    fn test_double_value() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_double_value(1.0, 2.into(), 3.into());
        assert_eq!(block.block_type(), Some(BlockType::DoubleValue));
        assert_eq!(*block.name_index(), 2);
        assert_eq!(*block.parent_index(), 3);
        assert_eq!(block.value(), 1.0);
        assert_8_bytes!(container, 0, [0x01, 0x06, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x3f]);

        let mut block = container.block_at_unchecked_mut::<Double>(BlockIndex::EMPTY);
        block.set(5.0);
        assert_eq!(block.value(), 5.0);
        assert_8_bytes!(container, 0, [0x01, 0x06, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x14, 0x40]);
    }

    #[fuchsia::test]
    fn test_int_value() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_int_value(1, 2.into(), 3.into());
        assert_eq!(block.block_type(), Some(BlockType::IntValue));
        assert_eq!(*block.name_index(), 2);
        assert_eq!(*block.parent_index(), 3);
        assert_eq!(block.value(), 1);
        assert_8_bytes!(container, 0, [0x1, 0x04, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let mut block = container.block_at_unchecked_mut::<Int>(BlockIndex::EMPTY);
        block.set(-5);
        assert_eq!(block.value(), -5);
        assert_8_bytes!(container, 0, [0x1, 0x04, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0xfb, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    }

    #[fuchsia::test]
    fn test_uint_value() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_uint_value(1, 2.into(), 3.into());
        assert_eq!(block.block_type(), Some(BlockType::UintValue));
        assert_eq!(*block.name_index(), 2);
        assert_eq!(*block.parent_index(), 3);
        assert_eq!(block.value(), 1);
        assert_8_bytes!(container, 0, [0x01, 0x05, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let mut block = container.block_at_unchecked_mut::<Uint>(BlockIndex::EMPTY);
        block.set(5);
        assert_eq!(block.value(), 5);
        assert_8_bytes!(container, 0, [0x01, 0x05, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_bool_value() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_bool_value(false, 2.into(), 3.into());
        assert_eq!(block.block_type(), Some(BlockType::BoolValue));
        assert_eq!(*block.name_index(), 2);
        assert_eq!(*block.parent_index(), 3);
        assert!(!block.value());
        assert_8_bytes!(container, 0, [0x01, 0x0D, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        let mut block = container.block_at_unchecked_mut::<Bool>(BlockIndex::EMPTY);
        block.set(true);
        assert!(block.value());
        assert_8_bytes!(container, 0, [0x01, 0x0D, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_become_node() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_node(2.into(), 3.into());
        assert_eq!(block.block_type(), Some(BlockType::NodeValue));
        assert_eq!(*block.name_index(), 2);
        assert_eq!(*block.parent_index(), 3);
        assert_8_bytes!(container, 0, [0x01, 0x03, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[fuchsia::test]
    fn test_property() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block =
            get_reserved(&mut container).become_property(2.into(), 3.into(), PropertyFormat::Bytes);
        assert_eq!(block.block_type(), Some(BlockType::BufferValue));
        assert_eq!(*block.name_index(), 2);
        assert_eq!(*block.parent_index(), 3);
        assert_eq!(block.format(), Some(PropertyFormat::Bytes));
        assert_8_bytes!(container, 0, [0x01, 0x07, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10]);

        let (mut bad_container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut bad_format_bytes = [0u8; constants::MIN_ORDER_SIZE];
        bad_format_bytes[15] = 0x30;
        bad_container.copy_from_slice(&bad_format_bytes);
        let bad_block = Block::<_, Buffer>::new(&bad_container, BlockIndex::EMPTY);
        assert_eq!(bad_block.format(), None);

        let mut block = container.block_at_unchecked_mut::<Buffer>(BlockIndex::EMPTY);
        block.set_extent_index(4.into());
        assert_eq!(*block.extent_index(), 4);
        assert_8_bytes!(container, 0, [0x01, 0x07, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x10]);

        let mut block = container.block_at_unchecked_mut::<Buffer>(BlockIndex::EMPTY);
        block.set_total_length(10);
        assert_eq!(block.total_length(), 10);
        assert_8_bytes!(container, 0, [0x01, 0x07, 0x03, 0x00, 0x00, 0x02, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x0a, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x10]);
    }

    #[fuchsia::test]
    fn test_name() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block = get_reserved(&mut container).become_name("test-rust-inspect");
        assert_eq!(block.block_type(), Some(BlockType::Name));
        assert_eq!(block.length(), 17);
        assert_eq!(block.contents().unwrap(), "test-rust-inspect");
        assert_8_bytes!(container, 0, [0x01, 0x09, 0x11, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let slice = container.get_slice_at(8, 17).unwrap();
        assert_eq!(slice, "test-rust-inspect".as_bytes());
        let slice = container.get_slice_at(25, 7).unwrap();
        assert_eq!(slice, [0, 0, 0, 0, 0, 0, 0]);

        container.set_value::<u8>(24, 0xff).unwrap();
        let bad_block = Block::<_, Name>::new(&container, BlockIndex::EMPTY);
        assert_eq!(bad_block.length(), 17); // Check we copied correctly
        assert!(bad_block.contents().is_err()); // Make sure we get Error not panic

        // Test to make sure UTF8 strings are truncated safely if they're too long for the block,
        // even if the last character is multibyte and would be chopped in the middle.
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block = get_reserved(&mut container).become_name("abcdefghijklmnopqrstuvwxyz");
        assert_eq!(block.contents().unwrap(), "abcdefghijklmnopqrstuvwx");

        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block = get_reserved(&mut container).become_name("😀abcdefghijklmnopqrstuvwxyz");
        assert_eq!(block.contents().unwrap(), "😀abcdefghijklmnopqrst");

        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block = get_reserved(&mut container).become_name("abcdefghijklmnopqrstu😀");
        assert_eq!(block.contents().unwrap(), "abcdefghijklmnopqrstu");
        let byte = container.get_value::<u8>(31).unwrap();
        assert_eq!(byte, 0);

        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 2).unwrap();
        let block = get_reserved(&mut container).become_name("🦀🦀🦀🦀🦀🦀🦀🦀");
        assert_eq!(block.contents().unwrap(), "🦀🦀🦀🦀🦀🦀");
    }

    #[fuchsia::test]
    fn test_invalid_type_for_array() {
        let (mut container, _storage) = Container::read_and_write(2048).unwrap();
        container.get_slice_mut_at(24, 2048 - 24).unwrap().fill(14);

        fn become_array<S: ArraySlotKind>(
            container: &mut Container,
            format: ArrayFormat,
        ) -> Result<Block<&mut Container, Array<S>>, Error> {
            get_reserved_of_order(container, 4).become_array_value::<S>(
                4,
                format,
                BlockIndex::EMPTY,
                BlockIndex::EMPTY,
            )
        }

        assert!(become_array::<Int>(&mut container, ArrayFormat::Default).is_ok());
        assert!(become_array::<Uint>(&mut container, ArrayFormat::Default).is_ok());
        assert!(become_array::<Double>(&mut container, ArrayFormat::Default).is_ok());
        assert!(become_array::<StringRef>(&mut container, ArrayFormat::Default).is_ok());

        for format in [ArrayFormat::LinearHistogram, ArrayFormat::ExponentialHistogram] {
            assert!(become_array::<Int>(&mut container, format).is_ok());
            assert!(become_array::<Uint>(&mut container, format).is_ok());
            assert!(become_array::<Double>(&mut container, format).is_ok());
            assert!(become_array::<StringRef>(&mut container, format).is_err());
        }
    }

    // In this test, we actually don't care about generating string reference
    // blocks at all. That is tested elsewhere. All we care about is that the indexes
    // put in a certain slot come out of the slot correctly. Indexes are u32, so
    // for simplicity this test just uses meaningless numbers instead of creating actual
    // indexable string reference values.
    #[fuchsia::test]
    fn test_string_arrays() {
        let (mut container, _storage) = Container::read_and_write(2048).unwrap();
        container.get_slice_mut_at(48, 2048 - 48).unwrap().fill(14);

        let parent_index = BlockIndex::new(0);
        let name_index = BlockIndex::new(1);
        let mut block = get_reserved(&mut container)
            .become_array_value::<StringRef>(4, ArrayFormat::Default, name_index, parent_index)
            .unwrap();

        for i in 0..4 {
            block.set_string_slot(i, ((i + 4) as u32).into());
        }

        for i in 0..4 {
            let read_index = block.get_string_index_at(i).unwrap();
            assert_eq!(*read_index, (i + 4) as u32);
        }

        assert_8_bytes!(
            container,
            0,
            [
                0x01, 0x0b, 0x00, /* parent */
                0x00, 0x00, 0x01, /* name_index */
                0x00, 0x00
            ]
        );
        assert_8_bytes!(container, 8, [0x0E, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        for i in 0..4 {
            let slice = container.get_slice_at(16 + (i * 4), 4).unwrap();
            assert_eq!(slice, [(i as u8 + 4), 0x00, 0x00, 0x00]);
        }
    }

    #[fuchsia::test]
    fn become_array() {
        // primarily these tests are making sure that arrays clear their payload space
        let (mut container, _storage) = Container::read_and_write(128).unwrap();
        container.get_slice_mut_at(16, 128 - 16).unwrap().fill(1);

        let _ = Block::free(&mut container, BlockIndex::EMPTY, 7, BlockIndex::EMPTY)
            .unwrap()
            .become_reserved()
            .become_array_value::<Int>(
                14,
                ArrayFormat::Default,
                BlockIndex::EMPTY,
                BlockIndex::EMPTY,
            )
            .unwrap();
        let slice = container.get_slice_at(16, 128 - 16).unwrap();
        slice
            .iter()
            .enumerate()
            .for_each(|(index, i)| assert_eq!(*i, 0, "failed: byte = {} at index {}", *i, index));

        container.get_slice_mut_at(16, 128 - 16).unwrap().fill(1);
        let _ = Block::free(&mut container, BlockIndex::EMPTY, 7, BlockIndex::EMPTY)
            .unwrap()
            .become_reserved()
            .become_array_value::<Int>(
                14,
                ArrayFormat::LinearHistogram,
                BlockIndex::EMPTY,
                BlockIndex::EMPTY,
            )
            .unwrap();
        let slice = container.get_slice_at(16, 128 - 16).unwrap();
        slice
            .iter()
            .enumerate()
            .for_each(|(index, i)| assert_eq!(*i, 0, "failed: byte = {} at index {}", *i, index));

        container.get_slice_mut_at(16, 128 - 16).unwrap().fill(1);
        let _ = Block::free(&mut container, BlockIndex::EMPTY, 7, BlockIndex::EMPTY)
            .unwrap()
            .become_reserved()
            .become_array_value::<Int>(
                14,
                ArrayFormat::ExponentialHistogram,
                BlockIndex::EMPTY,
                BlockIndex::EMPTY,
            )
            .unwrap();
        let slice = container.get_slice_at(16, 128 - 16).unwrap();
        slice
            .iter()
            .enumerate()
            .for_each(|(index, i)| assert_eq!(*i, 0, "failed: byte = {} at index {}", *i, index));

        container.get_slice_mut_at(16, 128 - 16).unwrap().fill(1);
        let _ = Block::free(&mut container, BlockIndex::EMPTY, 7, BlockIndex::EMPTY)
            .unwrap()
            .become_reserved()
            .become_array_value::<StringRef>(
                28,
                ArrayFormat::Default,
                BlockIndex::EMPTY,
                BlockIndex::EMPTY,
            )
            .unwrap();
        let slice = container.get_slice_at(16, 128 - 16).unwrap();
        slice
            .iter()
            .enumerate()
            .for_each(|(index, i)| assert_eq!(*i, 0, "failed: byte = {} at index {}", *i, index));
    }

    #[fuchsia::test]
    fn uint_array_value() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 4).unwrap();
        let mut block = Block::free(&mut container, BlockIndex::EMPTY, 2, BlockIndex::EMPTY)
            .unwrap()
            .become_reserved()
            .become_array_value::<Uint>(4, ArrayFormat::LinearHistogram, 3.into(), 2.into())
            .unwrap();

        assert_eq!(block.block_type(), Some(BlockType::ArrayValue));
        assert_eq!(*block.parent_index(), 2);
        assert_eq!(*block.name_index(), 3);
        assert_eq!(block.format(), Some(ArrayFormat::LinearHistogram));
        assert_eq!(block.slots(), 4);
        assert_eq!(block.entry_type(), Some(BlockType::UintValue));

        for i in 0..4 {
            block.set(i, (i as u64 + 1) * 5);
        }
        block.set(4, 3);
        block.set(7, 5);

        assert_8_bytes!(container, 0, [0x02, 0x0b, 0x02, 0x00, 0x00, 0x03, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x15, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        for i in 0..4 {
            assert_8_bytes!(
                container,
                8 * (i + 2),
                [(i as u8 + 1) * 5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
            );
        }

        let (mut bad_container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let mut bad_bytes = [0u8; constants::MIN_ORDER_SIZE];
        container.copy_bytes(&mut bad_bytes[..]);
        bad_bytes[8] = 0x12; // LinearHistogram; Header
        bad_container.copy_from_slice(&bad_bytes);
        let bad_block = Block::<_, Array<Uint>>::new(&bad_container, BlockIndex::EMPTY);
        assert_eq!(bad_block.format(), Some(ArrayFormat::LinearHistogram));
        // Make sure we get Error not panic or BlockType::Header
        assert_eq!(bad_block.entry_type(), None);

        bad_bytes[8] = 0xef; // Not in enum; Not in enum
        bad_container.copy_from_slice(&bad_bytes);
        let bad_block = Block::<_, Array<Uint>>::new(&bad_container, BlockIndex::EMPTY);
        assert_eq!(bad_block.format(), None);
        assert_eq!(bad_block.entry_type(), None);

        let block = container.block_at_unchecked::<Array<Uint>>(BlockIndex::EMPTY);
        for i in 0..4 {
            assert_eq!(block.get(i), Some((i as u64 + 1) * 5));
        }
        assert_eq!(block.get(4), None);
    }

    #[fuchsia::test]
    fn array_slots_bigger_than_block_order() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MAX_ORDER_SIZE).unwrap();
        // A block of size 7 (max) can hold 254 values: 2048B - 8B (header) - 8B (array metadata)
        // gives 2032, which means 254 values of 8 bytes each maximum.
        assert!(
            Block::free(&mut container, BlockIndex::EMPTY, 7, BlockIndex::EMPTY)
                .unwrap()
                .become_reserved()
                .become_array_value::<Int>(257, ArrayFormat::Default, 1.into(), 2.into())
                .is_err()
        );
        assert!(
            Block::free(&mut container, BlockIndex::EMPTY, 7, BlockIndex::EMPTY)
                .unwrap()
                .become_reserved()
                .become_array_value::<Int>(254, ArrayFormat::Default, 1.into(), 2.into())
                .is_ok()
        );

        // A block of size 2 can hold 6 values: 64B - 8B (header) - 8B (array metadata)
        // gives 48, which means 6 values of 8 bytes each maximum.
        assert!(
            Block::free(&mut container, BlockIndex::EMPTY, 2, BlockIndex::EMPTY)
                .unwrap()
                .become_reserved()
                .become_array_value::<Int>(8, ArrayFormat::Default, 1.into(), 2.into())
                .is_err()
        );
        assert!(
            Block::free(&mut container, BlockIndex::EMPTY, 2, BlockIndex::EMPTY)
                .unwrap()
                .become_reserved()
                .become_array_value::<Int>(6, ArrayFormat::Default, 1.into(), 2.into())
                .is_ok()
        );
    }

    #[fuchsia::test]
    fn array_clear() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE * 4).unwrap();

        // Write some sample data in the container after the slot fields.
        let sample = [0xff, 0xff, 0xff];
        container.copy_from_slice_at(48, &sample);

        let mut block = Block::free(&mut container, BlockIndex::EMPTY, 2, BlockIndex::EMPTY)
            .unwrap()
            .become_reserved()
            .become_array_value::<Uint>(4, ArrayFormat::LinearHistogram, 3.into(), 2.into())
            .unwrap();

        for i in 0..4 {
            block.set(i, (i + 1) as u64);
        }

        block.clear(1);

        assert_eq!(1, block.get(0).expect("get uint 0"));
        assert_8_bytes!(container, 16, [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);

        for i in 1..4 {
            let block = container.block_at_unchecked::<Array<Uint>>(BlockIndex::EMPTY);
            assert_eq!(0, block.get(i).expect("get uint"));
            assert_8_bytes!(
                container,
                16 + (i * 8),
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
            );
        }

        // Sample data shouldn't have been overwritten
        let slice = container.get_slice_at(48, 3).unwrap();
        assert_eq!(slice, &sample[..]);
    }

    #[fuchsia::test]
    fn become_link() {
        let (mut container, _storage) =
            Container::read_and_write(constants::MIN_ORDER_SIZE).unwrap();
        let block = get_reserved(&mut container).become_link(
            BlockIndex::new(1),
            BlockIndex::new(2),
            BlockIndex::new(3),
            LinkNodeDisposition::Inline,
        );
        assert_eq!(*block.name_index(), 1);
        assert_eq!(*block.parent_index(), 2);
        assert_eq!(*block.content_index(), 3);
        assert_eq!(block.block_type(), Some(BlockType::LinkValue));
        assert_eq!(block.link_node_disposition(), Some(LinkNodeDisposition::Inline));
        assert_8_bytes!(container, 0, [0x01, 0x0c, 0x02, 0x00, 0x00, 0x01, 0x00, 0x00]);
        assert_8_bytes!(container, 8, [0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10]);
    }

    #[test]
    fn array_capacity_numeric() {
        assert_eq!(2, max_array_capacity::<Int>(1));
        assert_eq!(2 + 4, max_array_capacity::<Int>(2));
        assert_eq!(2 + 4 + 8, max_array_capacity::<Int>(3));
        assert_eq!(2 + 4 + 8 + 16, max_array_capacity::<Int>(4));
        assert_eq!(2 + 4 + 8 + 16 + 32, max_array_capacity::<Int>(5));
        assert_eq!(2 + 4 + 8 + 16 + 32 + 64, max_array_capacity::<Int>(6));
        assert_eq!(2 + 4 + 8 + 16 + 32 + 64 + 128, max_array_capacity::<Int>(7),);
    }

    #[test]
    fn array_capacity_string_reference() {
        assert_eq!(4, max_array_capacity::<StringRef>(1));
        assert_eq!(4 + 8, max_array_capacity::<StringRef>(2));
        assert_eq!(4 + 8 + 16, max_array_capacity::<StringRef>(3));
        assert_eq!(4 + 8 + 16 + 32, max_array_capacity::<StringRef>(4));
        assert_eq!(4 + 8 + 16 + 32 + 64, max_array_capacity::<StringRef>(5));
        assert_eq!(4 + 8 + 16 + 32 + 64 + 128, max_array_capacity::<StringRef>(6));
        assert_eq!(4 + 8 + 16 + 32 + 64 + 128 + 256, max_array_capacity::<StringRef>(7));
    }

    fn get_header(container: &mut Container, size: usize) -> Block<&mut Container, Header> {
        get_reserved(container).become_header(size).unwrap()
    }

    fn get_reserved(container: &mut Container) -> Block<&mut Container, Reserved> {
        get_reserved_of_order(container, 1)
    }

    fn get_reserved_of_order(
        container: &mut Container,
        order: u8,
    ) -> Block<&mut Container, Reserved> {
        let block = Block::free(container, BlockIndex::EMPTY, order, BlockIndex::new(0)).unwrap();
        block.become_reserved()
    }
}
