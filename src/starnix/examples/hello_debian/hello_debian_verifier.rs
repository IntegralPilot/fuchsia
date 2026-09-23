// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use assert_matches::assert_matches;
use component_events::events::{EventStream, ExitStatus, Stopped, StoppedPayload};
use component_events::matcher::EventMatcher;
use diagnostics_reader::ArchiveReader;
use fuchsia_component_test::ScopedInstance;
use futures::StreamExt;
use log::info;

/// verify the launch behavior of a hello debian binary
#[derive(Debug, FromArgs)]
struct Options {
    /// name of the collection to launch in
    #[argh(option)]
    collection: String,

    /// name of the child component to create
    #[argh(option)]
    child_name: String,

    /// url of the child component to create
    #[argh(option)]
    child_url: String,

    /// log message substring to wait for indicating the child actually ran
    #[argh(option)]
    expected_log: String,
}

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();

    let Options { collection, child_name, child_url, expected_log } = argh::from_env();
    let moniker = format!("{collection}:{child_name}");

    let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

    let _instance =
        ScopedInstance::new_with_name(child_name.clone(), collection.clone(), child_url.clone())
            .await
            .unwrap();

    info!("waiting for {child_name} to stop...");
    let stopped = EventMatcher::ok().moniker(&moniker).wait::<Stopped>(&mut events).await.unwrap();
    assert_matches!(stopped.result(), Ok(StoppedPayload { status: ExitStatus::Clean, .. }));

    info!("waiting for expected log message that contains `{expected_log}`...");
    loop {
        let message = logs.next().await.unwrap().unwrap();
        if message.msg().unwrap().contains(&expected_log) {
            break;
        }
    }
}
