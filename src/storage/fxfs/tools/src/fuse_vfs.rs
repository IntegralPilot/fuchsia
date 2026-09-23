// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuse_attr::{create_file_attr, to_fxfs_time};
use crate::fuse_errors::{FuseErrorParser, FxfsResult};
use crate::fuse_fs::{FuseFs, FuseStrParser};
use fidl_fuchsia_io as fio;
use fuchsia as _;
use fuse3::Result;
use fuse3::raw::prelude::{Filesystem as FuseFilesystem, *};
use futures as _;
use futures_util::stream::Iter;
use futures_util::{StreamExt, stream};
use fxfs::errors::FxfsError;
use fxfs::filesystem::SyncOptions;
use fxfs::log::info;
use fxfs::object_handle::{ObjectHandle, WriteObjectHandle};
use fxfs::object_store::directory::{ReplacedChild, replace_child};
use fxfs::object_store::transaction::{LockKey, Options, lock_keys};
use fxfs::object_store::{ObjectDescriptor, Timestamp};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;
use std::time::Duration;
use std::vec::IntoIter;

/// The TTL duration of each reply.
const TTL: Duration = Duration::from_secs(1);

/// Inner Fxfs functions to handle the FUSE calls.
impl FuseFs {
    /// Look up a entry called `name` under `parent` directory and get its attributes.
    /// Return Ok with object's attributes if successful.
    /// Return NotFound if `parent` does not exist or `name` is not found under `parent`.
    async fn lookup_fxfs(&self, parent: u64, name: &OsStr) -> FxfsResult<ReplyEntry> {
        let dir = self.open_dir(parent).await?;
        let object = dir.lookup(name.osstr_to_str()?).await?;

        if let Some((object_id, object_descriptor, _)) = object {
            Ok(ReplyEntry {
                ttl: TTL,
                attr: self.create_object_attr(object_id, object_descriptor).await?,
                generation: 0,
            })
        } else {
            Err(FxfsError::NotFound.into())
        }
    }

