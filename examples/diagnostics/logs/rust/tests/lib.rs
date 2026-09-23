// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_assertions::assert_data_tree;
use diagnostics_reader::{ArchiveReader, Data, Logs, Severity};
use fidl_fuchsia_component::{BinderMarker, BinderProxy};
use fuchsia_async::Task;
use fuchsia_component::client::connect_to_protocol;
use futures::prelude::*;
use std::pin::pin;

#[fuchsia::test]
async fn launch_example_and_read_hello_world() {
    let url = "#meta/rust_logs_example.cm";
    let _: BinderProxy = connect_to_protocol::<BinderMarker>().expect("launched log example");

    let (logs, _task) = listen_to_logs();

    let mut logs = pin!(logs);

    let next = logs.next().await.unwrap();
    assert_eq!(next.metadata.severity, Severity::Info);
    assert_eq!(next.metadata.component_url, Some(url.into()));
    assert_eq!(next.moniker.to_string(), "logs_example");
    assert_data_tree!(next.payload.unwrap(), root:
    {
        "message": {
            "value": "should print",
        }
    });

    let next = logs.next().await.unwrap();

    assert_eq!(next.metadata.severity, Severity::Info);
    assert_eq!(next.metadata.component_url, Some(url.into()));
    assert_eq!(next.moniker.to_string(), "logs_example");
    assert_data_tree!(next.payload.unwrap(), root:
    {
        "message":
        {
            "value": "hello, world!",
        },
        "keys":
        {
            "foo": 1i64,
            "bar": "baz",
        }
    });
}

fn listen_to_logs() -> (impl Stream<Item = Data<Logs>>, Task<()>) {
    let reader = ArchiveReader::logs();
    let (logs, mut errors) = reader.snapshot_then_subscribe().unwrap().split_streams();
    let _check_errors = Task::spawn(async move {
        if let Some(error) = errors.next().await {
            panic!("log testing client encountered an error: {}", error);
        }
    });

    (logs, _check_errors)
}
