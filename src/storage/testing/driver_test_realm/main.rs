// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use futures::StreamExt;
use {fidl_fuchsia_driver_test as fdt, fidl_fuchsia_io as fio};

#[fuchsia::main]
async fn main() -> Result<()> {
    let dtr_exposes = vec![
        fidl_fuchsia_component_test::Capability::Service(fidl_fuchsia_component_test::Service {
            name: Some("fuchsia.hardware.ramdisk.Service".to_owned()),
            ..Default::default()
        }),
        fidl_fuchsia_component_test::Capability::Service(fidl_fuchsia_component_test::Service {
            name: Some("fuchsia.hardware.block.volume.Service".to_owned()),
            ..Default::default()
        }),
    ];

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(fdt::RealmArgs {
            root_driver: Some("fuchsia-boot:///platform-bus#meta/platform-bus.cm".to_owned()),
            dtr_exposes: Some(dtr_exposes),
            software_devices: Some(vec![
                fidl_fuchsia_driver_test::SoftwareDevice {
                    device_name: "ram-disk".to_string(),
                    device_id: bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_RAM_DISK,
                },
                fidl_fuchsia_driver_test::SoftwareDevice {
                    device_name: "ram-nand".to_string(),
                    device_id: bind_fuchsia_platform::BIND_PLATFORM_DEV_DID_RAM_NAND,
                },
            ]),
            ..Default::default()
        })
        .await?;

    let mut fs = ServiceFs::new();

    let (dir_client, dir_server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    instance
        .root
        .get_exposed_dir()
        .clone(fidl::endpoints::ServerEnd::new(dir_server.into_channel()))
        .context("clone failed")?;
    fs.add_remote("realm_builder_exposed_dir", dir_client);

    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}
