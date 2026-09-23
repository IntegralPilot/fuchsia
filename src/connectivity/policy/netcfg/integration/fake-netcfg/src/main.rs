// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_policy_properties as fnp_properties;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fidl_fuchsia_net_reachability as freachability;
use fuchsia_component::server::ServiceFs;
use futures::stream::StreamExt as _;
use log::{debug, error};

enum IncomingServices {
    NetworkRegistry(fnp_socketproxy::NetworkRegistryRequestStream),
    Networks(fnp_properties::NetworksRequestStream),
    Reachability(freachability::MonitorRequestStream),
}

impl std::fmt::Debug for IncomingServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkRegistry(_) => f.debug_tuple("NetworkRegistry").finish(),
            Self::Networks(_) => f.debug_tuple("Networks").finish(),
            Self::Reachability(_) => f.debug_tuple("Reachability").finish(),
        }
    }
}

#[fuchsia::main]
async fn main() {
    debug!("Starting fake-netcfg");
    let mut fs = ServiceFs::new_local();
    let _ = fs
        .dir("svc")
        .add_fidl_service(IncomingServices::NetworkRegistry)
        .add_fidl_service(IncomingServices::Networks)
        .add_fidl_service(IncomingServices::Reachability);
    let _ = fs.take_and_serve_directory_handle().expect("must serve ServiceFs");
    let mut fs = fs.fuse();

    let mut networks_service = netcfg::network::NetpolNetworksService::default();

    // Multiplex incoming FIDL service connections and internal network service events.
    loop {
        futures::select! {
            req_stream = fs.select_next_some() => {
                match req_stream {
                    IncomingServices::NetworkRegistry(rs) => networks_service.add_stream(rs),
                    IncomingServices::Networks(rs) => networks_service.add_stream(rs),
                    IncomingServices::Reachability(rs) => networks_service.add_stream(rs),
                }
            }
            netcfg_event = networks_service.select_next_some() => {
                if let Err(e) = networks_service.handle_event(netcfg_event).await {
                    error!("Encountered issue handling event: {e:?}");
                }
            }
        }
    }
}
