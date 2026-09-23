// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::device::DeviceMode;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};

use starnix_uapi::errors::Errno;
use std::borrow::Cow;

pub struct DevicesFile;
impl DevicesFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self)
    }
}

impl BytesFileOps for DevicesFile {
    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let registery = &current_task.kernel().device_registry;
        let char_devices = registery.list_major_devices(DeviceMode::Char);
        let block_devices = registery.list_major_devices(DeviceMode::Block);
        let mut contents = String::new();
        contents.push_str("Character devices:\n");
        for (major, name) in char_devices {
            contents.push_str(&format!("{:3} {}\n", major, name));
        }
        contents.push_str("\n");
        contents.push_str("Block devices:\n");
        for (major, name) in block_devices {
            contents.push_str(&format!("{:3} {}\n", major, name));
        }
        Ok(contents.into_bytes().into())
    }
}
