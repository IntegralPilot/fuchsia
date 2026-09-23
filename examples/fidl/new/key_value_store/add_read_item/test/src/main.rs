// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use example_tester::{Client, Server, TestKind, assert_logs_eq_to_golden, run_test};
use fidl::prelude::*;
use fidl_examples_keyvaluestore_addreaditem::StoreMarker;

use fuchsia_component_test::{ChildRef, RealmBuilder};

#[fuchsia::test]
async fn test_read_item_success() -> Result<(), Error> {
    let test_name = "test_read_item_success";
    let client = Client::new(test_name, "#meta/keyvaluestore_addreaditem_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_addreaditem_server.cm");
    let key = "verse_1";

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_from_package(&client).await?;
            builder.set_config_value(&client, "read_items", vec![key].into()).await?;
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
async fn test_read_item_error() -> Result<(), Error> {
    let test_name = "test_read_item_error";
    let client = Client::new(test_name, "#meta/keyvaluestore_addreaditem_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_addreaditem_server.cm");
    let key = "verse_1000";

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_from_package(&client).await?;
            builder.set_config_value(&client, "read_items", vec![key].into()).await?;
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