    /// Create a directory called `name` under `parent` directory.
    /// Return Ok with the new directory's attributes if successful.
    /// Return NotFound if `parent` does not exist.
    /// Return NotDir if `parent` is not a directory.
    /// Return AlreadyExists if `name` already exists under `parent` directory.
    async fn mkdir_fxfs(&self, parent: u64, name: &OsStr) -> FxfsResult<ReplyEntry> {
        let dir = self.open_dir(parent).await?;

        if dir.lookup(name.osstr_to_str()?).await?.is_none() {
            let mut transaction = self
                .fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        self.default_store.store_object_id(),
                        dir.object_id()
                    )],
                    Options::default(),
                )
                .await?;

            let child_dir = dir.create_child_dir(&mut transaction, name.osstr_to_str()?).await?;
            transaction.commit().await?;

            Ok(ReplyEntry {
                ttl: TTL,
                attr: self
                    .create_object_attr(child_dir.object_id(), ObjectDescriptor::Directory)
                    .await?,
                generation: 0,
            })
        } else {
            Err(FxfsError::AlreadyExists.into())
        }
    }

    /// Remove a file or symlink called `name` under `parent` directory.
    /// Return Ok if the object is successfully removed.
    /// Return NotDir if `parent` is not a directory.
    /// Return NotFile if `name` is a directory under `parent`.
    /// Return NotFound if `name` is not found under `parent`.
    async fn unlink_fxfs(&self, parent: u64, name: &OsStr) -> FxfsResult<()> {
        let dir = self.open_dir(parent).await?;

        let replace_context =
            dir.acquire_context_for_replace(None, name.osstr_to_str()?, true).await?;
        let mut transaction = replace_context.transaction;

        let object_descriptor = match replace_context.dst_id_and_descriptor {
            Some((_, object_descriptor)) => object_descriptor,
            None => return Err(FxfsError::NotFound.into()),
        };
        match object_descriptor {
            ObjectDescriptor::File | ObjectDescriptor::Symlink => {
                let replaced_child =
                    replace_child(&mut transaction, None, (&dir, name.osstr_to_str()?)).await?;
                transaction.commit().await?;

                // If the object has no remaining links, immediately tombstones it in the graveyard.
                if let ReplacedChild::Object(object_id) = replaced_child {
                    self.fs
                        .graveyard()
                        .tombstone_object(
                            dir.store().store_object_id(),
                            object_id,
                            replace_context.truncate_guard.as_ref(),
                        )
                        .await?;

                    // Remove object's handle from cache if it exists.
                    self.object_handle_cache.write().await.remove(&object_id);
                }

                Ok(())
            }
            _ => Err(FxfsError::NotFile.into()),
        }
    }

    /// Remove a directory called `name` under `parent` directory.
    /// Return Ok if the directory called `name` is successfully removed.
    /// Return NotDir if `name` is not a directory.
    /// Return NotEmpty if `name` directory is not empty.
    /// Return NotFound if `name` is not found under `parent`.
    async fn rmdir_fxfs(&self, parent: u64, name: &OsStr) -> FxfsResult<()> {
        let dir = self.open_dir(parent).await?;

        let replace_context =
            dir.acquire_context_for_replace(None, name.osstr_to_str()?, true).await?;
        let mut transaction = replace_context.transaction;
        let object_descriptor = match replace_context.dst_id_and_descriptor {
            Some((_, object_descriptor)) => object_descriptor,
            None => return Err(FxfsError::NotFound.into()),
        };
        match object_descriptor {
            ObjectDescriptor::Directory => {
                replace_child(&mut transaction, None, (&dir, name.osstr_to_str()?)).await?;
                transaction.commit().await?;
                Ok(())
            }
            _ => Err(FxfsError::NotDir.into()),
        }
    }

    /// Rename an object called `name` under `parent` to an object called
    /// `new_name` under `new_parent`.
    /// Return Ok if the rename of object is successful.
    /// Return NotDir if `parent` or `new_parent` is not a directory.
    /// Return NotFound if `parent` or `new_parent` does not exist, or `name`
    /// or `new_name` does not exist under their parents.
    async fn rename_fxfs(
        &self,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
    ) -> FxfsResult<()> {
        let old_dir = self.open_dir(parent).await?;
        let new_dir: fxfs::object_store::Directory<fxfs::object_store::ObjectStore> =
            self.open_dir(new_parent).await?;
        let replace_context = new_dir
            .acquire_context_for_replace(
                Some((&old_dir, name.osstr_to_str()?)),
                new_name.osstr_to_str()?,
                true,
            )
            .await?;
        let mut transaction = replace_context.transaction;

        if replace_context.src_id_and_descriptor.is_some() {
            let replaced_child = replace_child(
                &mut transaction,
                Some((&old_dir, name.osstr_to_str()?)),
                (&new_dir, new_name.osstr_to_str()?),
            )
            .await?;

            transaction.commit().await?;

            // If the object is a file without remaining links,
            // immediately tombstones it in the graveyard.
            if let ReplacedChild::Object(object_id) = replaced_child {
                self.fs
                    .graveyard()
                    .tombstone_object(
                        new_dir.store().store_object_id(),
                        object_id,
                        replace_context.truncate_guard.as_ref(),
                    )
                    .await?;

                // Remove object's handle from cache if it exists.
                self.object_handle_cache.write().await.remove(&object_id);
            }

            Ok(())
        } else {
            Err(FxfsError::NotFound.into())
        }
    }

    /// Read data from a file object with id `inode`,
    /// which starts from `offset` with a length of `size`.
    /// Return Ok with the read data if successful.
    /// Return NotFound if `inode` does not exist.
    /// Return NotFile if `inode` is a directory.
    async fn read_fxfs(&self, inode: u64, offset: u64, size: u32) -> FxfsResult<ReplyData> {
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            let handle = self.get_object_handle(inode).await?;
            let mut out: Vec<u8> = Vec::new();
            let align = offset % self.fs.block_size();

            let mut buf = handle.allocate_buffer(handle.block_size().get() as usize).await;
            // Round down for the block alignment.
            let mut ofs = offset - align;
            let len = size as u64 + align + ofs;

            loop {
                let bytes = handle.read(ofs, buf.as_mut()).await?;
                if len - ofs > bytes as u64 {
                    // Read `bytes` size of content from buf.
                    ofs += bytes as u64;
                    buf.subslice(..bytes).append_to(&mut out);
                    if bytes as u64 != handle.block_size() {
                        break;
                    }
                } else {
                    // Read the remaining content from buf.
                    let target_len = (len - ofs) as usize;
                    buf.subslice(..target_len).append_to(&mut out);
                    break;
                }
            }
            let out: Vec<u8> = if (align as usize) < out.len() {
                out.drain((align as usize)..).collect()
            } else {
                vec![]
            };

            Ok(ReplyData { data: out.into() })
        } else {
            Err(FxfsError::NotFile.into())
        }
    }

    /// Write data into a file object with id `inode`, starting from `offset`.
    /// Return Ok with the length of written data if successful.
    /// Return NotFile if `inode` is a directory.
    /// Return NotFound if `inode` does not exist.
    async fn write_fxfs(&self, inode: u64, offset: u64, data: &[u8]) -> FxfsResult<ReplyWrite> {
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            let handle = self.get_object_handle(inode).await?;
            let mut buf = handle.allocate_buffer(data.len()).await;

            buf.copy_from_slice(data);
            handle.write_or_append(Some(offset), buf.as_ref()).await?;
            handle.flush().await?;

            Ok(ReplyWrite { written: data.len() as u32 })
        } else {
            Err(FxfsError::NotFile.into())
        }
    }

    /// Release an open file object with id `inode` to indicate its end of read or write.
    /// Return OK and remove `inode`'s file type and handle from cache if successful.
    /// Return NotFile if `inode` is a directory.
    /// Return NotFound if `inode` does not exist.
    async fn release_fxfs(&self, inode: u64) -> FxfsResult<()> {
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            // Remove the handle from cache because the file is released.
            self.release_object_handle(inode).await;
            Ok(())
        } else {
            Err(FxfsError::NotFile.into())
        }
    }

    /// Get attributes of an object with id `inode`.
    /// Return Ok with `inode`'s attributes if successful.
    /// Return NotFound if `inode` does not exist.
    async fn getattr_fxfs(&self, inode: u64) -> FxfsResult<ReplyAttr> {
        Ok(ReplyAttr {
            ttl: TTL,
            attr: self.create_object_attr(inode, self.get_object_type(inode).await?).await?,
        })
    }

    /// Set attributes of an object with id `inode` including its timestamps and object size.
    /// Return Ok with `inode`'s new attributes if successful.
    /// Return NotFound if `inode` does not exist.
    async fn setattr_fxfs(&self, inode: u64, set_attr: SetAttr) -> FxfsResult<ReplyAttr> {
        let object_type = self.get_object_type(inode).await?;
        let mtime: Option<Timestamp> = match set_attr.mtime {
            Some(t) => Some(to_fxfs_time(t)),
            None => None,
        };
        let atime: Option<Timestamp> = match set_attr.atime {
            Some(t) => Some(to_fxfs_time(t)),
            None => None,
        };
        let ctime: Option<Timestamp> = match set_attr.ctime {
            Some(t) => Some(to_fxfs_time(t)),
            None => None,
        };

        if object_type == ObjectDescriptor::File {
            let handle = self.get_object_handle(inode).await?;
            let mut transaction = self
                .fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        self.default_store.store_object_id(),
                        handle.object_id()
                    )],
                    Options::default(),
                )
                .await?;
            handle
                .update_attributes(
                    &mut transaction,
                    Some(&fio::MutableNodeAttributes {
                        modification_time: mtime.map(|t| t.as_nanos()),
                        access_time: atime.map(|t| t.as_nanos()),
                        ..Default::default()
                    }),
                    ctime,
                )
                .await?;
            transaction.commit().await?;

            // Truncate the file size if size attribute needs to be set.
            if let Some(size) = set_attr.size {
                handle.truncate(size).await?;
                handle.flush().await?;
            }
        } else if object_type == ObjectDescriptor::Directory {
            let transaction = self
                .fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(self.default_store.store_object_id(), inode)],
                    Options::default(),
                )
                .await?;
            let dir = self.open_dir(inode).await?;
            dir.update_attributes(
                transaction,
                Some(&fio::MutableNodeAttributes {
                    modification_time: mtime.map(|t| t.as_nanos()),
                    access_time: atime.map(|t| t.as_nanos()),
                    ..Default::default()
                }),
                0,
                ctime,
            )
            .await?;
        }

        Ok(ReplyAttr { ttl: TTL, attr: self.create_object_attr(inode, object_type).await? })
    }

    /// Synchronize the filesystem contents.
    async fn fsync_fxfs(&self) -> FxfsResult<()> {
        self.fs.sync(SyncOptions::default()).await
    }

    /// Check access permission of `inode`.
    /// Return Ok if `inode` exists.
    /// Return NotFound if `inode` does not exist.
    async fn access_fxfs(&self, inode: u64) -> FxfsResult<()> {
        self.get_object_type(inode).await?;
        Ok(())
    }

    /// Create a file called `name` under `parent` directory.
    /// Return Ok with the new file's attributes if successful.
    /// Return NotFound if `parent` does not exist.
    /// Return NotDir if `parent` is not a directory.
    /// Return AlreadyExists if `name` already exists under `parent`.
    async fn create_fxfs(&self, parent: u64, name: &OsStr, flags: u32) -> FxfsResult<ReplyCreated> {
        let dir = self.open_dir(parent).await?;
        if dir.lookup(name.osstr_to_str()?).await?.is_none() {
            let mut transaction = self
                .fs
                .root_store()
                .new_transaction(
                    lock_keys![LockKey::object(
                        self.default_store.store_object_id(),
                        dir.object_id()
                    )],
                    Options::default(),
                )
                .await?;

            let child_file = dir.create_child_file(&mut transaction, name.osstr_to_str()?).await?;
            transaction.commit().await?;
            let child_id = child_file.object_id();

            let properties = child_file.get_properties().await?;
            let attributes = create_file_attr(
                child_id,
                properties.data_attribute_size,
                properties.creation_time,
                properties.modification_time,
                properties.refs as u32,
            );

            // Store the object handle in cache.
            let mut object_handle_cache = self.object_handle_cache.write().await;
            object_handle_cache.insert(child_id, (Arc::new(child_file), 1));

            Ok(ReplyCreated { ttl: TTL, attr: attributes, generation: 0, fh: 0, flags })
        } else {
            Err(FxfsError::AlreadyExists.into())
        }
    }

    /// Open a file object with id `inode` for read or write.
    /// Return Ok and store file's handle and type in cache if successful.
    /// Return NotFound if `inode` does not exist.
    /// Return NotFile if `inode` is a directory.
    async fn open_fxfs(&self, inode: u64) -> FxfsResult<ReplyOpen> {
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            // Store the object handle in cache.
            self.load_object_handle(inode).await?;

            Ok(ReplyOpen { fh: 0, flags: 0 })
        } else {
            Err(FxfsError::NotFile.into())
        }
    }

    /// Open a directory with id `inode` for read.
    /// Return Ok if `inode` exists as a directory.
    /// Return NotDir if `inode` is not a directory.
    /// Return NotFound if `inode` does not exist.
    async fn opendir_fxfs(&self, inode: u64) -> FxfsResult<ReplyOpen> {
        if self.get_object_type(inode).await? == ObjectDescriptor::Directory {
            Ok(ReplyOpen { fh: 0, flags: 0 })
        } else {
            Err(FxfsError::NotDir.into())
        }
    }

    /// Read the entries of a directory with id `parent`.
    /// Return Ok with a stream of `parent`'s entries with their object id,
    /// name and type if successful.
    /// Return NotDir if `parent` is not a directory.
    /// Return NotFound if `parent` does not exist.
    async fn readdir_fxfs(
        &self,
        parent: u64,
        offset: i64,
    ) -> FxfsResult<ReplyDirectory<Iter<IntoIter<Result<DirectoryEntry>>>>> {
        if self.get_object_type(parent).await? == ObjectDescriptor::Directory {
            let dir = self.open_dir(parent).await?;
            let mut pre_children = vec![
                (parent, FileType::Directory, OsString::from("."), 1),
                // The attributes of ".." directory are automatically set by FUSE.
                (parent, FileType::Directory, OsString::from(".."), 2),
            ];

            let layer_set = dir.store().tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = dir.iter(&mut merger).await?;
            // Start the offset from 3 because the first two are "." and ".." directories.
            let mut entry_ofs = 3i64;

            while let Some((name, object_id, descriptor)) = iter.get() {
                let file_type = match descriptor {
                    ObjectDescriptor::File => FileType::RegularFile,
                    ObjectDescriptor::Directory => FileType::Directory,
                    ObjectDescriptor::Symlink => FileType::Symlink,
                    // Volumes are treated as Directories for now when reading directory.
                    ObjectDescriptor::Volume => FileType::Directory,
                };

                pre_children.push((object_id, file_type, OsString::from(name), entry_ofs));
                entry_ofs += 1;
                iter.advance().await?;
            }

            let pre_children = stream::iter(pre_children.into_iter());
            let children = pre_children
                .map(|(inode, kind, name, offset)| DirectoryEntry { inode, kind, name, offset })
                .skip(offset as _)
                .map(Ok)
                .collect::<Vec<_>>()
                .await;

            Ok(ReplyDirectory { entries: stream::iter(children) })
        } else {
            Err(FxfsError::NotDir.into())
        }
    }

    /// Read the entries with attributes of a directory with id `parent`.
    /// Return Ok with a stream of `inode`'s entries with their attributes if successful.
    /// Return NotDir if `parent` is not a directory.
    /// Return NotFound if `parent` does not exist.
    async fn readdirplus_fxfs(
        &self,
        parent: u64,
        offset: u64,
    ) -> FxfsResult<ReplyDirectoryPlus<Iter<IntoIter<Result<DirectoryEntryPlus>>>>> {
        if self.get_object_type(parent).await? == ObjectDescriptor::Directory {
            let parent_attr = self.create_object_attr(parent, ObjectDescriptor::Directory).await?;
            let dir = self.open_dir(parent).await?;
            let mut pre_children = vec![
                (parent, FileType::Directory, OsString::from("."), parent_attr, 1),
                // The attributes of ".." directory are automatically set by FUSE.
                (parent, FileType::Directory, OsString::from(".."), parent_attr, 2),
            ];

            let layer_set = dir.store().tree().layer_set();
            let mut merger = layer_set.merger();
            let mut iter = dir.iter(&mut merger).await?;
            // Start the offset from 3 because the first two are "." and ".." directories.
            let mut entry_ofs = 3i64;

            while let Some((name, object_id, descriptor)) = iter.get() {
                let file_type = match descriptor {
                    ObjectDescriptor::File => FileType::RegularFile,
                    ObjectDescriptor::Directory => FileType::Directory,
                    ObjectDescriptor::Symlink => FileType::Symlink,
                    // Volumes are treated as Directories for now when reading directory.
                    ObjectDescriptor::Volume => FileType::Directory,
                };
                let child_attr = self.create_object_attr(object_id, descriptor.clone()).await?;
                pre_children.push((
                    object_id,
                    file_type,
                    OsString::from(name),
                    child_attr,
                    entry_ofs,
                ));
                entry_ofs += 1;
                iter.advance().await.unwrap();
            }

            let pre_children = stream::iter(pre_children.into_iter());
            let children = pre_children
                .map(|(inode, kind, name, attr, offset)| DirectoryEntryPlus {
                    inode,
                    generation: 0,
                    kind,
                    name,
                    offset,
                    attr,
                    entry_ttl: TTL,
                    attr_ttl: TTL,
                })
                .skip(offset as _)
                .map(Ok)
                .collect::<Vec<_>>()
                .await;

            Ok(ReplyDirectoryPlus { entries: stream::iter(children) })
        } else {
            Err(FxfsError::NotDir.into())
        }
    }

    /// Allocate space for a file with id `inode`, starting from position
    /// `offset` with size `length`.
    /// Return Ok if the space is successfully allocated.
    /// Return NotFile if `inode` is a directory.
    /// Return NotFound if `inode` does not exist.
    async fn fallocate_fxfs(&self, inode: u64, offset: u64, length: u64) -> FxfsResult<()> {
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            let handle = self.get_object_handle(inode).await?;
            handle.truncate(offset + length).await?;
            handle.flush().await?;
            Ok(())
        } else {
            Err(FxfsError::NotFile.into())
        }
    }

    /// Find next data or hole after the specified offset with seek option `whence`
    /// in a file with id `inode`.
    /// Return `offset` if `whence` is SEEK_CUR or SEEK_SET.
    /// Return the length of remaining content if `whence` is SEEK_END.
    /// Return NotFile if `inode` is not a file.
    /// Return NotFound if `inode` does not exist.
    async fn lseek_fxfs(&self, inode: u64, offset: u64, whence: u32) -> FxfsResult<ReplyLSeek> {
        let whence = whence as i32;
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            let offset = if whence == libc::SEEK_CUR || whence == libc::SEEK_SET {
                offset
            } else if whence == libc::SEEK_END {
                let content_size = self
                    .get_object_properties(inode, ObjectDescriptor::File)
                    .await?
                    .data_attribute_size;
                if content_size >= offset { content_size - offset } else { 0 }
            } else {
                return Err(FxfsError::InvalidArgs.into());
            };
            Ok(ReplyLSeek { offset })
        } else {
            Err(FxfsError::NotFile.into())
        }
    }

    /// Copy a range of data from `inode` starting from position `off_in` into
    /// `inode_out` starting from position `off_out` with a size of `length`.
    /// Return Ok with copied length if successful.
    /// Return NotFile if `inode` or `inode_out` is not a file.
    /// Return NotFound if `inode` or `inode_out` does not exist.
    async fn copy_file_range_fxfs(
        &self,
        inode: u64,
        off_in: u64,
        inode_out: u64,
        off_out: u64,
        length: u64,
    ) -> FxfsResult<ReplyCopyFileRange> {
        let data = self.read_fxfs(inode, off_in, length as _).await?;
        let data = data.data.as_ref();
        let ReplyWrite { written } = self.write_fxfs(inode_out, off_out, data).await?;

        Ok(ReplyCopyFileRange { copied: u64::from(written) })
    }

    /// Read symlink with id `inode`.
    /// Return Ok with link of the symlink in bytes.
    /// Return NotFound if `inode` does not exist or `inode` is not a symlink.
    async fn readlink_fxfs(&self, inode: u64) -> FxfsResult<ReplyData> {
        Ok(ReplyData { data: self.default_store.read_symlink(inode).await?.into() })
    }

    /// Create a symlink called `name` under `parent` linking to `link`.
    /// Return the attributes of symlink if successful.
    /// Return NotDir if `parent` is not a directory.
    /// Return AlreadyExists if `name` already exists under `parent`.
    /// Return NotFound if `parent` does not exist.
    async fn symlink_fxfs(
        &self,
        parent: u64,
        name: &OsStr,
        link: &OsStr,
    ) -> FxfsResult<ReplyEntry> {
        let dir = self.open_dir(parent).await?;
        let mut transaction = self
            .fs
            .root_store()
            .new_transaction(
                lock_keys![LockKey::object(self.default_store.store_object_id(), dir.object_id())],
                Options::default(),
            )
            .await?;

        let symlink_id =
            dir.create_symlink(&mut transaction, link.as_bytes(), name.osstr_to_str()?).await?;

        transaction.commit().await?;
        Ok(ReplyEntry {
            ttl: TTL,
            attr: self.create_object_attr(symlink_id, ObjectDescriptor::Symlink).await?,
            generation: 0,
        })
    }

    /// Create a hard link called `new_name` under `new_parent` linking to `inode`.
    /// Return the updated attributes of `inode` if successful.
    /// Return NotDir if `parent` is not a directory.
    /// Return AlreadyExists if `name` already exists under `parent`.
    /// Return NotFound if `parent` or `inode` does not exist.
    /// Return NotFile if `inode` is a directory.
    async fn link_fxfs(
        &self,
        inode: u64,
        new_parent: u64,
        new_name: &OsStr,
    ) -> FxfsResult<ReplyEntry> {
        if self.get_object_type(inode).await? == ObjectDescriptor::File {
            let dir = self.open_dir(new_parent).await?;

            let mut transaction = self
                .fs
                .root_store()
                .new_transaction(
                    lock_keys![
                        LockKey::object(self.default_store.store_object_id(), dir.object_id()),
                        LockKey::object(self.default_store.store_object_id(), inode),
                    ],
                    Options::default(),
                )
                .await?;
            if dir.lookup(new_name.osstr_to_str()?).await?.is_none() {
                dir.insert_child(
                    &mut transaction,
                    new_name.osstr_to_str()?,
                    inode,
                    ObjectDescriptor::File,
                )
                .await?;

                // Increase the number of refs to the file by 1.
                self.default_store.adjust_refs(&mut transaction, inode, 1).await?;
                transaction.commit().await?;

                Ok(ReplyEntry {
                    ttl: TTL,
                    attr: self.create_object_attr(inode, ObjectDescriptor::File).await?,
                    generation: 0,
                })
            } else {
                Err(FxfsError::AlreadyExists.into())
            }
        } else {
            Err(FxfsError::NotFile.into())
        }
    }
}

