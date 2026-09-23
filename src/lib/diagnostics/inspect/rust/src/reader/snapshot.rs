// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A snapshot represents all the loaded blocks of the VMO in a way that we can reconstruct the
//! implicit tree.

use crate::Inspector;
use crate::reader::LinkValue;
use crate::reader::error::ReaderError;
use crate::reader::readable_tree::SnapshotSource;
use diagnostics_hierarchy::{ArrayContent, Property};
use inspect_format::{
    Array, Block, BlockAccessorExt, BlockContainer, BlockIndex, BlockType, Bool, Buffer, Container,
    CopyBytes, Double, Extent, Header, Int, Link, Name, PropertyFormat, ReadBytes, StringRef, Uint,
    Unknown, ValueBlockKind, constants, utils,
};
use std::cmp;

pub use crate::reader::tree_reader::SnapshotTree;

/// Enables to scan all the blocks in a given buffer.
#[derive(Debug)]
pub struct Snapshot {
    /// The buffer read from an Inspect VMO.
    buffer: BackingBuffer,
}

/// A scanned block.
pub type ScannedBlock<'a, K> = Block<&'a BackingBuffer, K>;

const SNAPSHOT_TRIES: u64 = 1024;

impl Snapshot {
    /// Returns an iterator that returns all the Blocks in the buffer.
    pub fn scan(&self) -> BlockIterator<'_> {
        BlockIterator::from(&self.buffer)
    }

    /// Gets the block at the given |index|.
    pub fn get_block(&self, index: BlockIndex) -> Result<ScannedBlock<'_, Unknown>, ReaderError> {
        if index.offset() < self.buffer.len() {
            Ok(self.buffer.block_at(index))
        } else {
            Err(ReaderError::GetBlock(index))
        }
    }

    /// Try to take a consistent snapshot of the given VMO once.
    ///
    /// Returns a Snapshot on success or an Error if a consistent snapshot could not be taken.
    pub fn try_once_from_vmo(source: &SnapshotSource) -> Result<Snapshot, ReaderError> {
        Snapshot::try_once_with_callback(source, &mut || {})
    }

    fn try_once_with_callback<F>(
        source: &SnapshotSource,
        read_callback: &mut F,
    ) -> Result<Snapshot, ReaderError>
    where
        F: FnMut(),
    {
        // Read the generation count one time
        let mut header_bytes: [u8; 32] = [0; 32];
        source.copy_bytes(&mut header_bytes);
        let Some(header_block) = header_bytes.maybe_block_at::<Header>(BlockIndex::HEADER) else {
            return Err(ReaderError::InvalidVmo);
        };
        let generation = header_block.generation_count();
        if generation == constants::VMO_FROZEN {
            #[cfg(target_os = "fuchsia")]
            {
                let info = source.info().map_err(ReaderError::Vmo)?;
                if !info.flags.contains(zx::VmoInfoFlags::IMMUTABLE) {
                    return Err(ReaderError::Vmo(zx::Status::BAD_STATE));
                }
            }
            if let Ok(buffer) = BackingBuffer::try_from(source) {
                return Ok(Snapshot { buffer });
            }
        }

        // Read the buffer
        let vmo_size = if let Some(vmo_size) = header_block.vmo_size()? {
            cmp::min(vmo_size as usize, constants::MAX_VMO_SIZE)
        } else {
            cmp::min(source.len(), constants::MAX_VMO_SIZE)
        };
        let mut buffer = vec![0u8; vmo_size];
        source.copy_bytes(&mut buffer);
        if cfg!(test) {
            read_callback();
        }

        // Read the generation count one more time to ensure the previous buffer read is
        // consistent. It's safe to unwrap this time, we already checked we can read 32 bytes from
        // the slice.
        source.copy_bytes(&mut header_bytes);
        match header_generation_count(&header_bytes) {
            None => Err(ReaderError::InconsistentSnapshot),
            Some(new_generation) if new_generation != generation => {
                Err(ReaderError::InconsistentSnapshot)
            }
            Some(_) => Ok(Snapshot { buffer: BackingBuffer::from(buffer) }),
        }
    }

    fn try_from_with_callback<F>(
        source: &SnapshotSource,
        mut read_callback: F,
    ) -> Result<Snapshot, ReaderError>
    where
        F: FnMut(),
    {
        let mut i = 0;
        loop {
            match Snapshot::try_once_with_callback(source, &mut read_callback) {
                Ok(snapshot) => return Ok(snapshot),
                Err(e) => {
                    if i >= SNAPSHOT_TRIES {
                        return Err(e);
                    }
                }
            };
            i += 1;
        }
    }

    pub(crate) fn get_name(&self, index: BlockIndex) -> Option<String> {
        let block = self.get_block(index).ok()?;
        match block.block_type()? {
            BlockType::Name => self.load_name(block.cast::<Name>().unwrap()),
            BlockType::StringReference => {
                self.load_string_reference(block.cast::<StringRef>().unwrap()).ok()
            }
            _ => None,
        }
    }

    pub(crate) fn load_name(&self, block: ScannedBlock<'_, Name>) -> Option<String> {
        block.contents().ok().map(|s| s.to_string())
    }

    pub(crate) fn load_string_reference(
        &self,
        block: ScannedBlock<'_, StringRef>,
    ) -> Result<String, ReaderError> {
        let mut data = block.inline_data()?.to_vec();
        let total_length = block.total_length();
        if total_length <= data.len() {
            return Ok(String::from_utf8_lossy(&data[..total_length]).to_string());
        }

        let extent_index = block.next_extent();
        let still_to_read_length = total_length - data.len();
        data.append(&mut self.read_extents(still_to_read_length, extent_index)?);

        Ok(String::from_utf8_lossy(&data).to_string())
    }

    pub(crate) fn parse_primitive_property<'a, K>(
        &self,
        block: ScannedBlock<'a, K>,
    ) -> Result<Property, ReaderError>
    where
        ScannedBlock<'a, K>: MakePrimitiveProperty,
        K: ValueBlockKind,
    {
        let name_index = block.name_index();
        let name = self.get_name(name_index).ok_or(ReaderError::ParseName(name_index))?;
        Ok(block.make_property(name))
    }

    pub(crate) fn parse_array_property(
        &self,
        block: ScannedBlock<'_, Array<Unknown>>,
    ) -> Result<Property, ReaderError> {
        let name_index = block.name_index();
        let name = self.get_name(name_index).ok_or(ReaderError::ParseName(name_index))?;
        let array_slots = block.slots();
        // Safety: So long as the array is valid, array_capacity will return a valid value.
        let capacity = block.capacity().ok_or(ReaderError::InvalidVmo)?;
        if capacity < array_slots {
            return Err(ReaderError::AttemptedToReadTooManyArraySlots(block.index()));
        }
        let value_indexes = 0..array_slots;
        let format = block.format().ok_or(ReaderError::InvalidVmo)?;
        let parsed_property = match block.entry_type() {
            Some(BlockType::IntValue) => {
                let block = block.cast_array_unchecked::<Int>();
                let values = value_indexes
                    .map(|i| block.get(i).ok_or(ReaderError::InvalidVmo))
                    .collect::<Result<Vec<i64>, _>>()?;
                Property::IntArray(
                    name,
                    // Safety: if the block is an array, it must have an array format.
                    // We have already verified it is an array.
                    ArrayContent::new(values, format)?,
                )
            }
            Some(BlockType::UintValue) => {
                let block = block.cast_array_unchecked::<Uint>();
                let values = value_indexes
                    .map(|i| block.get(i).ok_or(ReaderError::InvalidVmo))
                    .collect::<Result<Vec<u64>, _>>()?;
                Property::UintArray(
                    name,
                    // Safety: if the block is an array, it must have an array format.
                    // We have already verified it is an array.
                    ArrayContent::new(values, format)?,
                )
            }
            Some(BlockType::DoubleValue) => {
                let block = block.cast_array_unchecked::<Double>();
                let values = value_indexes
                    .map(|i| block.get(i).ok_or(ReaderError::InvalidVmo))
                    .collect::<Result<Vec<f64>, _>>()?;
                Property::DoubleArray(
                    name,
                    // Safety: if the block is an array, it must have an array format.
                    // We have already verified it is an array.
                    ArrayContent::new(values, format)?,
                )
            }
            Some(BlockType::StringReference) => {
                let block = block.cast_array_unchecked::<StringRef>();
                let values = value_indexes
                    .map(|value_index| {
                        let string_idx = block
                            .get_string_index_at(value_index)
                            .ok_or(ReaderError::InvalidVmo)?;
                        // default initialize unset values -- 0 index is never a string, it is always
                        // the header block
                        if string_idx == BlockIndex::EMPTY {
                            return Ok(String::new());
                        }

                        let ref_block = self
                            .get_block(string_idx)?
                            .cast::<StringRef>()
                            .ok_or(ReaderError::InvalidVmo)?;
                        self.load_string_reference(ref_block)
                    })
                    .collect::<Result<Vec<String>, _>>()?;
                Property::StringList(name, values)
            }
            _ => return Err(ReaderError::UnexpectedArrayEntryFormat(block.entry_type_raw())),
        };
        Ok(parsed_property)
    }

    pub(crate) fn parse_property(
        &self,
        block: ScannedBlock<'_, Buffer>,
    ) -> Result<Property, ReaderError> {
        let name_index = block.name_index();
        let name = self.get_name(name_index).ok_or(ReaderError::ParseName(name_index))?;
        let data_index = block.extent_index();
        match block.format().ok_or(ReaderError::InvalidVmo)? {
            PropertyFormat::String => {
                let total_length = block.total_length();
                let buffer = self.read_extents(total_length, data_index)?;
                Ok(Property::String(name, String::from_utf8_lossy(&buffer).to_string()))
            }
            PropertyFormat::Bytes => {
                let total_length = block.total_length();
                let buffer = self.read_extents(total_length, data_index)?;
                Ok(Property::Bytes(name, buffer))
            }
            PropertyFormat::StringReference => {
                let data_head = self
                    .get_block(data_index)?
                    .cast::<StringRef>()
                    .ok_or(ReaderError::InvalidVmo)?;
                Ok(Property::String(name, self.load_string_reference(data_head)?))
            }
        }
    }

    pub(crate) fn parse_link(
        &self,
        block: ScannedBlock<'_, Link>,
    ) -> Result<LinkValue, ReaderError> {
        let name_index = block.name_index();
        let name = self.get_name(name_index).ok_or(ReaderError::ParseName(name_index))?;
        let link_content_index = block.content_index();
        let content =
            self.get_name(link_content_index).ok_or(ReaderError::ParseName(link_content_index))?;
        let disposition = block.link_node_disposition().ok_or(ReaderError::InvalidVmo)?;
        Ok(LinkValue { name, content, disposition })
    }

    // Incrementally add the contents of each extent in the extent linked list
    // until we reach the last extent or the maximum expected length.
    pub(crate) fn read_extents(
        &self,
        total_length: usize,
        first_extent: BlockIndex,
    ) -> Result<Vec<u8>, ReaderError> {
        if total_length > self.buffer.len() {
            return Err(ReaderError::InvalidVmo);
        }
        let mut buffer = vec![0u8; total_length];
        let mut offset = 0;
        let mut extent_index = first_extent;
        while extent_index != BlockIndex::EMPTY && offset < total_length {
            let extent = self
                .get_block(extent_index)
                .and_then(|b| b.cast::<Extent>().ok_or(ReaderError::InvalidVmo))?;
            let content = extent.contents()?;
            if content.is_empty() {
                break;
            }
            let extent_length = cmp::min(total_length - offset, content.len());
            buffer[offset..offset + extent_length].copy_from_slice(&content[..extent_length]);
            offset += extent_length;
            extent_index = extent.next_extent();
        }

        Ok(buffer)
    }

    // Used for snapshot tests.
    #[cfg(test)]
    pub fn build(bytes: &[u8]) -> Self {
        Snapshot { buffer: BackingBuffer::from(bytes.to_vec()) }
    }
}

