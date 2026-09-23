// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fidl_fuchsia_component as fcomponent;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};

async fn start_nested_cm_and_wait_for_clean_stop(root_url: &str, moniker_to_wait_on: &str) {
    let builder = RealmBuilder::new().await.unwrap();
    let root = builder.add_child("root", root_url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .capability(Capability::event_stream("destroyed"))
                .capability(Capability::event_stream("started"))
                .capability(Capability::event_stream("capability_requested"))
                .from(Ref::parent())
                .to(&root),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    let proxy: fcomponent::EventStreamProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    proxy.wait_for_ready().await.unwrap();

    let mut event_stream = EventStream::new(proxy);

    instance.start_component_tree().await.unwrap();

    // Expect the component to stop
    EventMatcher::ok()
        .stop(Some(ExitStatusMatcher::Clean))
        .moniker(moniker_to_wait_on)
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();
}

#[fuchsia::test]
async fn component_manager_exposes_inspect() {
    start_nested_cm_and_wait_for_clean_stop(
        "#meta/component-manager-inspect.cm",
        "./root/reporter",
    )
    .await;
}
