// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_component::{BinderMarker, CreateChildArgs, RealmMarker};
use fidl_fuchsia_component_decl::{
    Child, CollectionRef, ConfigOverride, ConfigSingleValue, ConfigValue, StartupMode,
};
use fuchsia_component::client::{
    connect_to_protocol, connect_to_protocol_at_dir_root, open_childs_exposed_directory,
};
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};

const CHILD_URL: &str = "#meta/config_example.cm";
const COLLECTION_NAME: &str = "realm_api_collection";

#[fuchsia::test]
async fn inspect_parent_override() -> anyhow::Result<()> {
    let child_name = "dynamic_child_realm_api_parent_values";
    let expected_greeting = "parent component";
    let realm = connect_to_protocol::<RealmMarker>()?;

    // [START create_child]
    let child_decl = Child {
        name: Some(child_name.to_string()),
        url: Some(String::from(CHILD_URL)),
        startup: Some(StartupMode::Lazy),
        config_overrides: Some(vec![ConfigOverride {
            key: Some("greeting".to_string()),
            value: Some(ConfigValue::Single(ConfigSingleValue::String(
                expected_greeting.to_string(),
            ))),
            ..ConfigOverride::default()
        }]),
        ..Child::default()
    };

    let collection_ref = CollectionRef { name: COLLECTION_NAME.to_string() };
    realm
        .create_child(&collection_ref, &child_decl, CreateChildArgs::default())
        .await
        .context("sending create child message")?
        .map_err(|e| anyhow::format_err!("creating child: {e:?}"))?;
    // [END create_child]

    let exposed_dir = open_childs_exposed_directory(child_name, Some(COLLECTION_NAME.to_string()))
        .await
        .context("opening exposed directory")?;
    connect_to_protocol_at_dir_root::<BinderMarker>(&exposed_dir)
        .context("connecting to Binder")?;

    let example_payload = ArchiveReader::inspect()
        .add_selector(format!("{COLLECTION_NAME}\\:{child_name}:root"))
        .with_minimum_schema_count(1)
        .snapshot()
        .await
        .context("snapshotting inspect")?
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .context("getting first paylod")?;

    assert_eq!(example_payload.children.len(), 1, "selector must return exactly one child");

    assert_data_tree!(example_payload, root: {
        config: {
            greeting: expected_greeting,
        }
    });

    Ok(())
}

/// This test ensures that the test above is passing because parent overrides work rather than
/// lucking into the same value as the packaged config.
#[fuchsia::test]
async fn inspect_default_value() -> anyhow::Result<()> {
    let child_name = "dynamic_child_realm_api_default_values";
    let expected_greeting = "World";
    let realm = connect_to_protocol::<RealmMarker>()?;

    let child_decl = Child {
        name: Some(child_name.to_string()),
        url: Some(String::from(CHILD_URL)),
        startup: Some(StartupMode::Lazy),
        ..Child::default()
    };

    let collection_ref = CollectionRef { name: COLLECTION_NAME.to_string() };
    realm
        .create_child(&collection_ref, &child_decl, CreateChildArgs::default())
        .await
        .context("sending create child message")?
        .map_err(|e| anyhow::format_err!("creating child: {e:?}"))?;

    let exposed_dir = open_childs_exposed_directory(child_name, Some(COLLECTION_NAME.to_string()))
        .await
        .context("opening exposed directory")?;
    connect_to_protocol_at_dir_root::<BinderMarker>(&exposed_dir)
        .context("connecting to Binder")?;

    let example_payload = ArchiveReader::inspect()
        .add_selector(format!("{COLLECTION_NAME}\\:{child_name}:root"))
        .with_minimum_schema_count(1)
        .snapshot()
        .await
        .context("snapshotting inspect")?
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .context("getting first paylod")?;

    assert_eq!(example_payload.children.len(), 1, "selector must return exactly one child");

    assert_data_tree!(example_payload, root: {
        config: {
            greeting: expected_greeting,
        }
    });

    Ok(())
}

#[fuchsia::test]
async fn inspect_realm_builder_parent_override() {
    let mut child_overrides: Vec<fidl_fuchsia_component_decl::ConfigOverride> =
        Vec::with_capacity(1);

    child_overrides.push(fidl_fuchsia_component_decl::ConfigOverride {
        key: Some("greeting".to_string()),
        value: Some(fidl_fuchsia_component_decl::ConfigValue::Single(
            fidl_fuchsia_component_decl::ConfigSingleValue::String("parent component".to_string()),
        )),
        ..Default::default()
    });

    let builder = RealmBuilder::new().await.unwrap();
    let config_component = builder
        .add_child(
            "config_example_realm_builder_parent_override",
            CHILD_URL,
            ChildOptions::new().eager().config_overrides(child_overrides),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .from(Ref::parent())
                .to(&config_component),
        )
        .await
        .unwrap();

    let _instance = builder.build().await.unwrap();

    let inspector = ArchiveReader::inspect()
        .add_selector("*/config_example_realm_builder_parent_override:root")
        .with_minimum_schema_count(1)
        .snapshot()
        .await
        .unwrap()
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .unwrap();

    assert_eq!(inspector.children.len(), 1, "selector must return exactly one child");

    assert_data_tree!(inspector, root: {
        config: {
            greeting: "parent component",
        }
    })
}
