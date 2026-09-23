// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::directory::ExtDirectory;
use crate::file::ExtFile;
use crate::symlink::ExtSymlink;
use crate::types::ExtAttributes;
use ext4_lib::processor::Ext4Processor;
use ext4_lib::readers::{BlockDeviceReader, ReaderWriter, VmoReader};
use ext4_lib::structs::{self, EntryType, MIN_EXT4_SIZE};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_storage_block::BlockMarker;
use log::error;
use std::sync::Arc;

mod directory;
mod file;
mod node;
mod symlink;
mod types;

pub enum FsSourceType {
    BlockDevice(ClientEnd<BlockMarker>),
    Vmo(zx::Vmo),
}

#[derive(Debug, PartialEq)]
pub enum ConstructFsError {
    VmoReadError(zx::Status),
    ParsingError(structs::ParsingError),
    FileVmoError(zx::Status),
    NodeError(zx::Status),
}

impl From<structs::ParsingError> for ConstructFsError {
    fn from(value: structs::ParsingError) -> Self {
        Self::ParsingError(value)
    }
}

pub fn construct_fs(
    source: FsSourceType,
    read_only: bool,
    inspector: &fuchsia_inspect::Inspector,
) -> Result<Arc<ExtDirectory>, ConstructFsError> {
    let reader: Arc<dyn ReaderWriter> = match source {
        FsSourceType::BlockDevice(block_device) => {
            Arc::new(BlockDeviceReader::from_client_end(block_device).map_err(|e| {
                error!("Error constructing file system: {}", e);
                ConstructFsError::VmoReadError(zx::Status::IO_INVALID)
            })?)
        }
        FsSourceType::Vmo(vmo) => {
            let size = vmo.get_size().map_err(ConstructFsError::VmoReadError)?;
            if size < MIN_EXT4_SIZE as u64 {
                // Too small to even fit the first copy of the ext4 Super Block.
                return Err(ConstructFsError::VmoReadError(zx::Status::NO_SPACE));
            }

            Arc::new(VmoReader::new(Arc::new(vmo)))
        }
    };
    let processor = Arc::new(Ext4Processor::new(reader, read_only));
    let dir = build_fs_dir(processor.clone(), structs::ROOT_INODE_NUM, read_only)?;
    processor.record_statistics(inspector.root());
    Ok(dir)
}

