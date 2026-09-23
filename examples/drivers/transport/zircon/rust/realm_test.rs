// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl::endpoints::create_endpoints;
use fidl_fuchsia_driver_development::ManagerProxy;
use fidl_fuchsia_driver_framework::{NodePropertyKey, NodePropertyValue};
use fidl_fuchsia_driver_test::RealmArgs;
use fidl_fuchsia_hardware_i2c as i2c;
use fuchsia_component::client::Service;
use fuchsia_component_test::{Capability, RealmBuilder};
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};

#[fuchsia::test]
async fn test_sample_driver() -> Result<()> {
    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    let dtr_exposes = vec![Capability::service::<i2c::ServiceMarker>().into()];
    builder.driver_test_realm_add_dtr_exposes(&dtr_exposes).await?;
    // Build the Realm.
    let instance = builder.build().await?;
    // Start DriverTestRealm
    instance
        .driver_test_realm_start(RealmArgs { dtr_exposes: Some(dtr_exposes), ..Default::default() })
        .await?;

    let client = Service::open_from_dir(instance.root.get_exposed_dir(), i2c::ServiceMarker)?
        .watch_for_any()
        .await?
        .connect_to_device()?;
    let device_name = client.get_name().await?.unwrap();

    println!("device name: {device_name}");

    let manager: ManagerProxy = instance.root.connect_to_protocol_at_exposed_dir()?;
    let (node_iter, node_iter_server) = create_endpoints();
    manager.get_node_info(
        &["dev.sys.test.zircon_transport_rust_child.transport-child".to_owned()],
        node_iter_server,
        true,
    )?;
    let node_iter = node_iter.into_proxy();
    let nodes = node_iter.get_next().await?;
    let Some(node) = nodes.into_iter().next() else {
        panic!("could not find the 'transport-child' node");
    };
    let expected_key = NodePropertyKey::StringValue("fuchsia.test.TEST_CHILD".to_owned());
    let expected_value = NodePropertyValue::StringValue(device_name);
    let prop_found = node
        .node_property_list
        .expect("node property list to be filled in")
        .into_iter()
        .any(|prop| prop.key == expected_key && prop.value == expected_value);
    assert!(prop_found);

    Ok(())
}
