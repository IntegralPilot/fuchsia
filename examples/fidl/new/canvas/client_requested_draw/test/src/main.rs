// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use diagnostics_data::{Data, Logs};
use example_tester::{Client, Server, TestKind, assert_filtered_logs_eq_to_golden, run_test};
use fidl::prelude::*;
use fidl_examples_canvas_clientrequesteddraw::InstanceMarker;

use fuchsia_component_test::{ChildRef, RealmBuilder};

#[fuchsia::test]
async fn test_draw_success() -> Result<(), Error> {
    let test_name = "test_draw_success";
    let client = Client::new(test_name, "#meta/canvas_clientrequesteddraw_client.cm");
    let server = Server::new(test_name, "#meta/canvas_clientrequesteddraw_server.cm");
    let filter = |raw_log: &&Data<Logs>| {
        let msg = raw_log.payload_message().expect("payload not found").properties[0]
            .string()
            .expect("message is not string");

        // C++ warning emitted when killing a thread.
        if msg.contains("libc++abi: terminating") {
            return false;
        }
        true
    };

    run_test(
        InstanceMarker::PROTOCOL_NAME,
        TestKind::ClientAndServer { client: &client, server: &server },
        |builder: RealmBuilder, client: ChildRef| async move {
            builder.init_mutable_config_to_empty(&client).await?;
            builder
                .set_config_value(
                    &client,
                    "script",
                    vec![
                        "-5,0:0,0",
                        "-2,2:5,6",
                        "PUSH",
                        "4,-3:-2,-2",
                        "WAIT",
                        "6,-1:7,0",
                        "7,-8:-4,3",
                        "PUSH",
                        "WAIT",
                    ]
                    .into(),
                )
                .await?;
            Ok::<(RealmBuilder, ChildRef), Error>((builder, client))
        },
        |log_reader| {
            let client_clone = client.clone();
            let server_clone = server.clone();
            async move {
                assert_filtered_logs_eq_to_golden(&log_reader, &client_clone, filter).await;
                assert_filtered_logs_eq_to_golden(&log_reader, &server_clone, filter).await;
            }
        },
    )
    .await
}
