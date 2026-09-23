// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fuchsia_component::client;
use futures::stream::{FusedStream, StreamExt};
use {fidl_fuchsia_component as fcomponent, fidl_fuchsia_component_tests as fctests};

const CHILD_MONIKER: &str = "./looper";
const NUM_CONNECTIONS: u64 = 3;

#[fuchsia::test]
async fn binder() {
    let mut event_stream = EventStream::open().await.unwrap();

    let mut binders = (0..NUM_CONNECTIONS)
        .map(|_| {
            let binder = client::connect_to_protocol::<fcomponent::BinderMarker>()
                .expect("failed to connect to fuchsia.component.Binder");
            binder.take_event_stream()
        })
        .collect::<Vec<_>>();

    // First, assert that a connection to fuchsia.component.Binder triggers a start.
    EventMatcher::ok()
        .r#type(fcomponent::EventType::Started)
        .moniker(CHILD_MONIKER.to_owned())
        .wait::<Started>(&mut event_stream)
        .await
        .expect("failed to observe events");

    // Next, assert that channel is still open.
    for evt_stream in binders.iter_mut() {
        assert!(!evt_stream.is_terminated());
    }

    let shutdowner = client::connect_to_protocol::<fctests::ShutdownerMarker>()
        .expect("failed to connect to fuchsia.component.tests.Shutdowner");
    let () = shutdowner.shutdown().expect("failed to call Shutdown()");
    EventMatcher::ok()
        .r#type(fcomponent::EventType::Stopped)
        .moniker(CHILD_MONIKER.to_owned())
        .wait::<Stopped>(&mut event_stream)
        .await
        .expect("failed to observe events");

    // Channel should be closing now.
    for evt_stream in binders.iter_mut() {
        assert_matches::assert_matches!(evt_stream.next().await, None);
    }
}
