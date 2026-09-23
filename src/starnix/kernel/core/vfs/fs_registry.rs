// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::security;
use crate::task::CurrentTask;
use crate::vfs::{FileSystemHandle, FileSystemOptions, FsStr, FsString};
use starnix_sync::{FsRegistryLock, LockDepMutex};
use starnix_uapi::errors::Errno;
use starnix_uapi::fs_type::FileSystemTypeFlags;
use std::collections::BTreeMap;
use std::sync::Arc;

type CreateFs = Arc<
    dyn Fn(&CurrentTask, FileSystemOptions) -> Result<FileSystemHandle, Errno>
        + Send
        + Sync
        + 'static,
>;

#[derive(Clone)]
struct FsRegistryEntry {
    flags: FileSystemTypeFlags,
    create_fs: CreateFs,
}

#[derive(Default)]
pub struct FsRegistry {
    registry: LockDepMutex<BTreeMap<FsString, FsRegistryEntry>, FsRegistryLock>,
}

impl FsRegistry {
    pub fn register<F>(&self, fs_type: &FsStr, flags: FileSystemTypeFlags, create_fs: F)
    where
        F: Fn(&CurrentTask, FileSystemOptions) -> Result<FileSystemHandle, Errno>
            + Send
            + Sync
            + 'static,
    {
        let entry = FsRegistryEntry { flags, create_fs: Arc::new(create_fs) };
        let existing = self.registry.lock().insert(fs_type.into(), entry);
        assert!(existing.is_none());
    }

    pub fn create(
        &self,
        current_task: &CurrentTask,
        fs_type: &FsStr,
        options: FileSystemOptions,
    ) -> Option<Result<FileSystemHandle, Errno>> {
        let create_fs = self.registry.lock().get(fs_type).map(|e| Arc::clone(&e.create_fs))?;
        Some(create_fs(current_task, options).and_then(|fs| {
            assert_eq!(fs_type, fs.name(), "FileSystem::name() must match the registered name.");
            security::file_system_resolve_security(&current_task, &fs)?;
            Ok(fs)
        }))
    }

    pub fn list_all(&self) -> Vec<(FsString, FileSystemTypeFlags)> {
        self.registry.lock().iter().map(|(name, entry)| (name.clone(), entry.flags)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fs_registry_list_all_flags() {
        let registry = FsRegistry::default();
        registry.register(
            b"test_nodev".into(),
            FileSystemTypeFlags::empty(),
            |_task, _options| unreachable!(),
        );
        registry.register(
            b"test_dev".into(),
            FileSystemTypeFlags::REQUIRES_DEV,
            |_task, _options| unreachable!(),
        );

        let list = registry.list_all();
        assert_eq!(
            list,
            vec![
                (b"test_dev".into(), FileSystemTypeFlags::REQUIRES_DEV),
                (b"test_nodev".into(), FileSystemTypeFlags::empty()),
            ]
        );
    }
}
