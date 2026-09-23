// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use example_tester::{Client, Server, TestKind, assert_logs_eq_to_golden, run_test};
use fidl::prelude::*;
use fidl_examples_keyvaluestore_usegenericvalues::StoreMarker;

use fuchsia_component_test::{ChildRef, RealmBuilder};

async fn test_write_items_twice(
    test_name: &str,
    set_overwrite: bool,
    set_concat: bool,
) -> Result<(), Error> {
    let client = Client::new(test_name, "#meta/keyvaluestore_usegenericvalues_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_usegenericvalues_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder.set_config_value(&client, "set_overwrite_option", set_overwrite.into()).await?;
            builder.set_config_value(&client, "set_concat_option", set_concat.into()).await?;
            builder.set_config_value(&client, "write_bytes", vec!["foo", "bar"].into()).await?;
            builder.set_config_value(&client, "write_strings", vec!["baz", "qux"].into()).await?;
            builder.set_config_value(&client, "write_uint64s", vec![1u64, 101u64].into()).await?;
            builder
                .set_config_value(&client, "write_int64s", vec![-2 as i64, -102 as i64].into())
                .await?;
            builder.set_config_value(&client, "write_uint128s", vec![3u64, 103u64].into()).await?;
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
async fn test_write_items_overwrite_success() -> Result<(), Error> {
    test_write_items_twice("test_write_items_overwrite_success", true, false).await
}

#[fuchsia::test]
async fn test_write_items_concat_success() -> Result<(), Error> {
    test_write_items_twice("test_write_items_concat_success", false, true).await
}

#[fuchsia::test]
async fn test_write_items_default_error() -> Result<(), Error> {
    test_write_items_twice("test_write_items_default_error", false, false).await
}

#[fuchsia::test]
async fn test_write_items_default_success() -> Result<(), Error> {
    let test_name = "test_write_items_default_success";
    let client = Client::new(test_name, "#meta/keyvaluestore_usegenericvalues_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_usegenericvalues_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder.set_config_value(&client, "set_overwrite_option", false.into()).await?;
            builder.set_config_value(&client, "set_concat_option", false.into()).await?;
            builder.set_config_value(&client, "write_bytes", vec!["foo"].into()).await?;
            builder.set_config_value(&client, "write_strings", vec!["baz"].into()).await?;
            builder.set_config_value(&client, "write_uint64s", vec![1u64].into()).await?;
            builder.set_config_value(&client, "write_int64s", vec![-2 as i64].into()).await?;
            builder.set_config_value(&client, "write_uint128s", vec![3u64].into()).await?;
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
