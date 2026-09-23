// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_tracing_controller::SessionManagerRequestStream;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;

mod data;
mod tracing_protocol;

#[fuchsia::main]
async fn main() {
    let protocol = tracing_protocol::TracingProtocol::default();
    let mut fs = ServiceFs::new();
    let p_clone = protocol.clone();
    fs.dir("svc").add_fidl_service(move |stream: SessionManagerRequestStream| {
        let p = p_clone.clone();
        fuchsia_async::Task::local(async move {
            if let Err(e) = p.serve(stream).await {
                log::error!("Error handling session manager requests: {:?}", e);
            }
        })
        .detach();
    });

    // Check for starting tracing when starting up.
    fuchsia_async::Task::local(async move {
        if let Err(e) = protocol.check_for_on_boot_tracing().await {
            log::error!("Error checking for on-boot tracing: {:?}", e);
        }
    })
    .detach();

    fs.take_and_serve_directory_handle().expect("take and serve for TracingProtocol.");
    fs.collect::<()>().await;
}