/// Reads the given 16 bytes as an Inspect Block Header and returns the
/// generation count if the header is valid: correct magic number, version number
/// and nobody is writing to it.
fn header_generation_count<T: ReadBytes>(bytes: &T) -> Option<u64> {
    if bytes.len() < 16 {
        return None;
    }
    let block = bytes.maybe_block_at::<Header>(BlockIndex::HEADER)?;
    if block.magic_number() == constants::HEADER_MAGIC_NUMBER
        && block.version() <= constants::HEADER_VERSION_NUMBER
        && !block.is_locked()
    {
        return Some(block.generation_count());
    }
    None
}

/// Construct a snapshot from a byte vector.
impl TryFrom<Vec<u8>> for Snapshot {
    type Error = ReaderError;

    fn try_from(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        if header_generation_count(&bytes).is_some() {
            Ok(Snapshot { buffer: BackingBuffer::from(bytes) })
        } else {
            Err(ReaderError::MissingHeaderOrLocked)
        }
    }
}

impl TryFrom<&Inspector> for Snapshot {
    type Error = ReaderError;

    fn try_from(inspector: &Inspector) -> Result<Self, Self::Error> {
        let handle = inspector.get_storage_handle();
        let storage = handle.as_ref().ok_or(ReaderError::NoOpInspector)?;
        Snapshot::try_from_with_callback(storage, || {})
    }
}

