// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of the network socket proxy.
//!
//! Runs proxied versions of fuchsia.posix.socket.Provider and fuchsia.posix.socket.raw.Provider.
//! Exposes fuchsia.net.policy.socketproxy.StarnixNetworks.

use anyhow::Context as _;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_posix_socket::{self as fposix_socket, OptionalUint32};
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use fuchsia_inspect::health::Reporter;
use fuchsia_inspect_derive::{Inspect, WithInspect as _};
use futures::StreamExt as _;
use futures::channel::mpsc;
use futures::lock::Mutex;
use log::error;
use std::sync::Arc;

mod mark_watcher;
pub mod registry;
mod socket_provider;

pub use registry::NetworkRegistryError;

#[derive(Copy, Clone, Debug)]
struct SocketMarks {
    mark_1: OptionalUint32,
    mark_2: OptionalUint32,
}

impl From<SocketMarks> for fnet::Marks {
    fn from(SocketMarks { mark_1, mark_2 }: SocketMarks) -> Self {
        let into_option_u32 = |opt| match opt {
            OptionalUint32::Unset(fposix_socket::Empty) => None,
            OptionalUint32::Value(val) => Some(val),
        };
        Self {
            mark_1: into_option_u32(mark_1),
            mark_2: into_option_u32(mark_2),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

impl SocketMarks {
    fn has_value(&self) -> bool {
        match (self.mark_1, self.mark_2) {
            (OptionalUint32::Value(_), _) => true,
            (_, OptionalUint32::Value(_)) => true,
            _ => false,
        }
    }

    fn set_mark(&mut self, domain: fnet::MarkDomain, value: Option<u32>) {
        let value = match value {
            Some(value) => fposix_socket::OptionalUint32::Value(value),
            None => fposix_socket::OptionalUint32::Unset(fposix_socket::Empty),
        };

        match domain {
            fnet::MarkDomain::Mark1 => self.mark_1 = value,
            fnet::MarkDomain::Mark2 => self.mark_2 = value,
        }
    }
}

impl Default for SocketMarks {
    fn default() -> Self {
        Self {
            mark_1: OptionalUint32::Unset(fposix_socket::Empty),
            mark_2: OptionalUint32::Unset(fposix_socket::Empty),
        }
    }
}

#[derive(Inspect)]
struct SocketProxy {
    registry: registry::Registry,
    socket_provider: socket_provider::SocketProvider,
}

impl SocketProxy {
    fn new(
        forwarder_tx: mpsc::Sender<crate::registry::NetworkRegistryRequest>,
    ) -> Result<Self, anyhow::Error> {
        let mark = Arc::new(Mutex::new(SocketMarks::default()));
        Ok(Self {
            registry: registry::Registry::new(mark.clone(), forwarder_tx)
                .context("while creating registry")?,
            socket_provider: socket_provider::SocketProvider::new(mark),
        })
    }
}

enum IncomingService {
    StarnixNetworks(fnp_socketproxy::StarnixNetworksRequestStream),
    PosixSocket(fidl_fuchsia_posix_socket::ProviderRequestStream),
    PosixSocketRaw(fidl_fuchsia_posix_socket_raw::ProviderRequestStream),
}

/// Main entry point for the network socket proxy.
pub async fn run() -> Result<(), anyhow::Error> {
    fuchsia_inspect::component::health().set_starting_up();

    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());

    // Use a generous buffer size without making it unbounded. If the
    // server (netcfg) is not processing messages quickly enough,
    // this indicates more significant system issues.
    let (forwarder_tx, forwarder_rx) = mpsc::channel(50);
    let mut request_forwarder = registry::RequestForwarder::new(forwarder_rx)?;

    let proxy = Arc::new(SocketProxy::new(forwarder_tx)?.with_inspect(inspector.root(), "root")?);

    let mut fs = ServiceFs::new_local();
    let _: &mut ServiceFsDir<'_, _> = fs
        .dir("svc")
        .add_fidl_service(IncomingService::StarnixNetworks)
        .add_fidl_service(IncomingService::PosixSocket)
        .add_fidl_service(IncomingService::PosixSocketRaw);

    let _: &mut ServiceFs<_> = fs.take_and_serve_directory_handle()?;

    fuchsia_inspect::component::health().set_ok();

    let proxy_for_service = Arc::clone(&proxy);
    let service_fut = fs.for_each_concurrent(100, move |service| {
        let proxy = Arc::clone(&proxy_for_service);
        async move {
            match service {
                IncomingService::StarnixNetworks(stream) => {
                    proxy.registry.run_starnix(stream).await
                }
                IncomingService::PosixSocket(stream) => proxy.socket_provider.run(stream).await,
                IncomingService::PosixSocketRaw(stream) => {
                    proxy.socket_provider.run_raw(stream).await
                }
            }
            .unwrap_or_else(|e| error!("{e:?}"))
        }
    });

    let scope = fasync::Scope::new();

    let _ = scope.spawn_local(async move {
        let res = request_forwarder.run().await;
        error!("RequestForwarder future has terminated: {res:?}");
    });

    let proxy_clone = Arc::clone(&proxy);
    let _ = scope.spawn_local(async move {
        mark_watcher::watch_properties(proxy_clone).await;
    });

    let _ = scope.spawn_local(async move {
        service_fut.await;
        error!("The main services future has terminated. It should never terminate");
        // Abort the scope to signal that the main service loop has unexpectedly ended.
        fasync::Scope::current().abort().await;
    });

    scope.join().await;

    Ok(())
}
