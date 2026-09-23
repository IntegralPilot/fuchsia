// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use anyhow::Result;
use fidl_test_configexample::{ConfigUserMarker, ConfigUserProxy};
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route};
use test_case::test_case;

async fn create_realm(suspend_enabled: bool) -> Result<RealmInstance> {
    let builder = RealmBuilder::new().await?;

    let client = builder
        .add_child(
            "power_config_client",
            "power_config_client_package#meta/power_config_client.cm",
            ChildOptions::new(),
        )
        .await?;

    // Add the fuchsia.power.SuspendEnabled capability with its value set to suspend_enabled, and
    // route it to the config client.
    builder
        .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
            name: "fuchsia.power.SuspendEnabled".parse()?,
            value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::Bool(suspend_enabled)),
        }))
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::configuration("fuchsia.power.SuspendEnabled"))
                .from(Ref::self_())
                .to(&client),
        )
        .await?;

    // The test will use the config client's implementation of ConfigUser to confirm that it
    // utilizes fuchsia.power.SuspendEnabled as expected.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<ConfigUserMarker>())
                .from(&client)
                .to(Ref::parent()),
        )
        .await?;

    let realm = builder.build().await?;
    Ok(realm)
}

#[test_case(true; "should_manage_power")]
#[test_case(false; "suspend_disabled")]
#[fuchsia::test]
async fn test_config_user(suspend_enabled: bool) {
    let realm = create_realm(suspend_enabled).await.expect("Failed to create realm");

    log::info!("Connecting to user protocol");
    let proxy: ConfigUserProxy =
        realm.root.connect_to_protocol_at_exposed_dir().expect("Failed to connect");

    let is_managing_power = proxy.is_managing_power().await.expect("Failed to query ConfigUser");
    assert_eq!(is_managing_power, suspend_enabled);
}
