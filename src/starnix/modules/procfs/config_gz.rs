// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_logging::log_error;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;

#[derive(Clone, Debug)]
pub struct ConfigFile;
impl ConfigFile {
    pub fn new_node() -> impl FsNodeOps {
        DynamicFile::new_node(Self)
    }
}

impl DynamicFileSource for ConfigFile {
    fn generate(
        &self,
        _current_task: &CurrentTask,
        sink: &mut DynamicFileBuf,
    ) -> Result<(), Errno> {
        let contents = std::fs::read("/pkg/data/config.gz").map_err(|e| {
            log_error!("Error reading /pkg/data/config.gz: {e}");
            errno!(EIO)
        })?;
        sink.write(&contents);
        Ok(())
    }
}
