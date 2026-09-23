// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::device::DeviceMode;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};

use starnix_uapi::device_id::{DeviceId, MISC_MAJOR};
use starnix_uapi::errors::Errno;
use std::borrow::Cow;

#[derive(Clone, Debug)]
pub struct MiscFile;
impl MiscFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self)
    }
}

impl BytesFileOps for MiscFile {
    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let registery = &current_task.kernel().device_registry;
        let devices = registery.list_minor_devices(
            DeviceMode::Char,
            DeviceId::new_range(MISC_MAJOR, DeviceMode::Char.minor_range()),
        );
        let mut contents = String::new();
        for (device_type, name) in devices {
            contents.push_str(&format!("{:3} {}\n", device_type.minor(), name));
        }
        Ok(contents.into_bytes().into())
    }
}
