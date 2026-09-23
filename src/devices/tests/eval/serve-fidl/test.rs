// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fuchsia_driver_test as fdt;
use fidl_fuchsia_examples as fecho;
use fuchsia_component::client;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

async fn run_test(root_driver_url: &str) -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;

    // We expect the driver to expose the EchoService.
    let expose = fuchsia_component_test::Capability::service::<fecho::EchoServiceMarker>().into();
    let exposes = vec![expose];

    let args =
        fdt::RealmArgs { root_driver: Some(root_driver_url.to_string()), ..Default::default() };

    builder.driver_test_realm_setup(Options2::new().driver_exposes(exposes), args).await?;

    // Build the Realm.
    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    // Connect to the `EchoService`.
    let _device =
        client::Service::open_from_dir(realm.root.get_exposed_dir(), fecho::EchoServiceMarker)
            .context("Failed to open service")?
            .watch_for_any()
            .await
            .context("Failed to find service instance (driver must serve it)")?;

    realm.destroy().await?;
    Ok(())
}

#[fuchsia::test]
async fn test_driver() {
    run_test("#meta/eval_driver_serve_fidl.cm").await.unwrap();
}
