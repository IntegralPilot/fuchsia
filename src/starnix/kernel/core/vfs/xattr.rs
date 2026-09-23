// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::vfs::{FsStr, FsString, XattrOp, XattrStorage};
use starnix_rcu::rcu_hash_map::Entry;
use starnix_rcu::{RcuHashMap, RcuReadScope};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};

pub struct MemoryXattrStorage {
    // Arbitrary userspace programs can define xattr keys so we use a collision-resistant hasher.
    xattrs: RcuHashMap<FsString, FsString, std::collections::hash_map::RandomState>,
}

impl Default for MemoryXattrStorage {
    fn default() -> Self {
        Self { xattrs: RcuHashMap::with_hasher(std::collections::hash_map::RandomState::new()) }
    }
}

impl XattrStorage for MemoryXattrStorage {
    fn get_xattr(&self, name: &FsStr) -> Result<FsString, Errno> {
        self.xattrs.get(&RcuReadScope::new(), name).cloned().ok_or_else(|| errno!(ENODATA))
    }

    fn set_xattr(&self, name: &FsStr, value: &FsStr, op: XattrOp) -> Result<(), Errno> {
        let mut xattrs = self.xattrs.lock();
        match xattrs.entry(name.to_owned()) {
            Entry::Vacant(_) if op == XattrOp::Replace => return error!(ENODATA),
            Entry::Occupied(_) if op == XattrOp::Create => return error!(EEXIST),
            Entry::Vacant(v) => {
                v.insert(value.to_owned());
            }
            Entry::Occupied(mut o) => {
                o.insert(value.to_owned());
            }
        };
        Ok(())
    }

    fn remove_xattr(&self, name: &FsStr) -> Result<(), Errno> {
        let mut xattrs = self.xattrs.lock();
        if xattrs.remove(name).is_none() {
            return error!(ENODATA);
        }
        Ok(())
    }

    fn list_xattrs(&self) -> Result<Vec<FsString>, Errno> {
        Ok(self.xattrs.keys(&RcuReadScope::new()).cloned().collect())
    }
}