#[cfg(target_os = "fuchsia")]
impl TryFrom<&zx::Vmo> for Snapshot {
    type Error = ReaderError;

    fn try_from(vmo: &zx::Vmo) -> Result<Self, Self::Error> {
        Snapshot::try_from_with_callback(vmo, || {})
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl TryFrom<&Vec<u8>> for Snapshot {
    type Error = ReaderError;

    fn try_from(buffer: &Vec<u8>) -> Result<Self, Self::Error> {
        Snapshot::try_from_with_callback(buffer, || {})
    }
}

/// Iterates over a byte array containing Inspect API blocks and returns the
/// blocks in order.
pub struct BlockIterator<'a> {
    /// Current offset at which the iterator is reading.
    offset: usize,

    /// The bytes being read.
    container: &'a BackingBuffer,
}

impl<'a> From<&'a BackingBuffer> for BlockIterator<'a> {
    fn from(container: &'a BackingBuffer) -> Self {
        BlockIterator { offset: 0, container }
    }
}

impl<'a> Iterator for BlockIterator<'a> {
    type Item = ScannedBlock<'a, Unknown>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.container.len() {
            return None;
        }
        let index = BlockIndex::from_offset(self.offset);
        let block = self.container.block_at(index);
        if self.container.len() - self.offset < utils::order_to_size(block.order()) {
            return None;
        }
        self.offset += utils::order_to_size(block.order());
        Some(block)
    }
}

