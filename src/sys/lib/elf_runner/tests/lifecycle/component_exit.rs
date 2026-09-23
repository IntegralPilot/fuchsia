// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::matcher::ExitStatusMatcher;

use component_events::events::{self as events, Event, EventStream};
use component_events::matcher::EventMatcher;
use component_events::sequence::EventSequence;
use fidl_fidl_test_components as test_protocol;
use fuchsia_component_test::ScopedInstance;

#[fuchsia::test]
async fn test_exit_detection() {
    let event_stream = EventStream::open_at_path("/events/stopped").await.unwrap();
    let collection_name = String::from("test-collection-0");

    let instance = ScopedInstance::new(
        collection_name.clone(),
        String::from("#meta/immediate_exit_component.cm"),
    )
    .await
    .unwrap();

    instance.connect_to_binder().unwrap();

    let target_moniker = ".";

    EventSequence::new()
        .then(EventMatcher::ok().r#type(events::Stopped::TYPE).moniker(target_moniker))
        .expect(event_stream)
        .await
        .unwrap();
}

#[fuchsia::test]
async fn test_exit_after_rendezvous() {
    // Get the event source, install our service injector, and then start the
    // component tree.
    let mut event_stream = EventStream::open_at_path("/events/started_stopped").await.unwrap();
    // Launch the component under test.
    let instance = ScopedInstance::new(
        "test-collection-1".to_string(),
        String::from("#meta/rendezvous_exit_component.cm"),
    )
    .await
    .unwrap();

    instance.connect_to_binder().unwrap();

    let target_moniker = ".";

    // First, ensure that component has started.
    EventMatcher::ok()
        .moniker(target_moniker)
        .wait::<events::Started>(&mut event_stream)
        .await
        .expect("failed to observe events");

    // Rendezvous with the component
    let trigger: test_protocol::TriggerProxy =
        instance.connect_to_protocol_at_exposed_dir().unwrap();
    let result = trigger.run().await.unwrap();
    assert_eq!(result, "Rendezvous complete!");

    // Then, wait to get confirmation that the component under test exited.
    EventMatcher::ok()
        .stop(Some(ExitStatusMatcher::Clean))
        .wait::<events::Stopped>(&mut event_stream)
        .await
        .expect("failed to observe events");
}