/// FUSE VFS calls the inner Fxfs functions to handle the FUSE calls.
/// Some of the function parameters (i.e., request, mode, mask, fh, flags, unique
/// and lock_owner) are not supported by Fxfs and hence are ignored.
#[allow(refining_impl_trait)]
impl FuseFilesystem for FuseFs {
    /// Initialize filesystem. Called before any other filesystem method.
    async fn init(&self, _req: Request) -> Result<ReplyInit> {
        info!("init");
        // Advertise 1 MiB (1 << 20 bytes) maximum write buffer size, a common size for
        // modern FUSE drivers to enable efficient I/O chunks.
        Ok(ReplyInit { max_write: std::num::NonZeroU32::new(1 << 20).unwrap() })
    }

    /// Gracefully close filesystem. Currently fuse3 cannot handle unmount gracefully,
    /// see https://github.com/Sherlock-Holo/fuse3/issues/52.
    /// To fix it, destroy() is triggered when SIGTERM is received by the FUSE-Fxfs program.
    async fn destroy(&self, _req: Request) {
        info!("destroy");
        self.fs.close().await.expect("close failed");
    }

    /// Look up a entry called `name` under `parent` directory and get its attributes.
    /// Return Ok with object's attributes if successful.
    /// Return ENOENT if `parent` does not exist or `name` is not found under `parent`.
    async fn lookup(&self, _req: Request, parent: u64, name: &OsStr) -> Result<ReplyEntry> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("lookup (parent={:?}, name={:?}, name_len={:?})", parent, name, name.len());
        self.lookup_fxfs(parent, name).await.fxfs_result_to_fuse_result()
    }

    /// Create a directory called `name` under `parent` directory.
    /// Return Ok with the new directory's attributes if successful.
    /// Return ENOENT if `parent` does not exist.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return EEXIST if `name` already exists under `parent` directory.
    async fn mkdir(
        &self,
        _req: Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
    ) -> Result<ReplyEntry> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("mkdir (parent={:?}, name={:?})", parent, name);
        self.mkdir_fxfs(parent, name).await.fxfs_result_to_fuse_result()
    }

    /// Remove a file or symlink called `name` under `parent` directory.
    /// Return Ok if the object is successfully removed.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return EISDIR if `name` is a directory under `parent`.
    /// Return ENOENT if `name` is not found under `parent`.
    async fn unlink(&self, _req: Request, parent: u64, name: &OsStr) -> Result<()> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("unlink (parent={:?}, name={:?})", parent, name);
        self.unlink_fxfs(parent, name).await.fxfs_result_to_fuse_result()
    }

    /// Remove a directory called `name` under `parent` directory.
    /// Return Ok if the directory called `name` is successfully removed.
    /// Return ENOTDIR if `name` is not a directory.
    /// Return ENOTEMPTY if `name` directory is not empty.
    /// Return ENOENT if `name` is not found under `parent`.
    async fn rmdir(&self, _req: Request, parent: u64, name: &OsStr) -> Result<()> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("rmdir (parent={:?}, name={:?})", parent, name);
        self.rmdir_fxfs(parent, name).await.fxfs_result_to_fuse_result()
    }

    /// Rename an object called `name` under `parent` to an object called
    /// `new_name` under `new_parent`.
    /// Return Ok if the rename of object is successful.
    /// Return ENOTDIR if `parent` or `new_parent` is not a directory.
    /// Return ENOENT if `parent` or `new_parent` does not exist, or `name`
    /// or `new_name` does not exist under their parents.
    async fn rename(
        &self,
        _req: Request,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
    ) -> Result<()> {
        let parent = self.fuse_inode_to_object_id(parent);
        let new_parent = self.fuse_inode_to_object_id(new_parent);
        info!(
            "rename (parent={:?}, name={:?}, new_parent={:?}, new_name={:?})",
            parent, name, new_parent, new_name
        );
        self.rename_fxfs(parent, name, new_parent, new_name).await.fxfs_result_to_fuse_result()
    }

    /// Read data from a file object with id `inode`,
    /// which starts from `offset` with a length of `size`.
    /// Return Ok with the read data if successful.
    /// Return ENOENT if `inode` does not exist.
    /// Return EISDIR if `inode` is a directory.
    async fn read(
        &self,
        _req: Request,
        inode: u64,
        _fh: u64,
        offset: u64,
        size: u32,
    ) -> Result<ReplyData> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("read (inode={:?}, offset={:?}, size={:?})", inode, offset, size);
        self.read_fxfs(inode, offset, size).await.fxfs_result_to_fuse_result()
    }

    /// Write data into a file object with id `inode`, starting from `offset`.
    /// Return Ok with the length of written data if successful.
    /// Return EISDIR if `inode` is a directory.
    /// Return ENOENT if `inode` does not exist.
    async fn write(
        &self,
        _req: Request,
        inode: u64,
        _fh: u64,
        offset: u64,
        data: &[u8],
        _write_flags: u32,
        _flags: u32,
    ) -> Result<ReplyWrite> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("write (inode={:?}, offset={:?}, data_len={:?})", inode, offset, data.len());
        self.write_fxfs(inode, offset, data).await.fxfs_result_to_fuse_result()
    }

    /// Release an open file object with id `inode` to indicate its end of read or write.
    /// Parameter `flush` is unused because data is flushed at every read/write.
    /// Return OK and remove `inode`'s file type and handle from cache if successful.
    /// Return EISDIR if `inode` is a directory.
    /// Return ENOENT if `inode` does not exist.
    async fn release(
        &self,
        _req: Request,
        inode: u64,
        _fh: u64,
        _flags: u32,
        _lock_owner: u64,
        _flush: bool,
    ) -> Result<()> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("release (inode={:?})", inode);
        self.release_fxfs(inode).await.fxfs_result_to_fuse_result()
    }

    /// Forget an object with id `inode`.
    /// This is only called for objects with a limited lifetime, and hence not needed in Fxfs.
    /// TODO(https://fxbug.dev/42068673): Implement this after Fxfs objects support limied lifetime.
    /// TODO(https://fxbug.dev/42073102): Some applications may call this function to remove objects.
    async fn forget(&self, _req: Request, inode: u64, _nlookup: u64) {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("forget (inode={:?})", inode);
    }

    /// Get attributes of an object with id `inode`.
    /// Return Ok with `inode`'s attributes if successful.
    /// Return ENOENT if `inode` does not exist.
    async fn getattr(
        &self,
        _req: Request,
        inode: u64,
        _fh: Option<u64>,
        _flags: u32,
    ) -> Result<ReplyAttr> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("getattr (inode={:?})", inode);
        self.getattr_fxfs(inode).await.fxfs_result_to_fuse_result()
    }

    /// Set attributes of an object with id `inode` including its timestamps and object size.
    /// Return Ok with `inode`'s new attributes if successful.
    /// Return ENOENT if `inode` does not exist.
    async fn setattr(
        &self,
        _req: Request,
        inode: u64,
        _fh: Option<u64>,
        set_attr: SetAttr,
    ) -> Result<ReplyAttr> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!(
            "setattr (inode={:?}, size={:?}, ctime={:?}, mtime={:?})",
            inode, set_attr.size, set_attr.ctime, set_attr.mtime
        );
        self.setattr_fxfs(inode, set_attr).await.fxfs_result_to_fuse_result()
    }

    /// Set an extended attribute of an object with id `inode`.
    /// Fxfs currently doesn't support this feature.
    /// TODO(https://fxbug.dev/42072627): Add support for setting extended attributes.
    async fn setxattr(
        &self,
        _req: Request,
        inode: u64,
        name: &OsStr,
        _value: &[u8],
        _flags: u32,
        _position: u32,
    ) -> Result<()> {
        info!("setxattr {:?}, {:?}", inode, name);
        Err(libc::ENOSYS.into())
    }

    /// Get an extended attribute of an object with id `inode`.
    /// Fxfs currently doesn't support this feature.
    /// TODO(https://fxbug.dev/42072627): Add support for getting extended attributes.
    async fn getxattr(
        &self,
        _req: Request,
        inode: u64,
        name: &OsStr,
        size: u32,
    ) -> Result<ReplyXAttr> {
        info!("getxattr {:?}, {:?} {:?}", inode, name, size);
        Err(libc::ENOSYS.into())
    }

    /// Synchronize the filesystem contents.
    async fn fsync(&self, _req: Request, _inode: u64, _fh: u64, _datasync: bool) -> Result<()> {
        info!("fsync");
        self.fsync_fxfs().await.fxfs_result_to_fuse_result()
    }

    /// Flush the data.
    /// This function does nothing because data is flushed at every write in Fxfs.
    async fn flush(&self, _req: Request, inode: u64, _fh: u64, _lock_owner: u64) -> Result<()> {
        info!("flush (inode={:?})", inode);
        Ok(())
    }

    /// Check access permission of `inode`.
    /// Return Ok if `inode` exists.
    /// Return ENOENT if `inode` does not exist.
    async fn access(&self, _req: Request, inode: u64, _mask: u32) -> Result<()> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("access (inode={:?})", inode);
        self.access_fxfs(inode).await.fxfs_result_to_fuse_result()
    }

    /// Create a file called `name` under `parent` directory.
    /// Return Ok with the new file's attributes if successful.
    /// Return ENOENT if `parent` does not exist.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return EEXIST if `name` already exists under `parent`.
    async fn create(
        &self,
        _req: Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        flags: u32,
    ) -> Result<ReplyCreated> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("create (parent={:?}, name={:?})", parent, name);
        self.create_fxfs(parent, name, flags).await.fxfs_result_to_fuse_result()
    }

    /// Open a file object with id `inode` for read or write.
    /// Return Ok and store file's handle and type in cache if successful.
    /// Return ENOENT if `inode` does not exist.
    /// Return EISDIR if `inode` is a directory.
    async fn open(&self, _req: Request, inode: u64, _flags: u32) -> Result<ReplyOpen> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("open (inode={:?})", inode);
        self.open_fxfs(inode).await.fxfs_result_to_fuse_result()
    }

    /// Handle interrupt on the user side.
    async fn interrupt(&self, _req: Request, _unique: u64) -> Result<()> {
        info!("interrupt");
        Ok(())
    }

    /// Open a directory with id `inode` for read.
    /// Return Ok if `inode` exists as a directory.
    /// Return ENOTDIR if `inode` is not a directory.
    /// Return ENOENT if `inode` does not exist.
    async fn opendir(&self, _req: Request, inode: u64, _flags: u32) -> Result<ReplyOpen> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("opendir (inode={:?})", inode);
        self.opendir_fxfs(inode).await.fxfs_result_to_fuse_result()
    }

    /// Read the entries of a directory with id `parent`.
    /// Return Ok with a stream of `parent`'s entries with their object id,
    /// name and type if successful.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return ENOENT if `parent` does not exist.
    async fn readdir(
        &self,
        _req: Request,
        parent: u64,
        _fh: u64,
        offset: i64,
    ) -> Result<ReplyDirectory<Iter<IntoIter<Result<DirectoryEntry>>>>> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("readdir (parent={:?})", parent);
        self.readdir_fxfs(parent, offset).await.fxfs_result_to_fuse_result()
    }

    /// Read the entries with attributes of a directory with id `parent`.
    /// Return Ok with a stream of `inode`'s entries with their attributes if successful.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return ENOENT if `parent` does not exist.
    async fn readdirplus(
        &self,
        _req: Request,
        parent: u64,
        _fh: u64,
        offset: u64,
        _lock_owner: u64,
    ) -> Result<ReplyDirectoryPlus<Iter<IntoIter<Result<DirectoryEntryPlus>>>>> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("readdirplus (parent={:?})", parent);
        self.readdirplus_fxfs(parent, offset).await.fxfs_result_to_fuse_result()
    }

    /// Rename a file or directory with flags.
    /// Directly call `rename` because flags are not used in Fxfs.
    async fn rename2(
        &self,
        req: Request,
        parent: u64,
        name: &OsStr,
        new_parent: u64,
        new_name: &OsStr,
        _flags: u32,
    ) -> Result<()> {
        self.rename(req, parent, name, new_parent, new_name).await
    }

    /// Allocate space for a file with id `inode`, starting from position
    /// `offset` with size `length`.
    /// Return Ok if the space is successfully allocated.
    /// Return EISDIR if `inode` is a directory.
    /// Return ENOENT if `inode` does not exist.
    async fn fallocate(
        &self,
        _req: Request,
        inode: u64,
        _fh: u64,
        offset: u64,
        length: u64,
        _mode: u32,
    ) -> Result<()> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("fallocate (inode={:?}, offset={:?}, length={:?})", inode, offset, length);
        self.fallocate_fxfs(inode, offset, length).await.fxfs_result_to_fuse_result()
    }

    /// Find next data or hole after the specified offset with seek option `whence`
    /// in a file with id `inode`.
    /// Return `offset` if `whence` is SEEK_CUR or SEEK_SET.
    /// Return the length of remaining content if `whence` is SEEK_END.
    /// Return EISDIR if `inode` is not a file.
    /// Return ENOENT if `inode` does not exist.
    async fn lseek(
        &self,
        _req: Request,
        inode: u64,
        _fh: u64,
        offset: u64,
        whence: u32,
    ) -> Result<ReplyLSeek> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("lseek (inode={:?})", inode);
        self.lseek_fxfs(inode, offset, whence).await.fxfs_result_to_fuse_result()
    }

    /// Copy a range of data from `inode` starting from position `off_in` into
    /// `inode_out` starting from position `off_out` with a size of `length`.
    /// Return Ok with copied length if successful.
    /// Return EISDIR if `inode` or `inode_out` is not a file.
    /// Return ENOENT if `inode` or `inode_out` does not exist.
    async fn copy_file_range(
        &self,
        _req: Request,
        inode: u64,
        _fh_in: u64,
        off_in: u64,
        inode_out: u64,
        _fh_out: u64,
        off_out: u64,
        length: u64,
        _flags: u64,
    ) -> Result<ReplyCopyFileRange> {
        let inode = self.fuse_inode_to_object_id(inode);
        let inode_out = self.fuse_inode_to_object_id(inode_out);
        info!("copy_file_range (inode={:?}, inode_out={:?})", inode, inode_out);
        self.copy_file_range_fxfs(inode, off_in, inode_out, off_out, length)
            .await
            .fxfs_result_to_fuse_result()
    }

    /// Read symlink with id `inode`.
    /// Return Ok with link of the symlink in bytes.
    /// Return ENOENT if `inode` does not exist or `inode` is not a symlink.
    async fn readlink(&self, _req: Request, inode: u64) -> Result<ReplyData> {
        let inode = self.fuse_inode_to_object_id(inode);
        info!("readlink (inode={:?})", inode);
        self.readlink_fxfs(inode).await.fxfs_result_to_fuse_result()
    }

    /// Create a symlink called `name` under `parent` linking to `link`.
    /// Return the attributes of symlink if successful.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return EEXIST if `name` already exists under `parent`.
    /// Return ENOENT if `parent` does not exist.
    async fn symlink(
        &self,
        _req: Request,
        parent: u64,
        name: &OsStr,
        link: &OsStr,
    ) -> Result<ReplyEntry> {
        let parent = self.fuse_inode_to_object_id(parent);
        info!("symlink (parent={:?}, name={:?}, link={:?})", parent, name, link);
        self.symlink_fxfs(parent, name, link).await.fxfs_result_to_fuse_result()
    }

    /// Create a hard link called `new_name` under `new_parent` linking to `inode`.
    /// Return the updated attributes of `inode` if successful.
    /// Return ENOTDIR if `parent` is not a directory.
    /// Return EEXIST if `name` already exists under `parent`.
    /// Return ENOENT if `parent` or `inode` does not exist.
    /// Return EISDIR if `inode` is a directory.
    async fn link(
        &self,
        _req: Request,
        inode: u64,
        new_parent: u64,
        new_name: &OsStr,
    ) -> Result<ReplyEntry> {
        let inode = self.fuse_inode_to_object_id(inode);
        let new_parent = self.fuse_inode_to_object_id(new_parent);
        info!("link (inode={:?}, new_parent={:?}, new_name={:?})", inode, new_parent, new_name);
        self.link_fxfs(inode, new_parent, new_name).await.fxfs_result_to_fuse_result()
    }
}