fn build_fs_dir(
    processor: Arc<Ext4Processor>,
    ino: u32,
    read_only: bool,
) -> Result<Arc<ExtDirectory>, ConstructFsError> {
    let inode = processor.inode(ino)?;
    let entries = processor.entries_from_inode(&inode)?;
    let attributes = ExtAttributes::from_inode(inode);
    let xattrs = processor.inode_xattrs(ino)?;
    let dir = ExtDirectory::new(ino as u64, attributes, xattrs);

    for entry in entries {
        let entry_name = entry.name()?;
        if entry_name == "." || entry_name == ".." {
            continue;
        }

        let entry_ino = u32::from(entry.e2d_ino);
        match EntryType::from_u8(entry.e2d_type)? {
            EntryType::Directory => {
                dir.insert_child(
                    entry_name,
                    build_fs_dir(processor.clone(), entry_ino, read_only)?,
                )
                .map_err(ConstructFsError::NodeError)?;
            }
            EntryType::RegularFile => {
                dir.insert_child(
                    entry_name,
                    ExtFile::from_processor(processor.clone(), entry_ino, read_only)
                        .map_err(ConstructFsError::NodeError)?,
                )
                .map_err(ConstructFsError::NodeError)?;
            }
            EntryType::SymLink => {
                dir.insert_child(
                    entry_name,
                    ExtSymlink::from_processor(processor.clone(), entry_ino)
                        .map_err(ConstructFsError::NodeError)?,
                )
                .map_err(ConstructFsError::NodeError)?;
            }
            _ => {
                // TODO(https://fxbug.dev/42073143): Handle other types.
            }
        }
    }

    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::{ExtAttributes, ExtDirectory, ExtSymlink, FsSourceType, construct_fs};

    use ext4_lib::parser::XattrMap;
    use ext4_lib::structs::MIN_EXT4_SIZE;
    use fidl_fuchsia_io as fio;
    use fidl_fuchsia_storage_block as fblock;
    use fuchsia_async as fasync;
    use fuchsia_fs::directory::{DirEntry, DirentKind, open_file, open_node, readdir};
    use fuchsia_fs::file::{WriteError, read_to_string, write};
    use futures::stream::StreamExt as _;
    use std::fs;
    use std::sync::Arc;
    use test_vmo_backed_block_server::VmoBackedServer;
    use zx::{Status, Vmo};

    const BLOCK_SIZE: u32 = 512;

    #[fuchsia::test]
    fn image_too_small() {
        let vmo = Vmo::create(10).expect("VMO is created");
        vmo.write(b"too small", 0).expect("VMO write() succeeds");
        let buffer = FsSourceType::Vmo(vmo);

        assert!(
            construct_fs(buffer, true, &fuchsia_inspect::Inspector::default()).is_err(),
            "Expected failed parsing of VMO."
        );
    }

    #[fuchsia::test]
    fn invalid_fs() {
        let vmo = Vmo::create(MIN_EXT4_SIZE as u64).expect("VMO is created");
        vmo.write(b"not ext4", 0).expect("VMO write() succeeds");
        let buffer = FsSourceType::Vmo(vmo);

        assert!(
            construct_fs(buffer, true, &fuchsia_inspect::Inspector::default()).is_err(),
            "Expected failed parsing of VMO."
        );
    }

    #[fuchsia::test]
    async fn list_root() {
        let data = fs::read("/pkg/data/nest.img").expect("Unable to read file");
        let vmo = Vmo::create(data.len() as u64).expect("VMO is created");
        vmo.write(data.as_slice(), 0).expect("VMO write() succeeds");
        let buffer = FsSourceType::Vmo(vmo);

        let tree = construct_fs(buffer, true, &fuchsia_inspect::Inspector::default())
            .expect("construct_fs parses the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let expected = vec![
            DirEntry { name: String::from("file1"), kind: DirentKind::File },
            DirEntry { name: String::from("inner"), kind: DirentKind::Directory },
            DirEntry { name: String::from("lost+found"), kind: DirentKind::Directory },
        ];
        assert_eq!(readdir(&root).await.unwrap(), expected);

        let file = open_file(&root, "file1", fio::PERM_READABLE).await.unwrap();
        assert_eq!(read_to_string(&file).await.unwrap(), "file1 contents.\n");
        file.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        root.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    }

    #[fuchsia::test]
    async fn get_dac_attributes() {
        let data = fs::read("/pkg/data/dac_attributes.img").expect("Unable to read file");
        let vmo = Vmo::create(data.len() as u64).expect("VMO is created");
        vmo.write(data.as_slice(), 0).expect("VMO write() succeeds");
        let buffer = FsSourceType::Vmo(vmo);

        let tree = construct_fs(buffer, true, &fuchsia_inspect::Inspector::default())
            .expect("construct_fs parses the VMO");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let expected_entries = vec![
            DirEntry { name: String::from("dir_1000"), kind: DirentKind::Directory },
            DirEntry { name: String::from("dir_root"), kind: DirentKind::Directory },
            DirEntry { name: String::from("file_1000"), kind: DirentKind::File },
            DirEntry { name: String::from("file_root"), kind: DirentKind::File },
            DirEntry { name: String::from("lost+found"), kind: DirentKind::Directory },
        ];
        assert_eq!(readdir(&root).await.unwrap(), expected_entries);

        #[derive(Debug, PartialEq)]
        struct Node {
            name: String,
            mode: u32,
            uid: u32,
            gid: u32,
        }

        let expected_attributes = vec![
            Node { name: String::from("dir_1000"), mode: 0x416D, uid: 1000, gid: 1000 },
            Node { name: String::from("dir_root"), mode: 0x4140, uid: 0, gid: 0 },
            Node { name: String::from("file_1000"), mode: 0x8124, uid: 1000, gid: 1000 },
            Node { name: String::from("file_root"), mode: 0x8100, uid: 0, gid: 0 },
        ];

        let attributes_query = fio::NodeAttributesQuery::MODE
            | fio::NodeAttributesQuery::UID
            | fio::NodeAttributesQuery::GID;
        for expected_node in &expected_attributes {
            let node_proxy = open_node(&root, expected_node.name.as_str(), fio::PERM_READABLE)
                .await
                .expect("node open failed");
            let (mut_attrs, _immut_attrs) = node_proxy
                .get_attributes(attributes_query)
                .await
                .expect("node get_attributes() failed")
                .map_err(Status::err_from_raw)
                .expect("node get_attributes() error");

            let node = Node {
                name: expected_node.name.clone(),
                mode: mut_attrs.mode.expect("node attributes missing mode"),
                uid: mut_attrs.uid.expect("node attributes missing uid"),
                gid: mut_attrs.gid.expect("node attributes missing gid"),
            };

            node_proxy
                .close()
                .await
                .expect("node close failed")
                .map_err(Status::err_from_raw)
                .expect("node close error");

            assert_eq!(node, *expected_node);
        }

        root.close().await.unwrap().map_err(Status::err_from_raw).unwrap();
    }

    #[fuchsia::test]
    async fn test_constructing_writeable_fs_and_writing_to_allocated_region() {
        // Create a device that is Ext4 formatted.
        let server = Arc::new(VmoBackedServer::from_file(BLOCK_SIZE, "/pkg/data/nest.img"));

        let server_clone = server.clone();
        let (block_client_end1, block_server_end1) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task =
                executor.run_singlethreaded(server_clone.serve(block_server_end1.into_stream()));
        });

        // Write to the allocated extent of this file.
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end1),
            /* read_only= */ false,
            &fuchsia_inspect::Inspector::default(),
        )
        .expect("failed to parse the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );
        let file = open_file(&root, "file1", fio::PERM_READABLE | fio::PERM_WRITABLE)
            .await
            .expect("failed to open file");
        let original_contents = "file1 contents.\n";
        assert_eq!(read_to_string(&file).await.expect("failed to read file"), original_contents);
        let new_contents = "new";
        let offset = 5;
        file.seek(fio::SeekOrigin::Start, offset)
            .await
            .expect("failed FIDL seek")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to seek file");
        write(&file, new_contents).await.expect("failed to write to file");
        file.close()
            .await
            .expect("failed FIDL file close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close file");
        root.close()
            .await
            .expect("failed FIDL dir close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close root");

        // Construct Ext4 fs again, and verify that the written data is still there.
        let server_clone = server.clone();
        let (block_client_end2, block_server_end2) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task =
                executor.run_singlethreaded(server_clone.serve(block_server_end2.into_stream()));
        });
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end2),
            /* read_only= */ true,
            &fuchsia_inspect::Inspector::default(),
        )
        .expect("construct_fs parses the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let file =
            open_file(&root, "file1", fio::PERM_READABLE).await.expect("failed to open file");
        let mut expected_bytes = original_contents.as_bytes().to_vec();
        expected_bytes[offset as usize..offset as usize + new_contents.len()]
            .copy_from_slice(new_contents.as_bytes());
        assert_eq!(
            read_to_string(&file).await.expect("failed to read file"),
            String::from_utf8(expected_bytes).unwrap()
        );
        file.close()
            .await
            .expect("failed FIDL file close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close file");
        root.close()
            .await
            .expect("failed FIDL dir close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close root");
    }

    #[fuchsia::test]
    async fn test_writing_past_eof_fails() {
        let server = Arc::new(VmoBackedServer::from_file(BLOCK_SIZE, "/pkg/data/nest.img"));

        let server_clone = server.clone();
        let (block_client_end1, block_server_end1) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task =
                executor.run_singlethreaded(server_clone.serve(block_server_end1.into_stream()));
        });

        // Write to the allocated extent of this file.
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end1),
            /* read_only= */ false,
            &fuchsia_inspect::Inspector::default(),
        )
        .expect("failed to parse the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );
        let file = open_file(&root, "file1", fio::PERM_READABLE | fio::PERM_WRITABLE)
            .await
            .expect("failed to open file");
        let original_contents = read_to_string(&file).await.expect("failed to read file");

        // There is not enough allocated bytes in this file to write this new content.
        let new_contents = [1u8; 8192];
        file.seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("failed FIDL seek")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to seek file");
        let error = write(&file, &new_contents)
            .await
            .expect_err("write to unallocated region passed unexpectedly");
        match error {
            WriteError::WriteError(status) => assert_eq!(status, zx::Status::NOT_SUPPORTED),
            _ => panic!("Unexpected error: {:?}", error),
        }

        file.close()
            .await
            .expect("failed FIDL file close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close file");
        root.close()
            .await
            .expect("failed FIDL dir close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close root");

        // Construct Ext4 fs again, and verify that the written data is still there.
        let server_clone = server.clone();
        let (block_client_end2, block_server_end2) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task =
                executor.run_singlethreaded(server_clone.serve(block_server_end2.into_stream()));
        });
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end2),
            /* read_only= */ true,
            &fuchsia_inspect::Inspector::default(),
        )
        .expect("construct_fs parses the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );
        let file =
            open_file(&root, "file1", fio::PERM_READABLE).await.expect("failed to open file");
        assert_eq!(read_to_string(&file).await.expect("failed to read file"), original_contents);
        file.close()
            .await
            .expect("failed FIDL file close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close file");
        root.close()
            .await
            .expect("failed FIDL dir close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close root");
    }

    #[fuchsia::test]
    async fn test_file_sync() {
        let data = fs::read("/pkg/data/nest.img").expect("failed to read file");
        let vmo = Vmo::create(data.len() as u64).expect("failed to create VMO");
        vmo.write(data.as_slice(), 0).expect("failed to write to VMO");

        // Clone VMO to observe underlying changes made by the server.
        let vmo_clone = vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).expect("failed to clone vmo");

        let server =
            VmoBackedServer::from_vmo(BLOCK_SIZE, vmo).expect("Failed to create VmoBackedServer");

        let (block_client_end, block_server_end) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task = executor.run_singlethreaded(server.serve(block_server_end.into_stream()));
        });

        // Write to the allocated extent of this file.
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end),
            /* read_only= */ false,
            &fuchsia_inspect::Inspector::default(),
        )
        .expect("failed to parse the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );
        let file = open_file(&root, "file1", fio::PERM_READABLE | fio::PERM_WRITABLE)
            .await
            .expect("failed to open file");

        let mut old_vmo_contents = vec![0u8; data.len()];
        vmo_clone.read(&mut old_vmo_contents, 0).expect("failed to read from vmo clone");

        let new_contents = "FILE1 CONTENTS!\n";
        file.seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("failed FIDL seek")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to seek file");
        write(&file, new_contents).await.expect("failed to write to file");

        let mut vmo_contents_after_write = vec![0u8; data.len()];
        vmo_clone.read(&mut vmo_contents_after_write, 0).expect("failed to read from vmo clone");

        // The write is stored in the device cache but has not yet been been flushed to the
        // underlying VMO.
        assert_eq!(
            old_vmo_contents, vmo_contents_after_write,
            "Data should be cached and not yet flushed to VmoBackedServer"
        );

        // Closing the file will call sync to flush the contents to the backing VMO.
        file.close()
            .await
            .expect("sync check failed")
            .map_err(zx::Status::err_from_raw)
            .expect("sync error");

        let mut vmo_contents_after_sync = vec![0u8; data.len()];
        vmo_clone.read(&mut vmo_contents_after_sync, 0).expect("failed to read from vmo clone");

        // Data should now be flushed to underlying VMO.
        assert_ne!(
            old_vmo_contents, vmo_contents_after_sync,
            "Data should be flushed to VmoBackedServer after sync"
        );

        root.close()
            .await
            .expect("failed FIDL dir close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close root");
    }

    #[fuchsia::test]
    async fn test_metrics_of_fs_with_multiple_files() {
        let server = VmoBackedServer::from_file(BLOCK_SIZE, "/pkg/data/nest.img");

        let (block_client_end, block_server_end) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task = executor.run_singlethreaded(server.serve(block_server_end.into_stream()));
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end),
            /* read_only= */ false,
            &inspector,
        )
        .expect("failed to parse the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        let file1 = open_file(
            &root,
            "file1",
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FILE_TRUNCATE,
        )
        .await
        .expect("open with truncate should succeed");
        let contents = read_to_string(&file1).await.expect("failed to read file");
        assert_eq!(contents, "");
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            file_metrics: {
                num_open_requests: 1u64,
                num_read_requests: 1u64,
                num_truncate_requests: 1u64,
                num_write_requests: 0u64,
                num_writes_past_eof_attempts: 0u64,
                num_successful_overwrites: 0u64,
                num_blocks_overwritten: 0u64,
            }
        });
        file1
            .seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("failed to seek")
            .map_err(zx::Status::err_from_raw)
            .expect("seek error");
        write(&file1, "FILE1 CONTENTS!\n").await.expect("failed to write to file");
        file1
            .seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("failed to seek")
            .map_err(zx::Status::err_from_raw)
            .expect("seek error");
        // `read_to_string` loops read until no bytes are read back. So for non-empty strings, we
        // expect to see two more read requests.
        let new_contents = read_to_string(&file1).await.expect("failed to read file");
        assert_eq!(new_contents, "FILE1 CONTENTS!\n");
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            file_metrics: {
                num_open_requests: 1u64,
                num_read_requests: 3u64,
                num_truncate_requests: 1u64,
                num_write_requests: 1u64,
                num_writes_past_eof_attempts: 0u64,
                num_successful_overwrites: 1u64,
                num_blocks_overwritten: 1u64,
            }
        });

        // Perform opens and reads on another file. Should see them reflected in the inspector
        // metrics.
        let file2 = open_file(&root, "inner/file2", fio::PERM_READABLE | fio::PERM_WRITABLE)
            .await
            .expect("failed to open inner/file2");
        let _contents = read_to_string(&file2).await.expect("failed to read file2");

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            file_metrics: {
                num_open_requests: 2u64,
                num_read_requests: 5u64,
                num_truncate_requests: 1u64,
                num_write_requests: 1u64,
                num_writes_past_eof_attempts: 0u64,
                num_successful_overwrites: 1u64,
                num_blocks_overwritten: 1u64,
            }
        });

        root.close()
            .await
            .expect("failed FIDL dir close")
            .map_err(zx::Status::err_from_raw)
            .expect("failed to close root");
    }

    #[fuchsia::test]
    async fn test_truncate_and_write() {
        let server = VmoBackedServer::from_file(BLOCK_SIZE, "/pkg/data/1file.img");

        let (block_client_end, block_server_end) =
            fidl::endpoints::create_endpoints::<fblock::BlockMarker>();
        std::thread::spawn(move || {
            let mut executor = fasync::TestExecutor::new();
            let _task = executor.run_singlethreaded(server.serve(block_server_end.into_stream()));
        });

        let inspector = fuchsia_inspect::Inspector::default();
        let tree = construct_fs(
            FsSourceType::BlockDevice(block_client_end),
            /* read_only= */ false,
            &inspector,
        )
        .expect("failed to parse the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        // Check original contents
        let file = open_file(&root, "file1", fio::PERM_READABLE).await.expect("open failed");
        let original_contents = read_to_string(&file).await.expect("read failed");
        assert_eq!(original_contents, "file1 contents.\n");
        file.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();

        // Open with TRUNCATE, reading from this should return empty string.
        let file = open_file(
            &root,
            "file1",
            fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::FILE_TRUNCATE,
        )
        .await
        .expect("open with truncate failed");
        assert_eq!(read_to_string(&file).await.expect("read failed"), "");

        // Write to the file and verify we see new contents.
        let new_content = "FILE1 CONTENTS.\n";
        write(&file, new_content).await.expect("write failed");
        file.seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("seek failed")
            .map_err(zx::Status::err_from_raw)
            .expect("seek error");
        assert_eq!(read_to_string(&file).await.expect("read failed"), new_content);

        // Check that writing past original file size fails.
        let huge_content = vec![1u8; 50];
        let error =
            write(&file, &huge_content).await.expect_err("write past allocated size should fail");
        match error {
            WriteError::WriteError(status) => assert_eq!(status, zx::Status::NOT_SUPPORTED),
            _ => panic!("Unexpected error: {:?}", error),
        }

        // Check that overwriting the file partially is not supported.
        let partial_content = vec![1u8; 2];
        let error = write(&file, &partial_content)
            .await
            .expect_err("write past allocated size should fail");
        match error {
            WriteError::WriteError(status) => assert_eq!(status, zx::Status::NOT_SUPPORTED),
            _ => panic!("Unexpected error: {:?}", error),
        }

        // We see the content written previously.
        file.seek(fio::SeekOrigin::Start, 0)
            .await
            .expect("seek failed")
            .map_err(zx::Status::err_from_raw)
            .expect("seek error");
        assert_eq!(read_to_string(&file).await.expect("read failed"), new_content);

        file.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        root.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    }

    #[fuchsia::test]
    async fn test_symlink() {
        let root_dir = ExtDirectory::new(1, ExtAttributes::default(), XattrMap::default());
        let symlink_target = b"target/path";
        let symlink_node = ExtSymlink::new(
            2,
            ExtAttributes::default(),
            XattrMap::default(),
            symlink_target.to_vec(),
        );

        root_dir.insert_child("my_symlink", symlink_node).unwrap();

        let root = vfs::directory::serve(
            root_dir,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        // Verify readdir lists the symlink with DirentKind::Symlink
        let expected =
            vec![DirEntry { name: String::from("my_symlink"), kind: DirentKind::Symlink }];
        assert_eq!(readdir(&root).await.unwrap(), expected);

        // Verify we can open the symlink and read its target
        let (symlink_proxy, server_end) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
        root.open(
            "my_symlink",
            fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE,
            &fio::Options::default(),
            server_end.into_channel().into(),
        )
        .expect("open symlink failed");

        let target_bytes = symlink_proxy.describe().await.expect("describe failed").target.unwrap();
        assert_eq!(target_bytes, symlink_target);

        // Verify that trying to traverse through the symlink fails with
        // NOT_DIR.
        let (node_proxy, server_end) = fidl::endpoints::create_proxy::<fio::NodeMarker>();
        root.open(
            "my_symlink/child",
            fio::Flags::empty(),
            &fio::Options::default(),
            server_end.into_channel().into(),
        )
        .expect("open path through symlink failed");

        let mut event_stream = node_proxy.take_event_stream();
        let event = event_stream.next().await.unwrap().expect_err("expected closed channel error");
        match event {
            fidl::Error::ClientChannelClosed { epitaph, .. } => {
                assert_eq!(epitaph, Status::NOT_DIR)
            }
            other => panic!("Unexpected event error: {:?}", other),
        }

        symlink_proxy.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        root.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    }

    #[fuchsia::test]
    async fn test_symlink_from_image() {
        let data = fs::read("/pkg/data/symlink.img").expect("Unable to read file");
        let vmo = Vmo::create(data.len() as u64).expect("VMO is created");
        vmo.write(data.as_slice(), 0).expect("VMO write() succeeds");
        let buffer = FsSourceType::Vmo(vmo);

        let tree = construct_fs(buffer, true, &fuchsia_inspect::Inspector::default())
            .expect("construct_fs parses the vmo");
        let root = vfs::directory::serve(
            tree,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE,
        );

        let expected = vec![
            DirEntry { name: String::from("file1"), kind: DirentKind::File },
            DirEntry { name: String::from("lost+found"), kind: DirentKind::Directory },
            DirEntry { name: String::from("symlink1"), kind: DirentKind::Symlink },
        ];
        let mut actual = readdir(&root).await.unwrap();
        actual.sort_by_key(|e| e.name.clone());
        assert_eq!(actual, expected);

        let (symlink_proxy, server_end) = fidl::endpoints::create_proxy::<fio::SymlinkMarker>();
        root.open(
            "symlink1",
            fio::Flags::PROTOCOL_SYMLINK | fio::PERM_READABLE,
            &fio::Options::default(),
            server_end.into_channel().into(),
        )
        .expect("open symlink failed");

        let target_bytes = symlink_proxy.describe().await.expect("describe failed").target.unwrap();
        assert_eq!(target_bytes, b"file1");

        symlink_proxy.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
        root.close().await.unwrap().map_err(zx::Status::err_from_raw).unwrap();
    }
}
