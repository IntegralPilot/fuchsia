// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use example_tester::{Client, Server, TestKind, assert_logs_eq_to_golden, run_test};
use fidl::prelude::*;
use fidl_examples_keyvaluestore_supporttrees::StoreMarker;

use fuchsia_component_test::{ChildRef, RealmBuilder};

#[fuchsia::test]
async fn test_write_success() -> Result<(), Error> {
    let test_name = "test_write_success";
    let client = Client::new(test_name, "#meta/keyvaluestore_supporttrees_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_supporttrees_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder
                .set_config_value(&client, "write_items", vec!["verse_1", "verse_2"].into())
                .await?;
            builder
                .set_config_value(
                    &client,
                    "write_nested",
                    vec!["rest_of_poem\nverse_3\nverse_4"].into(),
                )
                .await?;
            builder.set_config_value(&client, "write_null", vec!["null_verse"].into()).await?;
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
async fn test_empty_nested() -> Result<(), Error> {
    let test_name = "test_empty_nested";
    let client = Client::new(test_name, "#meta/keyvaluestore_supporttrees_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_supporttrees_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder.set_config_value(&client, "write_items", Vec::<&str>::new().into()).await?;
            builder.set_config_value(&client, "write_nested", vec!["invalid_empty"].into()).await?;
            builder.set_config_value(&client, "write_null", Vec::<&str>::new().into()).await?;
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
