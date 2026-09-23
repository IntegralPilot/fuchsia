// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::parser::Parser;
use crate::readers::ReaderWriter;
use crate::structs::{FIRST_BG_PADDING, InvalidAddressErrorType, ParsingError};
use fuchsia_sync::Mutex;
use futures::future::FutureExt;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct Ext4FileMetrics {
    num_read_requests: u64,
    num_open_requests: u64,
    num_truncate_requests: u64,
    num_write_requests: u64,
    num_writes_past_eof_attempts: u64,
    num_successful_overwrites: u64,
    num_blocks_overwritten: u64,
}

/// A processor that wraps an ext4 parser and adds write functionality if not in read-only mode.
/// Note that syncing of metadata to the persistent storage is *NOT* supported.
pub struct Ext4Processor {
    fs: Parser,
    reader_writer: Arc<dyn ReaderWriter>,
    read_only: bool,
    file_metrics: Arc<Mutex<Ext4FileMetrics>>,
    file_size: Mutex<HashMap<u32, u64>>,
}

impl std::ops::Deref for Ext4Processor {
    type Target = Parser;

    fn deref(&self) -> &Self::Target {
        &self.fs
    }
}

impl Ext4Processor {
    pub fn new(reader_writer: Arc<dyn ReaderWriter>, read_only: bool) -> Self {
        Self {
            fs: Parser::new(Box::new(reader_writer.clone())),
            reader_writer,
            read_only,
            file_metrics: Arc::new(Mutex::new(Ext4FileMetrics::default())),
            file_size: Mutex::new(HashMap::new()),
        }
    }

    pub fn record_read_metrics(&self) {
        let mut metrics = self.file_metrics.lock();
        metrics.num_read_requests += 1;
    }

    pub fn record_open_metrics(&self) {
        let mut metrics = self.file_metrics.lock();
        metrics.num_open_requests += 1;
    }

