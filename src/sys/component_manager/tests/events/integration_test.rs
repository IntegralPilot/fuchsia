// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use cm_types::{LongName, Name, Path};
use component_events::events::*;
use component_events::matcher::*;
use fidl::endpoints::Proxy;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, Ref, Route};
use futures::channel::mpsc;
use futures::{FutureExt, SinkExt, StreamExt};
use std::collections::BTreeMap;

// TODO(https://fxbug.dev/42172627): Deduplicate this function. It is used in other CM integration tests
async fn start_nested_cm_and_wait_for_clean_stop(root_url: &str, moniker_to_wait_on: &str) {
    let builder = RealmBuilder::new().await.unwrap();
    let root = builder.add_child("root", root_url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::protocol_by_name("fuchsia.process.Launcher"))
                .capability(Capability::event_stream("started").with_scope(&root))
                .capability(Capability::event_stream("stopped").with_scope(&root))
                .capability(Capability::event_stream("destroyed").with_scope(&root))
                .capability(Capability::event_stream("capability_requested").with_scope(&root))
                .from(Ref::parent())
                .to(&root),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let proxy = instance.root.connect_to_protocol_at_exposed_dir().unwrap();

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
async fn from_framework_should_not_work() {
    let root_url = "#meta/async_reporter.cm";
    let moniker_to_wait_on = "./root";
    let builder = RealmBuilder::new().await.unwrap();
    let root = builder.add_child("root", root_url, ChildOptions::new().eager()).await.unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink"))
                .capability(Capability::event_stream("started").with_scope(&root))
                .capability(Capability::event_stream("stopped").with_scope(&root))
                .capability(Capability::event_stream("destroyed").with_scope(&root))
                .capability(Capability::event_stream("capability_requested").with_scope(&root))
                .from(Ref::framework())
                .to(&root),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let proxy = instance.root.connect_to_protocol_at_exposed_dir().unwrap();

    let mut event_stream = EventStream::new(proxy);

    instance.start_component_tree().await.unwrap();

    // Expect the component to stop
    EventMatcher::ok()
        .stop(Some(ExitStatusMatcher::AnyCrash))
        .moniker(moniker_to_wait_on)
        .wait::<Stopped>(&mut event_stream)
        .await
        .unwrap();
}

#[fuchsia::test]
async fn async_event_source_test() {
    start_nested_cm_and_wait_for_clean_stop("#meta/async_reporter.cm", "./root").await;
}

#[fuchsia::test]
async fn scoped_events_test() {
    start_nested_cm_and_wait_for_clean_stop("#meta/echo_realm.cm", "./root/echo_reporter").await;
}

#[fuchsia::test]
async fn realm_offered_event_source_test() {
    start_nested_cm_and_wait_for_clean_stop(
        "#meta/realm_offered_root.cm",
        "./root/nested_realm/reporter",
    )
    .await;
}

#[fuchsia::test]
async fn nested_event_source_test() {
    start_nested_cm_and_wait_for_clean_stop("#meta/nested_reporter.cm", "./root").await;
}

#[fuchsia::test]
async fn event_capability_requested() {
    start_nested_cm_and_wait_for_clean_stop("#meta/capability_requested_root.cm", "./root").await;
}

#[derive(Debug)]
enum EventOrDirRequest {
    DirRequest(fio::DirectoryRequest),
    Event(fcomponent::Event),
}

/// Adds a child to the given realm builder. The child's manifest contains the given use
/// declaration, and the realm's manifest contains the given offer declaration. The child will
/// connect to its event stream, and then report any incoming events and any incoming requests to
/// its outgoing directory over the returned mpsc receiver.
async fn set_up_capability_requested_realm(
    builder: &RealmBuilder,
    offer: cm_rust::offer::OfferDecl,
    use_: cm_rust::UseDecl,
) -> mpsc::UnboundedReceiver<EventOrDirRequest> {
    let (events_sender, events_receiver) = mpsc::unbounded();
    let event_receiver = builder
        .add_local_child(
            "event_receiver",
            move |h| {
                let mut events_sender = events_sender.clone();
                async move {
                    let event_stream_proxy =
                        h.connect_to_protocol::<fcomponent::EventStreamProxy>().unwrap();

                    let mut outgoing_dir_request_stream = h.outgoing_dir.into_stream();
                    let mut events_sender_clone = events_sender.clone();
                    let scope = fasync::Scope::new();
                    let _task = scope.spawn(async move {
                        while let Some(Ok(dir_request)) = outgoing_dir_request_stream.next().await {
                            events_sender_clone
                                .send(EventOrDirRequest::DirRequest(dir_request))
                                .await
                                .unwrap();
                        }
                    });

                    loop {
                        let next_events = event_stream_proxy.get_next().await.unwrap();
                        for event in next_events {
                            events_sender.send(EventOrDirRequest::Event(event)).await.unwrap();
                        }
                    }
                }
                .boxed()
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();

    let mut realm_decl = builder.get_realm_decl().await.unwrap();
    cm_rust::push_box(&mut realm_decl.offers, offer);
    builder.replace_realm_decl(realm_decl).await.unwrap();

    let mut child_decl = builder.get_component_decl(&event_receiver).await.unwrap();
    cm_rust::push_box(&mut child_decl.uses, use_);
    builder.replace_component_decl(&event_receiver, child_decl).await.unwrap();

    events_receiver
}

/// Confirms that a component that's configured to receive capability requests through its event
/// stream will get those requests through the event stream.
#[fuchsia::test]
async fn receive_protocol_through_capability_requested() {
    let mut filter = BTreeMap::new();
    filter.insert("name".to_string(), cm_rust::DictionaryValue::Str("example-name".to_string()));
    let builder = RealmBuilder::new().await.unwrap();
    let mut events_receiver = set_up_capability_requested_realm(
        &builder,
        cm_rust::offer::OfferDecl::EventStream(Box::new(cm_rust::offer::OfferEventStreamDecl {
            source: cm_rust::offer::OfferSource::Parent,
            scope: None,
            source_name: Name::new("capability_requested").unwrap(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("event_receiver").unwrap(),
                collection: None,
            }),
            target_name: Name::new("capability_requested").unwrap(),
            availability: cm_rust::Availability::Required,
        })),
        cm_rust::UseDecl::EventStream(Box::new(cm_rust::UseEventStreamDecl {
            source_name: Name::new("capability_requested").unwrap(),
            source: cm_rust::UseSource::Parent,
            scope: None,
            target_path: Path::new("/svc/fuchsia.component.EventStream").unwrap(),
            filter: Some(filter),
            availability: cm_rust::Availability::Required,
        })),
    )
    .await;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("example-name"))
                .from(Ref::child("event_receiver"))
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let proxy = instance
        .root
        .connect_to_named_protocol_at_exposed_dir::<fcomponent::RealmMarker>("example-name")
        .unwrap();

    let what_happened = events_receiver.next().await.unwrap();
    match what_happened {
        EventOrDirRequest::Event(fcomponent::Event {
            header: Some(header),
            payload:
                Some(fcomponent::EventPayload::CapabilityRequested(
                    fcomponent::CapabilityRequestedPayload {
                        name: Some(name),
                        capability: Some(channel),
                        ..
                    },
                )),
            ..
        }) if header.event_type == Some(fcomponent::EventType::CapabilityRequested)
            && header.moniker == Some(".".to_string())
            && name == "example-name".to_string() =>
        {
            assert_eq!(
                channel.koid().unwrap(),
                proxy.as_channel().as_ref().basic_info().unwrap().related_koid,
            );
        }
        something_else => panic!("something unexpected happened: {something_else:?}"),
    }
}

/// When a component is configured to receive one capability through its event stream, other
/// capability requests should still be delivered over its outgoing directory.
#[fuchsia::test]
async fn receive_protocol_through_outgoing_dir_when_outside_filter() {
    let mut filter = BTreeMap::new();
    filter.insert(
        "name".to_string(),
        cm_rust::DictionaryValue::Str("different-example-name".to_string()),
    );
    let builder = RealmBuilder::new().await.unwrap();
    let mut events_receiver = set_up_capability_requested_realm(
        &builder,
        cm_rust::offer::OfferDecl::EventStream(Box::new(cm_rust::offer::OfferEventStreamDecl {
            source: cm_rust::offer::OfferSource::Parent,
            scope: None,
            source_name: Name::new("capability_requested").unwrap(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("event_receiver").unwrap(),
                collection: None,
            }),
            target_name: Name::new("capability_requested").unwrap(),
            availability: cm_rust::Availability::Required,
        })),
        cm_rust::UseDecl::EventStream(Box::new(cm_rust::UseEventStreamDecl {
            source_name: Name::new("capability_requested").unwrap(),
            source: cm_rust::UseSource::Parent,
            scope: None,
            target_path: Path::new("/svc/fuchsia.component.EventStream").unwrap(),
            filter: Some(filter),
            availability: cm_rust::Availability::Required,
        })),
    )
    .await;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("example-name"))
                .from(Ref::child("event_receiver"))
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    let instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let _proxy = instance
        .root
        .connect_to_named_protocol_at_exposed_dir::<fcomponent::RealmMarker>("example-name")
        .unwrap();

    let what_happened = events_receiver.next().await.unwrap();
    match what_happened {
        EventOrDirRequest::DirRequest(fio::DirectoryRequest::Open { path, .. }) => {
            assert_eq!(path, "svc/example-name");
        }
        something_else => panic!("something unexpected happened: {something_else:?}"),
    }
}

