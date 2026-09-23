// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use crate::vfs::FsNodeOps;
use crate::vfs::pseudo::simple_file::{BytesFile, BytesFileOps, parse_i32_file};
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use std::borrow::Cow;

pub struct PowerSyncOnSuspendFile;

impl PowerSyncOnSuspendFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self {})
    }
}

impl BytesFileOps for PowerSyncOnSuspendFile {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let value = parse_i32_file(&data)?;
        let enabled = match value {
            0 => false,
            1 => true,
            _ => return error!(EINVAL),
        };
        current_task.kernel().suspend_resume_manager.set_sync_on_suspend(enabled);
        Ok(())
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let bytes = if current_task.kernel().suspend_resume_manager.sync_on_suspend_enabled() {
            b"1\n"
        } else {
            b"0\n"
        };
        Ok(bytes.into())
    }
}
