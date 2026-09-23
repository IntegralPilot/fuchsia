// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;

use crate::bound_virtio_socket::create_bound_virtio_socket;
use crate::microfuchsia_control;
use binder_proxy_config::Config;

use rpcbinder;

pub struct BinderProxy {
    server: rpcbinder::RpcServer,
    _shared_mem_vmo: Option<zx::Vmo>,
}

impl BinderProxy {
    pub fn new(config: &Config, port: u32, shared_mem_vmo: Option<zx::Vmo>) -> Result<Self, Error> {
        let socket_fd = create_bound_virtio_socket(config, port)?;
        let service = microfuchsia_control::new_binder();
        let server = rpcbinder::RpcServer::new_bound_socket(service, socket_fd)?;
        Ok(Self { server, _shared_mem_vmo: shared_mem_vmo })
    }

    pub fn run(&self) -> Result<(), Error> {
        self.server.join();
        Ok(())
    }
}
