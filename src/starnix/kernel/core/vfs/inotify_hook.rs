// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::vfs::{FileHandleKey, FsStr, WdNumber, WeakFileHandle};
use starnix_sync::{InotifyWatchersLock, LockDepMutex};
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::inotify_mask::InotifyMask;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct InotifyWatcher {
    pub watch_id: WdNumber,
    pub mask: InotifyMask,
}

#[derive(Default)]
pub struct InotifyWatchers {
    pub watchers: LockDepMutex<BTreeMap<FileHandleKey, InotifyWatcher>, InotifyWatchersLock>,
}

impl InotifyWatchers {
    pub fn add(&self, mask: InotifyMask, watch_id: WdNumber, inotify: FileHandleKey) {
        let mut watchers = self.watchers.lock();
        watchers.insert(inotify, InotifyWatcher { watch_id, mask });
    }

    // Checks if inotify is already part of watchers. Replaces mask if found and returns the WdNumber.
    // Combines mask if IN_MASK_ADD is specified in mask. Returns None if no present in watchers.
    //
    // Errors if:
    //  - both IN_MASK_ADD and IN_MASK_CREATE are specified in mask, or
    //  - IN_MASK_CREATE is specified and existing entry is found.
    pub fn maybe_update(
        &self,
        mask: InotifyMask,
        inotify: &FileHandleKey,
    ) -> Result<Option<WdNumber>, Errno> {
        let combine_existing = mask.contains(InotifyMask::MASK_ADD);
        let create_new = mask.contains(InotifyMask::MASK_CREATE);
        if combine_existing && create_new {
            return error!(EINVAL);
        }

        let mut watchers = self.watchers.lock();
        if let Some(watcher) = watchers.get_mut(inotify) {
            if create_new {
                return error!(EEXIST);
            }

            if combine_existing {
                watcher.mask.insert(mask);
            } else {
                watcher.mask = mask;
            }
            Ok(Some(watcher.watch_id))
        } else {
            Ok(None)
        }
    }

    pub fn get(&self, inotify: &FileHandleKey) -> Option<InotifyWatcher> {
        self.watchers.lock().get(inotify).cloned()
    }

    pub fn remove(&self, inotify: &FileHandleKey) {
        let mut watchers = self.watchers.lock();
        watchers.remove(inotify);
    }

    pub fn remove_by_ref(&self, inotify: &WeakFileHandle) {
        let mut watchers = self.watchers.lock();
        watchers.retain(|weak_key, _| weak_key.0.strong_count() > 0 && weak_key != inotify)
    }
}

pub trait NotifyHook: Send + Sync + 'static {
    fn notify(
        &self,
        watchers: &InotifyWatchers,
        event_mask: InotifyMask,
        cookie: u32,
        name: &FsStr,
        mode: FileMode,
        is_dead: bool,
    );

    fn get_next_cookie(&self) -> u32;
}
