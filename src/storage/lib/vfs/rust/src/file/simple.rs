// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ObjectRequestRef;
use crate::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use crate::execution_scope::ExecutionScope;
use crate::file::{FidlIoConnection, File, FileIo, FileLike, FileOptions, SyncMode};
use crate::node::Node;
use flex_fuchsia_io as fio;
use std::sync::Arc;
use zx_status::Status;

#[cfg(test)]
mod tests;

/// A file with a byte array for content, useful for testing.
pub struct SimpleFile {
    data: Vec<u8>,
}

impl SimpleFile {
    /// Create a new read-only test file with the provided content.
    pub fn read_only(content: impl AsRef<[u8]>) -> Arc<Self> {
        Arc::new(SimpleFile { data: content.as_ref().to_vec() })
    }
}

impl DirectoryEntry for SimpleFile {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl GetEntryInfo for SimpleFile {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::File)
    }
}

impl Node for SimpleFile {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        let content_size: u64 = self.data.len().try_into().unwrap();
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::FILE,
                abilities: fio::Operations::GET_ATTRIBUTES | fio::Operations::READ_BYTES,
                content_size: content_size,
                storage_size: content_size,
            }
        ))
    }
}

impl FileIo for SimpleFile {
    async fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        let content_size = self.data.len().try_into().unwrap();
        if offset >= content_size {
            return Ok(0u64);
        }
        let read_len: u64 = std::cmp::min(content_size - offset, buffer.len().try_into().unwrap());
        let read_len_usize: usize = read_len.try_into().unwrap();
        buffer[..read_len_usize]
            .copy_from_slice(&self.data[offset.try_into().unwrap()..][..read_len_usize]);
        Ok(read_len)
    }

    async fn write_at(&self, _offset: u64, _content: &[u8]) -> Result<u64, Status> {
        return Err(Status::NOT_SUPPORTED);
    }

    async fn append(&self, _content: &[u8]) -> Result<(u64, u64), Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

impl File for SimpleFile {
    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn executable(&self) -> bool {
        false
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, _length: u64) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn get_size(&self) -> Result<u64, Status> {
        Ok(self.data.len().try_into().unwrap())
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        Ok(())
    }
}

impl FileLike for SimpleFile {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        options: FileOptions,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        FidlIoConnection::create_sync(scope, self, options, object_request.take());
        Ok(())
    }
}
