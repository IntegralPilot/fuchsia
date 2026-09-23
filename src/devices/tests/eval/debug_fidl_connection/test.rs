// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl_fidl_examples_echo as fecho;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

async fn run_test(root_driver_url: &str) -> Result<()> {
    let builder = RealmBuilder::new().await?;

    let expose = fuchsia_component_test::Capability::service::<fecho::EchoServiceMarker>().into();
    let exposes = vec![expose];

    let args =
        fdt::RealmArgs { root_driver: Some(root_driver_url.to_string()), ..Default::default() };

    builder.driver_test_realm_setup(Options2::new().driver_exposes(exposes), args).await?;

    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    // Try to connect to EchoService. This will fail if the driver doesn't expose it.
    let _device =
        client::Service::open_from_dir(realm.root.get_exposed_dir(), fecho::EchoServiceMarker)
            .context("Failed to open service")?
            .watch_for_any()
            .await
            .context("Failed to find service instance (driver must serve and expose it)")?;

    realm.destroy().await?;
    Ok(())
}

#[fasync::run_singlethreaded(test)]
async fn test_driver() {
    run_test("#meta/debug_fidl.cm").await.unwrap();
}
