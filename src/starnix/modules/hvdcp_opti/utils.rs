// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::task::CurrentTask;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps, SimpleFileNode};
use starnix_core::vfs::{
    FileOps, FsNodeOps, fileops_impl_dataless, fileops_impl_nonseekable, fileops_impl_noop_sync,
};
use starnix_sync::{LockDepMutex, ReadWriteBytesFileLock};
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, error};
use std::borrow::Cow;

const HVDCP_OPTI_DIRECTORY: &str = "/svc/fuchsia.hardware.qcom.hvdcpopti.Service";

// TODO(b/415333931): Change the connection logic to not eagerly connect upon module initialization
// or panic if the server is not available.

pub fn connect_to_device_channel(name: &str) -> Result<zx::Channel, Errno> {
    let mut dir = std::fs::read_dir(HVDCP_OPTI_DIRECTORY).map_err(|_| errno!(EINVAL))?;
    let Some(Ok(entry)) = dir.next() else {
        return error!(EBUSY);
    };
    let path =
        entry.path().join(name).into_os_string().into_string().map_err(|_| errno!(EINVAL))?;

    let (client_channel, server_channel) = zx::Channel::create();
    fdio::service_connect(&path, server_channel).map_err(|_| errno!(EINVAL))?;
    Ok(client_channel)
}

// Current QBG context dump size is 2448 bytes (612 u32 members).
// Use greater buffer to accommodate future additions to QBG context.
const QBG_CONTEXT_LOCAL_BUF_SIZE: usize = 3072;
#[derive(Default)]
pub struct ReadWriteBytesFile {
    data: LockDepMutex<Vec<u8>, ReadWriteBytesFileLock>,
}

impl ReadWriteBytesFile {
    pub fn new_node() -> impl FsNodeOps {
        BytesFile::new_node(Self::default())
    }
}

impl BytesFileOps for ReadWriteBytesFile {
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let data: Vec<u8> = std::mem::take(self.data.lock().as_mut());
        Ok(data.into())
    }

    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        if data.len() > QBG_CONTEXT_LOCAL_BUF_SIZE {
            return error!(EINVAL);
        }

        *self.data.lock() = data;
        Ok(())
    }
}

pub struct InvalidFile;

impl InvalidFile {
    pub fn new_node() -> impl FsNodeOps {
        SimpleFileNode::new(move |_| Ok(InvalidFile))
    }
}

impl FileOps for InvalidFile {
    fileops_impl_dataless!();
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();
}
