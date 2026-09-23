// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::{
    DirEntryChildKey, DirectoryEntryType, DirentSink, FileObject, FileOps, SeekTarget,
    default_seek, fileops_impl_directory, fileops_impl_noop_sync,
};
use fuchsia_rcu::RcuReadScope;
use starnix_sync::{LockDepMutex, MemoryDirectoryReaddirPositionLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::{error, off_t};
use std::ops::Bound;

pub struct MemoryDirectoryFile {
    /// The current position for readdir.
    ///
    /// When readdir is called multiple times, we need to return subsequent
    /// directory entries. This field records where the previous readdir
    /// stopped.
    ///
    /// The state is actually recorded twice: once in the offset for this
    /// [`FileObject`] and again here. Recovering the state from the offset is slow
    /// because we would need to iterate through the keys of the [`BTreeMap`]. Having
    /// the [`DirEntryChildKey`] cached lets us search the keys of the [`BTreeMap`] faster.
    ///
    /// The initial "." and ".." entries are not recorded here. They are
    /// represented only in the offset field in the [`FileObject`].
    readdir_position: LockDepMutex<Bound<DirEntryChildKey>, MemoryDirectoryReaddirPositionLock>,
}

impl MemoryDirectoryFile {
    pub fn new() -> MemoryDirectoryFile {
        MemoryDirectoryFile { readdir_position: Bound::Unbounded.into() }
    }
}

/// If the offset is less than 2, emits . and .. entries for the specified file.
///
/// The offset will always be at least 2 after this function returns successfully. It's often
/// necessary to subtract 2 from the offset in subsequent logic.
pub fn emit_dotdot(file: &FileObject, sink: &mut dyn DirentSink) -> Result<(), Errno> {
    if sink.offset() == 0 {
        sink.add(file.node().ino, 1, DirectoryEntryType::DIR, ".".into())?;
    }
    if sink.offset() == 1 {
        sink.add(
            file.name.entry.parent_or_self().node.ino,
            2,
            DirectoryEntryType::DIR,
            "..".into(),
        )?;
    }
    Ok(())
}

impl FileOps for MemoryDirectoryFile {
    fileops_impl_directory!();
    fileops_impl_noop_sync!();

    fn seek(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        current_offset: off_t,
        target: SeekTarget,
    ) -> Result<off_t, Errno> {
        let new_offset = default_seek(current_offset, target, || error!(EINVAL))?;
        // Nothing to do.
        if current_offset == new_offset {
            return Ok(new_offset);
        }

        let mut readdir_position = self.readdir_position.lock();

        // We use 0 and 1 for "." and ".."
        if new_offset <= 2 {
            *readdir_position = Bound::Unbounded;
        } else {
            file.name.entry.get_children(|children| {
                let count = (new_offset - 2) as usize;
                *readdir_position = children
                    .iter()
                    .take(count)
                    .next_back()
                    .map_or(Bound::Unbounded, |(name, _)| Bound::Excluded(name.clone()));
            });
        }

        Ok(new_offset)
    }

    fn readdir(
        &self,
        file: &FileObject,
        _current_task: &CurrentTask,
        sink: &mut dyn DirentSink,
    ) -> Result<(), Errno> {
        emit_dotdot(file, sink)?;

        let mut readdir_position = self.readdir_position.lock();
        let scope = RcuReadScope::new();
        file.name.entry.get_children(|children| {
            for (name, maybe_entry) in children.range((readdir_position.clone(), Bound::Unbounded))
            {
                if let Some(entry) = maybe_entry.upgrade() {
                    sink.add(
                        entry.node.ino,
                        sink.offset() + 1,
                        DirectoryEntryType::from_mode(entry.node.info().mode),
                        entry.local_name(&scope),
                    )?;
                    *readdir_position = Bound::Excluded(name.clone());
                }
            }
            Ok(())
        })
    }
}
