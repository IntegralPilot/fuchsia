// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::ArchiveReader;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};

const CHILD_URL: &str = "#meta/config_example.cm";

#[fuchsia::test]
async fn inspect_rust() {
    let builder = RealmBuilder::new().await.unwrap();
    let config_component = builder
        .add_child("config_example_replace_none", CHILD_URL, ChildOptions::new().eager())
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
        .add_selector("*/config_example_replace_none:root")
        .with_minimum_schema_count(1)
        .snapshot()
        .await
        .unwrap()
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .unwrap();

    assert_eq!(inspector.children.len(), 1, "selector must return exactly one child");

    // Verify that the published values match the static package definition in ../../rust/BUILD.gn.
    assert_data_tree!(inspector, root: {
        config: {
            greeting: "World",
            delay_ms: 100u64,
        }
    })
}

#[fuchsia::test]
async fn inspect_rust_replace_some_values() {
    let builder = RealmBuilder::new().await.unwrap();
    let config_component = builder
        .add_child("config_example_replace_some", CHILD_URL, ChildOptions::new().eager())
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

    // [START config_load]
    builder.init_mutable_config_from_package(&config_component).await.unwrap();
    // [END config_load]

    // [START config_replace]
    builder.set_config_value(&config_component, "greeting", "Fuchsia".into()).await.unwrap();
    // [END config_replace]

    let _instance = builder.build().await.unwrap();

    let inspector = ArchiveReader::inspect()
        .add_selector("*/config_example_replace_some:root")
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
            greeting: "Fuchsia",
            delay_ms: 100u64,
        }
    })
}

#[fuchsia::test]
async fn inspect_rust_replace_all_packaged_values() {
    let builder = RealmBuilder::new().await.unwrap();
    let config_component = builder
        .add_child("config_example_replace_all_packaged", CHILD_URL, ChildOptions::new().eager())
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

    builder.init_mutable_config_from_package(&config_component).await.unwrap();

    builder.set_config_value(&config_component, "greeting", "Fuchsia".into()).await.unwrap();
    builder.set_config_value(&config_component, "delay_ms", 200u64.into()).await.unwrap();

    let _instance = builder.build().await.unwrap();

    let inspector = ArchiveReader::inspect()
        .add_selector("*/config_example_replace_all_packaged:root")
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
            greeting: "Fuchsia",
            delay_ms: 200u64,
        }
    })
}

// Same test as above except the config is initialized to empty.
#[fuchsia::test]
async fn inspect_rust_set_all_values_when_empty() {
    let builder = RealmBuilder::new().await.unwrap();
    let config_component = builder
        .add_child("config_example_set_all", CHILD_URL, ChildOptions::new().eager())
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

    // [START config_empty]
    builder.init_mutable_config_to_empty(&config_component).await.unwrap();
    // [END config_empty]

    builder.set_config_value(&config_component, "greeting", "Fuchsia".into()).await.unwrap();
    builder.set_config_value(&config_component, "delay_ms", (200 as u64).into()).await.unwrap();

    let _instance = builder.build().await.unwrap();

    let inspector = ArchiveReader::inspect()
        .add_selector("*/config_example_set_all:root")
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
            greeting: "Fuchsia",
            delay_ms: 200u64,
        }
    })
}
