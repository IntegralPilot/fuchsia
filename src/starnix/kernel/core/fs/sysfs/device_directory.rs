// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::device::kobject::{Device, UEventFsNode};
use crate::task::CurrentTask;
use crate::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use crate::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use crate::vfs::pseudo::stub_empty_file::StubEmptyFile;
use crate::vfs::{DEFAULT_BYTES_PER_BLOCK, FsNodeOps};
use starnix_logging::{bug_ref, track_stub};
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use std::borrow::Cow;
use std::sync::Weak;

pub fn build_device_directory(device: &Device, dir: &SimpleDirectoryMutator) {
    if let Some(metadata) = &device.metadata {
        dir.entry(
            "dev",
            BytesFile::new_node(format!("{}\n", metadata.devt).into_bytes()),
            mode!(IFREG, 0o444),
        );
    }
    dir.entry("uevent", UEventFsNode::new(device.clone()), mode!(IFREG, 0o644));
}

pub fn build_block_device_directory(
    device: &Device,
    block_info: Weak<dyn BlockDeviceInfo>,
    dir: &SimpleDirectoryMutator,
) {
    build_device_directory(device, dir);
    dir.subdir("queue", 0o755, |dir| {
        dir.entry("nr_requests", BytesFile::new_node(NrRequestsFile::new()), mode!(IFREG, 0o644));
        dir.entry("read_ahead_kb", BytesFile::new_node(ReadAheadKbFile), mode!(IFREG, 0o644));
        dir.entry(
            "scheduler",
            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/322907749")),
            mode!(IFREG, 0o644),
        );
    });
    dir.subdir("holders", 0o755, |_dir| {});
    dir.entry("size", BlockDeviceSizeFile::new_node(block_info), mode!(IFREG, 0o444));
}

pub trait BlockDeviceInfo: Send + Sync {
    fn size(&self) -> Result<usize, Errno>;
}

struct BlockDeviceSizeFile {
    block_info: Weak<dyn BlockDeviceInfo>,
}

impl BlockDeviceSizeFile {
    pub fn new_node(block_info: Weak<dyn BlockDeviceInfo>) -> impl FsNodeOps {
        BytesFile::new_node(Self { block_info })
    }
}

impl BytesFileOps for BlockDeviceSizeFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let size = self.block_info.upgrade().ok_or_else(|| errno!(EINVAL))?.size()?;
        let size_blocks = size / DEFAULT_BYTES_PER_BLOCK;
        Ok(format!("{size_blocks}").into_bytes().into())
    }
}

struct NrRequestsFile;

impl NrRequestsFile {
    fn new() -> Self {
        Self
    }
}

impl BytesFileOps for NrRequestsFile {
    fn write(&self, _current_task: &CurrentTask, _data: Vec<u8>) -> Result<(), Errno> {
        // Silently ignore incoming writes for now. We don't currently support the concept of
        // controlling the I/O queue depth of a given block device.
        track_stub!(TODO("https://fxbug.dev/322906857"), "updating nr_requests");
        Ok(())
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        // Always reply with '128', the default value on Linux. The only use of this value today is
        // to read it and write it back to the `nr_requests` of a different node, which we ignore
        // using the logic in the `write` function.
        track_stub!(TODO("https://fxbug.dev/322906857"), "reading nr_requests");
        Ok(b"128\n".into())
    }
}

struct ReadAheadKbFile;

impl BytesFileOps for ReadAheadKbFile {
    fn write(&self, _current_task: &CurrentTask, _data: Vec<u8>) -> Result<(), Errno> {
        track_stub!(TODO("https://fxbug.dev/297295673"), "updating read_ahead_kb");
        Ok(())
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        Ok(b"0".into())
    }
}