/// When a component receives a capability through an event stream that has a scope applied to it,
/// the target moniker of the route should have the scope's prefix stripped from it.
#[fuchsia::test]
async fn smaller_scope_impacts_moniker() {
    let mut filter = BTreeMap::new();
    filter.insert("name".to_string(), cm_rust::DictionaryValue::Str("example-name".to_string()));

    let builder = RealmBuilder::new().await.unwrap();
    let protocol_consumer_parent = builder
        .add_child_realm("protocol_consumer_parent", ChildOptions::new().eager())
        .await
        .unwrap();
    protocol_consumer_parent
        .add_local_child(
            "protocol_consumer",
            |h| {
                async move {
                    let _proxy =
                        h.connect_to_named_protocol::<fcomponent::RealmProxy>("example-name");
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();

    let mut events_receiver = set_up_capability_requested_realm(
        &builder,
        cm_rust::offer::OfferDecl::EventStream(Box::new(cm_rust::offer::OfferEventStreamDecl {
            source: cm_rust::offer::OfferSource::Parent,
            scope: Some(Box::new([cm_rust::EventScope::Child(cm_rust::ChildRef {
                name: LongName::new("protocol_consumer_parent").unwrap(),
                collection: None,
            })])),
            source_name: Name::new("capability_requested").unwrap(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("event_receiver").unwrap(),
                collection: None,
            }),
            target_name: Name::new("capability_requested").unwrap(),
            availability: cm_rust::Availability::Required,
        })),
        cm_rust::UseDecl::EventStream(Box::new(cm_rust::UseEventStreamDecl {
            source_name: Name::new("capability_requested").unwrap(),
            source: cm_rust::UseSource::Parent,
            scope: None,
            target_path: Path::new("/svc/fuchsia.component.EventStream").unwrap(),
            filter: Some(filter),
            availability: cm_rust::Availability::Required,
        })),
    )
    .await;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("example-name"))
                .from(Ref::child("event_receiver"))
                .to(Ref::child("protocol_consumer_parent")),
        )
        .await
        .unwrap();
    protocol_consumer_parent
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("example-name"))
                .from(Ref::parent())
                .to(Ref::child("protocol_consumer")),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    let what_happened = events_receiver.next().await.unwrap();
    match what_happened {
        EventOrDirRequest::Event(fcomponent::Event {
            header: Some(header),
            payload:
                Some(fcomponent::EventPayload::CapabilityRequested(
                    fcomponent::CapabilityRequestedPayload {
                        name: Some(name),
                        capability: Some(_channel),
                        ..
                    },
                )),
            ..
        }) if header.event_type == Some(fcomponent::EventType::CapabilityRequested)
            && header.moniker == Some("protocol_consumer".to_string())
            && name == "example-name".to_string() =>
        {
            // Success!
        }
        something_else => panic!("something unexpected happened: {something_else:?}"),
    }
}

