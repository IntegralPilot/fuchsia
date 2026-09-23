// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result, format_err};
use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::{ArchiveReader, DiagnosticsHierarchy};
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

async fn get_inspect_hierarchy(moniker: String) -> Result<DiagnosticsHierarchy, Error> {
    ArchiveReader::inspect()
        .add_selector(format!("{}:[name=root-driver]root", moniker))
        .snapshot()
        .await?
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .ok_or_else(|| format_err!("expected one inspect hierarchy"))
}

#[fuchsia::test]
async fn test_driver_inspect() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    let args = fdt::RealmArgs {
        root_driver: Some("fuchsia-boot:///dtr#meta/test-parent-sys.cm".to_string()),
        ..Default::default()
    };
    builder.driver_test_realm_setup(Options2::new(), args).await?;
    let instance = builder.build().await?;
    instance.wait_for_bootup().await?;

    // Connect to our driver.
    let dev = instance.driver_test_realm_connect_to_dev()?;
    let driver = device_watcher::recursive_wait_and_open::<
        fidl_fuchsia_inspect_test::HandshakeMarker,
    >(&dev, "sys/test/root-driver")
    .await?;

    let moniker = format!(
        "realm_builder\\:{}/driver_test_realm/boot-drivers\\:dev.sys.test",
        instance.root.child_name()
    );
    // Check the inspect metrics.
    let mut hierarchy = get_inspect_hierarchy(moniker.clone()).await?;
    assert_data_tree!(hierarchy, root: contains {
        connection_info: contains {
            request_count: 0u64,
        }
    });

    // Do the request and check the inspect metrics again.
    driver.r#do().await.unwrap();

    hierarchy = get_inspect_hierarchy(moniker).await?;
    assert_data_tree!(hierarchy, root: contains {
        connection_info: contains {
            request_count: 1u64,
        }
    });

    instance.destroy().await?;
    Ok(())
}
