// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use binder_proxy_config::Config;

use crate::bound_virtio_socket::create_bound_virtio_socket;
use crate::trusted_app;

pub struct RPCSession {
    server: rpcbinder::RpcServer,
}

impl RPCSession {
    pub fn new(config: &Config, port: u32, uuid: &str) -> Result<Self, Error> {
        let socket_fd = create_bound_virtio_socket(config, port)?;
        let service = trusted_app::TrustedApp::new_binder(uuid);
        let server = rpcbinder::RpcServer::new_bound_socket(service, socket_fd)?;

        Ok(RPCSession { server })
    }

    pub fn start(&self) {
        self.server.start();
    }
}
