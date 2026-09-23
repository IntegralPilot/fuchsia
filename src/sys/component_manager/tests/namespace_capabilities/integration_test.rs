// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::*;
use component_events::matcher::*;
use fuchsia_component_test::*;

#[fuchsia::test]
async fn component_manager_namespace() {
    // Define the realm inside component manager.
    let builder = RealmBuilder::new().await.unwrap();
    let realm = builder
        .add_child("realm", "#meta/integration-test-root.cm", ChildOptions::new().eager())
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fidl.examples.routing.echo.Echo"))
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .capability(Capability::directory("test-pkg"))
                .from(Ref::parent())
                .to(&realm),
        )
        .await
        .unwrap();

    let component_manager_realm =
        builder.with_nested_component_manager("#meta/component-manager.cm").await.unwrap();

    let echo_server = component_manager_realm
        .add_child("echo_server", "#meta/echo_server.cm", ChildOptions::new())
        .await
        .unwrap();

    component_manager_realm
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fidl.examples.routing.echo.Echo"))
                .from(&echo_server)
                .to(Ref::self_()),
        )
        .await
        .unwrap();

    let instance = component_manager_realm.build().await.unwrap();

    let proxy = instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let mut event_stream = component_events::events::EventStream::new(proxy);

    // Unblock the component_manager.
    instance.start_component_tree().await.unwrap();

    EventMatcher::ok()
        .stop(Some(ExitStatusMatcher::Clean))
        .moniker_regex("realm")
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();
}
