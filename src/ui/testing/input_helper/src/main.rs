// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl_fuchsia_ui_test_input::RegistryRequestStream;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::info;

enum Service {
    RegistryServer(RegistryRequestStream),
}

/// Note to contributors: This component is test-only, so it should panic liberally. Loud crashes
/// are much easier to debug than silent failures. Please use `expect()` and `panic!` where
/// applicable.
#[fuchsia::main(logging_tags = ["input_helper"])]
async fn main() -> Result<(), Error> {
    info!("starting input synthesis test component");
    fuchsia_trace_provider::trace_provider_create_with_fdio();

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(Service::RegistryServer);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(None, |conn| async move {
        let Service::RegistryServer(stream) = conn;
        input_testing::handle_registry_request_stream(stream).await;
    })
    .await;

    info!("input helper exit");

    Ok(())
}
