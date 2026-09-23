/*
 * SPDX-License-Identifier: BSD-2-Clause-FreeBSD
 *
 * Copyright (c) 2012, 2010 Zheng Liu <lz@freebsd.org>
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 *
 * $FreeBSD$
 */

// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::readers::Reader;
use crate::structs::{
    BlockGroupDesc32, BlockGroupDesc64, DirEntry2, DirEntryHeader, EntryType, Extent, ExtentHeader,
    ExtentIndex, ExtentTreeNode, FIRST_BG_PADDING, INode, InvalidAddressErrorType, MIN_EXT4_SIZE,
    MINIMUM_INODE_SIZE, ParseToStruct, ParsingError, ROOT_INODE_NUM, SuperBlock, XattrEntryHeader,
    XattrHeader,
};
use once_cell::sync::OnceCell;
use std::collections::BTreeMap;
use std::mem::{size_of, size_of_val};
use std::path::{Component, Path};
use std::str;
use zerocopy::byteorder::little_endian::U32 as LEU32;
use zerocopy::{IntoBytes, SplitByteSlice};

// Assuming/ensuring that we are on a 64bit system where u64 == usize.
assert_eq_size!(u64, usize);

pub struct Parser {
    reader: Box<dyn Reader>,
    super_block: OnceCell<SuperBlock>,
}

pub type XattrMap = BTreeMap<Vec<u8>, Vec<u8>>;

/// An abstraction over an appendable buffer for reading out files.
pub trait DataWriter {
    /// Reserves `size` in the underlying buffer.
    ///
    /// Does not affect the write offset within the buffer.
    fn reserve(&mut self, size: u64) -> Result<(), ParsingError>;

    /// Appends `bytes` to the buffer.
    ///
    /// The internal write offset is incremented by `bytes.len()`. Partial
    /// writes must return an error.
    fn append(&mut self, bytes: &[u8]) -> Result<(), ParsingError>;

    /// Pads the buffer to `target_size`.
    ///
    /// Moves the buffer's write offset _forward_ to `target_size` filling with
    /// zeroes.
    /// Returns an error if the write offset is larger than `target_size`.
    fn pad_to(&mut self, target_size: u64) -> Result<(), ParsingError>;
}

impl DataWriter for Vec<u8> {
    fn reserve(&mut self, size: u64) -> Result<(), ParsingError> {
        let size: usize = size
            .try_into()
            .map_err(|_| ParsingError::Incompatible("Target size out of bounds".to_string()))?;
        self.reserve(size);
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), ParsingError> {
        self.extend_from_slice(bytes);
        Ok(())
    }

    fn pad_to(&mut self, target_size: u64) -> Result<(), ParsingError> {
        let target_size: usize = target_size
            .try_into()
            .map_err(|_| ParsingError::Incompatible("Target size out of bounds".to_string()))?;
        let len = self.len();
        if len > target_size {
            return Err(ParsingError::NotSupported(format!(
                "invalid target size {target_size} for {len}"
            )));
        }
        self.resize(target_size, 0);
        Ok(())
    }
}

#[cfg(target_os = "fuchsia")]
pub struct VmoWriter {
    vmo: zx::Vmo,
    stream: zx::Stream,
}

#[cfg(target_os = "fuchsia")]
impl VmoWriter {
    /// Creates a new `VmoWriter` backed by a VMO of `size` bytes.
    pub fn new(size: u64) -> Result<Self, zx::Status> {
        let vmo = zx::Vmo::create(size)?;
        vmo.set_stream_size(0)?;
        let stream = zx::Stream::create(
            zx::StreamOptions::MODE_APPEND | zx::StreamOptions::MODE_WRITE,
            &vmo,
            0,
        )?;
        Ok(Self { vmo, stream })
    }

    /// Consumes the writer, returning the underlying VMO.
    ///
    /// The inner VMO is guaranteed to be the size given to [`VmoWriter::new`],
    /// but its stream size is set to the amount of bytes written to it via the
    /// [`DataWriter`] implementation.
    pub fn into_vmo(self) -> zx::Vmo {
        self.vmo
    }
}

#[cfg(target_os = "fuchsia")]
impl DataWriter for VmoWriter {
    fn reserve(&mut self, size: u64) -> Result<(), ParsingError> {
        if size > self.vmo.get_size().map_err(ParsingError::VmoError)? {
            return Err(ParsingError::VmoError(zx::Status::NOT_SUPPORTED));
        }
        Ok(())
    }

    fn append(&mut self, bytes: &[u8]) -> Result<(), ParsingError> {
        let written = self
            .stream
            .write(zx::StreamWriteOptions::empty(), bytes)
            .map_err(ParsingError::VmoError)?;
        if written != bytes.len() {
            return Err(ParsingError::VmoError(zx::Status::IO_REFUSED));
        }
        Ok(())
    }

    fn pad_to(&mut self, target_size: u64) -> Result<(), ParsingError> {
        let cur = self.vmo.get_stream_size().map_err(ParsingError::VmoError)?;
        if cur > target_size {
            return Err(ParsingError::NotSupported(format!(
                "invalid target size {target_size} for {cur}"
            )));
        }
        // Note: Given we created the VMO ourselves and we're ensuring we can
        // only seek forward, we're guaranteed this is filled with 0s.
        self.vmo.set_stream_size(target_size).map_err(ParsingError::VmoError)?;
        Ok(())
    }
}

