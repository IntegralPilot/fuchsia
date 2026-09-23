// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdomain_fuchsia_io as fio;
use futures::future::BoxFuture;
use std::any::Any;
use std::future::ready;
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{DirEntryExt, FileTypeExt, MetadataExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use vfs::ObjectRequestRef;
use vfs::directory::dirents_sink;
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::{Directory, DirectoryWatcher, MutableDirectory};
use vfs::directory::mutable::connection::MutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::execution_scope::ExecutionScope;
use vfs::file::connection::FidlIoConnection;
use vfs::file::{File, FileIo, FileLike, FileOptions, SyncMode};
use vfs::node::Node;
use vfs::path::Path as VfsPath;
use zx_status::Status;
use zx_status_ext::IoErrorKindExt;

/// Convert a Rust [`std::fs::Metadata`] to a [`fio::NodeAttributes2`] FIDL struct.
fn metadata_to_node_attributes2(
    metadata: std::fs::Metadata,
    abilities: fio::Operations,
) -> fio::NodeAttributes2 {
    fio::NodeAttributes2 {
        mutable_attributes: fio::MutableNodeAttributes {
            creation_time: metadata.ctime_nsec().try_into().ok(),
            modification_time: metadata.mtime_nsec().try_into().ok(),
            mode: Some(metadata.mode()),
            uid: Some(metadata.uid()),
            gid: Some(metadata.gid()),
            rdev: Some(metadata.rdev()),
            access_time: metadata.atime_nsec().try_into().ok(),
            ..Default::default()
        },
        immutable_attributes: fio::ImmutableNodeAttributes {
            protocols: Some(fio::NodeProtocolKinds::DIRECTORY),
            abilities: Some(abilities),
            content_size: Some(metadata.size()),
            storage_size: Some(metadata.size()),
            link_count: Some(metadata.nlink()),
            id: Some(metadata.ino()),
            change_time: metadata.mtime_nsec().try_into().ok(),
            ..Default::default()
        },
    }
}

/// Convert a Rust [`std::fs::FileType`] to a [`fio::DirentType`] FIDL struct.
fn file_type_to_dirent_type(file_type: std::fs::FileType) -> fio::DirentType {
    if file_type.is_block_device() {
        fio::DirentType::BlockDevice
    } else if file_type.is_symlink() {
        fio::DirentType::Symlink
    } else if file_type.is_dir() {
        fio::DirentType::Directory
    } else if file_type.is_file() {
        fio::DirentType::File
    } else {
        fio::DirentType::Unknown
    }
}

/// A file on the host mapped into a Fuchsia filesystem.
pub struct HostFile {
    file: Mutex<Option<std::fs::File>>,
    path: PathBuf,
}

impl DirectoryEntry for HostFile {
    fn open_entry(
        self: Arc<Self>,
        request: OpenRequest<'_>,
    ) -> std::prelude::v1::Result<(), Status> {
        request.open_file(self)
    }
}

impl GetEntryInfo for HostFile {
    fn entry_info(&self) -> EntryInfo {
        if let Ok(metadata) = std::fs::metadata(&self.path) {
            EntryInfo::new(metadata.ino(), file_type_to_dirent_type(metadata.file_type()))
        } else {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Unknown)
        }
    }
}

impl FileLike for HostFile {
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

impl File for HostFile {
    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        !std::fs::metadata(&self.path).map(|x| x.permissions().readonly()).unwrap_or(true)
    }
    fn executable(&self) -> bool {
        false
    }

    async fn open_file(&self, options: &FileOptions) -> Result<(), Status> {
        let writable = options.rights.contains(fio::Operations::WRITE_BYTES);
        let file = std::fs::OpenOptions::new()
            .append(options.is_append)
            .read(true)
            .write(writable)
            .open(&self.path)
            .map_err(|e| e.kind().to_status())?;

        *self.file.lock().unwrap() = Some(file);

        Ok(())
    }

    async fn truncate(&self, length: u64) -> Result<(), Status> {
        let file = self.file.lock().unwrap();
        let file = file.as_ref().ok_or(Status::NOT_CONNECTED)?;
        file.set_len(length).map_err(|e| e.kind().to_status())?;
        Ok(())
    }

