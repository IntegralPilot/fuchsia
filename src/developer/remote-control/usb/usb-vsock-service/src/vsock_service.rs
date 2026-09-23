// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_vsock::{self as vsock, Addr, CallbacksProxy};
use fuchsia_async::{Scope, Socket};
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt};
use log::{debug, error, info};
use std::io::Error;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{self, Arc, Weak};
use usb_vsock::{Address, Connection, PacketBuffer};
use zx::Status;

use crate::ConnectionRequest;

/// Implements the fuchsia.hardware.vsock service against a [`Connection`].
pub struct VsockService<B> {
    connection: sync::Mutex<Option<Weak<Connection<B, Socket>>>>,
    callback: CallbacksProxy,
    current_cid: AtomicU32,
    scope: Scope,
}

impl<B: PacketBuffer> VsockService<B> {
    /// Waits for the start message from the client and returns a constructed [`VsockService`]
    pub async fn wait_for_start(
        incoming_connections: mpsc::Receiver<ConnectionRequest>,
        requests: &mut vsock::DeviceRequestStream,
    ) -> Result<Self, Error> {
        use vsock::DeviceRequest::*;

        let scope = Scope::new_with_name("vsock-service");
        let Some(req) = requests.try_next().await.map_err(Error::other)? else {
            return Err(Error::other(
                "vsock client connected and disconnected without sending start message",
            ));
        };

        match req {
            Start { cb, responder } => {
                info!("Client callback set for vsock client");
                let connection = Default::default();
                let callback = cb.into_proxy();
                // since we aren't connected to a host yet, we claim to be cid 3 (the first
                // non-reserved cid). The host can override this when a new connection is
                // established.
                let current_cid = AtomicU32::new(3);
                scope.spawn(Self::run_incoming_loop(incoming_connections, callback.clone()));
                responder.send(Ok(())).map_err(Error::other)?;
                Ok(Self { connection, callback, scope, current_cid })
            }
            other => {
                Err(Error::other(format!("unexpected message before start message: {other:?}")))
            }
        }
    }

    pub fn current_cid(&self) -> u32 {
        self.current_cid.load(Ordering::Relaxed)
    }

    /// Set the current connection to be used by the vsock service server.
    ///
    /// # Panics
    ///
    /// Panics if the current socket is already set.
    pub async fn set_connection(&self, conn: Arc<Connection<B, Socket>>, cid: u32) {
        self.current_cid.store(cid, Ordering::Relaxed);
        self.callback.transport_reset(cid).await.unwrap_or_else(log_callback_error);
        let mut current = self.connection.lock().unwrap();
        if current.as_ref().and_then(Weak::upgrade).is_some() {
            panic!("Can only have one active connection set at a time");
        }
        current.replace(Arc::downgrade(&conn));
    }

    /// Gets the current connection if one is set.
    fn get_connection(&self) -> Option<Arc<Connection<B, Socket>>> {
        self.connection.lock().unwrap().as_ref().and_then(Weak::upgrade)
    }

    async fn send_request(&self, addr: Addr, data: zx::Socket) -> Result<(), Status> {
        log::trace!("sending request for connection to address {addr:?}");
        let cb = self.callback.clone();
        let connection = self.get_connection();
        let device_cid = self.current_cid();
        self.scope.spawn(async move {
            let Some(connection) = connection else {
                // immediately reject a connection request if we don't have a usb connection to
                // put it on
                debug!("connection to {addr:?} rejected due to lack of a host connection");
                cb.rst(&addr).unwrap_or_else(log_callback_error);
                return;
            };
            let status = match connection
                .connect(from_fidl_addr(device_cid, addr), Socket::from_socket(data))
                .await
            {
                Ok(status) => status,
                Err(err) => {
                    // connection failed
                    debug!("Connection request failed to connect with err {err:?}");
                    cb.rst(&addr).unwrap_or_else(log_callback_error);
                    return;
                }
            };
            cb.response(&addr).unwrap_or_else(log_callback_error);
            status.wait_for_close().await.ok();
            cb.rst(&addr).unwrap_or_else(log_callback_error);
        });
        Ok(())
    }

