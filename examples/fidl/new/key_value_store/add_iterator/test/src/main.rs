// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use example_tester::{Client, Server, TestKind, assert_logs_eq_to_golden, run_test};
use fidl::prelude::*;
use fidl_examples_keyvaluestore_additerator::StoreMarker;

use fuchsia_component_test::{ChildRef, RealmBuilder};

async fn test_iteration(test_name: &str, iterate_from: &str) -> Result<(), Error> {
    let client = Client::new(test_name, "#meta/keyvaluestore_additerator_client.cm");
    let server = Server::new(test_name, "#meta/keyvaluestore_additerator_server.cm");

    run_test(
        StoreMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder
                .set_config_value(
                    &client,
                    "write_items",
                    vec!["verse_1", "verse_2", "verse_3", "verse_4"].into(),
                )
                .await?;
            builder.set_config_value(&client, "iterate_from", iterate_from.into()).await?;
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
async fn test_iteration_success() -> Result<(), Error> {
    test_iteration("test_iteration_success", "verse_2").await
}

#[fuchsia::test]
async fn test_iteration_error() -> Result<(), Error> {
    test_iteration("test_iteration_error", "does_not_exist").await
}
