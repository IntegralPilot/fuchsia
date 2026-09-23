// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::vfs::NamespaceNode;
use std::ops::Deref;
use std::sync::Arc;

use crate::nanohub_firmware_file::NanohubFirmwareFile;
use crate::nanohub_socket_file::NanohubSocketFile;
use fidl_fuchsia_hardware_sockettunnel::{self, DeviceMarker, DeviceRegisterSocketRequest};
use fuchsia_component as fcomponent;
use starnix_core::device::DeviceOps;
use starnix_core::device::kobject::Device;
use starnix_core::fs::fuchsia::new_remote_file_ops;
use starnix_core::fs_node_impl_not_dir;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use starnix_core::vfs::{FileOps, FsNode, FsNodeOps, FsStr, FsString};

use starnix_uapi::auth::Credentials;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errno;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;

#[derive(Clone)]
pub struct SocketTunnelFile {
    socket_label: Arc<FsString>,
}
#[derive(Clone)]
pub struct SocketTunnelSysfsFile {
    socket_label: Arc<FsString>,
}
#[derive(Clone)]
pub struct FirmwareFile {}

impl SocketTunnelFile {
    pub fn new(socket_label: FsString) -> SocketTunnelFile {
        SocketTunnelFile { socket_label: Arc::new(socket_label) }
    }
}
impl SocketTunnelSysfsFile {
    #[allow(dead_code)]
    // TODO(b/433761169): Remove this dead code.
    pub fn new(socket_label: FsString) -> SocketTunnelSysfsFile {
        SocketTunnelSysfsFile { socket_label: Arc::new(socket_label) }
    }
}

impl FirmwareFile {
    pub fn new() -> FirmwareFile {
        FirmwareFile {}
    }
}

impl FsNodeOps for FirmwareFile {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(NanohubFirmwareFile::new())
    }
}

fn connect(
    current_task: &CurrentTask,
    socket_label: Arc<FsString>,
) -> Result<Box<dyn FileOps>, Errno> {
    let device_proxy = fcomponent::client::connect_to_protocol_sync::<DeviceMarker>()
        .map_err(|_| errno!(ENOENT))?;

    // Create the socket pair for local/remote sides
    let (tx, rx) = zx::Socket::create_datagram();

    let register_socket_params = DeviceRegisterSocketRequest {
        server_socket: rx.into(),
        socket_label: Some(socket_label.deref().clone().to_string()),
        ..Default::default()
    };
    // Execute command, check if the FIDL connection succeeded, and extract the Status
    device_proxy
        .register_socket(register_socket_params, zx::MonotonicInstant::INFINITE)
        .map_err(|_| errno!(ENOENT))?
        .map_err(|_| errno!(ENOENT))?;

    // This will only be reached if the status was OK. Credentials are ignored for sockets.
    new_remote_file_ops(current_task, tx.into(), Credentials::root())
}

impl DeviceOps for SocketTunnelFile {
    fn open(
        &self,
        current_task: &CurrentTask,
        _id: DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        connect(current_task, self.socket_label.clone())
    }
}

impl FsNodeOps for SocketTunnelSysfsFile {
    fs_node_impl_not_dir!();

    fn create_file_ops(
        &self,
        _node: &FsNode,
        current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(NanohubSocketFile::new(connect(current_task, self.socket_label.clone())?))
    }
}

/// Create and register a device node backed by a SocketTunnelFile
pub fn register_socket_tunnel_device(
    kernel: &Kernel,
    socket_label: &FsStr,
    dev_node_name: &FsStr,
    dev_class_name: &FsStr,
    build_directory: impl FnOnce(&Device, &SimpleDirectoryMutator),
) {
    let registry = &kernel.device_registry;

    let device_class =
        registry.objects.get_or_create_class(dev_class_name, registry.objects.virtual_bus());

    registry
        .register_dyn_device_with_dir(
            kernel,
            dev_node_name.into(),
            device_class,
            build_directory,
            SocketTunnelFile::new(socket_label.to_owned()),
        )
        .expect("Can register socket tunnel file");
}
