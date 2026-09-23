// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use component_events::events::{EventStream, ExitStatus, Stopped, StoppedPayload};
use component_events::matcher::EventMatcher;
use diagnostics_reader::ArchiveReader;
use fuchsia_component_test::ScopedInstance;
use futures::StreamExt;
use log::info;

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();
    let collection = "linux_children";
    let child_name = "hello_starnix";
    let url = "hello_starnix#meta/hello_starnix.cm";
    let moniker = format!("{collection}:{child_name}");

    let mut logs = ArchiveReader::logs().snapshot_then_subscribe().unwrap();

    let _instance = ScopedInstance::new_with_name(child_name.into(), collection.into(), url.into())
        .await
        .unwrap();

    info!("waiting for hello_starnix to stop...");
    let stopped = EventMatcher::ok().moniker(&moniker).wait::<Stopped>(&mut events).await.unwrap();
    assert_matches!(stopped.result(), Ok(StoppedPayload { status: ExitStatus::Clean, .. }));

    info!("waiting for expected log message...");
    loop {
        let message = logs.next().await.unwrap().unwrap();
        if message.msg().unwrap().starts_with("hello starnix") {
            break;
        }
    }
}