/// Abstracts over block group descriptors of different size
enum BlockGroupDescriptor {
    BGD32(BlockGroupDesc32),
    BGD64(BlockGroupDesc64),
}

impl BlockGroupDescriptor {
    fn inode_table_block(&self) -> u64 {
        match self {
            Self::BGD32(bgd) => u64::from(bgd.ext2bgd_i_tables),
            Self::BGD64(bgd) => {
                u64::from(bgd.base.ext2bgd_i_tables) + (u64::from(bgd.ext4bgd_i_tables_hi) << 32)
            }
        }
    }
}

/// EXT4 Parser
///
/// Takes in a `Reader` that is able to read arbitrary chunks of data from the filesystem image.
///
/// Basic use:
/// let mut parser = Parser::new(VecReader::new(vec_of_u8));
/// let tree = parser.build_fuchsia_tree()
impl Parser {
    pub fn new(reader: Box<dyn Reader>) -> Self {
        Parser { reader, super_block: OnceCell::new() }
    }

    /// Returns the Super Block.
    ///
    /// If the super block has been parsed and saved before, return that.
    /// Else, parse the super block and save it and return it.
    ///
    /// We never need to re-parse the super block in this read-only
    /// implementation.
    fn super_block(&self) -> Result<&SuperBlock, ParsingError> {
        self.super_block.get_or_try_init(|| SuperBlock::parse(&self.reader))
    }

    /// Reads block size from the Super Block.
    pub fn block_size(&self) -> Result<u64, ParsingError> {
        self.super_block()?.block_size()
    }

    /// Reads full raw data from a given block number.
    pub(crate) fn block(&self, block_number: u64) -> Result<Box<[u8]>, ParsingError> {
        if block_number == 0 {
            return Err(ParsingError::InvalidAddress(
                InvalidAddressErrorType::Lower,
                0,
                FIRST_BG_PADDING,
            ));
        }
        let block_size = self.block_size()?;
        let address = block_number
            .checked_mul(block_size)
            .ok_or(ParsingError::BlockNumberOutOfBounds(block_number))?;

        let mut data = vec![0u8; block_size.try_into().unwrap()];
        self.reader.read(address, data.as_mut_slice()).map_err(Into::<ParsingError>::into)?;

        Ok(data.into_boxed_slice())
    }

    /// Returns the address of the given `inode_number` within `self.reader`.
    pub(crate) fn inode_addr(&self, inode_number: u32) -> Result<u64, ParsingError> {
        if inode_number < 1 {
            // INode number 0 is not allowed per ext4 spec.
            return Err(ParsingError::InvalidInode(inode_number));
        }
        let sb = self.super_block()?;
        let block_size = self.block_size()?;

        // The first Block Group starts with:
        // - 1024 byte padding
        // - 1024 byte Super Block
        // Then in the next block, there are many blocks worth of Block Group Descriptors.
        // If the block size is 2048 bytes or larger, then the 1024 byte padding, and the
        // Super Block both fit in the first block (0), and the Block Group Descriptors start
        // at block 1.
        //
        // A 1024 byte block size means the padding takes block 0 and the Super Block takes
        // block 1. This means the Block Group Descriptors start in block 2.
        let bgd_table_offset = if block_size >= MIN_EXT4_SIZE {
            // Padding and Super Block both fit in the first block, so offset to the next
            // block.
            block_size
        } else {
            // Block size is less than 2048. The only valid block size smaller than 2048 is 1024.
            // Padding and Super Block take one block each, so offset to the third block.
            block_size * 2
        };

        let bgd_offset = (inode_number - 1) as u64 / sb.e2fs_ipg.get() as u64
            * sb.block_group_descriptor_size() as u64;
        let bgd = if sb.is_64bit() {
            BlockGroupDescriptor::BGD64(BlockGroupDesc64::from_reader_with_offset(
                &self.reader,
                bgd_table_offset + bgd_offset,
            )?)
        } else {
            BlockGroupDescriptor::BGD32(BlockGroupDesc32::from_reader_with_offset(
                &self.reader,
                bgd_table_offset + bgd_offset,
            )?)
        };

        // Offset could really be anywhere, and the Reader will enforce reading within the
        // filesystem size. Not much can be checked here.
        let inode_table_offset =
            (inode_number - 1) as u64 % sb.e2fs_ipg.get() as u64 * sb.e2fs_inode_size.get() as u64;
        let inode_addr = (bgd.inode_table_block() * block_size) + inode_table_offset;
        if inode_addr < MIN_EXT4_SIZE {
            return Err(ParsingError::InvalidAddress(
                InvalidAddressErrorType::Lower,
                inode_addr,
                MIN_EXT4_SIZE,
            ));
        }
        Ok(inode_addr)
    }

    /// Reads the INode at the given inode number.
    pub fn inode(&self, inode_number: u32) -> Result<INode, ParsingError> {
        INode::from_reader_with_offset(&self.reader, self.inode_addr(inode_number)?)
    }

    /// Helper function to get the root directory INode.
    pub fn root_inode(&self) -> Result<INode, ParsingError> {
        self.inode(ROOT_INODE_NUM)
    }