#[derive(Debug)]
pub enum BackingBuffer {
    Bytes(Vec<u8>),
    Container(Container),
}

#[cfg(target_os = "fuchsia")]
impl TryFrom<&zx::Vmo> for BackingBuffer {
    type Error = ReaderError;
    fn try_from(source: &zx::Vmo) -> Result<Self, Self::Error> {
        let container = Container::read_only(source)?;
        Ok(BackingBuffer::Container(container))
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl TryFrom<&Vec<u8>> for BackingBuffer {
    type Error = ReaderError;
    fn try_from(source: &Vec<u8>) -> Result<Self, Self::Error> {
        let container = Container::read_only(source);
        Ok(BackingBuffer::Container(container))
    }
}

impl From<Vec<u8>> for BackingBuffer {
    fn from(v: Vec<u8>) -> Self {
        BackingBuffer::Bytes(v)
    }
}

impl ReadBytes for BackingBuffer {
    fn get_slice_at(&self, offset: usize, size: usize) -> Option<&[u8]> {
        match &self {
            BackingBuffer::Container(m) => m.get_slice_at(offset, size),
            BackingBuffer::Bytes(b) => b.get_slice_at(offset, size),
        }
    }
}

impl BlockContainer for BackingBuffer {
    type Data = Self;
    type ShareableData = ();

    fn len(&self) -> usize {
        match &self {
            BackingBuffer::Container(m) => m.len(),
            BackingBuffer::Bytes(v) => v.len(),
        }
    }
}

pub(crate) trait MakePrimitiveProperty {
    fn make_property(&self, name: String) -> Property;
}

impl MakePrimitiveProperty for ScannedBlock<'_, Int> {
    fn make_property(&self, name: String) -> Property {
        Property::Int(name, self.value())
    }
}

impl MakePrimitiveProperty for ScannedBlock<'_, Uint> {
    fn make_property(&self, name: String) -> Property {
        Property::Uint(name, self.value())
    }
}

impl MakePrimitiveProperty for ScannedBlock<'_, Double> {
    fn make_property(&self, name: String) -> Property {
        Property::Double(name, self.value())
    }
}

