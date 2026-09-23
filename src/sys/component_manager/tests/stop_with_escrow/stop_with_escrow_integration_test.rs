// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use component_events::events::{EventStream, ExitStatus, Started, Stopped};
use component_events::matcher::EventMatcher;
use fidl_fidl_test_components::{TriggerMarker, TriggerProxy};
use fidl_fuchsia_component::{Event, EventHeader, EventType, StoppedPayload};
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route, ScopedInstanceFactory,
};
use futures::StreamExt;
use futures::future::{self, Either};
use std::pin::pin;
use {
    fidl_fuchsia_component as fcomponent, fidl_fuchsia_process as fprocess, fuchsia_async as fasync,
};

/// Tests a component can stop with a request buffered in its outgoing dir,
/// and that request is handled on the next start (which should be automatic).
#[fuchsia::test]
async fn stop_with_pending_request() {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_root.cm"),
    )
    .await
    .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    // Start the component with an eventpair `USER_0` handle for synchronization.
    let realm = instance
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("failed to connect to RealmQuery");
    let factory = ScopedInstanceFactory::new("coll").with_realm_proxy(realm);
    let instance = factory
        .new_named_instance("stop_with_pending_request", "#meta/stop_with_pending_request.cm")
        .await
        .unwrap();

    let (user_0, user_0_peer) = zx::EventPair::create();
    let execution_controller = instance
        .start_with_args(fcomponent::StartChildArgs {
            numbered_handles: Some(vec![fprocess::HandleInfo {
                handle: user_0_peer.into(),
                id: fuchsia_runtime::HandleType::User0 as u32,
            }]),
            ..Default::default()
        })
        .await
        .unwrap();

    // Queue request.
    let trigger: TriggerProxy = instance.connect_to_protocol_at_exposed_dir().unwrap();
    let call = trigger.run().check().unwrap();

    // Tell the component to stop.
    drop(user_0);

    // Observe that the component has stopped.
    let stop_payload = execution_controller.wait_for_stop().await.unwrap();
    assert_eq!(stop_payload.status.unwrap(), 0);

    // Observe that the component is started without the numbered handle and thus
    // handled our request.
    assert_eq!(&call.await.unwrap(), "hello");
}

/// Tests a component can stop with a request buffered in the framework waiting
/// for the server endpoint, and that request is handled when we write to the
/// client endpoint, which causes the component to be automatically started.
#[fuchsia::test]
async fn stop_with_delivery_on_readable_request() {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_root.cm"),
    )
    .await
    .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    // Start the component and send it a connection request, but no messages
    // on the connection.
    let realm = instance
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("failed to connect to RealmQuery");
    let factory = ScopedInstanceFactory::new("coll").with_realm_proxy(realm);
    let instance = factory
        .new_named_instance(
            "stop_with_delivery_on_readable_request",
            "#meta/stop_with_delivery_on_readable_request.cm",
        )
        .await
        .unwrap();
    let execution_controller = instance.start().await.unwrap();
    let trigger: TriggerProxy = instance.connect_to_protocol_at_exposed_dir().unwrap();

    // Cause the framework to deliver the request to the component.
    assert_eq!(&trigger.run().await.unwrap(), "hello");

    // Wait for the component to stop because the connection stalled.
    let stop_payload = execution_controller.wait_for_stop().await.unwrap();
    assert_eq!(stop_payload.status.unwrap(), 0);

    // Now make a two-way call on the connection again and it should still work
    // (by starting the component again).
    assert_eq!(&trigger.run().await.unwrap(), "hello");
}

