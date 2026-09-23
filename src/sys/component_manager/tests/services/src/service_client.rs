// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_examples as fexamples;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let service_proxy = client::open_service::<fexamples::EchoServiceMarker>()?;

    let mut fs = ServiceFs::new();
    fs.dir("svc").add_remote("fuchsia.examples.EchoService", service_proxy);
    fs.take_and_serve_directory_handle()?;

    fs.collect::<()>().await;
    Ok(())
}