impl MakePrimitiveProperty for ScannedBlock<'_, Bool> {
    fn make_property(&self, name: String) -> Property {
        Property::Bool(name, self.value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use inspect_format::{BlockAccessorMutExt, WriteBytes};

    #[cfg(target_os = "fuchsia")]
    macro_rules! get_snapshot {
        ($container:ident, $storage:expr, $callback:expr) => {
            Snapshot::try_from_with_callback(&$storage, $callback)
        };
    }

    #[cfg(not(target_os = "fuchsia"))]
    macro_rules! get_snapshot {
        ($container:ident, $storage:expr, $callback:expr) => {{
            let _storage = $storage;
            let slice = $container.get_slice($container.len()).unwrap().to_vec();
            Snapshot::try_from_with_callback(&slice, $callback)
        }};
    }

    #[fuchsia::test]
    fn test_string_reference_short_total_length() -> Result<(), Error> {
        let size = 4096;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;

        let mut str_block = Block::free(&mut container, 2.into(), 0, BlockIndex::EMPTY)?
            .become_reserved()
            .become_string_reference();
        str_block.write_inline(b"hello");
        str_block.set_total_length(0);

        let snapshot = get_snapshot!(container, storage, || {})?;
        let name = snapshot.get_name(2.into());
        assert_eq!(name, Some("".to_string()));

        Ok(())
    }

    #[fuchsia::test]
    fn scan() -> Result<(), Error> {
        let size = 4096;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        let _ = Block::free(&mut container, 2.into(), 2, BlockIndex::EMPTY)?
            .become_reserved()
            .become_extent(6.into());
        let _ = Block::free(&mut container, 6.into(), 0, BlockIndex::EMPTY)?
            .become_reserved()
            .become_int_value(1, 3.into(), 4.into());

        let snapshot = get_snapshot!(container, storage, || {})?;

        // Scan blocks
        let mut blocks = snapshot.scan();

        let block = blocks.next().unwrap().cast::<Header>().unwrap();
        assert_eq!(block.block_type(), Some(BlockType::Header));
        assert_eq!(*block.index(), 0);
        assert_eq!(block.order(), constants::HEADER_ORDER);
        assert_eq!(block.magic_number(), constants::HEADER_MAGIC_NUMBER);
        assert_eq!(block.version(), constants::HEADER_VERSION_NUMBER);

        let block = blocks.next().unwrap().cast::<Extent>().unwrap();
        assert_eq!(block.block_type(), Some(BlockType::Extent));
        assert_eq!(*block.index(), 2);
        assert_eq!(block.order(), 2);
        assert_eq!(*block.next_extent(), 6);

        let block = blocks.next().unwrap().cast::<Int>().unwrap();
        assert_eq!(block.block_type(), Some(BlockType::IntValue));
        assert_eq!(*block.index(), 6);
        assert_eq!(block.order(), 0);
        assert_eq!(*block.name_index(), 3);
        assert_eq!(*block.parent_index(), 4);
        assert_eq!(block.value(), 1);

        assert!(blocks.all(|b| b.block_type() == Some(BlockType::Free)));

        // Verify get_block
        assert_eq!(snapshot.get_block(0.into()).unwrap().block_type(), Some(BlockType::Header));
        assert_eq!(snapshot.get_block(2.into()).unwrap().block_type(), Some(BlockType::Extent));
        assert_eq!(snapshot.get_block(6.into()).unwrap().block_type(), Some(BlockType::IntValue));
        assert_eq!(snapshot.get_block(7.into()).unwrap().block_type(), Some(BlockType::Free));
        let bad_index = BlockIndex::from(4096);
        assert_matches!(
            snapshot.get_block(bad_index),
            Err(ReaderError::GetBlock(index)) if index == bad_index
        );

        Ok(())
    }

    #[fuchsia::test]
    fn scan_bad_header() -> Result<(), Error> {
        let (mut container, storage) = Container::read_and_write(4096).unwrap();

        // create a header block with an invalid version number
        container.copy_from_slice(&[
            0x00, /* order/reserved */
            0x02, /* type */
            0xff, /* invalid version number */
            b'I', b'N', b'S', b'P',
        ]);
        assert!(get_snapshot!(container, storage, || {}).is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn invalid_type() -> Result<(), Error> {
        let (mut container, storage) = Container::read_and_write(4096).unwrap();
        container.copy_from_slice(&[0x00, 0xff, 0x01]);
        assert!(get_snapshot!(container, storage, || {}).is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn invalid_order() -> Result<(), Error> {
        let (mut container, storage) = Container::read_and_write(4096).unwrap();
        container.copy_from_slice(&[0xff, 0xff]);
        assert!(get_snapshot!(container, storage, || {}).is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn invalid_pending_write() -> Result<(), Error> {
        let size = 4096;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let mut header = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        header.lock();
        assert!(get_snapshot!(container, storage, || {}).is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn invalid_magic_number() -> Result<(), Error> {
        let size = 4096;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let mut header = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        header.set_magic(3);
        assert!(get_snapshot!(container, storage, || {}).is_err());
        Ok(())
    }

    #[fuchsia::test]
    fn invalid_generation_count() -> Result<(), Error> {
        let size = 4096;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        let result = get_snapshot!(container, storage, || {
            let mut header = container.block_at_unchecked_mut::<Header>(BlockIndex::HEADER);
            header.lock();
            header.unlock();
        });
        #[cfg(target_os = "fuchsia")]
        assert!(result.is_err());
        // When in the host, we don't have underlying shared memory, so this can't fail as we
        // had already cloned the underlying vector.
        #[cfg(not(target_os = "fuchsia"))]
        assert!(result.is_ok());
        Ok(())
    }

    #[fuchsia::test]
    fn snapshot_from_few_bytes() {
        let values = (0u8..16).collect::<Vec<u8>>();
        assert!(Snapshot::try_from(values.clone()).is_err());
        assert!(Snapshot::try_from(values).is_err());
        assert!(Snapshot::try_from(vec![]).is_err());
        assert!(Snapshot::try_from(vec![0u8, 1, 2, 3, 4]).is_err());
    }

    #[fuchsia::test]
    fn snapshot_frozen_vmo() -> Result<(), Error> {
        let size = 4096;
        let (mut container, parent_storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        container.copy_from_slice_at(8, &[0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

        let snapshot;
        #[cfg(target_os = "fuchsia")]
        {
            let storage = parent_storage.create_child(
                zx::VmoChildOptions::SNAPSHOT | zx::VmoChildOptions::NO_WRITE,
                0,
                size as u64,
            )?;
            snapshot = get_snapshot!(container, storage, || {})?;
        };
        #[cfg(not(target_os = "fuchsia"))]
        {
            // Silence unused variable warning, which happens to be set to a unit value.
            #[allow(clippy::let_unit_value)]
            let _ = parent_storage;
            snapshot = get_snapshot!(container, (), || {})?
        };
        assert!(matches!(snapshot.buffer, BackingBuffer::Container(_)));

        let (mut container2, storage2) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container2,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        container2.copy_from_slice_at(8, &[2u8; 8]);
        let snapshot = get_snapshot!(container2, storage2, || {})?;
        assert!(matches!(snapshot.buffer, BackingBuffer::Bytes(_)));

        Ok(())
    }

    // Check that snapshot fails if the VMO is frozen but not immutable.
    // This test is only valid on Fuchsia, where VMOs can be used.
    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn snapshot_frozen_mutable_vmo_fails() -> Result<(), Error> {
        let size = 4096;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;
        container.copy_from_slice_at(8, &[0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

        assert_matches!(
            get_snapshot!(container, storage, || {}),
            Err(ReaderError::Vmo(zx::Status::BAD_STATE))
        );

        Ok(())
    }

    #[fuchsia::test]
    fn snapshot_vmo_with_unused_space() -> Result<(), Error> {
        let size = 4 * constants::PAGE_SIZE_BYTES;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(constants::PAGE_SIZE_BYTES)?;

        let snapshot = get_snapshot!(container, storage, || {})?;
        assert_eq!(snapshot.buffer.len(), constants::PAGE_SIZE_BYTES);

        Ok(())
    }

    #[fuchsia::test]
    fn snapshot_vmo_with_very_large_vmo() -> Result<(), Error> {
        let size = 2 * constants::MAX_VMO_SIZE;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let _ = Block::free(
            &mut container,
            BlockIndex::HEADER,
            constants::HEADER_ORDER,
            BlockIndex::EMPTY,
        )?
        .become_reserved()
        .become_header(size)?;

        let snapshot = get_snapshot!(container, storage, || {})?;
        assert_eq!(snapshot.buffer.len(), constants::MAX_VMO_SIZE);

        Ok(())
    }

    #[fuchsia::test]
    fn snapshot_vmo_with_header_without_size_info() -> Result<(), Error> {
        let size = 2 * constants::PAGE_SIZE_BYTES;
        let (mut container, storage) = Container::read_and_write(size).unwrap();
        let mut header = Block::free(&mut container, BlockIndex::HEADER, 0, BlockIndex::EMPTY)?
            .become_reserved()
            .become_header(constants::PAGE_SIZE_BYTES)?;
        header.set_order(0)?;

        let snapshot = get_snapshot!(container, storage, || {})?;
        assert_eq!(snapshot.buffer.len(), size);

        Ok(())
    }
}
