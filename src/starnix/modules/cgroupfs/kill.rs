// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of `cgroup.kill` file.
//!
//! This file only exists on non-root cgroups. Writing "1" kills all processes in this cgroup and
//! its descendants.
//!
//! Full details at https://docs.kernel.org/admin-guide/cgroup-v2.html#core-interface-files

use std::sync::{Arc, Weak};

use starnix_core::task::{CgroupOps, CurrentTask};
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};

pub struct KillFile {
    cgroup: Weak<dyn CgroupOps>,
}

impl KillFile {
    pub fn new_node(cgroup: Weak<dyn CgroupOps>) -> impl FsNodeOps {
        BytesFile::new_node(Self { cgroup })
    }

    fn cgroup(&self) -> Result<Arc<dyn CgroupOps>, Errno> {
        self.cgroup.upgrade().ok_or_else(|| errno!(ENODEV))
    }
}

impl BytesFileOps for KillFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let cgroup = self.cgroup()?;
        let data_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?.trim();
        if data_str == "1" { Ok(cgroup.kill()) } else { error!(EINVAL) }
    }
}