    /// Reads extent data from a leaf node into `writer`.
    fn extent_data<W: DataWriter>(
        &self,
        extent: &Extent,
        writer: &mut W,
        mut allowance: u64,
    ) -> Result<u64, ParsingError> {
        let block_number = extent.target_block_num();
        let block_count = extent.e_len.get() as u64;
        let block_size = self.block_size()?;
        let mut read_len;

        writer.reserve(block_size * block_count)?;
        let mut total_written = 0u64;

        for i in 0..block_count {
            let block_data = self.block(block_number + i as u64)?;
            if allowance >= block_size {
                read_len = block_size;
            } else {
                read_len = allowance;
            }
            let slice = &block_data[0..read_len.try_into().unwrap()];
            writer.append(slice)?;
            total_written += read_len;
            allowance -= read_len;
        }

        Ok(total_written)
    }

    /// Reads the inode size and raw extent data for a regular file.  Fails if the provided inode is
    /// not a regular file.
    pub fn read_extents(&self, inode_num: u32) -> Result<(u64, Vec<Extent>), ParsingError> {
        let inode = self.inode(inode_num)?;

        // Make sure this is a regular file.
        const IFMT: u16 = 0xf000;
        const IFREG: u16 = 0x8000;
        if u16::from(inode.e2di_mode) & IFMT != IFREG {
            return Err(ParsingError::NotFile);
        }

        let root_extent_tree_node = inode.extent_tree_node()?;
        let mut extents = Vec::new();

        self.iterate_extents_in_tree(&root_extent_tree_node, &mut |extent| {
            extents.push(extent.clone());
            Ok(())
        })?;

        Ok((inode.size(), extents))
    }

    /// Reads extent data from a leaf node into `writer`.
    ///
    /// # Arguments
    /// * `extent`: Extent from which to read data from.
    /// * `writer`: `DataWriter` where data that is read is added.
    /// * `allowance`: The maximum number of bytes to read from the extent. The
    ///    given file allowance is updated on each call to track sizing for an
    ///    entire extent tree.
    fn read_extent_data<W: DataWriter>(
        &self,
        extent: &Extent,
        writer: &mut W,
        allowance: &mut u64,
    ) -> Result<(), ParsingError> {
        let extent_len = self.extent_data(extent, writer, *allowance)?;
        if extent_len > *allowance {
            return Err(ParsingError::ExtentUnexpectedLength(extent_len, *allowance));
        }
        *allowance -= extent_len;
        Ok(())
    }

    /// Reads directory entries from an extent leaf node.
    fn read_dir_entries(
        &self,
        extent: &Extent,
        entries: &mut Vec<DirEntry2>,
    ) -> Result<(), ParsingError> {
        let block_size = self.block_size()?;
        let target_block_offset = extent.target_block_num() * block_size;

        // The `e2d_reclen` of the last entry will be large enough fill the
        // remaining space of the block.
        for block_index in 0..extent.e_len.get() {
            let mut dir_entry_offset = 0u64;
            while (dir_entry_offset + size_of::<DirEntryHeader>() as u64) < block_size {
                let offset =
                    dir_entry_offset + target_block_offset + (block_index as u64 * block_size);

                let de_header = DirEntryHeader::from_reader_with_offset(&self.reader, offset)?;
                let mut de = DirEntry2 {
                    e2d_ino: de_header.e2d_ino,
                    e2d_reclen: de_header.e2d_reclen,
                    e2d_namlen: de_header.e2d_namlen,
                    e2d_type: de_header.e2d_type,
                    e2d_name: [0u8; 255],
                };
                self.reader.read(
                    offset + size_of::<DirEntryHeader>() as u64,
                    &mut de.e2d_name[..de.e2d_namlen as usize],
                )?;

                dir_entry_offset += de.e2d_reclen.get() as u64;

                if de.e2d_ino.get() != 0 {
                    entries.push(de);
                }
            }
        }
        Ok(())
    }

    /// Handles an extent tree leaf node by invoking `extent_handler` for each contained extent.
    fn iterate_extents_in_leaf<B: SplitByteSlice, F: FnMut(&Extent) -> Result<(), ParsingError>>(
        &self,
        extent_tree_node: &ExtentTreeNode<B>,
        extent_handler: &mut F,
    ) -> Result<(), ParsingError> {
        for e_index in 0..extent_tree_node.header.eh_ecount.get() {
            let start = size_of::<Extent>() * e_index as usize;
            let end = start + size_of::<Extent>() as usize;
            let e = Extent::to_struct_ref(
                &(extent_tree_node.entries)[start..end],
                ParsingError::InvalidExtent(start as u64),
            )?;

            extent_handler(e)?;
        }

        Ok(())
    }