    pub fn record_statistics(&self, stats_node: &fuchsia_inspect::Node) {
        let metrics = self.file_metrics.clone();
        stats_node.record_lazy_child("file_metrics", move || {
            let metrics = metrics.clone();
            async move {
                let inspector = fuchsia_inspect::Inspector::default();
                let root = inspector.root();
                let metrics = metrics.lock();
                root.record_uint("num_read_requests", metrics.num_read_requests);
                root.record_uint("num_open_requests", metrics.num_open_requests);
                root.record_uint("num_truncate_requests", metrics.num_truncate_requests);
                root.record_uint("num_write_requests", metrics.num_write_requests);
                root.record_uint(
                    "num_writes_past_eof_attempts",
                    metrics.num_writes_past_eof_attempts,
                );
                root.record_uint("num_successful_overwrites", metrics.num_successful_overwrites);
                root.record_uint("num_blocks_overwritten", metrics.num_blocks_overwritten);
                Ok(inspector)
            }
            .boxed()
        });
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// Writes contiguous raw data starting at a given block number.
    fn write_blocks(&self, block_number: u64, data: &[u8]) -> Result<(), ParsingError> {
        if self.read_only {
            return Err(ParsingError::Incompatible("Cannot write to read-only Ext4".to_string()));
        }
        if block_number == 0 {
            return Err(ParsingError::InvalidAddress(
                InvalidAddressErrorType::Lower,
                0,
                FIRST_BG_PADDING,
            ));
        }

        let block_size = self.block_size()?;
        if data.len() as u64 % block_size != 0 {
            return Err(ParsingError::Incompatible(format!(
                "Data length {} is not a multiple of block size {}",
                data.len(),
                block_size
            )));
        }

        let address = block_number
            .checked_mul(block_size)
            .ok_or(ParsingError::BlockNumberOutOfBounds(block_number))?;

        self.reader_writer.write(address, data)?;

        Ok(())
    }

    pub fn file_size(&self, inode_num: u32) -> Result<u64, ParsingError> {
        let file_size = self.file_size.lock();
        if let Some(&size) = file_size.get(&inode_num) {
            Ok(size)
        } else {
            Ok(self.inode(inode_num)?.size())
        }
    }

    pub fn truncate_to_zero(&self, inode_num: u32) -> Result<(), ParsingError> {
        if self.read_only {
            return Err(ParsingError::Incompatible("Cannot write to read-only Ext4".to_string()));
        }
        let mut file_metrics = self.file_metrics.lock();
        file_metrics.num_truncate_requests += 1;

        self.file_size.lock().insert(inode_num, 0);
        Ok(())
    }

    /// Overwrites existing contents of a file with new data. This does not require any allocations.
    /// Note that this does not update the journal with timestamps.
    pub fn overwrite_file_contents(
        &self,
        inode_num: u32,
        data: impl AsRef<[u8]>,
        offset: u64,
    ) -> Result<(), ParsingError> {
        if self.read_only {
            return Err(ParsingError::Incompatible("Cannot write to read-only Ext4".to_string()));
        }
        let mut file_metrics = self.file_metrics.lock();
        file_metrics.num_write_requests += 1;

        let inode = self.inode(inode_num)?;
        let persisted_size = inode.size();
        // If the file has been truncated to zero, we must write to the same size of the persisted
        // size.
        if let Some(_) = self.file_size.lock().get(&inode_num) {
            if offset + data.as_ref().len() as u64 != persisted_size {
                file_metrics.num_writes_past_eof_attempts += 1;
                return Err(ParsingError::NotSupported(
                    "writing past EOF / partial writes".to_string(),
                ));
            }
        }
        if offset + data.as_ref().len() as u64 > persisted_size {
            file_metrics.num_writes_past_eof_attempts += 1;
            return Err(ParsingError::NotSupported(
                "writing past EOF / partial writes".to_string(),
            ));
        }
        if data.as_ref().len() == 0 {
            file_metrics.num_successful_overwrites += 1;
            return Ok(());
        }

        let root_extent_tree_node = inode.extent_tree_node()?;
        let request = offset..offset + data.as_ref().len() as u64;
        let block_size = self.block_size()?;

        self.iterate_extents_in_tree(&root_extent_tree_node, &mut |extent| {
            let range = (extent.e_blk.get() as u64 * block_size)
                ..((extent.e_blk.get() as u64 + extent.e_len.get() as u64) * block_size);
            let overlap =
                std::cmp::max(range.start, request.start)..std::cmp::min(range.end, request.end);
            if overlap.start >= overlap.end {
                // No overlap.
                return Ok(());
            }

            let mut physical_block_cursor =
                extent.target_block_num() + ((overlap.start - range.start) / block_size);
            let mut current_offset = overlap.start;
            while current_offset < overlap.end {
                let write_buf_cursor = (current_offset - request.start) as usize;
                let block_off = current_offset % block_size;
                let remaining_in_overlap = overlap.end - current_offset;

                if block_off == 0 && remaining_in_overlap >= block_size {
                    // Contiguous full blocks write
                    let full_blocks = remaining_in_overlap / block_size;
                    let write_len = full_blocks * block_size;
                    self.write_blocks(
                        physical_block_cursor,
                        &data.as_ref()[write_buf_cursor..write_buf_cursor + write_len as usize],
                    )?;
                    file_metrics.num_blocks_overwritten += full_blocks;

                    physical_block_cursor += full_blocks;
                    current_offset += write_len;
                } else {
                    // Write partial block by first reading the existing block and overwriting the
                    // relevant part.
                    let remaining_in_block = block_size - block_off;
                    let write_len = std::cmp::min(remaining_in_block, remaining_in_overlap);
                    let mut block_data = self.block(physical_block_cursor)?.into_vec();
                    block_data[block_off as usize..block_off as usize + write_len as usize]
                        .copy_from_slice(
                            &data.as_ref()[write_buf_cursor..write_buf_cursor + write_len as usize],
                        );
                    self.write_blocks(physical_block_cursor, &block_data)?;
                    file_metrics.num_blocks_overwritten += 1;

                    physical_block_cursor += 1;
                    current_offset += write_len;
                }
            }
            Ok(())
        })?;

        // Update size: size may be be 0 if `truncate_to_zero` was called before this. Removing it
        // means the next call to `file_size` will return the persisted size from the inode.
        self.file_size.lock().remove(&inode_num);

        file_metrics.num_successful_overwrites += 1;
        Ok(())
    }

    pub fn sync(&self) -> Result<(), ParsingError> {
        self.reader_writer.sync()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::readers::{BlockDeviceReader, VecReader};
    use crate::structs::{FIRST_BG_PADDING, InvalidAddressErrorType, ParsingError};
    use fidl_fuchsia_storage_block as fblock;
    use fuchsia_async as fasync;
    use std::fs;
    use std::path::Path;
    use test_vmo_backed_block_server::VmoBackedServer;

    #[fuchsia::test]
    async fn test_processor_read_only_blocks_write() {
        let data = fs::read("/pkg/data/1file.img").expect("Unable to read file");
        let read_only_processor = Ext4Processor::new(Arc::new(VecReader::new(data)), true);

        let error = read_only_processor
            .write_blocks(1, &[0u8; 1024])
            .expect_err("passed write_blocks unexpectedly");
        match error {
            ParsingError::Incompatible(_) => {}
            _ => panic!("Expected read-only error"),
        }

        // Test overwrite_file_contents
        let error = read_only_processor
            .overwrite_file_contents(2, &[0u8; 10], 0)
            .expect_err("passed overwrite_file_contents unexpectedly");
        match error {
            ParsingError::Incompatible(_) => {}
            _ => panic!("Expected read-only error"),
        }
    }

    #[fuchsia::test]
    async fn test_processor_write_block_invalid_address() {
        let data = fs::read("/pkg/data/1file.img").expect("Unable to read file");
        let processor = Ext4Processor::new(Arc::new(VecReader::new(data)), false);

        let error =
            processor.write_blocks(0, &[0u8; 1024]).expect_err("passed write_blocks unexpectedly");
        match error {
            ParsingError::InvalidAddress(InvalidAddressErrorType::Lower, 0, FIRST_BG_PADDING) => {}
            _ => panic!("Expected invalid address error, got {:?}", error),
        }
    }

    #[fuchsia::test]
    async fn test_processor_write_block_out_of_bounds() {
        let data = fs::read("/pkg/data/1file.img").expect("Unable to read file");
        let processor = Ext4Processor::new(Arc::new(VecReader::new(data)), false);

        let error = processor
            .write_blocks(u64::MAX, &[0u8; 1024])
            .expect_err("passed write_blocks unexpectedly");
        match error {
            ParsingError::BlockNumberOutOfBounds(u64::MAX) => {}
            _ => panic!("Expected out of bounds error, got {:?}", error),
        }
    }

    #[fuchsia::test]
    async fn test_processor_writeable_overwrite_extents() {
        let server = VmoBackedServer::from_file(512, "/pkg/data/1file.img");

        let (block_client_end1, block_server_end1) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task = executor.run_singlethreaded(server.serve(block_server_end1.into_stream()));
        });
        let inspector = fuchsia_inspect::Inspector::default();
        let rw_processor = Arc::new(Ext4Processor::new(
            Arc::new(
                BlockDeviceReader::from_client_end(block_client_end1)
                    .expect("failed to create block device reader"),
            ),
            false,
        ));
        rw_processor.record_statistics(inspector.root());

        let file_ino = rw_processor
            .entry_at_path(Path::new("file1"))
            .expect("failed entry at path")
            .e2d_ino
            .get();

        let mut expected = rw_processor.read_data(file_ino).expect("failed to read data");
        assert_eq!(
            str::from_utf8(expected.as_slice()).expect("failed to read data"),
            "file1 contents.\n"
        );
        let original_size = rw_processor.inode(file_ino).expect("failed to read inode").size();
        assert_eq!(original_size, expected.len() as u64);

        rw_processor
            .overwrite_file_contents(file_ino, &[1u8; 1], 1)
            .expect("failed to overwrite extents");
        expected[1] = 1;

        let new_data = rw_processor.read_data(file_ino).expect("failed to read data");
        assert_eq!(new_data, expected);
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            file_metrics: {
                num_open_requests: 0u64,
                num_read_requests: 0u64,
                num_truncate_requests: 0u64,
                num_write_requests: 1u64,
                num_writes_past_eof_attempts: 0u64,
                num_successful_overwrites: 1u64,
                num_blocks_overwritten: 1u64,
            }
        });

        // Test writing to the allocated extent, extending past the original file size (still within
        // the allocated block).
        let error = rw_processor
            .overwrite_file_contents(file_ino, &[1u8; 2], expected.len() as u64 + 2)
            .expect_err("overwrite past EOF should fail");
        match error {
            ParsingError::NotSupported(_) => {}
            _ => panic!("Expected NotSupported error, got {:?}", error),
        }

        // Verify that the file size has not updated.
        let new_size = rw_processor.inode(file_ino).expect("failed to read inode").size();
        assert_eq!(new_size, original_size);
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            file_metrics: {
                num_open_requests: 0u64,
                num_read_requests: 0u64,
                num_truncate_requests: 0u64,
                num_write_requests: 2u64,
                num_writes_past_eof_attempts: 1u64,
                num_successful_overwrites: 1u64,
                num_blocks_overwritten: 1u64,
            }
        });
    }
}