    async fn send_shutdown(&self, addr: Addr) -> Result<(), Status> {
        if let Some(connection) = self.get_connection() {
            connection.close(&from_fidl_addr(self.current_cid(), addr)).await;
        } else {
            // this connection can't exist so just tell the caller that it was reset.
            self.callback.rst(&addr).unwrap_or_else(log_callback_error);
        }
        Ok(())
    }

    async fn send_rst(&self, addr: Addr) -> Result<(), Status> {
        if let Some(connection) = self.get_connection() {
            connection.reset(&from_fidl_addr(self.current_cid(), addr)).await.ok();
        }
        Ok(())
    }

    async fn send_response(&self, addr: Addr, data: zx::Socket) -> Result<(), Status> {
        // We cheat here and reconstitute the ConnectionRequest ourselves rather than try to thread
        // it through the state machine. Since the main client of this particular api should be
        // keeping track on its own, and we will ignore accepts of unknown addresses, this should be
        // fine.
        let address = from_fidl_addr(self.current_cid(), addr);
        let request = ConnectionRequest::new(address.clone());
        let Some(connection) = self.get_connection() else {
            error!("Tried to accept connection for {address:?} on usb connection that is not open");
            return Err(Status::BAD_STATE);
        };
        connection.accept(request, Socket::from_socket(data)).await.map_err(|err| {
            error!("Failed to accept connection for {address:?}: {err:?}");
            Status::ADDRESS_UNREACHABLE
        })?;

        Ok(())
    }

    async fn run_incoming_loop(
        mut incoming_connections: mpsc::Receiver<ConnectionRequest>,
        proxy: CallbacksProxy,
    ) {
        loop {
            let Some(next) = incoming_connections.next().await else {
                return;
            };
            if let Err(err) = proxy.request(&from_vsock_addr(*next.address())) {
                error!("Error calling callback for incoming connection request: {err:?}");
                return;
            }
        }
    }

    /// Runs the request loop for [`vsock::DeviceRequest`] against whatever the current [`Connection`]
    /// is.
    pub async fn run(&self, mut requests: vsock::DeviceRequestStream) -> Result<(), Error> {
        use vsock::DeviceRequest::*;

        while let Some(req) = requests.try_next().await.map_err(Error::other)? {
            match req {
                start @ Start { .. } => {
                    return Err(Error::other(format!(
                        "unexpected start message after one was already sent {start:?}"
                    )));
                }
                SendRequest { addr, data, responder } => responder
                    .send(self.send_request(addr, data).await.map_err(Status::into_raw))
                    .map_err(Error::other)?,
                SendShutdown { addr, responder } => responder
                    .send(self.send_shutdown(addr).await.map_err(Status::into_raw))
                    .map_err(Error::other)?,
                SendRst { addr, responder } => responder
                    .send(self.send_rst(addr).await.map_err(Status::into_raw))
                    .map_err(Error::other)?,
                SendResponse { addr, data, responder } => responder
                    .send(self.send_response(addr, data).await.map_err(Status::into_raw))
                    .map_err(Error::other)?,
                GetCid { responder } => responder.send(self.current_cid()).map_err(Error::other)?,
            }
        }
        Ok(())
    }
}

fn log_callback_error<E: std::error::Error>(err: E) {
    error!("Error sending callback to vsock client: {err:?}")
}

fn from_fidl_addr(device_cid: u32, value: Addr) -> Address {
    Address {
        device_cid,
        host_cid: value.remote_cid,
        device_port: value.local_port,
        host_port: value.remote_port,
    }
}

/// Leaves [`Address::device_cid`] blank, to be filled in by the caller
fn from_vsock_addr(value: Address) -> Addr {
    Addr { local_port: value.device_port, remote_cid: value.host_cid, remote_port: value.host_port }
}
