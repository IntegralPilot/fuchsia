// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use component_events::events::{EventStream, Started, Stopped};
use component_events::matcher::EventMatcher;
use fidl_fuchsia_driver_development as fdd;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder2, DriverTestRealmInstance2, Options2};

#[fuchsia::test]
async fn test_index_stop_on_idle() -> Result<()> {
    let mut event_stream = EventStream::open().await?;

    // Create the RealmBuilder.
    let builder = RealmBuilder::new().await?;
    let args = fdt::RealmArgs { driver_index_stop_timeout_millis: Some(100), ..Default::default() };
    builder.driver_test_realm_setup(Options2::new(), args).await?;
    // Build the Realm.
    let realm = builder.build().await?;
    realm.wait_for_bootup().await?;

    // Add some composite node specs to the index.
    {
        let composite_manager: fdf::CompositeNodeManagerProxy =
            realm.root.connect_to_protocol_at_exposed_dir()?;
        composite_manager
            .add_spec(&fdf::CompositeNodeSpec {
                name: Some("composite_spec_name_unmatched".to_owned()),
                parents2: Some(vec![fdf::ParentSpec2 {
                    bind_rules: vec![fdf::BindRule2 {
                        key: "somekey".to_owned(),
                        condition: fdf::Condition::Accept,
                        values: vec![fdf::NodePropertyValue::StringValue("somevalue".to_owned())],
                    }],
                    properties: vec![fdf::NodeProperty2 {
                        key: "somekey".to_owned(),
                        value: fdf::NodePropertyValue::StringValue("somevalue".to_owned()),
                    }],
                }]),
                ..Default::default()
            })
            .await?
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;

        composite_manager
            .add_spec(&fdf::CompositeNodeSpec {
                name: Some("composite_spec_name_matched".to_owned()),
                parents2: Some(vec![
                    fdf::ParentSpec2 {
                        bind_rules: vec![fdf::BindRule2 {
                            key: "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY".to_owned(),
                            condition: fdf::Condition::Accept,
                            values: vec![fdf::NodePropertyValue::EnumValue(
                                "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY.DRIVER_LEFT"
                                    .to_owned(),
                            )],
                        }],
                        properties: vec![fdf::NodeProperty2 {
                            key: "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY".to_owned(),
                            value: fdf::NodePropertyValue::EnumValue(
                                "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY.DRIVER_LEFT"
                                    .to_owned(),
                            ),
                        }],
                    },
                    fdf::ParentSpec2 {
                        bind_rules: vec![fdf::BindRule2 {
                            key: "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY".to_owned(),
                            condition: fdf::Condition::Accept,
                            values: vec![fdf::NodePropertyValue::EnumValue(
                                "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY.DRIVER_RIGHT"
                                    .to_owned(),
                            )],
                        }],
                        properties: vec![fdf::NodeProperty2 {
                            key: "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY".to_owned(),
                            value: fdf::NodePropertyValue::EnumValue(
                                "fuchsia.nodegroupbind.test.TEST_BIND_PROPERTY.DRIVER_RIGHT"
                                    .to_owned(),
                            ),
                        }],
                    },
                ]),
                ..Default::default()
            })
            .await?
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;
    }

    // Make sure index is started.
    let _started = EventMatcher::ok()
        .moniker_regex(".+driver_index")
        .wait::<Started>(&mut event_stream)
        .await
        .unwrap();

    // Collect the composite node specs we added for comparison later.
    let mut all_specs = Vec::new();
    {
        let driver_dev: fdd::ManagerProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        let (node_spec, server) = fidl::endpoints::create_proxy();
        driver_dev.get_composite_node_specs(None, server)?;
        loop {
            let mut specs = node_spec.get_next().await?;
            if specs.is_empty() {
                break;
            }

            all_specs.append(&mut specs);
        }
    }

    // Index is now idle and will stop itself after the timeout is reached...

    // Wait for stop event.
    let _stopped = EventMatcher::ok()
        .moniker_regex(".+driver_index")
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();

    // Re-connect to the driver index which will cause it to start back up and resume from its
    // saved state. We will query the composite node specs to compare them with before.
    let mut all_specs_after = Vec::new();
    {
        let driver_dev: fdd::ManagerProxy = realm.root.connect_to_protocol_at_exposed_dir()?;

        let (node_spec, server) = fidl::endpoints::create_proxy();
        driver_dev.get_composite_node_specs(None, server)?;

        loop {
            let mut specs = node_spec.get_next().await?;
            if specs.is_empty() {
                break;
            }

            all_specs_after.append(&mut specs);
        }
    }

    // Sort them by name since on the index these are stored in a hashmap which is unordered,
    // but they get returned as a vector here where ordering is important to equality.
    all_specs.sort_by(|a, b| {
        let a_name = a.spec.as_ref().and_then(|s| s.name.as_ref());
        let b_name = b.spec.as_ref().and_then(|s| s.name.as_ref());
        a_name.cmp(&b_name)
    });
    all_specs_after.sort_by(|a, b| {
        let a_name = a.spec.as_ref().and_then(|s| s.name.as_ref());
        let b_name = b.spec.as_ref().and_then(|s| s.name.as_ref());
        a_name.cmp(&b_name)
    });

    // Should be the same.
    assert_eq!(all_specs, all_specs_after);

    // Catch the start event (for when we called into it again earlier).
    let _started = EventMatcher::ok()
        .moniker_regex(".+driver_index")
        .wait::<Started>(&mut event_stream)
        .await
        .unwrap();

    // Wait for it to go back to idle and stop again.
    let _stopped = EventMatcher::ok()
        .moniker_regex(".+driver_index")
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();

    realm.destroy().await?;
    Ok(())
}
