// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use binder_proxy_config::Config;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use vsock_sys::{create_virtio_stream_socket, sockaddr_vm};

pub fn create_bound_virtio_socket(config: &Config, port: u32) -> Result<OwnedFd> {
    // Make a virtio stream socket.
    let mut socket_fd = 0;
    if let Err(status) = zx::Status::ok(unsafe { create_virtio_stream_socket(&mut socket_fd) }) {
        anyhow::bail!("Could not create virtio stream socket: {status:?}");
    }
    let socket_fd = unsafe { OwnedFd::from_raw_fd(socket_fd) };
    let svm_cid = if config.bind_to_loopback {
        1 // VMADDR_CID_LOCAL
    } else {
        2 // VMADDR_CID_HOST
    };
    // Bind the socket to listen for connections from the host on the specified port.
    let addr = sockaddr_vm {
        svm_family: libc::AF_VSOCK as u16,
        svm_port: port as u32,
        svm_cid,
        ..Default::default()
    };
    let r = unsafe {
        let addr_ptr = &addr as *const sockaddr_vm as *const libc::sockaddr;
        libc::bind(
            socket_fd.as_raw_fd(),
            addr_ptr,
            std::mem::size_of::<sockaddr_vm>() as libc::socklen_t,
        )
    };
    if r == -1 {
        anyhow::bail!("Bind failed: {:?}", std::io::Error::last_os_error())
    }
    Ok(socket_fd)
}
