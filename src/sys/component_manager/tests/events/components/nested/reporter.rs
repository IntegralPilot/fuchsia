// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::EventStream;
use component_events::matcher::EventMatcher;
use component_events::sequence::*;

#[fuchsia::main(logging_tags = ["nested_reporter"])]
async fn main() {
    // Track all the starting child components.
    let event_stream = EventStream::open().await.unwrap();

    let realm_proxy =
        fuchsia_component::client::connect_to_protocol::<fidl_fuchsia_component::RealmMarker>()
            .unwrap();
    let mut execution_controllers = vec![];
    for child in ["child_a", "child_b", "child_c"] {
        let (controller_proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_component::ControllerMarker>();
        realm_proxy
            .open_controller(
                &fidl_fuchsia_component_decl::ChildRef {
                    name: child.to_string(),
                    collection: None,
                },
                server_end,
            )
            .await
            .unwrap()
            .unwrap();
        let (execution_controller_proxy, server_end) =
            fidl::endpoints::create_proxy::<fidl_fuchsia_component::ExecutionControllerMarker>();
        controller_proxy.start(Default::default(), server_end).await.unwrap().unwrap();
        execution_controllers.push(execution_controller_proxy);
    }

    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok().moniker("./child_a"),
                EventMatcher::ok().moniker("./child_b"),
                EventMatcher::ok().moniker("./child_c"),
            ],
            Ordering::Unordered,
        )
        .expect(event_stream)
        .await
        .unwrap();
}