    /// Handles traversal down an extent tree.
    pub(crate) fn iterate_extents_in_tree<
        B: SplitByteSlice,
        F: FnMut(&Extent) -> Result<(), ParsingError>,
    >(
        &self,
        extent_tree_node: &ExtentTreeNode<B>,
        extent_handler: &mut F,
    ) -> Result<(), ParsingError> {
        let block_size = self.block_size()?;

        match extent_tree_node.header.eh_depth.get() {
            0 => {
                self.iterate_extents_in_leaf(extent_tree_node, extent_handler)?;
            }
            1..=4 => {
                for e_index in 0..extent_tree_node.header.eh_ecount.get() {
                    let start: usize = size_of::<Extent>() * e_index as usize;
                    let end = start + size_of::<Extent>();
                    let e = ExtentIndex::to_struct_ref(
                        &(extent_tree_node.entries)[start..end],
                        ParsingError::InvalidExtent(start as u64),
                    )?;

                    let next_level_offset = e.target_block_num() as u64 * block_size;

                    let next_extent_header =
                        ExtentHeader::from_reader_with_offset(&self.reader, next_level_offset)?;

                    let entry_count = next_extent_header.eh_ecount.get() as usize;
                    let entry_size = match next_extent_header.eh_depth.get() {
                        0 => size_of::<Extent>(),
                        _ => size_of::<ExtentIndex>(),
                    };
                    let node_size = size_of::<ExtentHeader>() + (entry_count * entry_size);

                    let mut data = vec![0u8; node_size];
                    self.reader.read(next_level_offset, data.as_mut_slice())?;

                    let next_level_node = ExtentTreeNode::parse(data.as_slice())
                        .ok_or(ParsingError::InvalidExtent(next_level_offset))?;

                    self.iterate_extents_in_tree(&next_level_node, extent_handler)?;
                }
            }
            _ => return Err(ParsingError::InvalidExtentHeader),
        };

        Ok(())
    }

    /// Lists directory entries from the directory that is the given Inode.
    ///
    /// Errors if the Inode does not map to a Directory.
    pub fn entries_from_inode(&self, inode: &INode) -> Result<Vec<DirEntry2>, ParsingError> {
        let root_extent_tree_node = inode.extent_tree_node()?;
        let mut dir_entries = Vec::new();

        self.iterate_extents_in_tree(&root_extent_tree_node, &mut |extent| {
            self.read_dir_entries(extent, &mut dir_entries)
        })?;

        Ok(dir_entries)
    }

    /// Gets any DirEntry2 that isn't root.
    ///
    /// Root doesn't have a DirEntry2.
    ///
    /// When dynamic loading of files is supported, this is the required mechanism.
    pub fn entry_at_path(&self, path: &Path) -> Result<DirEntry2, ParsingError> {
        let root_inode = self.root_inode()?;
        let root_entries = self.entries_from_inode(&root_inode)?;
        let mut entry_map = DirEntry2::as_hash_map(root_entries)?;

        let mut components = path.components().peekable();
        let mut component = components.next();

        while component != None {
            match component {
                Some(Component::RootDir) => {
                    // Skip
                }
                Some(Component::Normal(name)) => {
                    let name = name.to_str().ok_or(ParsingError::InvalidInputPath)?;
                    if let Some(entry) = entry_map.remove(name) {
                        if components.peek() == None {
                            return Ok(entry);
                        }
                        match EntryType::from_u8(entry.e2d_type)? {
                            EntryType::Directory => {
                                let inode = self.inode(entry.e2d_ino.get())?;
                                entry_map =
                                    DirEntry2::as_hash_map(self.entries_from_inode(&inode)?)?;
                            }
                            _ => {
                                break;
                            }
                        }
                    }
                }
                _ => {
                    break;
                }
            }
            component = components.next();
        }

        match path.to_str() {
            Some(s) => Err(ParsingError::PathNotFound(s.to_string())),
            None => Err(ParsingError::PathNotFound(
                "Bad path - was not able to convert into string".to_string(),
            )),
        }
    }

    /// Reads all raw data for a given inode into the given writer.
    ///
    /// For a file, this will write the file data. For a symlink,
    /// this will write the symlink target.
    pub fn read_data_into<W: DataWriter>(
        &self,
        inode_num: u32,
        writer: &mut W,
    ) -> Result<(), ParsingError> {
        let inode = self.inode(inode_num)?;
        let mut size_remaining = inode.size();
        writer.reserve(size_remaining.try_into().unwrap())?;

        // Check for symlink with inline data.
        if u16::from(inode.e2di_mode) & 0xa000 != 0 && u32::from(inode.e2di_nblock) == 0 {
            let inline_data = &inode.e2di_blocks[..inode.size().try_into().unwrap()];
            writer.append(inline_data)?;
            return Ok(());
        }

        let root_extent_tree_node = inode.extent_tree_node()?;
        let mut extents = Vec::new();

        self.iterate_extents_in_tree(&root_extent_tree_node, &mut |extent| {
            extents.push(extent.clone());
            Ok(())
        })?;

        let block_size = self.block_size()?;
        let mut current_offset = 0u64;

        // Summarized from https://www.kernel.org/doc/ols/2007/ols2007v2-pages-21-34.pdf,
        // Section 2.2: Extent and ExtentHeader entries must be sorted by logical block number. This
        // enforces that when the extent tree is traversed depth first that a list of extents sorted
        // by logical block number is produced. This is a requirement to produce the proper ordering
        // of bytes within `data` here.
        for extent in extents {
            let buffer_offset = extent.e_blk.get() as u64 * block_size;

            // File may be sparse. Sparse files will have gaps
            // between logical blocks. Fill in any gaps with zeros.
            if buffer_offset > current_offset {
                let gap = buffer_offset - current_offset;
                size_remaining -= gap;
                writer.pad_to(buffer_offset)?;
                current_offset = buffer_offset;
            }

            let size_before = size_remaining;
            self.read_extent_data(&extent, writer, &mut size_remaining)?;
            let written = size_before - size_remaining;
            current_offset += written;
        }

        // If there are zero pages at the end of the file, they won't appear in the extents list.
        // Pad the data with zeroes to the full file length.
        // TODO(https://fxbug.dev/42073237): Add a test for this behavior, once better test infra exists.
        writer.pad_to(inode.size())?;

        Ok(())
    }