/// Tests a component can stop with an escrowed dictionary capability,
/// which will be read back on next start and used to maintain a call counter.
#[fuchsia::test]
async fn stop_with_escrowed_dictionary() {
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new().from_relative_url("#meta/test_root.cm"),
    )
    .await
    .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let event_stream: fcomponent::EventStreamProxy =
        instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    event_stream.wait_for_ready().await.unwrap();
    let mut event_stream = EventStream::new(event_stream);

    // Create the component.
    let realm = instance
        .root
        .connect_to_protocol_at_exposed_dir()
        .expect("failed to connect to RealmQuery");
    let factory = ScopedInstanceFactory::new("coll").with_realm_proxy(realm);
    let instance = factory
        .new_named_instance(
            "stop_with_escrowed_dictionary",
            "#meta/stop_with_escrowed_dictionary.cm",
        )
        .await
        .unwrap();

    // Send first request.
    let trigger: TriggerProxy = instance.connect_to_protocol_at_exposed_dir().unwrap();
    assert_eq!(trigger.run().await.unwrap(), "1");

    // Observe that the component has started then stopped.
    EventMatcher::ok()
        .moniker(instance.moniker())
        .wait::<Started>(&mut event_stream)
        .await
        .expect("failed to observe Started event");
    assert_eq!(
        EventMatcher::ok()
            .moniker(instance.moniker())
            .wait::<Stopped>(&mut event_stream)
            .await
            .expect("failed to observe Stopped event")
            .result()
            .expect("failed to extract Stopped result")
            .status,
        ExitStatus::Clean
    );

    // Send second request, which should start the component again and continue the counter.
    assert_eq!(trigger.run().await.unwrap(), "2");
    EventMatcher::ok()
        .moniker(instance.moniker())
        .wait::<Started>(&mut event_stream)
        .await
        .expect("failed to observe Started event");
    assert_eq!(
        EventMatcher::ok()
            .moniker(instance.moniker())
            .wait::<Stopped>(&mut event_stream)
            .await
            .expect("failed to observe Stopped event")
            .result()
            .expect("failed to extract Stopped result")
            .status,
        ExitStatus::Clean
    );
}

/// Tests a component can idle with a connection to a capability exposed by a
/// dynamic dictionary.
#[fuchsia::test]
async fn stop_with_dynamic_dictionary() {
    let builder = RealmBuilder::new().await.unwrap();

    let stop_with_dynamic_dictionary = builder
        .add_child(
            "stop_with_dynamic_dictionary",
            "#meta/stop_with_dynamic_dictionary.cm",
            ChildOptions::new(),
        )
        .await
        .unwrap();

    // Route the capability exposed by the dynamic dictionary to the test.
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fidl.test.components.Trigger-dynamic"))
                .from(Ref::dictionary(&stop_with_dynamic_dictionary, "bundle"))
                .to(Ref::parent()),
        )
        .await
        .unwrap();

    // Open the event stream before starting the realm, so as to not race with component startup.
    let event_stream = EventStream::open().await.unwrap();

    let instance = builder.build().await.unwrap();
    let moniker =
        format!("realm_builder:{}/stop_with_dynamic_dictionary", instance.root.child_name());
    let mut event_stream = event_stream.filter(|ev| {
        future::ready(matches!(
            &ev.header,
            Some(EventHeader { moniker: Some(m), .. }) if m == &moniker
        ))
    });

    // Connect to protocol which has side effect of starting the component.
    let trigger = instance
        .root
        .connect_to_named_protocol_at_exposed_dir::<TriggerMarker>(
            "fidl.test.components.Trigger-dynamic",
        )
        .unwrap();

    // Wait for the component to start.
    while let Some(ev) = event_stream.next().await {
        if matches!(ev.header, Some(EventHeader { event_type: Some(EventType::Started), .. })) {
            break;
        }
    }

    // Simulate a delay between opening the client channel and sending a
    // request. This should cause the component to go idle.
    assert_matches!(
        event_stream.next().await.unwrap(),
        Event {
            header: Some(EventHeader { event_type: Some(EventType::Stopped), .. }),
            payload: Some(fcomponent::EventPayload::Stopped(StoppedPayload {
                status: Some(0),
                ..
            })),
            ..
        }
    );

    assert_matches!(
        &trigger.run().await,
        Ok(m) if m == "hello from fidl.test.components.Trigger-static"
    );

    // Calling fidl.test.components.Trigger will cause Component Manager to
    // start the component and reconnect the server-end channel to the
    // component, which has a `use` declaration for the escrowed version of both
    // `fuchsia.component.runtime.Receiver` and the Trigger protocols.
    assert_matches!(
        event_stream.next().await.unwrap().header,
        Some(EventHeader { event_type: Some(EventType::Started), .. })
    );

    // The component can be escrowed again by idling the client-end
    // fidl.test.components.Trigger channel.
    assert_matches!(
        event_stream.next().await.unwrap(),
        Event {
            header: Some(EventHeader { event_type: Some(EventType::Stopped), .. }),
            payload: Some(fcomponent::EventPayload::Stopped(StoppedPayload {
                status: Some(0),
                ..
            })),
            ..
        }
    );

    // Ensure there are no missed events.
    let event = pin!(event_stream.next());
    let timeout = pin!(fasync::Timer::new(fasync::MonotonicDuration::from_seconds(2)));
    match futures::future::select(event, timeout).await {
        Either::Left((e, _)) => panic!("unexpectedly event: {:?}", e),
        Either::Right(_) => {}
    }
}
