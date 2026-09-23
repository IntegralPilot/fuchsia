// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of `cgroup.procs` file.
//!
//! Reading cgroup.procs produces all processes IDs that currently belong to the cgroup.
//! Writing a process ID to this file will move the process into this cgroup.
//!
//! Full details at https://docs.kernel.org/admin-guide/cgroup-v2.html#core-interface-files

use starnix_core::fs_node_impl_not_dir;
use starnix_core::task::{CgroupOps, CurrentTask, Kernel};
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_core::vfs::{AppendLockWriteGuard, FileOps, FsNode, FsNodeOps, InputBuffer};

use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{errno, pid_t};
use std::sync::{Arc, Weak};

pub struct ControlGroupNode {
    cgroup: Weak<dyn CgroupOps>,
}

impl ControlGroupNode {
    pub fn new(cgroup: Weak<dyn CgroupOps>) -> Self {
        ControlGroupNode { cgroup }
    }
}

impl FsNodeOps for ControlGroupNode {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(ControlGroupFile::new(current_task.kernel(), self.cgroup.clone())))
    }

    fn truncate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _length: u64,
    ) -> Result<(), Errno> {
        Ok(())
    }
}

impl DynamicFileSource for ControlGroupFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let cgroup = self.cgroup()?;
        for pid in cgroup.get_pids(self.kernel()?.as_ref()) {
            write!(sink, "{pid}\n")?;
        }
        Ok(())
    }

    fn write(
        &self,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let bytes = data.read_all()?;
        let pid_string = std::str::from_utf8(&bytes).map_err(|_| errno!(EINVAL))?;
        let pid = pid_string.trim().parse::<pid_t>().map_err(|_| errno!(EINVAL))?;

        // Check if the pid is a valid process.
        let thread_group = current_task.kernel().pids.get(pid)?.get_thread_group()?;
        self.cgroup()?.add_process(&thread_group)?;

        Ok(bytes.len())
    }
}

/// A `ControlGroupFile` currently represents the `cgroup.procs` file for the control group. Writing
/// to this file will add tasks to the control group.
pub struct ControlGroupFile {
    kernel: Weak<Kernel>,
    cgroup: Weak<dyn CgroupOps>,
}

impl ControlGroupFile {
    fn new(kernel: &Kernel, cgroup: Weak<dyn CgroupOps>) -> impl FileOps {
        DynamicFile::new(Self { kernel: kernel.weak_self.clone(), cgroup: cgroup.clone() })
    }

    fn kernel(&self) -> Result<Arc<Kernel>, Errno> {
        self.kernel.upgrade().ok_or_else(|| errno!(ENODEV))
    }

    fn cgroup(&self) -> Result<Arc<dyn CgroupOps>, Errno> {
        self.cgroup.upgrade().ok_or_else(|| errno!(ENODEV))
    }
}