    /// Reads all raw data for a given inode into a vector.
    pub fn read_data(&self, inode_num: u32) -> Result<Vec<u8>, ParsingError> {
        let mut data = Vec::new();
        self.read_data_into(inode_num, &mut data)?;
        Ok(data)
    }

    /// Progress through the entire directory tree starting from the given INode.
    ///
    /// If given the root directory INode, this will process through every directory entry in the
    /// filesystem in a DFS manner.
    ///
    /// Takes in a closure that will be called for each entry found.
    /// Closure should return `Ok(true)` in order to continue the process, otherwise the process
    /// will stop.
    ///
    /// Returns Ok(true) if it has indexed its subtree successfully. Otherwise, if the receiver
    /// chooses to cancel indexing early, an Ok(false) is returned and propagated up.
    pub fn index<R, E>(&self, inode: INode, prefix: Vec<&str>, receiver: &mut R) -> Result<bool, E>
    where
        E: From<ParsingError>,
        R: FnMut(&Parser, Vec<&str>, &DirEntry2) -> Result<bool, E>,
    {
        let entries = self.entries_from_inode(&inode)?;
        for entry in entries {
            let entry_name = entry.name()?;
            if entry_name == "." || entry_name == ".." {
                continue;
            }
            let mut name = Vec::new();
            name.append(&mut prefix.clone());
            name.push(entry_name);
            if !receiver(self, name.clone(), &entry)? {
                return Ok(false);
            }
            if EntryType::from_u8(entry.e2d_type)? == EntryType::Directory {
                let inode = self.inode(entry.e2d_ino.get())?;
                if !self.index(inode, name, receiver)? {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Returns the xattrs associated with `inode_number`.
    pub fn inode_xattrs(&self, inode_number: u32) -> Result<XattrMap, ParsingError> {
        let mut xattrs = BTreeMap::new();

        let inode_addr = self.inode_addr(inode_number).expect("Couldn't get inode address");
        let inode =
            INode::from_reader_with_offset(&self.reader, inode_addr).expect("Failed reader");

        let sb = self.super_block().expect("No super block for inode");
        let xattr_magic_addr = inode_addr
            + MINIMUM_INODE_SIZE
            + u64::from(inode.e4di_extra_isize(sb).unwrap_or_default());

        let mut magic = LEU32::ZERO;
        self.reader.read(xattr_magic_addr, magic.as_mut_bytes()).expect("Failed to read xattr");
        if magic.get() == Self::XATTR_MAGIC {
            let first_entry = xattr_magic_addr + size_of_val(&magic) as u64;
            self.read_xattr_entries_from_inode(
                first_entry,
                inode_addr + (sb.e2fs_inode_size.get() as u64),
                &mut xattrs,
            )?;
        }

        let block_number: u64 = inode.facl();
        if block_number > 0 {
            let block = self.block(block_number).expect("Couldn't find block");
            Self::read_xattr_entries_from_block(&block, &mut xattrs)?;
        }

        Ok(xattrs)
    }

    const XATTR_ALIGNMENT: u64 = 4;
    const XATTR_MAGIC: u32 = 0xea020000;

    fn round_up_to_align(x: u64, align: u64) -> u64 {
        let spare = x % align;
        if spare > 0 { x.checked_add(align - spare).expect("Overflow when aligning") } else { x }
    }

    fn is_valid_xattr_entry_header(header: &XattrEntryHeader) -> bool {
        !(header.e_name_len == 0
            && header.e_name_index == 0
            && header.e_value_offs.get() == 0
            && header.e_value_inum.get() == 0)
    }

    fn xattr_prefix_for_name_index(header: &XattrEntryHeader) -> Vec<u8> {
        match header.e_name_index {
            1 => b"user.".to_vec(),
            2 => b"system.posix_acl_access.".to_vec(),
            3 => b"system.posix_acl_default.".to_vec(),
            4 => b"trusted.".to_vec(),
            6 => b"security.".to_vec(),
            7 => b"system.".to_vec(),
            8 => b"system.richacl".to_vec(),
            _ => b"".to_vec(),
        }
    }

    /// Reads all the xattr entries, stored in the inode, from `entries_addr` into `xattrs`.
    fn read_xattr_entries_from_inode(
        &self,
        mut entries_addr: u64,
        inode_end: u64,
        xattrs: &mut XattrMap,
    ) -> Result<(), ParsingError> {
        let value_base_addr = entries_addr;
        while entries_addr + (std::mem::size_of::<XattrEntryHeader>() as u64) < inode_end {
            let head = XattrEntryHeader::from_reader_with_offset(&self.reader, entries_addr)?;
            if !Self::is_valid_xattr_entry_header(&head) {
                break;
            }

            let prefix = Self::xattr_prefix_for_name_index(&head);
            let mut name = Vec::with_capacity(prefix.len() + head.e_name_len as usize);
            name.extend_from_slice(&prefix);
            name.resize(prefix.len() + head.e_name_len as usize, 0);

            self.reader.read(
                entries_addr + size_of::<XattrEntryHeader>() as u64,
                &mut name[prefix.len()..],
            )?;

            let mut value = vec![0u8; head.e_value_size.get() as usize];
            self.reader.read(value_base_addr + u64::from(head.e_value_offs), &mut value)?;
            xattrs.insert(name, value);

            entries_addr += size_of::<XattrEntryHeader>() as u64 + head.e_name_len as u64;
            entries_addr = Self::round_up_to_align(entries_addr, Self::XATTR_ALIGNMENT);
        }
        Ok(())
    }

    /// Reads all the xattr entries, stored in the inode, from `entries_addr` into `xattrs`.
    fn read_xattr_entries_from_block(
        block: &[u8],
        xattrs: &mut XattrMap,
    ) -> Result<(), ParsingError> {
        let head = XattrHeader::to_struct_ref(
            &block[..std::mem::size_of::<XattrHeader>()],
            ParsingError::Incompatible("Invalid XattrHeader".to_string()),
        )?;

        if head.e_magic.get() != Self::XATTR_MAGIC {
            return Ok(());
        }

        let mut offset = Self::round_up_to_align(
            std::mem::size_of::<XattrHeader>() as u64,
            Self::XATTR_ALIGNMENT * 2,
        ) as usize;

        while offset + std::mem::size_of::<XattrEntryHeader>() < block.len() {
            let head = XattrEntryHeader::to_struct_ref(
                &block[offset..offset + std::mem::size_of::<XattrEntryHeader>()],
                ParsingError::Incompatible("Invalid XattrEntryHeader".to_string()),
            )?;

            if !Self::is_valid_xattr_entry_header(&head) {
                break;
            }

            let name_start = offset + std::mem::size_of::<XattrEntryHeader>();
            let name_end = name_start + head.e_name_len as usize;
            let mut name = Self::xattr_prefix_for_name_index(&head);
            name.extend_from_slice(&block[name_start..name_end]);

            let value_start = head.e_value_offs.get() as usize;
            let value_end = value_start + head.e_value_size.get() as usize;
            let value = block[value_start..value_end].to_vec();
            xattrs.insert(name, value);

            offset = Self::round_up_to_align(name_end as u64, 4) as usize;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::parser::Parser;
    #[cfg(target_os = "fuchsia")]
    use crate::parser::{DataWriter, VmoWriter};
    use crate::readers::VecReader;
    use crate::structs::{EntryType, ParsingError};
    use maplit::hashmap;
    use sha2::{Digest, Sha256};
    #[cfg(target_os = "fuchsia")]
    use std::assert_matches;
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::{fs, str};
    use test_case::test_case;

    #[fuchsia::test]
    fn list_root_1_file() {
        let data = fs::read("/pkg/data/1file.img").expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());
        let root_inode = parser.root_inode().expect("Parse INode");
        let entries = parser.entries_from_inode(&root_inode).expect("List entries");
        let mut expected_entries = vec!["file1", "lost+found", "..", "."];

        for de in &entries {
            assert_eq!(expected_entries.pop().unwrap(), de.name().unwrap());
        }
        assert_eq!(expected_entries.len(), 0);
    }

    #[test_case(
        "/pkg/data/nest.img",
        vec!["inner", "file1", "lost+found", "..", "."];
        "fs with a single directory")]
    #[test_case(
        "/pkg/data/extents.img",
        vec!["trailingzeropages", "a", "smallfile", "largefile", "sparsefile", "lost+found", "..", "."];
        "fs with multiple files with multiple extents")]
    fn list_root(ext4_path: &str, mut expected_entries: Vec<&str>) {
        let data = fs::read(ext4_path).expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());
        let root_inode = parser.root_inode().expect("Parse INode");
        let entries = parser.entries_from_inode(&root_inode).expect("List entries");

        for de in &entries {
            assert_eq!(expected_entries.pop().unwrap(), de.name().unwrap());
        }
        assert_eq!(expected_entries.len(), 0);
    }

