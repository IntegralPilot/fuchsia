// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_developer_ffx_speedtest as fspeedtest;
use futures::StreamExt as _;
use log::{error, info};

#[fuchsia::main]
async fn main() {
    info!("speedtest started");
    let mut fs = fuchsia_component::server::ServiceFs::new_local();
    let _: &mut fuchsia_component::server::ServiceFsDir<'_, _> =
        fs.dir("svc").add_fidl_service(|s: fspeedtest::SpeedtestRequestStream| s);

    let _: &mut fuchsia_component::server::ServiceFs<_> =
        fs.take_and_serve_directory_handle().expect("failed to serve ServiceFs directory");

    let mut requests = fs.fuse().flatten_unordered(None);

    let server = speedtest::server::Server::default();

    while let Some(req) = requests.next().await {
        req.and_then(|r| server.handle_request(r)).unwrap_or_else(|e| {
            if !e.is_closed() {
                error!("error handling request: {e:?}");
            }
        })
    }
}