    async fn get_size(&self) -> Result<u64, Status> {
        Ok(std::fs::metadata(&self.path).map_err(|e| e.kind().to_status())?.size())
    }

    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        // TODO(https://fxbug.dev/333800380) we won't need these until
        // playground has commands that modify file attributes.
        Err(Status::NOT_SUPPORTED)
    }

    async fn allocate(
        &self,
        _offset: u64,
        _length: u64,
        _mode: fio::AllocateMode,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn sync(&self, _mode: SyncMode) -> Result<(), Status> {
        let file = self.file.lock().unwrap();
        let file = file.as_ref().ok_or(Status::NOT_CONNECTED)?;
        file.sync_all().map_err(|e| e.kind().to_status())?;
        Ok(())
    }
}

impl FileIo for HostFile {
    async fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<u64, Status> {
        let mut file = self.file.lock().unwrap();
        let file = file.as_mut().ok_or(Status::NOT_CONNECTED)?;

        file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.kind().to_status())?;
        Ok(file.read(buffer).map(|x| x.try_into().unwrap()).map_err(|e| e.kind().to_status())?)
    }

    async fn write_at(&self, offset: u64, content: &[u8]) -> Result<u64, Status> {
        let mut file = self.file.lock().unwrap();
        let file = file.as_mut().ok_or(Status::NOT_CONNECTED)?;

        file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.kind().to_status())?;
        Ok(file.write(content).map(|x| x.try_into().unwrap()).map_err(|e| e.kind().to_status())?)
    }

    async fn append(&self, content: &[u8]) -> Result<(u64, u64), Status> {
        let mut file = self.file.lock().unwrap();
        let file = file.as_mut().ok_or(Status::NOT_CONNECTED)?;

        file.seek(std::io::SeekFrom::End(0)).map_err(|e| e.kind().to_status())?;
        let wrote =
            file.write(content).map(|x| x.try_into().unwrap()).map_err(|e| e.kind().to_status())?;
        let offset = file.stream_position().map_err(|e| e.kind().to_status())?;
        Ok((wrote, offset))
    }
}

impl Node for HostFile {
    async fn get_attributes(
        &self,
        _requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(metadata_to_node_attributes2(
            std::fs::metadata(&self.path).map_err(|e| e.kind().to_status())?,
            fio::Operations::READ_BYTES | fio::Operations::WRITE_BYTES,
        ))
    }

