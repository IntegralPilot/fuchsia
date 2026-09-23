// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_test_config_parentoverrides::{ReporterRequest, ReporterRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use received_config::Config;

enum IncomingClient {
    Reporter(ReporterRequestStream),
}

#[fuchsia::main]
async fn main() {
    let Config { parent_provided, .. } = Config::take_from_startup_handle();

    let mut service_fs = ServiceFs::new();
    service_fs.dir("svc").add_fidl_service(IncomingClient::Reporter);
    service_fs.take_and_serve_directory_handle().unwrap();

    service_fs
        .for_each_concurrent(None, |client: IncomingClient| async {
            match client {
                IncomingClient::Reporter(mut requests) => {
                    while let Some(request) = requests.next().await {
                        match request.unwrap() {
                            ReporterRequest::GetParentProvidedConfigString { responder } => {
                                responder.send(&parent_provided).unwrap();
                            }
                        }
                    }
                }
            }
        })
        .await;
}