#[cfg(test)]
mod tests {
    use crate::fuse_fs::{FuseFs, FuseStrParser};
    use fuse3::raw::prelude::{DirectoryEntry, DirectoryEntryPlus};
    use fuse3::raw::{Filesystem, Request};
    use fuse3::{Errno, FileType, Result, SetAttr, Timestamp};
    use futures::stream::StreamExt;
    use fxfs::errors::FxfsError;
    use fxfs::object_store::ObjectDescriptor;
    use std::ffi::OsStr;

    const DEFAULT_FILE_MODE: u32 = 0o755;
    const DEFAULT_FLAG: u32 = 0;
    const DEFAULT_TIME: Timestamp = Timestamp { sec: 10i64, nsec: 20u32 };
    const INVALID_INODE: u64 = 0;
    const TEST_DATA: &[u8] = b"hello";

    /// Create fake request for testing purpose.
    fn new_fake_request() -> Request {
        Request { unique: 0, uid: 0, gid: 0, pid: 0 }
    }

    #[fuchsia::test]
    async fn test_mkdir_create_directory_tree() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let (foo_id, foo_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(foo_descriptor, ObjectDescriptor::Directory);
        assert_eq!(mkdir_reply.attr.ino, foo_id);
        assert_eq!(mkdir_reply.attr.kind, FileType::Directory);

        let _mkdir_reply = fs
            .mkdir(new_fake_request(), foo_id, OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let child_dir = fs.open_dir(foo_id).await.expect("open_dir failed");
        let (bar_id, bar_descriptor, _) =
            child_dir.lookup(OsStr::new("bar").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(bar_descriptor, ObjectDescriptor::Directory);
        assert_eq!(_mkdir_reply.attr.ino, bar_id);
        assert_eq!(_mkdir_reply.attr.kind, FileType::Directory);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_mkdir_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_res = fs
            .mkdir(new_fake_request(), INVALID_INODE, OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await;
        assert_eq!(mkdir_res, Err(libc::ENOENT.into()));

        let lookup_res = dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap();
        assert_eq!(lookup_res, None);
        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_mkdir_fails_when_directory_already_exists() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        fs.mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let mkdir_res = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await;
        assert_eq!(mkdir_res, Err(libc::EEXIST.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_mkdir_fails_when_file_already_exists() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let dir = fs.root_dir().await.expect("root_dir failed");
        fs.create(
            new_fake_request(),
            dir.object_id(),
            OsStr::new("foo"),
            DEFAULT_FILE_MODE,
            DEFAULT_FLAG,
        )
        .await
        .expect("create file failed");
        let mkdir_res = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await;
        assert_eq!(mkdir_res, Err(libc::EEXIST.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rmdir_remove_directory_tree() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        fs.mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let (child_id, child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(child_descriptor, ObjectDescriptor::Directory);
        assert_ne!(child_id, dir.object_id());

        fs.rmdir(new_fake_request(), dir.object_id(), OsStr::new("foo"))
            .await
            .expect("rmdir failed");
        let result = dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap();
        assert_eq!(result, None);
        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rmdir_fails_when_directory_is_not_empty() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        fs.mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let (child_id, _, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        fs.mkdir(new_fake_request(), child_id, OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");

        let rmdir_res = fs.rmdir(new_fake_request(), dir.object_id(), OsStr::new("foo")).await;
        assert_eq!(rmdir_res, Err(libc::ENOTEMPTY.into()));
        let (_child_id, child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(child_id, _child_id);
        assert_eq!(child_descriptor, ObjectDescriptor::Directory);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rmdir_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let rmdir_res = fs.rmdir(new_fake_request(), INVALID_INODE, OsStr::new("foo")).await;
        assert_eq!(rmdir_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rmdir_fails_when_object_is_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let dir = fs.root_dir().await.expect("root_dir failed");
        fs.create(
            new_fake_request(),
            dir.object_id(),
            OsStr::new("foo"),
            DEFAULT_FILE_MODE,
            DEFAULT_FLAG,
        )
        .await
        .expect("create file failed");
        let rmdir_res = fs.rmdir(new_fake_request(), dir.object_id(), OsStr::new("foo")).await;
        assert_eq!(rmdir_res, Err(libc::ENOTDIR.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lookup_search_for_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");

        let lookup_reply = fs
            .lookup(new_fake_request(), dir.object_id(), OsStr::new("foo"))
            .await
            .expect("lookup failed");

        assert_eq!(lookup_reply.attr.ino, mkdir_reply.attr.ino);
        assert_eq!(lookup_reply.attr.kind, FileType::Directory);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lookup_search_for_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let file_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create file failed");
        let lookup_reply = fs
            .lookup(new_fake_request(), dir.object_id(), OsStr::new("foo"))
            .await
            .expect("lookup failed");

        assert_eq!(lookup_reply.attr.ino, file_reply.attr.ino);
        assert_eq!(lookup_reply.attr.kind, FileType::RegularFile);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lookup_search_for_symlink() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let symlink_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("symlink failed");
        let lookup_reply = fs
            .lookup(new_fake_request(), dir.object_id(), OsStr::new("link"))
            .await
            .expect("lookup failed");

        assert_eq!(lookup_reply.attr.ino, symlink_reply.attr.ino);
        assert_eq!(lookup_reply.attr.kind, FileType::Symlink);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lookup_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let lookup_res = fs.lookup(new_fake_request(), INVALID_INODE, OsStr::new("foo")).await;
        assert_eq!(lookup_res, Err(libc::ENOENT.into()));
        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lookup_fails_when_name_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");
        let lookup_res = fs.lookup(new_fake_request(), dir.object_id(), OsStr::new("foo")).await;
        assert_eq!(lookup_res, Err(libc::ENOENT.into()));
        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_unlink_remove_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let file_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create file failed");
        let unlink_res = fs.unlink(new_fake_request(), dir.object_id(), OsStr::new("foo")).await;
        let lookup_res = dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap();
        assert!(FxfsError::NotFound.matches(
            &fs.get_object_type(file_reply.attr.ino).await.expect_err("get_object_type succeeded")
        ));

        assert_eq!(unlink_res, Ok(()));
        assert_eq!(lookup_res, None);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_unlink_remove_symlink() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");
        let symlin_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("symlink failed");

        let unlink_res = fs.unlink(new_fake_request(), dir.object_id(), OsStr::new("link")).await;
        let lookup_res = dir.lookup(OsStr::new("link").osstr_to_str().unwrap()).await.unwrap();
        assert!(
            FxfsError::NotFound.matches(
                &fs.get_object_type(symlin_reply.attr.ino)
                    .await
                    .expect_err("get_object_type succeeded")
            )
        );

        assert_eq!(unlink_res, Ok(()));
        assert_eq!(lookup_res, None);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_unlink_fails_when_object_is_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");
        fs.mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");

        let unlink_res = fs.unlink(new_fake_request(), dir.object_id(), OsStr::new("foo")).await;
        assert_eq!(unlink_res, Err(libc::EISDIR.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_unlink_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let unlink_res = fs.unlink(new_fake_request(), INVALID_INODE, OsStr::new("foo")).await;
        assert_eq!(unlink_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_create_new_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let (child_id, child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(child_descriptor, ObjectDescriptor::File);
        assert_eq!(create_reply.attr.ino, child_id);
        assert_eq!(create_reply.attr.kind, FileType::RegularFile);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_create_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_res = fs
            .create(new_fake_request(), INVALID_INODE, OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await;
        assert_eq!(create_res, Err(libc::ENOENT.into()));

        let lookup_res = dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap();
        assert_eq!(lookup_res, None);
        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_create_fails_when_file_already_exists() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        fs.create(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let create_res = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await;
        assert_eq!(create_res, Err(libc::EEXIST.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_create_fails_when_directory_already_exists() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        fs.mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let create_res = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await;
        assert_eq!(create_res, Err(libc::EEXIST.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_move_file_to_another_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let foo_mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let bar_mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let src_dir = foo_mkdir_reply.attr.ino;
        let dst_dir = bar_mkdir_reply.attr.ino;

        fs.create(new_fake_request(), src_dir, OsStr::new("old"), DEFAULT_FILE_MODE, DEFAULT_FLAG)
            .await
            .expect("create failed");
        fs.rename(new_fake_request(), src_dir, OsStr::new("old"), dst_dir, OsStr::new("new"))
            .await
            .expect("rename failed");
        let bar_child_dir = fs.open_dir(dst_dir).await.expect("open_dir failed");
        let bar_lookup_res = bar_child_dir
            .lookup(OsStr::new("new").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(bar_lookup_res.is_some(), true);
        assert_eq!(bar_lookup_res.unwrap().1, ObjectDescriptor::File);

        let foo_child_dir = fs.open_dir(src_dir).await.expect("open_dir failed");
        let foo_lookup_res = foo_child_dir
            .lookup(OsStr::new("old").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(foo_lookup_res, None);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_rename_file_in_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let new_dir = mkdir_reply.attr.ino;

        fs.create(new_fake_request(), new_dir, OsStr::new("old"), DEFAULT_FILE_MODE, DEFAULT_FLAG)
            .await
            .expect("create failed");
        fs.rename(new_fake_request(), new_dir, OsStr::new("old"), new_dir, OsStr::new("new"))
            .await
            .expect("rename failed");
        let child_dir = fs.open_dir(new_dir).await.expect("open_dir failed");
        let lookup_res = child_dir
            .lookup(OsStr::new("new").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(lookup_res.is_some(), true);
        assert_eq!(lookup_res.unwrap().1, ObjectDescriptor::File);

        let _lookup_res = child_dir
            .lookup(OsStr::new("old").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(_lookup_res, None);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_fails_when_old_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let dst_dir = mkdir_reply.attr.ino;

        let rename_res = fs
            .rename(
                new_fake_request(),
                INVALID_INODE,
                OsStr::new("old"),
                dst_dir,
                OsStr::new("new"),
            )
            .await;
        assert_eq!(rename_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_fails_when_old_name_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let _mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let src_dir = mkdir_reply.attr.ino;
        let dst_dir = mkdir_reply.attr.ino;

        let rename_res = fs
            .rename(new_fake_request(), src_dir, OsStr::new("old"), dst_dir, OsStr::new("new"))
            .await;
        assert_eq!(rename_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_fails_when_new_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let src_dir = mkdir_reply.attr.ino;

        fs.create(new_fake_request(), src_dir, OsStr::new("old"), DEFAULT_FILE_MODE, DEFAULT_FLAG)
            .await
            .expect("create failed");
        let rename_res = fs
            .rename(
                new_fake_request(),
                src_dir,
                OsStr::new("old"),
                INVALID_INODE,
                OsStr::new("new"),
            )
            .await;
        assert_eq!(rename_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_move_file_to_another_directory_where_new_name_exists() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let foo_mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let bar_mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let src_dir = foo_mkdir_reply.attr.ino;
        let dst_dir = bar_mkdir_reply.attr.ino;

        fs.create(new_fake_request(), src_dir, OsStr::new("old"), DEFAULT_FILE_MODE, DEFAULT_FLAG)
            .await
            .expect("create failed");
        fs.create(new_fake_request(), dst_dir, OsStr::new("new"), DEFAULT_FILE_MODE, DEFAULT_FLAG)
            .await
            .expect("create failed");
        fs.rename(new_fake_request(), src_dir, OsStr::new("old"), dst_dir, OsStr::new("new"))
            .await
            .expect("rename failed");
        let bar_child_dir = fs.open_dir(dst_dir).await.expect("open_dir failed");
        let bar_lookup_res = bar_child_dir
            .lookup(OsStr::new("new").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(bar_lookup_res.is_some(), true);
        assert_eq!(bar_lookup_res.unwrap().1, ObjectDescriptor::File);

        let foo_child_dir = fs.open_dir(src_dir).await.expect("open_dir failed");
        let foo_lookup_res = foo_child_dir
            .lookup(OsStr::new("old").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(foo_lookup_res, None);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_rename_move_directory_to_under_another_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let foo_mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let bar_mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("bar"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let src_dir = foo_mkdir_reply.attr.ino;
        let dst_dir = bar_mkdir_reply.attr.ino;

        fs.mkdir(new_fake_request(), src_dir, OsStr::new("old"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        fs.rename(new_fake_request(), src_dir, OsStr::new("old"), dst_dir, OsStr::new("new"))
            .await
            .expect("rename failed");
        let bar_child_dir = fs.open_dir(dst_dir).await.expect("open_dir failed");
        let bar_lookup_res = bar_child_dir
            .lookup(OsStr::new("new").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(bar_lookup_res.is_some(), true);
        assert_eq!(bar_lookup_res.unwrap().1, ObjectDescriptor::Directory);

        let foo_child_dir = fs.open_dir(src_dir).await.expect("open_dir failed");
        let foo_lookup_res = foo_child_dir
            .lookup(OsStr::new("old").osstr_to_str().unwrap())
            .await
            .expect("lookup failed");
        assert_eq!(foo_lookup_res, None);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_open_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let open_res = fs.open(new_fake_request(), create_reply.attr.ino, DEFAULT_FLAG).await;
        assert_eq!(open_res.is_ok(), true);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_open_fails_when_file_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let open_res = fs.open(new_fake_request(), INVALID_INODE, DEFAULT_FLAG).await;
        assert_eq!(open_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_open_fails_when_object_is_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let open_res = fs.open(new_fake_request(), mkdir_reply.attr.ino, DEFAULT_FLAG).await;
        assert_eq!(open_res, Err(libc::EISDIR.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_same_length_as_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_offset_smaller_than_block_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 1, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA[1..]);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_offset_equal_to_block_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                fs.fs.block_size().get(),
                TEST_DATA.len() as _,
            )
            .await
            .expect("read failed");
        assert_eq!(read_reply.data.len(), 0);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_offset_larger_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 512, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data.len(), 0);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_smaller_length_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, (TEST_DATA.len() / 2) as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA[0..TEST_DATA.len() / 2]);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_non_zero_offset_and_length_smaller_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 1, (TEST_DATA.len() / 2) as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA[1..TEST_DATA.len() / 2 + 1]);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_length_larger_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, (TEST_DATA.len() * 2) as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_with_non_zero_offset_and_length_larger_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 1, (TEST_DATA.len() * 2) as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA[1..]);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_when_file_is_empty() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data.len(), 0);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_fails_when_file_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let read_res = fs.read(new_fake_request(), INVALID_INODE, 0, 0, TEST_DATA.len() as _).await;
        assert_eq!(read_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(read_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_read_fails_when_object_is_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let read_res =
            fs.read(new_fake_request(), mkdir_reply.attr.ino, 0, 0, TEST_DATA.len() as _).await;
        assert_eq!(read_res.is_err(), true);
        let err: Errno = libc::EISDIR.into();
        assert_eq!(read_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_to_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data, TEST_DATA);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_with_offset_smaller_than_file_size_and_file_size_unchanged() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 1, b"aa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, 2u32);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"haalo";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_with_offset_smaller_than_file_size_and_file_size_changed() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 4, b"aaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, 3u32);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, (TEST_DATA.len() + 2) as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"hellaaa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_with_offset_equal_to_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 5, b"aaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, 3u32);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, (TEST_DATA.len() + 3) as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"helloaaa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_with_offset_larger_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 6, b"aaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, 3u32);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, (TEST_DATA.len() + 4) as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"hello\0aaa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_when_file_is_empty() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, b"", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, 0u32);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        assert_eq!(read_reply.data.len(), 0);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_fails_when_file_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let write_res =
            fs.write(new_fake_request(), INVALID_INODE, 0, 0, TEST_DATA, 0, DEFAULT_FLAG).await;
        assert_eq!(write_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(write_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_write_fails_when_object_is_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let write_res = fs
            .write(new_fake_request(), mkdir_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await;
        assert_eq!(write_res.is_err(), true);
        let err: Errno = libc::EISDIR.into();
        assert_eq!(write_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_getattr_on_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let attr_reply = fs
            .getattr(new_fake_request(), mkdir_reply.attr.ino, Some(0), DEFAULT_FLAG)
            .await
            .expect("getattr failed");
        assert_eq!(attr_reply.attr.ino, mkdir_reply.attr.ino);
        assert_eq!(attr_reply.attr.kind, FileType::Directory);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_getattr_on_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let attr_reply = fs
            .getattr(new_fake_request(), create_reply.attr.ino, Some(0), DEFAULT_FLAG)
            .await
            .expect("getattr failed");
        assert_eq!(attr_reply.attr.ino, create_reply.attr.ino);
        assert_eq!(attr_reply.attr.kind, FileType::RegularFile);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_getattr_on_symlink() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let symlink_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("create failed");
        let attr_reply = fs
            .getattr(new_fake_request(), symlink_reply.attr.ino, Some(0), DEFAULT_FLAG)
            .await
            .expect("getattr failed");
        assert_eq!(attr_reply.attr.ino, symlink_reply.attr.ino);
        assert_eq!(attr_reply.attr.kind, FileType::Symlink);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_getattr_fails_when_object_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let attr_res = fs.getattr(new_fake_request(), INVALID_INODE, Some(0), DEFAULT_FLAG).await;
        assert_eq!(attr_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_on_directory_with_time() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let mut set_attr = SetAttr::default();
        set_attr.mtime = Some(DEFAULT_TIME);

        let setattr_reply = fs
            .setattr(new_fake_request(), mkdir_reply.attr.ino, Some(0), set_attr)
            .await
            .expect("setattr failed");
        assert_eq!(setattr_reply.attr.mtime, DEFAULT_TIME);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_on_file_with_time() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let mut set_attr = SetAttr::default();
        set_attr.mtime = Some(DEFAULT_TIME);

        let setattr_reply = fs
            .setattr(new_fake_request(), create_reply.attr.ino, Some(0), set_attr)
            .await
            .expect("setattr failed");
        assert_eq!(setattr_reply.attr.mtime, DEFAULT_TIME);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_on_file_with_zero_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);
        let mut set_attr = SetAttr::default();
        set_attr.size = Some(0u64);

        let setattr_reply = fs
            .setattr(new_fake_request(), create_reply.attr.ino, Some(0), set_attr)
            .await
            .expect("setattr failed");
        assert_eq!(setattr_reply.attr.size, 0);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_on_file_with_shrinked_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);
        let mut set_attr = SetAttr::default();
        set_attr.size = Some(2u64);

        let setattr_reply = fs
            .setattr(new_fake_request(), create_reply.attr.ino, Some(0), set_attr)
            .await
            .expect("setattr failed");
        assert_eq!(setattr_reply.attr.size, 2);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"he";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_on_file_with_increased_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);
        let mut set_attr = SetAttr::default();
        set_attr.size = Some(8u64);

        let setattr_reply = fs
            .setattr(new_fake_request(), create_reply.attr.ino, Some(0), set_attr)
            .await
            .expect("setattr failed");
        assert_eq!(setattr_reply.attr.size, 8);

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"hello";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_on_symlink_with_time() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let symlink_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("symlink failed");
        let mut set_attr = SetAttr::default();
        set_attr.mtime = Some(DEFAULT_TIME);

        let setattr_reply = fs
            .setattr(new_fake_request(), symlink_reply.attr.ino, Some(0), set_attr)
            .await
            .expect("setattr failed");
        assert_eq!(setattr_reply.attr.ino, symlink_reply.attr.ino);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_setattr_fails_when_object_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let mut set_attr = SetAttr::default();
        set_attr.atime = Some(DEFAULT_TIME);
        let attr_res = fs.setattr(new_fake_request(), INVALID_INODE, Some(0), set_attr).await;
        assert_eq!(attr_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_fallocate_alllocate_space_to_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("mkdir failed");
        fs.fallocate(new_fake_request(), create_reply.attr.ino, 0, 0, 128, DEFAULT_FILE_MODE)
            .await
            .expect("fallocate failed");

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_fallocate_allocate_zero_space_to_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        fs.fallocate(new_fake_request(), create_reply.attr.ino, 0, 0, 0, DEFAULT_FILE_MODE)
            .await
            .expect("fallocate failed");

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_fallocate_fails_when_file_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let fallocate_res =
            fs.fallocate(new_fake_request(), INVALID_INODE, 0, 0, 128, DEFAULT_FILE_MODE).await;
        assert_eq!(fallocate_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_fallocate_fails_when_object_is_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let fallocate_res = fs
            .fallocate(new_fake_request(), mkdir_reply.attr.ino, 0, 0, 128, DEFAULT_FILE_MODE)
            .await;
        assert_eq!(fallocate_res, Err(libc::EISDIR.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_fallocate_with_offset_smaller_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("mkdir failed");

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        fs.fallocate(new_fake_request(), create_reply.attr.ino, 0, 3, 128, DEFAULT_FILE_MODE)
            .await
            .expect("fallocate failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"hello";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_fallocate_with_offset_larger_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("mkdir failed");

        let write_reply = fs
            .write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");
        assert_eq!(write_reply.written, TEST_DATA.len() as u32);

        fs.fallocate(
            new_fake_request(),
            create_reply.attr.ino,
            0,
            TEST_DATA.len() as u64 + 2,
            128,
            DEFAULT_FILE_MODE,
        )
        .await
        .expect("fallocate failed");

        let read_reply = fs
            .read(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA.len() as _)
            .await
            .expect("read failed");
        let result_data: &[u8] = b"hello";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_opendir_open_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        fs.opendir(new_fake_request(), mkdir_reply.attr.ino, 0).await.expect("opendir failed");

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_opendir_fails_when_directory_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let opendir_res = fs.opendir(new_fake_request(), INVALID_INODE, 0).await;
        assert_eq!(opendir_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_opendir_fails_when_object_is_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let opendir_res = fs.opendir(new_fake_request(), create_reply.attr.ino, 0).await;
        assert_eq!(opendir_res, Err(libc::ENOTDIR.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdir_read_directory_entries() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let symlink_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("symlink failed");
        let readdir_reply =
            fs.readdir(new_fake_request(), dir.object_id(), 0, 0).await.expect("readdir failed");
        let entries = readdir_reply.entries.collect::<Vec<Result<DirectoryEntry>>>().await;
        assert_eq!(entries.len(), 5);

        let root_entry = entries[0].clone().unwrap();
        let parent_entry = entries[1].clone().unwrap();
        let file_entry = entries[2].clone().unwrap();
        let dir_entry = entries[3].clone().unwrap();
        let symlink_entry = entries[4].clone().unwrap();

        assert_eq!(root_entry.inode, dir.object_id());
        assert_eq!(parent_entry.inode, dir.object_id());
        assert_eq!(dir_entry.inode, mkdir_reply.attr.ino);
        assert_eq!(file_entry.inode, create_reply.attr.ino);
        assert_eq!(symlink_entry.inode, symlink_reply.attr.ino);

        assert_eq!(root_entry.kind, FileType::Directory);
        assert_eq!(parent_entry.kind, FileType::Directory);
        assert_eq!(dir_entry.kind, FileType::Directory);
        assert_eq!(file_entry.kind, FileType::RegularFile);
        assert_eq!(symlink_entry.kind, FileType::Symlink);

        assert_eq!(root_entry.name, OsStr::new("."));
        assert_eq!(parent_entry.name, OsStr::new(".."));
        assert_eq!(dir_entry.name, OsStr::new("foo"));
        assert_eq!(file_entry.name, OsStr::new("bar"));
        assert_eq!(symlink_entry.name, OsStr::new("link"));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdir_read_empty_directory_entries() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let readdir_reply =
            fs.readdir(new_fake_request(), dir.object_id(), 0, 0).await.expect("readdir failed");
        let entries = readdir_reply.entries.collect::<Vec<Result<DirectoryEntry>>>().await;
        assert_eq!(entries.len(), 2);

        let root_entry = entries[0].clone().unwrap();
        let parent_entry = entries[1].clone().unwrap();

        assert_eq!(root_entry.inode, dir.object_id());
        assert_eq!(parent_entry.inode, dir.object_id());

        assert_eq!(root_entry.kind, FileType::Directory);
        assert_eq!(parent_entry.kind, FileType::Directory);

        assert_eq!(root_entry.name, OsStr::new("."));
        assert_eq!(parent_entry.name, OsStr::new(".."));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdir_fails_when_directory_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let readdir_res = fs.readdir(new_fake_request(), INVALID_INODE, 0, 0).await;
        assert_eq!(readdir_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(readdir_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdir_fails_when_object_is_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let readdir_res = fs.readdir(new_fake_request(), create_reply.attr.ino, 0, 0).await;
        assert_eq!(readdir_res.is_err(), true);
        let err: Errno = libc::ENOTDIR.into();
        assert_eq!(readdir_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdirplus_read_directory_entries() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let symlink_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("symlink failed");
        let readdir_reply = fs
            .readdirplus(new_fake_request(), dir.object_id(), 0, 0, 0)
            .await
            .expect("readdirplus failed");
        let entries = readdir_reply.entries.collect::<Vec<Result<DirectoryEntryPlus>>>().await;
        assert_eq!(entries.len(), 5);

        let root_entry = entries[0].clone().unwrap();
        let parent_entry = entries[1].clone().unwrap();
        let file_entry = entries[2].clone().unwrap();
        let dir_entry = entries[3].clone().unwrap();
        let symlink_entry = entries[4].clone().unwrap();

        assert_eq!(root_entry.inode, dir.object_id());
        assert_eq!(parent_entry.inode, dir.object_id());
        assert_eq!(dir_entry.inode, mkdir_reply.attr.ino);
        assert_eq!(file_entry.inode, create_reply.attr.ino);
        assert_eq!(symlink_entry.inode, symlink_reply.attr.ino);

        assert_eq!(root_entry.kind, FileType::Directory);
        assert_eq!(parent_entry.kind, FileType::Directory);
        assert_eq!(dir_entry.kind, FileType::Directory);
        assert_eq!(file_entry.kind, FileType::RegularFile);
        assert_eq!(symlink_entry.kind, FileType::Symlink);

        assert_eq!(root_entry.name, OsStr::new("."));
        assert_eq!(parent_entry.name, OsStr::new(".."));
        assert_eq!(dir_entry.name, OsStr::new("foo"));
        assert_eq!(file_entry.name, OsStr::new("bar"));
        assert_eq!(symlink_entry.name, OsStr::new("link"));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdirplus_read_empty_directory_entries() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let readdir_reply = fs
            .readdirplus(new_fake_request(), dir.object_id(), 0, 0, 0)
            .await
            .expect("readdirplus failed");
        let entries = readdir_reply.entries.collect::<Vec<Result<DirectoryEntryPlus>>>().await;
        assert_eq!(entries.len(), 2);

        let root_entry = entries[0].clone().unwrap();
        let parent_entry = entries[1].clone().unwrap();

        assert_eq!(root_entry.inode, dir.object_id());
        assert_eq!(parent_entry.inode, dir.object_id());

        assert_eq!(root_entry.kind, FileType::Directory);
        assert_eq!(parent_entry.kind, FileType::Directory);

        assert_eq!(root_entry.name, OsStr::new("."));
        assert_eq!(parent_entry.name, OsStr::new(".."));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdirplus_fails_when_directory_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let readdir_res = fs.readdirplus(new_fake_request(), INVALID_INODE, 0, 0, 0).await;
        assert_eq!(readdir_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(readdir_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_readdirplus_fails_when_object_is_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let readdir_res = fs.readdirplus(new_fake_request(), create_reply.attr.ino, 0, 0, 0).await;
        assert_eq!(readdir_res.is_err(), true);
        let err: Errno = libc::ENOTDIR.into();
        assert_eq!(readdir_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lseek_seek_for_cur_in_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let lseek_reply = fs
            .lseek(new_fake_request(), create_reply.attr.ino, 0, 0, libc::SEEK_CUR as u32)
            .await
            .expect("lseek failed");
        assert_eq!(lseek_reply.offset, 0);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lseek_seek_for_cur_in_file_with_offset() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let lseek_reply = fs
            .lseek(new_fake_request(), create_reply.attr.ino, 0, 1, libc::SEEK_CUR as u32)
            .await
            .expect("lseek failed");
        assert_eq!(lseek_reply.offset, 1);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lseek_seek_for_end_with_offset_smaller_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let lseek_reply = fs
            .lseek(new_fake_request(), create_reply.attr.ino, 0, 1, libc::SEEK_END as u32)
            .await
            .expect("lseek failed");
        assert_eq!(lseek_reply.offset, 4);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_lseek_seek_for_end_with_offset_larger_than_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        let lseek_reply = fs
            .lseek(new_fake_request(), create_reply.attr.ino, 0, 8, libc::SEEK_END as u32)
            .await
            .expect("lseek failed");
        assert_eq!(lseek_reply.offset, 0);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_from_one_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the start of a file A to the start of another file B,
        // with the exact length of the data.
        // The data of A should be completely copied to B.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                0,
                create_reply_new.attr.ino,
                0,
                0,
                TEST_DATA.len() as _,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, TEST_DATA.len() as u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, (TEST_DATA.len() + 1) as _)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"helloa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_with_input_offset_smaller_than_input_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from offset 1 of a file A to the start of another file B,
        // with the exact length of the data.
        // The data starting from offset 1 of A should be copied to B.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                1,
                create_reply_new.attr.ino,
                0,
                0,
                (TEST_DATA.len() - 1) as _,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, (TEST_DATA.len() - 1) as u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, (TEST_DATA.len() + 1) as _)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"elloaa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_with_input_offset_larger_than_input_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the end of a file A to the start of another file B,
        // with the exact length of the data.
        // No data should be copied to B.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                TEST_DATA.len() as _,
                create_reply_new.attr.ino,
                0,
                0,
                TEST_DATA.len() as _,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, 0u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, (TEST_DATA.len() + 1) as _)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"aaaaaa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_with_output_offset_smaller_than_output_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the start of a file A to offset 1 of another file B,
        // with the exact length of the data.
        // The data of A should be copied to B starting from B's offset 1.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                0,
                create_reply_new.attr.ino,
                0,
                1,
                TEST_DATA.len() as _,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, TEST_DATA.len() as u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, (TEST_DATA.len() + 1) as _)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"ahello";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_with_output_offset_larger_than_output_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the start of a file A to the end of another file B,
        // with the exact length of the data.
        // The data of A should be copied and appended to the end of B.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                0,
                create_reply_new.attr.ino,
                0,
                7,
                TEST_DATA.len() as _,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, 5u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, 12)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"aaaaaa\0hello";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_with_copied_length_smaller_than_input_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the start of a file A to the start of another file B,
        // with a length of one byte.
        // The first byte of data in A should be copied to B.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                0,
                create_reply_new.attr.ino,
                0,
                0,
                1,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, 1u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, (TEST_DATA.len() + 1) as _)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"haaaaa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_with_copied_length_larger_than_input_file_size() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the start of a file A to the start of another file B,
        // with a length larger than the size of A.
        // The data of A should be copied to B.
        let copy_reply = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                0,
                create_reply_new.attr.ino,
                0,
                0,
                20,
                0,
            )
            .await
            .expect("copy_file_range failed");
        assert_eq!(copy_reply.copied, 5u64);

        let read_reply = fs
            .read(new_fake_request(), create_reply_new.attr.ino, 0, 0, (TEST_DATA.len() + 1) as _)
            .await
            .expect("read failed");

        let result_data: &[u8] = b"helloa";
        assert_eq!(read_reply.data, result_data);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_fails_when_input_file_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply_new = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply_new.attr.ino, 0, 0, b"aaaaaa", 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from a file that does not exist.
        let copy_res = fs
            .copy_file_range(
                new_fake_request(),
                INVALID_INODE,
                0,
                0,
                create_reply_new.attr.ino,
                0,
                0,
                TEST_DATA.len() as _,
                0,
            )
            .await;
        assert_eq!(copy_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_copy_file_range_when_output_file_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        fs.write(new_fake_request(), create_reply.attr.ino, 0, 0, TEST_DATA, 0, DEFAULT_FLAG)
            .await
            .expect("write failed");

        // Copy the data from the start of a file to another file which does not exist.
        let copy_res = fs
            .copy_file_range(
                new_fake_request(),
                create_reply.attr.ino,
                0,
                0,
                INVALID_INODE,
                0,
                0,
                TEST_DATA.len() as _,
                0,
            )
            .await;
        assert_eq!(copy_res, Err(libc::ENOENT.into()));

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_symlink_create_new_symlink() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let symlink_reply = fs
            .symlink(new_fake_request(), dir.object_id(), OsStr::new("link"), OsStr::new("foo"))
            .await
            .expect("symlink failed");
        let readlink_reply =
            fs.readlink(new_fake_request(), symlink_reply.attr.ino).await.expect("readlink failed");
        let link: Vec<u8> = "foo".to_owned().as_bytes().to_vec();
        assert_eq!(readlink_reply.data, link);

        let lookup_reply = fs
            .lookup(new_fake_request(), dir.object_id(), OsStr::new("link"))
            .await
            .expect("lookup failed");
        assert_eq!(lookup_reply.attr.ino, symlink_reply.attr.ino);

        assert_eq!(
            fs.get_object_type(symlink_reply.attr.ino).await.expect("get_object_type failed"),
            ObjectDescriptor::Symlink
        );

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_symlink_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;

        let symlink_res = fs
            .symlink(new_fake_request(), INVALID_INODE, OsStr::new("link"), OsStr::new("foo"))
            .await;

        assert_eq!(symlink_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(symlink_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_hardlink_create_hardlink_on_file() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");
        let hardlink_reply = fs
            .link(new_fake_request(), create_reply.attr.ino, dir.object_id(), OsStr::new("bar"))
            .await
            .expect("link failed");
        assert_eq!(hardlink_reply.attr.ino, create_reply.attr.ino);

        let lookup_reply = fs
            .lookup(new_fake_request(), dir.object_id(), OsStr::new("bar"))
            .await
            .expect("lookup failed");
        assert_eq!(lookup_reply.attr.ino, create_reply.attr.ino);

        assert_eq!(
            fs.get_object_type(hardlink_reply.attr.ino).await.expect("get_object_type failed"),
            ObjectDescriptor::File
        );

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_hardlink_fails_when_parent_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        let hardlink_res = fs
            .link(new_fake_request(), create_reply.attr.ino, INVALID_INODE, OsStr::new("bar"))
            .await;

        assert_eq!(hardlink_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(hardlink_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_hardlink_fails_when_file_to_link_does_not_exist() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let hardlink_res =
            fs.link(new_fake_request(), INVALID_INODE, dir.object_id(), OsStr::new("bar")).await;

        assert_eq!(hardlink_res.is_err(), true);
        let err: Errno = libc::ENOENT.into();
        assert_eq!(hardlink_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_hardlink_fails_when_name_already_exists() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create failed");

        let hardlink_res = fs
            .link(new_fake_request(), create_reply.attr.ino, dir.object_id(), OsStr::new("foo"))
            .await;

        assert_eq!(hardlink_res.is_err(), true);
        let err: Errno = libc::EEXIST.into();
        assert_eq!(hardlink_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_hardlink_fails_when_object_is_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), 0, DEFAULT_FLAG)
            .await
            .expect("mkdir failed");

        let hardlink_res = fs
            .link(new_fake_request(), mkdir_reply.attr.ino, dir.object_id(), OsStr::new("bar"))
            .await;

        assert_eq!(hardlink_res.is_err(), true);
        let err: Errno = libc::EISDIR.into();
        assert_eq!(hardlink_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_hardlink_fails_when_parent_is_not_directory() {
        let fs = FuseFs::new_in_memory(String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("foo"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create_reply failed");

        let _create_reply = fs
            .create(
                new_fake_request(),
                dir.object_id(),
                OsStr::new("bar"),
                DEFAULT_FILE_MODE,
                DEFAULT_FLAG,
            )
            .await
            .expect("create_reply failed");

        let hardlink_res = fs
            .link(
                new_fake_request(),
                create_reply.attr.ino,
                _create_reply.attr.ino,
                OsStr::new("link"),
            )
            .await;

        assert_eq!(hardlink_res.is_err(), true);
        let err: Errno = libc::ENOTDIR.into();
        assert_eq!(hardlink_res.err().unwrap(), err);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_mkdir_create_directory_tree_and_reopen() {
        let fs = FuseFs::new_file_backed("/tmp/fuse_dir_test", String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let mkdir_reply = fs
            .mkdir(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("mkdir failed");
        let (child_id, child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(child_descriptor, ObjectDescriptor::Directory);
        assert_eq!(mkdir_reply.attr.ino, child_id);
        assert_eq!(mkdir_reply.attr.kind, FileType::Directory);

        fs.destroy(new_fake_request()).await;

        let fs = FuseFs::open_file_backed("/tmp/fuse_dir_test", String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let (_child_id, _child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(_child_descriptor, ObjectDescriptor::Directory);

        fs.fs.close().await.expect("failed to close filesystem");
    }

    #[fuchsia::test]
    async fn test_mkdir_create_file_and_reopen() {
        let fs = FuseFs::new_file_backed("/tmp/fuse_file_test", String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let create_reply = fs
            .create(new_fake_request(), dir.object_id(), OsStr::new("foo"), DEFAULT_FILE_MODE, 0)
            .await
            .expect("open_file failed");
        let (child_id, child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(child_descriptor, ObjectDescriptor::File);
        assert_eq!(create_reply.attr.ino, child_id);
        assert_eq!(create_reply.attr.kind, FileType::RegularFile);

        fs.destroy(new_fake_request()).await;

        let fs = FuseFs::open_file_backed("/tmp/fuse_file_test", String::new()).await;
        let dir = fs.root_dir().await.expect("root_dir failed");

        let (_child_id, child_descriptor, _) =
            dir.lookup(OsStr::new("foo").osstr_to_str().unwrap()).await.unwrap().unwrap();
        assert_eq!(child_descriptor, ObjectDescriptor::File);

        fs.fs.close().await.expect("failed to close filesystem");
    }
}