    async fn link_into(
        self: Arc<Self>,
        _destination_dir: Arc<dyn MutableDirectory>,
        _name: vfs::name::Name,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    /// Returns information about the filesystem.
    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn list_extended_attributes(&self) -> Result<Vec<Vec<u8>>, Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn get_extended_attribute(&self, _name: Vec<u8>) -> Result<Vec<u8>, Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn set_extended_attribute(
        &self,
        _name: Vec<u8>,
        _value: Vec<u8>,
        _mode: fio::SetExtendedAttributeMode,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    async fn remove_extended_attribute(&self, _name: Vec<u8>) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

/// A directory on the host mapped into a Fuchsia filesystem.
pub struct HostDirectory(PathBuf);

impl HostDirectory {
    /// Create a new [`HostDirectory`]
    pub fn new(path: impl AsRef<std::path::Path>) -> Arc<Self> {
        Arc::new(HostDirectory(path.as_ref().to_owned()))
    }
}

impl DirectoryEntry for HostDirectory {
    fn open_entry(
        self: Arc<Self>,
        request: OpenRequest<'_>,
    ) -> std::prelude::v1::Result<(), Status> {
        request.open_dir(self)
    }
}

impl GetEntryInfo for HostDirectory {
    fn entry_info(&self) -> EntryInfo {
        if let Ok(metadata) = std::fs::metadata(&self.0) {
            EntryInfo::new(metadata.ino(), file_type_to_dirent_type(metadata.file_type()))
        } else {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Unknown)
        }
    }
}
impl Node for HostDirectory {
    async fn get_attributes(
        &self,
        _requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(metadata_to_node_attributes2(
            std::fs::metadata(&self.0).map_err(|e| e.kind().to_status())?,
            fio::Operations::GET_ATTRIBUTES
                | fio::Operations::UPDATE_ATTRIBUTES
                | fio::Operations::ENUMERATE
                | fio::Operations::TRAVERSE
                | fio::Operations::MODIFY_DIRECTORY,
        ))
    }

    async fn link_into(
        self: Arc<Self>,
        _destination_dir: Arc<dyn MutableDirectory>,
        _name: vfs::name::Name,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    /// Returns information about the filesystem.
    fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

impl Directory for HostDirectory {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: VfsPath,
        flags: fio::Flags,
        object_request: ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        let path = self.0.join(path.as_ref());
        if !path.exists() {
            Err(Status::NOT_FOUND)
        } else if path.is_dir() {
            let directory = Arc::new(HostDirectory(path));
            object_request
                .take()
                .create_connection_sync::<MutableConnection<_>, _>(scope, directory, flags);
            Ok(())
        } else {
            let file = Arc::new(HostFile { file: Mutex::new(None), path });
            vfs::file::serve(file, scope, &flags, object_request)
        }
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        mut sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        if let TraversalPosition::End = pos {
            return Ok((TraversalPosition::End, sink.seal()));
        }

        let mut iter = std::fs::read_dir(&self.0).map_err(|e| e.kind().to_status())?;
        let mut count = 0;
        let mut found = false;
        while let Some(entry) = iter.next().transpose().map_err(|e| e.kind().to_status())? {
            if let TraversalPosition::Index(idx) = pos {
                if count < *idx {
                    count += 1;
                    continue;
                }
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or(Status::BAD_PATH)?;
            if let TraversalPosition::Name(waiting_name) = pos {
                found = found || waiting_name == name;

                if !found {
                    count += 1;
                    continue;
                }
            }
            let ty = file_type_to_dirent_type(entry.file_type().map_err(|e| e.kind().to_status())?);
            let entry = EntryInfo::new(entry.ino(), ty);
            match sink.append(&entry, name) {
                dirents_sink::AppendResult::Ok(sink_out) => sink = sink_out,
                dirents_sink::AppendResult::Sealed(sealed) => {
                    return Ok((TraversalPosition::Index(count), sealed));
                }
            }
            count += 1;
        }

        Ok((TraversalPosition::End, sink.seal()))
    }

    fn register_watcher(
        self: Arc<Self>,
        _scope: ExecutionScope,
        _mask: fio::WatchMask,
        _watcher: DirectoryWatcher,
    ) -> Result<(), Status> {
        // Most things in playground don't support watchers yet and we don't
        // have any commands in the roadmap that will need them.
        Err(Status::NOT_SUPPORTED)
    }

    fn unregister_watcher(self: Arc<Self>, _key: usize) {
        // Most things in playground don't support watchers yet and we don't
        // have any commands in the roadmap that will need them.
    }
}

impl MutableDirectory for HostDirectory {
    /// Adds a child entry to this directory.  If the target exists, it should fail with
    /// ZX_ERR_ALREADY_EXISTS.
    fn link<'a>(
        self: Arc<Self>,
        _name: String,
        _source_dir: Arc<dyn Any + Send + Sync>,
        _source_name: &'a str,
    ) -> BoxFuture<'a, Result<(), Status>> {
        Box::pin(ready(Err(Status::NOT_SUPPORTED)))
    }

    /// Set the mutable attributes of this directory based on the values in `attributes`.
    async fn update_attributes(
        &self,
        _attributes: fio::MutableNodeAttributes,
    ) -> Result<(), Status> {
        // TODO(https://fxbug.dev/333800380) we won't need these until
        // playground has commands that modify file attributes.
        Err(Status::NOT_SUPPORTED)
    }

    /// Removes an entry from this directory.
    async fn unlink(self: Arc<Self>, name: &str, must_be_directory: bool) -> Result<(), Status> {
        let path = self.0.join(name);
        if path.is_dir() {
            std::fs::remove_dir(path).map_err(|e| e.kind().to_status())?;
        } else if must_be_directory {
            return Err(Status::NOT_DIR);
        } else {
            std::fs::remove_file(path).map_err(|e| e.kind().to_status())?;
        }
        Ok(())
    }

    /// Syncs the directory.
    async fn sync(&self) -> Result<(), Status> {
        // No good way to do this in async_fs
        Ok(())
    }

    /// Renames into this directory.
    fn rename(
        self: Arc<Self>,
        _src_dir: Arc<dyn MutableDirectory>,
        _src_name: VfsPath,
        _dst_name: VfsPath,
    ) -> BoxFuture<'static, Result<(), Status>> {
        //TODO(https://fxbug.dev/333799815)
        Box::pin(ready(Err(Status::NOT_SUPPORTED)))
    }

    /// Creates a symbolic link.
    async fn create_symlink(
        &self,
        _name: String,
        _target: Vec<u8>,
        _connection: Option<fdomain_client::fidl::ServerEnd<fio::SymlinkMarker>>,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[fuchsia::test]
    async fn list_dir() {
        let client = fdomain_local::local_client_empty();
        let tmp_dir = tempfile::tempdir().unwrap();
        let _ = std::fs::File::create(tmp_dir.path().join("A")).unwrap();
        let _ = std::fs::File::create(tmp_dir.path().join("B")).unwrap();
        let _ = std::fs::File::create(tmp_dir.path().join("C")).unwrap();
        let client = vfs::directory::serve_read_only(
            HostDirectory::new(tmp_dir.path()),
            vfs::execution_scope::ExecutionScope::new(client.clone()),
        );
        let mut dirs: Vec<_> = fuchsia_fs_fdomain::directory::readdir(&client)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect();
        dirs.sort();
        assert_eq!(vec!["A", "B", "C"], dirs.iter().map(|x| x.as_str()).collect::<Vec<_>>());
    }

    #[fuchsia::test]
    async fn list_subdir() {
        let client = fdomain_local::local_client_empty();
        let tmp_dir = tempfile::tempdir().unwrap();
        let sub_path = tmp_dir.path().join("subdir");
        let _ = std::fs::create_dir(&sub_path).unwrap();
        let _ = std::fs::File::create(sub_path.join("A")).unwrap();
        let _ = std::fs::File::create(sub_path.join("B")).unwrap();
        let _ = std::fs::File::create(sub_path.join("C")).unwrap();
        let client = vfs::directory::serve_read_only(
            HostDirectory::new(tmp_dir.path()),
            vfs::execution_scope::ExecutionScope::new(client.clone()),
        );
        let sub_dir =
            fuchsia_fs_fdomain::directory::open_directory(&client, "subdir", fio::PERM_READABLE)
                .await
                .unwrap();
        let mut dirs: Vec<_> = fuchsia_fs_fdomain::directory::readdir(&sub_dir)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.name)
            .collect();
        dirs.sort();
        assert_eq!(vec!["A", "B", "C"], dirs.iter().map(|x| x.as_str()).collect::<Vec<_>>());
    }

    #[fuchsia::test]
    async fn open_file() {
        let client = fdomain_local::local_client_empty();
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(tmp_dir.path().join("A")).unwrap();
        let test_str =
            b"I literally can't leave this room, so I'm just going to ignore my feelings.";
        f.write_all(test_str).unwrap();
        let client = vfs::directory::serve_read_only(
            HostDirectory::new(tmp_dir.path()),
            vfs::execution_scope::ExecutionScope::new(client.clone()),
        );
        let file = fuchsia_fs_fdomain::directory::open_file(&client, "A", fio::PERM_READABLE)
            .await
            .unwrap();

        let got = fuchsia_fs_fdomain::file::read(&file).await.unwrap();
        assert_eq!(test_str, got.as_slice());
    }

    #[fuchsia::test]
    async fn split_read() {
        let client = fdomain_local::local_client_empty();
        let tmp_dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(tmp_dir.path().join("A")).unwrap();
        let test_str =
            b"I literally can't leave this room, so I'm just going to ignore my feelings.";
        f.write_all(test_str).unwrap();
        let client = vfs::directory::serve_read_only(
            HostDirectory::new(tmp_dir.path()),
            vfs::execution_scope::ExecutionScope::new(client.clone()),
        );
        let file = fuchsia_fs_fdomain::directory::open_file(&client, "A", fio::PERM_READABLE)
            .await
            .unwrap();

        let split_len = test_str.len() / 2;

        let got = fuchsia_fs_fdomain::file::read_num_bytes(&file, split_len as u64).await.unwrap();
        assert_eq!(&test_str[..split_len], got.as_slice());
        let got = fuchsia_fs_fdomain::file::read(&file).await.unwrap();
        assert_eq!(&test_str[split_len..], got.as_slice());
    }
}
