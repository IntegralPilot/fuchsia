// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_io as fio;
use std::sync::Arc;
use vfs::ObjectRequestRef;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::execution_scope::ExecutionScope;
use vfs::file::{FidlIoConnection, File, FileIo, FileLike, FileOptions, SyncMode};
use vfs::node::Node;
use zx::Status;

pub struct BuiltinDevVnode {
    is_null: bool,
}

impl BuiltinDevVnode {
    pub fn new(is_null: bool) -> Arc<Self> {
        Arc::new(Self { is_null })
    }
}

impl GetEntryInfo for BuiltinDevVnode {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(
            fio::INO_UNKNOWN,
            fio::DirentType::from_primitive_allow_unknown(2 /* DT_CHR */),
        )
    }
}

impl DirectoryEntry for BuiltinDevVnode {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_file(self)
    }
}

impl Node for BuiltinDevVnode {
    async fn get_attributes(
        &self,
        _query: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(fio::NodeAttributes2 {
            mutable_attributes: fio::MutableNodeAttributes {
                mode: Some(0x2180), // S_IFCHR | S_IRUSR | S_IWUSR
                ..Default::default()
            },
            immutable_attributes: fio::ImmutableNodeAttributes { ..Default::default() },
        })
    }
}

impl FileIo for BuiltinDevVnode {
    async fn read_at(&self, _offset: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        if self.is_null {
            // /dev/null implementation.
            Ok(0)
        } else {
            // /dev/zero implementation.
            buffer.fill(0);
            Ok(buffer.len() as u64)
        }
    }

    async fn write_at(&self, _offset: u64, content: &[u8]) -> Result<u64, Status> {
        if self.is_null {
            // /dev/null implementation.
            Ok(content.len() as u64)
        } else {
            Err(Status::NOT_SUPPORTED)
        }
    }

    async fn append(&self, _content: &[u8]) -> Result<(u64, u64), Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

impl File for BuiltinDevVnode {
    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        self.is_null
    }

    fn executable(&self) -> bool {
        false
    }

    async fn open_file(&self, _options: &FileOptions) -> Result<(), Status> {
        Ok(())
    }

    async fn truncate(&self, _length: u64) -> Result<(), Status> {
        if self.is_null { Ok(()) } else { Err(Status::NOT_SUPPORTED) }
    }

    async fn get_size(&self) -> Result<u64, Status> {
        Ok(0)
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

impl FileLike for BuiltinDevVnode {
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