    #[fuchsia::test]
    fn get_from_path() {
        let data = fs::read("/pkg/data/nest.img").expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());

        let entry = parser.entry_at_path(Path::new("/inner")).expect("Entry at path");
        assert_eq!(entry.e2d_ino.get(), 12);
        assert_eq!(entry.name().unwrap(), "inner");

        let entry = parser.entry_at_path(Path::new("/inner/file2")).expect("Entry at path");
        assert_eq!(entry.e2d_ino.get(), 17);
        assert_eq!(entry.name().unwrap(), "file2");
    }

    #[fuchsia::test]
    fn read_data() {
        let data = fs::read("/pkg/data/1file.img").expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());

        let entry = parser.entry_at_path(Path::new("file1")).expect("Entry at path");
        assert_eq!(entry.e2d_ino.get(), 15);
        assert_eq!(entry.name().unwrap(), "file1");

        let data = parser.read_data(entry.e2d_ino.into()).expect("File data");
        let compare = "file1 contents.\n";
        assert_eq!(data.len(), compare.len());
        assert_eq!(str::from_utf8(data.as_slice()).expect("File data"), compare);
    }

    #[fuchsia::test]
    fn fail_inode_zero() {
        let data = fs::read("/pkg/data/1file.img").expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.inode(0).is_err());
    }

    #[fuchsia::test]
    fn index() {
        let data = fs::read("/pkg/data/nest.img").expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());

        let mut count = 0;
        let mut entries: HashSet<u32> = HashSet::new();
        let root_inode = parser.root_inode().expect("Root inode");

        parser
            .index(root_inode, Vec::new(), &mut |_, _, entry| {
                count += 1;

                // Make sure each inode only appears once.
                assert_ne!(entries.contains(&entry.e2d_ino.get()), true);
                entries.insert(entry.e2d_ino.get());

                Ok::<bool, ParsingError>(true)
            })
            .expect("Index");

        assert_eq!(count, 4);
    }

    #[fuchsia::test]
    fn xattr() {
        let data = fs::read("/pkg/data/xattr.img").expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());
        let root_inode = parser.root_inode().expect("Root inode");
        let mut found_files = HashSet::new();

        parser
            .index(root_inode, Vec::new(), &mut |_, _, entry| {
                let name = entry.e2d_name;
                let inode = entry.e2d_ino.get();
                let attributes = parser.inode_xattrs(inode).expect("Extended attributes");
                match name {
                    name if &name[0..10] == b"lost+found" => {
                        assert_eq!(attributes.len(), 0);
                        found_files.insert("lost+found");
                    }
                    name if &name[0..5] == b"file1" => {
                        assert_eq!(attributes.len(), 1);
                        assert_eq!(attributes[&b"user.test".to_vec()], b"test value".to_vec());
                        found_files.insert("file1");
                    }
                    name if &name[0..9] == b"file_many" => {
                        assert_eq!(attributes.len(), 6);
                        assert_eq!(
                            attributes[&b"user.long".to_vec()],
                            b"vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv".to_vec()
                        );
                        found_files.insert("file_many");
                    }
                    name if &name[0..6] == b"subdir" => {
                        assert_eq!(attributes.len(), 1);
                        assert_eq!(attributes[&b"user.type".to_vec()], b"dir".to_vec());
                        found_files.insert("subdir");
                    }
                    name if &name[0..5] == b"file2" => {
                        assert_eq!(attributes.len(), 2);
                        assert_eq!(
                            attributes[&b"user.test_one".to_vec()],
                            b"test value 1".to_vec()
                        );
                        assert_eq!(
                            attributes[&b"user.test_two".to_vec()],
                            b"test value 2".to_vec()
                        );
                        found_files.insert("file2");
                    }
                    _ => {}
                }
                Ok::<bool, ParsingError>(true)
            })
            .expect("Index");

        assert_eq!(found_files.len(), 5);
    }

    #[test_case(
        "/pkg/data/extents.img",
        hashmap!{
            "largefile".to_string() => "de2cf635ae4e0e727f1e412f978001d6a70d2386dc798d4327ec8c77a8e4895d".to_string(),
            "smallfile".to_string() => "5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03".to_string(),
            "sparsefile".to_string() => "3f411e42c1417cd8845d7144679812be3e120318d843c8c6e66d8b2c47a700e9".to_string(),
            "trailingzeropages".to_string() => "afc5cc689fd3cb8d00c147d60dc911a70d36b7afb03cc7f15de9c78a52be978d".to_string(),
            "a/multi/dir/path/within/this/crowded/extents/test/img/empty".to_string() => "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        },
        vec!["a/multi/dir/path/within/this/crowded/extents/test/img", "lost+found"];
        "fs with multiple files with multiple extents")]
    #[test_case(
        "/pkg/data/1file.img",
        hashmap!{
            "file1".to_string() => "6bc35bfb2ca96c75a1fecde205693c19a827d4b04e90ace330048f3e031487dd".to_string(),
        },
        vec!["lost+found"];
        "fs with one small file")]
    #[test_case(
        "/pkg/data/nest.img",
        hashmap!{
            "file1".to_string() => "6bc35bfb2ca96c75a1fecde205693c19a827d4b04e90ace330048f3e031487dd".to_string(),
            "inner/file2".to_string() => "215ca145cbac95c9e2a6f5ff91ca1887c837b18e5f58fd2a7a16e2e5a3901e10".to_string(),
        },
        vec!["inner", "lost+found"];
        "fs with a single directory")]
    #[test_case(
        "/pkg/data/nest64.img",
        hashmap!{
            "file1".to_string() => "6bc35bfb2ca96c75a1fecde205693c19a827d4b04e90ace330048f3e031487dd".to_string(),
            "inner/file2".to_string() => "215ca145cbac95c9e2a6f5ff91ca1887c837b18e5f58fd2a7a16e2e5a3901e10".to_string(),
        },
        vec!["inner", "lost+found"];
        "fs with 64bit enabled and a single directory")]
    #[test_case(
        "/pkg/data/longdir.img",
        {
            let mut hash = HashMap::new();
            for i in 1..=1000 {
                hash.insert(i.to_string(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string());
            }
            hash
        },
        vec!["lost+found"];
        "fs with many entries in a directory")]
    fn check_data(
        ext4_path: &str,
        mut file_hashes: HashMap<String, String>,
        expected_dirs: Vec<&str>,
    ) {
        let data = fs::read(ext4_path).expect("Unable to read file");
        let parser = Parser::new(Box::new(VecReader::new(data)));
        assert!(parser.super_block().expect("Super Block").check_magic().is_ok());

        let root_inode = parser.root_inode().expect("Root inode");

        parser
            .index(root_inode, Vec::new(), &mut |my_self, path, entry| {
                let entry_type = EntryType::from_u8(entry.e2d_type).expect("Entry Type");
                let file_path = path.join("/");

                match entry_type {
                    EntryType::RegularFile => {
                        let data = my_self.read_data(entry.e2d_ino.into()).expect("File data");

                        let mut hasher = Sha256::new();
                        hasher.update(&data);
                        assert_eq!(
                            file_hashes.remove(&file_path).unwrap(),
                            hex::encode(hasher.finalize())
                        );
                    }
                    EntryType::Directory => {
                        let mut found = false;

                        // These should be the only possible directories.
                        for expected_dir in expected_dirs.iter() {
                            if expected_dir.starts_with(&file_path) {
                                found = true;
                                break;
                            }
                        }
                        assert!(found, "Unexpected path {}", file_path);
                    }
                    _ => {
                        assert!(false, "No other types should exist in this image.");
                    }
                }
                Ok::<bool, ParsingError>(true)
            })
            .expect("Index");
        assert!(file_hashes.is_empty(), "Expected files were not found {:?}", file_hashes);
    }

    #[cfg(target_os = "fuchsia")]
    #[test_case(true; "pad")]
    #[test_case(true; "no_pad")]
    #[fuchsia::test]
    fn vmo_writer_size_and_stream_size(pad: bool) {
        let size = zx::system_get_page_size() as u64;
        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        writer.reserve(100).expect("reserve failed");
        let contents = b"hello world";
        writer.append(contents).expect("append failed");
        let expect_stream_size = if pad {
            let pad = 500;
            writer.pad_to(pad).expect("pad_to failed");
            pad
        } else {
            contents.len() as u64
        };

        let vmo = writer.into_vmo();
        assert_eq!(vmo.get_size().expect("get_size failed"), size);
        assert_eq!(vmo.get_stream_size().expect("get_stream_size failed"), expect_stream_size);
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn vmo_writer_write_or_reserve_past_size_fails() {
        let size = zx::system_get_page_size() as u64;

        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        assert_eq!(
            writer.reserve(size + 1),
            Err(ParsingError::VmoError(zx::Status::NOT_SUPPORTED))
        );

        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        let large_buffer = vec![0u8; (size + 1) as usize];
        assert_eq!(
            writer.append(&large_buffer),
            Err(ParsingError::VmoError(zx::Status::IO_REFUSED))
        );

        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        assert_eq!(writer.pad_to(size + 1), Err(ParsingError::VmoError(zx::Status::OUT_OF_RANGE)));

        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        writer.append(&vec![0u8; size as usize]).expect("append failed");
        assert_eq!(writer.append(b"a"), Err(ParsingError::VmoError(zx::Status::OUT_OF_RANGE)));
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn vmo_writer_pad_to_lower_than_offset_fails() {
        let size = zx::system_get_page_size() as u64;
        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        writer.append(b"hello world").expect("append failed");
        assert_matches!(writer.pad_to(5), Err(ParsingError::NotSupported(_)));
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn vmo_writer_pad_to_sets_stream_size_and_zeroes() {
        let size = zx::system_get_page_size() as u64;
        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        let contents = b"hello";
        writer.append(contents).expect("append failed");
        let pad: usize = 100;
        writer.pad_to(pad as u64).expect("pad_to failed");

        let vmo = writer.into_vmo();
        assert_eq!(vmo.get_stream_size().expect("get_stream_size failed"), pad as u64);

        let mut buf = vec![0u8; pad];
        vmo.read(&mut buf, 0).expect("vmo read failed");
        assert_eq!(&buf[0..contents.len()], b"hello");
        assert_eq!(&buf[contents.len()..pad], &vec![0u8; pad - contents.len()]);
    }

    #[cfg(target_os = "fuchsia")]
    #[fuchsia::test]
    fn vmo_writer_with_no_writes_has_zero_stream_size() {
        let size = zx::system_get_page_size() as u64;
        let writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        let vmo = writer.into_vmo();
        assert_eq!(vmo.get_size().expect("get_size"), size);
        assert_eq!(vmo.get_stream_size().expect("get_stream_size"), 0);
    }

    #[cfg(target_os = "fuchsia")]
    #[test_case(true; "pad")]
    #[test_case(true; "no_pad")]
    #[fuchsia::test]
    fn vmo_writer_multiple_appends(pad: bool) {
        let size = zx::system_get_page_size() as u64;
        let mut writer = VmoWriter::new(size).expect("failed to create VmoWriter");
        let mut expect = vec![];
        for i in 0..5 {
            let b = format!("hello payload {i}");
            expect.extend_from_slice(b.as_bytes());
            writer.append(b.as_bytes()).expect("append");
            if pad {
                let pad = [0u8; 10];
                expect.extend_from_slice(&pad);
                writer.pad_to(expect.len() as u64).expect("pad");
            }
        }

        let vmo = writer.into_vmo();
        let mut buf = vec![0u8; expect.len()];
        assert_eq!(vmo.get_stream_size().expect("get_stream_size"), expect.len() as u64);
        vmo.read(&mut buf, 0).expect("read");
        assert_eq!(buf, expect);
    }
}
