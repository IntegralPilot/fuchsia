// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::directory::FatDirectory;
use crate::refs::FatfsDirRef;
use crate::types::{Dir, Disk, FileSystem};
use crate::util::fatfs_error_to_status;
use crate::{FATFS_INFO_NAME, MAX_FILENAME_LEN};
use anyhow::Error;
use fatfs::{DefaultTimeProvider, FsOptions, LossyOemCpConverter};
use fidl_fuchsia_io as fio;
use fuchsia_async::{MonotonicInstant, Timer};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;
use zx::{Event, MonotonicDuration, Status};

pub struct FatFilesystem {
    filesystem: FileSystem,
    dirty_task: RefCell<Option<MonotonicInstant>>,
    fs_id: Event,
    scope: ExecutionScope,
}

impl FatFilesystem {
    /// Get the root fatfs Dir.
    pub fn fatfs_root_dir(&self) -> Dir<'_> {
        self.filesystem.root_dir()
    }

    pub fn with_disk<F, T>(&self, func: F) -> T
    where
        F: FnOnce(&Box<dyn Disk>) -> T,
    {
        self.filesystem.with_disk(func)
    }

    pub fn cluster_size(&self) -> u32 {
        self.filesystem.cluster_size()
    }

    pub fn total_clusters(&self) -> Result<u32, Status> {
        Ok(self.filesystem.stats().map_err(fatfs_error_to_status)?.total_clusters())
    }

    pub fn free_clusters(&self) -> Result<u32, Status> {
        Ok(self.filesystem.stats().map_err(fatfs_error_to_status)?.free_clusters())
    }

    /// Create a new FatFilesystem.
    pub fn new(
        disk: Box<dyn Disk>,
        options: FsOptions<DefaultTimeProvider, LossyOemCpConverter>,
        scope: ExecutionScope,
    ) -> Result<(Rc<Self>, Arc<FatDirectory>), Error> {
        let filesystem = fatfs::FileSystem::new(disk, options)?;
        let result = Rc::new(FatFilesystem {
            filesystem,
            dirty_task: RefCell::new(None),
            fs_id: Event::create(),
            scope,
        });
        Ok((result.clone(), result.root_dir()))
    }

    #[cfg(test)]
    pub fn from_filesystem(filesystem: FileSystem) -> (Rc<Self>, Arc<FatDirectory>) {
        let result = Rc::new(FatFilesystem {
            filesystem,
            dirty_task: RefCell::new(None),
            fs_id: Event::create(),
            scope: ExecutionScope::new(),
        });
        (result.clone(), result.root_dir())
    }

    pub fn fs_id(&self) -> &Event {
        &self.fs_id
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    /// Get the FatDirectory that represents the root directory of this filesystem.
    /// Note this should only be called once per filesystem, otherwise multiple conflicting
    /// FatDirectories will exist.
    /// We only call it from new() and from_filesystem().
    fn root_dir(self: Rc<Self>) -> Arc<FatDirectory> {
        // We start with an empty FatfsDirRef and an open_count of zero.
        let dir = FatfsDirRef::empty(self);
        FatDirectory::new(dir, None, "/".to_owned())
    }

    pub fn shut_down(self) -> Result<(), Status> {
        self.filesystem.unmount().map_err(fatfs_error_to_status)
    }

    /// Mark the filesystem as dirty. This will cause the disk to automatically be flushed after
    /// one second, and cancel any previous pending flushes.
    pub fn mark_dirty(self: &Rc<Self>) {
        let deadline = MonotonicInstant::after(MonotonicDuration::from_seconds(1));
        match &mut *self.dirty_task.borrow_mut() {
            Some(time) => *time = deadline,
            x @ None => {
                *x = Some(deadline);
                let this = Rc::downgrade(self);
                self.scope.spawn_local(async move {
                    loop {
                        let deadline;
                        {
                            let this_rc = match this.upgrade() {
                                Some(a) => a,
                                None => return,
                            };
                            let mut task = this_rc.dirty_task.borrow_mut();
                            if let Some(t) = task.as_ref() {
                                deadline = *t;
                            } else {
                                break;
                            }
                            if MonotonicInstant::now() >= deadline {
                                *task = None;
                                break;
                            }
                        }
                        Timer::new(deadline).await;
                    }
                    if let Some(this_rc) = this.upgrade() {
                        let _ = this_rc.filesystem.flush();
                    }
                });
            }
        }
    }

    pub fn query_filesystem(&self) -> Result<fio::FilesystemInfo, Status> {
        let cluster_size = self.cluster_size() as u64;
        let total_clusters = self.total_clusters()? as u64;
        let free_clusters = self.free_clusters()? as u64;
        let total_bytes = cluster_size * total_clusters;
        let used_bytes = cluster_size * (total_clusters - free_clusters);

        Ok(fio::FilesystemInfo {
            total_bytes,
            used_bytes,
            total_nodes: 0,
            used_nodes: 0,
            free_shared_pool_bytes: 0,
            fs_id: self.fs_id().koid()?.raw_koid(),
            block_size: cluster_size as u32,
            max_filename_size: MAX_FILENAME_LEN,
            fs_type: fidl_fuchsia_fs::VfsType::Fatfs.into_primitive(),
            padding: 0,
            name: FATFS_INFO_NAME,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::Node;
    use crate::tests::{TestDiskContents, TestFatDisk};
    use fidl::endpoints::Proxy;
    use scopeguard::defer;

    const TEST_DISK_SIZE: u64 = 2048 << 10; // 2048K

    #[fuchsia::test]
    #[ignore] // TODO(https://fxbug.dev/42133844): Clean up tasks to prevent panic on drop in FatfsFileRef
    async fn test_automatic_flush() {
        let disk = TestFatDisk::empty_disk(TEST_DISK_SIZE);
        let structure = TestDiskContents::dir().add_child("test", "Hello".into());
        structure.create(&disk.root_dir());

        let fs = disk.into_fatfs();
        let dir = fs.get_fatfs_root();
        dir.open_ref().unwrap();
        defer! { dir.close_ref() };

        let proxy = vfs::serve_file(
            dir.clone(),
            vfs::Path::validate_and_split("test").unwrap(),
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );
        assert!(fs.filesystem().dirty_task.borrow().is_none());
        let file = fio::FileProxy::new(proxy.into_channel().unwrap());
        file.write("hello there".as_bytes()).await.unwrap().map_err(Status::err_from_raw).unwrap();
        {
            let fs_inner = fs.filesystem();
            // fs should be dirty until the timer expires.
            assert!(fs_inner.filesystem.is_dirty());
        }
        // Wait some time for the flush to happen.
        Timer::new(MonotonicInstant::after(MonotonicDuration::from_millis(1500))).await;
        {
            let fs_inner = fs.filesystem();
            assert_eq!(fs_inner.filesystem.is_dirty(), false);
        }
    }
}
