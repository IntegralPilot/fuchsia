// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use example_tester::{Client, Server, TestKind, assert_logs_eq_to_golden, run_test};
use fidl::prelude::*;
use fidl_examples_keyvaluestore_baseline::StoreMarker;

use fuchsia_component_test::{ChildRef, RealmBuilder};

#[fuchsia::test]
async fn test_write_item_success() -> Result<(), Error> {
    let test_name = "test_write_item_success";
    let client = Client::new(test_name, "#meta/keyvaluestore_baseline_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_baseline_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder.set_config_value(&client, "write_items", vec!["verse_1"].into()).await?;
            Ok::<(RealmBuilder, ChildRef), Error>((builder, client))
        },
        |log_reader| {
            let client_clone = client.clone();
            let server_clone = server.clone();
            async move {
                assert_logs_eq_to_golden(&log_reader, &client_clone).await;
                assert_logs_eq_to_golden(&log_reader, &server_clone).await;
            }
        },
    )
    .await
}

async fn test_write_item_invalid(test_name: &str, input: &str) -> Result<(), Error> {
    let client = Client::new(test_name, "#meta/keyvaluestore_baseline_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_baseline_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder.set_config_value(&client, "write_items", vec![input].into()).await?;
            Ok::<(RealmBuilder, ChildRef), Error>((builder, client))
        },
        |log_reader| {
            let client_clone = client.clone();
            let server_clone = server.clone();
            async move {
                assert_logs_eq_to_golden(&log_reader, &client_clone).await;
                assert_logs_eq_to_golden(&log_reader, &server_clone).await;
            }
        },
    )
    .await
}

#[fuchsia::test]
async fn test_write_item_error_invalid_key() -> Result<(), Error> {
    test_write_item_invalid(
        "test_write_item_error_invalid_key",
        // A trailing underscore makes for an invalid key per the rules in
        // keyvaluestore.test.fidl, hence the odd name here.
        "error_invalid_key_",
    )
    .await
}

#[fuchsia::test]
async fn test_write_item_error_invalid_value() -> Result<(), Error> {
    test_write_item_invalid("test_write_item_error_invalid_value", "error_invalid_value").await
}

#[fuchsia::test]
async fn test_write_item_error_already_found() -> Result<(), Error> {
    let test_name = "test_write_item_error_already_found";
    let client = Client::new(test_name, "#meta/keyvaluestore_baseline_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_baseline_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder
                .set_config_value(&client, "write_items", vec!["verse_1", "verse_1"].into())
                .await?;
            Ok::<(RealmBuilder, ChildRef), Error>((builder, client))
        },
        |log_reader| {
            let client_clone = client.clone();
            let server_clone = server.clone();
            async move {
                assert_logs_eq_to_golden(&log_reader, &client_clone).await;
                assert_logs_eq_to_golden(&log_reader, &server_clone).await;
            }
        },
    )
    .await
}
