// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::prelude::*;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::StreamExt;
use log::info;
use {
    fidl_fuchsia_driver_test as fdt, fidl_test_structuredconfig_receiver as scr,
    fidl_test_structuredconfig_receiver_shim as scrs,
};

enum IncomingRequest {
    Puppet(scr::ConfigReceiverPuppetRequestStream),
}

#[fuchsia::main]
async fn main() -> anyhow::Result<()> {
    // Create the RealmBuilder and start the driver.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;

    let expose = fuchsia_component_test::Capability::service::<scrs::ConfigServiceMarker>().into();
    let dtr_exposes = vec![expose];

    builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await?;
    let realm = builder.build().await?;

    let args = fdt::RealmArgs {
        root_driver: Some("#meta/cpp_driver_receiver.cm".to_string()),
        dtr_exposes: Some(dtr_exposes),
        ..Default::default()
    };
    info!("about to start driver test realm");
    realm.driver_test_realm_start(args).await?;
    info!("started driver test realm");

    let config_service = fuchsia_component::client::Service::open_from_dir(
        realm.root.get_exposed_dir(),
        scrs::ConfigServiceMarker,
    )?
    .watch_for_any()
    .await?;

    // Serve this configuration back to the test
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingRequest::Puppet);
    fs.take_and_serve_directory_handle().unwrap();
    fs.for_each_concurrent(None, |request: IncomingRequest| async {
        match request {
            IncomingRequest::Puppet(stream) => {
                // TOOD(https://fxbug.dev/42072863): Make this conversion less verbose.
                let server_end: fidl::endpoints::ServerEnd<scr::ConfigReceiverPuppetMarker> =
                    std::sync::Arc::try_unwrap(stream.into_inner().0)
                        .unwrap()
                        .into_channel()
                        .into_zx_channel()
                        .into();
                config_service.connect_channel_to_puppet(server_end).unwrap()
            }
        }
    })
    .await;
    Ok(())
}
