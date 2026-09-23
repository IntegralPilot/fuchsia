// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fidl_fuchsia_component as fcomponent;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};

#[fuchsia::test]
async fn echo_with_args() {
    run_single_test("#meta/reporter_args.cm").await
}

#[fuchsia::test]
async fn echo_without_args() {
    run_single_test("#meta/reporter_no_args.cm").await
}

async fn run_single_test(url: &str) {
    let builder = RealmBuilder::new().await.unwrap();
    let reporter = builder.add_child("reporter", url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .from(Ref::parent())
                .to(&reporter),
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

    EventMatcher::ok()
        .stop(Some(ExitStatusMatcher::Clean))
        .moniker("./reporter")
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();
}
