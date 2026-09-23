// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_registrar as fdr;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::{RealmBuilder, RealmInstance};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

// Note: The component manifest name is the same for FAKE_DRIVER_URL and EPHEMERAL_FAKE_DRIVER_URL
// One is in bootfs, the other is through a package url.
// This is because we can't have an in-tree driver not be in bootfs.
//
// In this test the ephemeral one is still being resolved through the package resolver and going
// through the register flow when we call the register api.
//
// Unfortunately this means that we can't test whether we can load the new driver using
// bind_all_unbound_nodes.
// Whatever the bind rules on the fake driver are will be identical to the one in bootfs
// and if there is a device that would bind to it, it would bind to the bootfs one first.
const SAMPLE_DRIVER_URL: &str = "fuchsia-boot:///dtr#meta/sample_driver.cm";
const PARENT_DRIVER_URL: &str = "fuchsia-boot:///dtr#meta/test-parent-sys.cm";
const FAKE_DRIVER_URL: &str = "fuchsia-boot:///dtr#meta/driver-test-realm-fake-driver.cm";
const EPHEMERAL_FAKE_DRIVER_URL: &str =
    "fuchsia-pkg://fuchsia.com/driver-test-realm-fake-driver#meta/driver-test-realm-fake-driver.cm";

async fn set_up_test_driver_realm()
-> Result<(RealmInstance, fdd::ManagerProxy, fdr::DriverRegistrarProxy)> {
    const ROOT_DRIVER_URL: &str = PARENT_DRIVER_URL;

    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    let instance = builder.build().await?;

    let mut realm_args = fdt::RealmArgs::default();
    // DriverTestRealm attempts to bind the .so of test-parent-sys if not explicitly requested otherwise.
    realm_args.root_driver = Some(ROOT_DRIVER_URL.to_owned());
    instance.driver_test_realm_start(realm_args).await?;

    let driver_dev = instance.root.connect_to_protocol_at_exposed_dir()?;
    let driver_registar = instance.root.connect_to_protocol_at_exposed_dir()?;

    Ok((instance, driver_dev, driver_registar))
}

fn send_get_driver_info_request(
    service: &fdd::ManagerProxy,
    driver_filter: &[String],
) -> Result<fdd::DriverInfoIteratorProxy> {
    let (iterator, iterator_server) =
        fidl::endpoints::create_proxy::<fdd::DriverInfoIteratorMarker>();

    service
        .get_driver_info(driver_filter, iterator_server)
        .context("FIDL call to get driver info failed")?;

    Ok(iterator)
}

async fn get_driver_info(
    service: &fdd::ManagerProxy,
    driver_filter: &[String],
) -> Result<Vec<fdf::DriverInfo>> {
    let iterator = send_get_driver_info_request(service, driver_filter)?;

    let mut driver_infos = Vec::new();
    loop {
        let mut driver_info =
            iterator.get_next().await.context("FIDL call to get driver info failed")?;
        if driver_info.len() == 0 {
            break;
        }
        driver_infos.append(&mut driver_info)
    }
    Ok(driver_infos)
}

fn assert_contains_driver_url(driver_infos: &Vec<fdf::DriverInfo>, expected_driver_url: &str) {
    assert!(
        driver_infos
            .iter()
            .find(|driver_info| driver_info.url.as_ref().expect("Missing device URL")
                == expected_driver_url)
            .is_some()
    );
}

#[fuchsia::test]
async fn test_register_driver() -> Result<()> {
    let (_instance, driver_dev, driver_registrar) = set_up_test_driver_realm().await?;
    let driver_infos = get_driver_info(&driver_dev, &[]).await?;

    // Before register we should have 3 drivers, the ones in bootfs.
    assert_eq!(driver_infos.len(), 3);
    assert_contains_driver_url(&driver_infos, SAMPLE_DRIVER_URL);
    assert_contains_driver_url(&driver_infos, PARENT_DRIVER_URL);
    assert_contains_driver_url(&driver_infos, FAKE_DRIVER_URL);

    // Register the driver through a package url.
    driver_registrar
        .register(EPHEMERAL_FAKE_DRIVER_URL)
        .await
        .map_err(|e| anyhow!("Failed to call register driver: {}", e))?
        .map_err(|e| anyhow!("Failed to register driver with err: {}", e))?;

    // Now we should have 4 drivers, the original 3, plus the new ephemeral one.
    let driver_infos = get_driver_info(&driver_dev, &[]).await?;
    assert_eq!(driver_infos.len(), 4);
    assert_contains_driver_url(&driver_infos, SAMPLE_DRIVER_URL);
    assert_contains_driver_url(&driver_infos, PARENT_DRIVER_URL);
    assert_contains_driver_url(&driver_infos, FAKE_DRIVER_URL);
    assert_contains_driver_url(&driver_infos, EPHEMERAL_FAKE_DRIVER_URL);

    Ok(())
}
