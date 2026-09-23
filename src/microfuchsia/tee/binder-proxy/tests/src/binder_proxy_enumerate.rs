// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use android_system_microfuchsia_vm_service::aidl::android::system::microfuchsia::vm_service::IMicrofuchsia::{
    IMicrofuchsia, GUEST_PORT,
};
use anyhow::Error;
use rpcbinder::RpcSession;
use std::{os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd}, sync::Once};
use vsock_sys::{create_virtio_stream_socket, sockaddr_vm};
use binder_proxy_tests_config;

unsafe extern "C" {
    fn register_dev_urandom_compat() -> zx::sys::zx_status_t;
}

static INIT_DEV_URANDOM_COMPAT: Once = Once::new();

fn init_dev_urandom_compat_once() {
    INIT_DEV_URANDOM_COMPAT.call_once(|| {
        if let Err(status) = zx::Status::ok(unsafe { register_dev_urandom_compat() }) {
            panic!("Could not register /dev/urandom compatibility device {status}");
        }
    });
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    log::info!("binder proxy enumerate main");
    init_dev_urandom_compat_once();

    let session = RpcSession::new();
    let service: binder::Strong<dyn IMicrofuchsia> = session
        .setup_preconnected_client(|| {
            // TODO: Why doesn't the polling loop in the binder library work?
            // We are probably returning the wrong error on connection failure.
            std::thread::sleep(std::time::Duration::from_secs(2));
            // Make a virtio stream socket.
            let mut socket_fd = 0;
            if let Err(status) =
                zx::Status::ok(unsafe { create_virtio_stream_socket(&mut socket_fd) })
            {
                log::error!("Could not create virtio stream socket: {status:?}");
                return None;
            }
            let socket_fd = unsafe { OwnedFd::from_raw_fd(socket_fd) };

            // connect to loopback 5680
            let addr = sockaddr_vm {
                svm_family: libc::AF_VSOCK as u16,
                svm_port: GUEST_PORT as u32,
                svm_cid: 1, // VMADDR_CID_LOCAL
                ..Default::default()
            };

            let r = unsafe {
                let addr_ptr = &addr as *const sockaddr_vm as *const libc::sockaddr;
                libc::connect(
                    socket_fd.as_raw_fd(),
                    addr_ptr,
                    std::mem::size_of::<sockaddr_vm>() as libc::socklen_t,
                )
            };
            if r == -1 {
                log::error!("connect failed: {:?}", std::io::Error::last_os_error());
                return None;
            }

            Some(socket_fd.into_raw_fd())
        })
        .map_err(|e| anyhow::anyhow!("setup_preconnected_client error {e:?}"))?;
    let tee_uuid = service.trustedAppUuids()?;

    let config = binder_proxy_tests_config::Config::take_from_startup_handle();

    assert_eq!(tee_uuid, config.expected_uuids);

    Ok(())
}
