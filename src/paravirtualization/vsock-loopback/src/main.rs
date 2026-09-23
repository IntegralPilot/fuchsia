// Copyright 2024 The Fuchsia Authors. All rights reserved
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_hardware_vsock::{
    Addr, CallbacksMarker, CallbacksProxy, DeviceRequest, DeviceRequestStream, VMADDR_CID_LOCAL,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use futures::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use thiserror::Error;

#[derive(Error, Debug)]
enum Error {
    #[error("Error whilst communication with client")]
    ClientCommunication(#[source] anyhow::Error),
}

impl Error {
    pub fn is_comm_failure(&self) -> bool {
        matches!(self, Error::ClientCommunication(_))
    }
}

// We forwards connection requests to remote CID 1 and 2 as a loopback to CID 2
// Example sequence:
// * sshd sends Listen local port: 22
// * vsock-service records local port 22 as claimed.
// * test sends Connect with socket-1 to remote cid: 1 (or 2) remote port: 22
// * vsock-service sends Request with socket-1 remote cid: 1 (or 2) remote port:22 local port: 8000
// * vsock-loopback records socket-1
// * vsock-loopback sends CallbackRequest with remote cid: 2 remote port: 8000 local port: 22
// * vsock-service signals sshd about new connection
// * sshd sends Accept with socket-2
// * vsock-service sends SendResponse with socket-2 remote_cid: 2 remote port: 8000 local_port: 22
// * vsock-loopback records socket-2
// * vsock-loopback creates socket-1 <-> socket-2 bidirectional forwarding.
// * vsock-loopback sends CallbackResponse with remote_cid: 1 (or 2) remote_port: 22 local port: 8000
// * vsock-loopback completes Connect to test

type Port = u32;

// Value picked arbitrarily, not tuned.
const PROXY_BUFFER_SIZE: usize = 65536;

/// Shovels all data read from socket1 into socket2 and vice versa. If the kernel implemented
/// splice or fuse then we could avoid this!
async fn proxy_data(socket1: zx::Socket, socket2: zx::Socket) {
    // TODO(http://fxbug.dev/368618086): Propagate socket disposition changes.

    let socket1 = fasync::Socket::from_socket(socket1);
    let socket2 = fasync::Socket::from_socket(socket2);
    // Split sockets for bi-directional communication.
    let (read_half_sock1, mut write_half_sock1) = futures::AsyncReadExt::split(socket1);
    let (read_half_sock2, mut write_half_sock2) = futures::AsyncReadExt::split(socket2);

    let sock1_reader = futures::io::BufReader::with_capacity(PROXY_BUFFER_SIZE, read_half_sock1);
    let sock2_reader = futures::io::BufReader::with_capacity(PROXY_BUFFER_SIZE, read_half_sock2);

    let sock1_copier = futures::io::copy_buf(sock1_reader, &mut write_half_sock2);
    let sock2_copier = futures::io::copy_buf(sock2_reader, &mut write_half_sock1);

    // Bail when both copiers
    let (_, _) = futures::join!(sock1_copier, sock2_copier);
}

fn swap_ports(addr: &Addr) -> Addr {
    Addr { local_port: addr.remote_port, remote_cid: addr.remote_cid, remote_port: addr.local_port }
}

struct Inner {
    callbacks: Option<CallbacksProxy>,
    pending_requests: HashMap<Port, VecDeque<zx::Socket>>,
    proxy_tasks: fasync::Scope,
}

impl Inner {
    fn start(&mut self, cb: ClientEnd<CallbacksMarker>) -> Result<(), zx::Status> {
        if self.callbacks.is_some() {
            return Err(zx::Status::ALREADY_BOUND);
        }
        // TODO(surajmalhotra): Reset all connections on observing PEER_CLOSED.
        self.callbacks = Some(cb.into_proxy());
        Ok(())
    }

    fn send_request(&mut self, addr: Addr, data: zx::Socket) -> Result<(), zx::Status> {
        let Some(ref callbacks) = self.callbacks else {
            return Err(zx::Status::BAD_STATE);
        };
        if addr.remote_cid != VMADDR_CID_LOCAL {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        let addr = swap_ports(&addr);
        callbacks.request(&addr).map_err(|_| zx::Status::INTERNAL)?;
        self.pending_requests.entry(addr.local_port).or_default().push_back(data);
        Ok(())
    }

    fn send_shutdown(&mut self, addr: Addr) -> Result<(), zx::Status> {
        let Some(ref callbacks) = self.callbacks else {
            return Err(zx::Status::BAD_STATE);
        };
        if addr.remote_cid != VMADDR_CID_LOCAL {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        let addr = swap_ports(&addr);
        callbacks.shutdown(&addr).map_err(|_| zx::Status::INTERNAL)?;
        return Err(zx::Status::NOT_SUPPORTED);
    }

    fn send_reset(&mut self, addr: Addr) -> Result<(), zx::Status> {
        let Some(ref callbacks) = self.callbacks else {
            return Err(zx::Status::BAD_STATE);
        };
        if addr.remote_cid != VMADDR_CID_LOCAL {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        let addr = swap_ports(&addr);
        callbacks.rst(&addr).map_err(|_| zx::Status::INTERNAL)?;
        Ok(())
    }

    fn send_response(&mut self, addr: Addr, data: zx::Socket) -> Result<(), zx::Status> {
        let Some(ref callbacks) = self.callbacks else {
            return Err(zx::Status::BAD_STATE);
        };
        if addr.remote_cid != VMADDR_CID_LOCAL {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        let Some(entry) = self.pending_requests.get_mut(&addr.local_port) else {
            return Err(zx::Status::NO_RESOURCES);
        };
        let Some(socket) = entry.pop_front() else {
            return Err(zx::Status::NO_RESOURCES);
        };

        self.proxy_tasks.spawn_local(proxy_data(socket, data));
        let addr = swap_ports(&addr);
        callbacks.response(&addr).map_err(|_| zx::Status::INTERNAL)?;
        Ok(())
    }
}

#[derive(Clone)]
struct LoopbackDevice(Rc<RefCell<Inner>>);

impl LoopbackDevice {
    pub fn new() -> LoopbackDevice {
        LoopbackDevice(Rc::new(RefCell::new(Inner {
            callbacks: None,
            pending_requests: HashMap::new(),
            proxy_tasks: fasync::Scope::new(),
        })))
    }

    async fn handle_request(&self, request: DeviceRequest) -> Result<(), Error> {
        match request {
            DeviceRequest::Start { cb, responder } => {
                responder.send(self.0.borrow_mut().start(cb).map_err(zx::Status::into_raw))
            }
            DeviceRequest::SendRequest { addr, data, responder } => responder
                .send(self.0.borrow_mut().send_request(addr, data).map_err(zx::Status::into_raw)),
            DeviceRequest::SendShutdown { addr, responder } => responder
                .send(self.0.borrow_mut().send_shutdown(addr).map_err(zx::Status::into_raw)),
            DeviceRequest::SendRst { addr, responder } => {
                responder.send(self.0.borrow_mut().send_reset(addr).map_err(zx::Status::into_raw))
            }
            DeviceRequest::SendResponse { addr, data, responder } => responder
                .send(self.0.borrow_mut().send_response(addr, data).map_err(zx::Status::into_raw)),
            DeviceRequest::GetCid { responder } => responder.send(1),
        }
        .map_err(|e| Error::ClientCommunication(e.into()))
    }

    pub async fn run_handler(self, request: DeviceRequestStream) {
        let self_ref = &self;
        let fut = request
            .map_err(|err| Error::ClientCommunication(err.into()))
            .try_for_each_concurrent(None, |request| {
                self_ref
                    .handle_request(request)
                    .or_else(|e| future::ready(if e.is_comm_failure() { Err(e) } else { Ok(()) }))
            });
        if let Err(e) = fut.await {
            log::info!("Failed to handle request {}", e);
        }
    }
}

enum IncomingRequest {
    Device(DeviceRequestStream),
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), anyhow::Error> {
    let mut service_fs = ServiceFs::new_local();

    // Initialize inspect
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    component::health().set_starting_up();

    service_fs.dir("svc").add_fidl_service(IncomingRequest::Device);

    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    component::health().set_ok();
    log::debug!("Initialized.");

    let device = LoopbackDevice::new();
    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| {
            let device_clone = device.clone();
            async move {
                match request {
                    IncomingRequest::Device(request) => {
                        device_clone.clone().run_handler(request).await
                    }
                }
            }
        })
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    #[fuchsia::test]
    async fn smoke_test() {
        assert!(true);
    }
}
