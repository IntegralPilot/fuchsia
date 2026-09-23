// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of `cgroup.freeze` file.
//!
//! This file only exists on non-root cgroups. Writing "1" freezes this cgroup all of its
//! descendants, writing "0" unfreezes the cgroup, but it can still be in a frozen state if any of
//! its ancestors are frozen. Reading cgroup.freeze produces the previously written value, or 0 by
//! default. cgroup.events should be used to check the effective freezer state of the cgroup.
//!
//! Full details at https://docs.kernel.org/admin-guide/cgroup-v2.html#core-interface-files

use starnix_core::task::{CgroupOps, CurrentTask};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};

use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::borrow::Cow;
use std::sync::{Arc, Weak};

pub struct FreezeFile {
    cgroup: Weak<dyn CgroupOps>,
}

impl FreezeFile {
    pub fn new_node(cgroup: Weak<dyn CgroupOps>) -> impl FsNodeOps {
        BytesFile::new_node(Self { cgroup })
    }

    fn cgroup(&self) -> Result<Arc<dyn CgroupOps>, Errno> {
        self.cgroup.upgrade().ok_or_else(|| errno!(ENODEV))
    }
}

impl BytesFileOps for FreezeFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let state_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let cgroup = self.cgroup()?;
        let freezer = cgroup.freezer().ok_or_else(|| errno!(ENODEV))?;
        match state_str.trim() {
            "1" => Ok(freezer.freeze()),
            "0" => Ok(freezer.thaw()),
            _ => error!(EINVAL),
        }
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let cgroup = self.cgroup()?;
        let freezer = cgroup.freezer().ok_or_else(|| errno!(ENODEV))?;
        let state_str = format!("{}\n", freezer.get_freezer_state().self_freezer_state);
        Ok(state_str.as_bytes().to_owned().into())
    }
}