/// When one component attempts to access a capability provided by another, the provider is
/// configured to accept capability requests over an event stream, and the requesting component is
/// out of scope according to the event stream, then the capability request should not be delivered
/// to the provider.
#[fuchsia::test]
async fn out_of_scope_is_not_delivered() {
    let mut filter = BTreeMap::new();
    filter.insert("name".to_string(), cm_rust::DictionaryValue::Str("example-name".to_string()));

    let builder = RealmBuilder::new().await.unwrap();
    builder
        .add_local_child(
            "protocol_consumer",
            |h| {
                async move {
                    let _proxy =
                        h.connect_to_named_protocol::<fcomponent::RealmProxy>("example-name");
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();

    let mut events_receiver = set_up_capability_requested_realm(
        &builder,
        cm_rust::offer::OfferDecl::EventStream(Box::new(cm_rust::offer::OfferEventStreamDecl {
            source: cm_rust::offer::OfferSource::Parent,
            scope: Some(Box::new([cm_rust::EventScope::Child(cm_rust::ChildRef {
                name: LongName::new("event_receiver").unwrap(),
                collection: None,
            })])),
            source_name: Name::new("capability_requested").unwrap(),
            target: cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                name: LongName::new("event_receiver").unwrap(),
                collection: None,
            }),
            target_name: Name::new("capability_requested").unwrap(),
            availability: cm_rust::Availability::Required,
        })),
        cm_rust::UseDecl::EventStream(Box::new(cm_rust::UseEventStreamDecl {
            source_name: Name::new("capability_requested").unwrap(),
            source: cm_rust::UseSource::Parent,
            scope: None,
            target_path: Path::new("/svc/fuchsia.component.EventStream").unwrap(),
            filter: Some(filter),
            availability: cm_rust::Availability::Required,
        })),
    )
    .await;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol_by_name("example-name"))
                .from(Ref::child("event_receiver"))
                .to(Ref::child("protocol_consumer")),
        )
        .await
        .unwrap();
    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();

    let mut next_fut = events_receiver.next().fuse();
    let mut timer_fut = std::pin::pin!(fasync::Timer::new(std::time::Duration::from_millis(5000)));
    futures::select!(
        what_happened = next_fut => panic!("something unexpected happened: {what_happened:?}"),
        _ = timer_fut => (),
    );
}
