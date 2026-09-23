// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_driver_development::ManagerProxy;
use fidl_fuchsia_driver_framework::{NodePropertyKey, NodePropertyValue};
use fidl_fuchsia_driver_test::RealmArgs;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

// TODO(https://fxbug.dev/470088116): re-enable after fixing flake.
#[ignore]
#[fuchsia::test]
async fn test_sample_driver() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance.driver_test_realm_start(RealmArgs::default()).await?;

    let manager: ManagerProxy = instance.root.connect_to_protocol_at_exposed_dir()?;
    let (node_iter, node_iter_server) = create_endpoints();
    manager.get_node_info(
        &["dev.sys.test.driver_transport_rust_next_child.transport-child".to_owned()],
        node_iter_server,
        true,
    )?;
    let node_iter = node_iter.into_proxy();
    let nodes = node_iter.get_next().await?;
    let Some(node) = nodes.into_iter().next() else {
        panic!("could not find the 'transport-child' node");
    };

    let expected_key = NodePropertyKey::StringValue("fuchsia.test.TEST_CHILD".to_owned());
    let expected_value = NodePropertyValue::IntValue(0x1234ABCD);
    let prop_found = node
        .node_property_list
        .expect("node property list to be filled in")
        .into_iter()
        .any(|prop| prop.key == expected_key && prop.value == expected_value);
    assert!(prop_found);

    Ok(())
}
