// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{Destroyed, Event, EventStream, Started};
use component_events::matcher::{EventMatcher, ExitStatusMatcher};
use component_events::sequence::{EventSequence, Ordering};
use fuchsia_component_test::ScopedInstance;
use log::info;
/// This test invokes components which don't stop when they're told to. We
/// still expect them to be stopped when the system kills them.
#[fuchsia::test]
async fn test_stop_timeouts() {
    let started_event_stream = EventStream::open_at_path("/events/started").await.unwrap();

    let event_stream_root = EventStream::open_at_path("/events/stopped_destroyed").await.unwrap();
    let event_stream_custom = EventStream::open_at_path("/events/stopped_destroyed").await.unwrap();
    let event_stream_inherited =
        EventStream::open_at_path("/events/stopped_destroyed").await.unwrap();

    let collection_name = String::from("test-collection");

    let (root_moniker, custom_moniker, inherited_moniker) = {
        // What is going on here? A scoped dynamic instance is created and then
        // dropped. When a the instance is dropped it stops the instance.
        let instance = ScopedInstance::new(
            collection_name.clone(),
            String::from(concat!(
                "fuchsia-pkg://fuchsia.com/elf_runner_lifecycle_test",
                "#meta/lifecycle_timeout_unresponsive_root.cm"
            )),
        )
        .await
        .unwrap();

        // Make sure we start the root component, since it has no runtime, this
        // is sufficient.
        instance.connect_to_binder().unwrap();

        let root_moniker = format!("./{}:{}", collection_name, instance.child_name());
        let custom_timeout_child = format!("{}/custom-timeout-child", root_moniker);
        let inherited_timeout_child = format!("{}/inherited-timeout-child", root_moniker);

        EventSequence::new()
            .all_of(
                vec![
                    EventMatcher::ok().r#type(Started::TYPE).moniker(&root_moniker),
                    EventMatcher::ok().r#type(Started::TYPE).moniker(&custom_timeout_child),
                    EventMatcher::ok().r#type(Started::TYPE).moniker(&inherited_timeout_child),
                ],
                Ordering::Unordered,
            )
            .expect(started_event_stream)
            .await
            .unwrap();

        (root_moniker, custom_timeout_child, inherited_timeout_child)
    };

    info!("All components started");

    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok()
                    .moniker(root_moniker.clone())
                    .stop(Some(ExitStatusMatcher::Clean)),
                EventMatcher::ok().r#type(Destroyed::TYPE).moniker(root_moniker.clone()),
            ],
            // TODO(https://fxbug.dev/42080589): Strengthen ordering
            Ordering::Unordered,
        )
        .expect(event_stream_root)
        .await
        .unwrap();

    info!("Parent exited");

    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok()
                    .moniker(custom_moniker.clone())
                    .stop(Some(ExitStatusMatcher::AnyCrash)),
                EventMatcher::ok().r#type(Destroyed::TYPE).moniker(custom_moniker.clone()),
            ],
            // TODO(https://fxbug.dev/42080589): Strengthen ordering
            Ordering::Unordered,
        )
        .expect(event_stream_custom)
        .await
        .unwrap();

    info!("Custom timeout child exited");

    EventSequence::new()
        .has_subset(
            vec![
                EventMatcher::ok()
                    .moniker(inherited_moniker.clone())
                    .stop(Some(ExitStatusMatcher::AnyCrash)),
                EventMatcher::ok().r#type(Destroyed::TYPE).moniker(inherited_moniker.clone()),
            ],
            // TODO(https://fxbug.dev/42080589): Strengthen ordering
            Ordering::Unordered,
        )
        .expect(event_stream_inherited)
        .await
        .unwrap();

    info!("Inherited timeout child exited");
}
