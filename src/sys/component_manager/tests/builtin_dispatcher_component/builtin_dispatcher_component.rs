// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use cm_rust::push_box;
use cm_types::Name;
use component_events::events::{Destroyed, Event, EventStream, Started};
use component_events::matcher::EventMatcher;
use component_events::sequence::{EventSequence, Ordering};
use fidl::endpoints::{ClientEnd, DiscoverableProtocolMarker};
use fidl_fidl_examples_routing_echo as fecho;
use fidl_fuchsia_component as fcomponent;
use fuchsia_async;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::new::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, RealmInstance, Ref, Route,
};
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt, select};

/// Implements a component that does two things: attempt to connect to a
/// fidl.examples.routing.echo.Echo protocol, and also implements the same protocol.
async fn echo_server_mock(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    let mut tasks = vec![];

    // In one test we want to test that the automatic offer passthrough works. We do this by
    // testing if a worker component launched by the dispatcher can connect to a protocol. To avoid
    // needing to pull in a different FIDL protocol, we use the same protocol name for both what
    // the worker is expected to provide to the system and what it attempts to connect to.
    let _: Result<ClientEnd<fecho::EchoMarker>, _> = handles.connect_to_protocol();

    let (exit_sender, mut exit_receiver) = mpsc::unbounded();
    fs.dir("svc").add_fidl_service(move |mut stream: fecho::EchoRequestStream| {
        let exit_sender = exit_sender.clone();
        tasks.push(fuchsia_async::Task::local(async move {
            while let Some(Ok(fecho::EchoRequest::EchoString { value, responder })) =
                stream.next().await
            {
                responder.send(value.as_ref().map(|s| &**s)).expect("failed to send echo response");
            }
            // Exit the component once the first connection is closed. This lets us observe that
            // the child gets destroyed on stop as expected.
            exit_sender.unbounded_send(()).unwrap();
        }));
    });

    fs.serve_connection(handles.outgoing_dir)?;
    let mut collect_fut = fs.collect::<()>().fuse();
    select! {
        _ = exit_receiver.next() => (),
        _ = collect_fut => (),
    }
    Ok(())
}

/// Registers a new worker component with realm builder, and returns the URL for the worker and the
/// async task that will handle any start requests for it.
async fn get_worker_url() -> (String, fuchsia_async::Task<()>) {
    let builder = RealmBuilder::new().await.unwrap();
    let child = builder
        .add_local_child("worker", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let mut child_decl = builder.get_component_decl(&child).await.unwrap();
    push_box(
        &mut child_decl.uses,
        cm_rust::UseDecl::Protocol(cm_rust::UseProtocolDecl {
            source: cm_rust::UseSource::Parent,
            source_name: cm_types::Name::new(fecho::EchoMarker::PROTOCOL_NAME).unwrap(),
            source_dictionary: cm_types::RelativePath::dot(),
            target_path: Some(
                format!("/svc/{}", fecho::EchoMarker::PROTOCOL_NAME).parse().unwrap(),
            ),
            numbered_handle: None,
            dependency_type: cm_rust::DependencyType::Strong,
            availability: cm_rust::Availability::Required,
        }),
    );
    builder.replace_realm_decl(child_decl).await.unwrap();
    builder
        .add_capability(cm_rust::CapabilityDecl::Protocol(cm_rust::ProtocolDecl {
            name: cm_types::Name::new(fecho::EchoMarker::PROTOCOL_NAME).unwrap(),
            source_path: Some(
                format!("/svc/{}", fecho::EchoMarker::PROTOCOL_NAME).parse().unwrap(),
            ),
            delivery: cm_rust::DeliveryType::Immediate,
        }))
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(Ref::self_())
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    builder.initialize().await.unwrap()
}

struct Test {
    _worker_task: fuchsia_async::Task<()>,
    realm_instance: RealmInstance,
    event_stream: EventStream,
}

async fn setup_test() -> Test {
    setup_test_with_extra(|builder| async move { builder }.boxed()).await
}

/// Sets up a new realm that has the dispatcher component in it, configured to dispatch new
/// protocol requests for fidl.examples.routing.echo.Echo to a worker created by get_worker_url(),
/// and relies on a local component to connect to and exfiltrate an event stream for the realm.
/// If any additional realm configuring is necessary, it can be done in extra_setup_fn, which is
/// invoked right before the realm is built. The realm is launched in a nested component manager.
async fn setup_test_with_extra(
    extra_setup_fn: impl Fn(RealmBuilder) -> BoxFuture<'static, RealmBuilder>,
) -> Test {
    let (worker_url, _worker_task) = get_worker_url().await;

    let mut builder = RealmBuilder::new().await.unwrap();
    let dispatcher = builder
        .add_child("dispatcher", "fuchsia-builtin://#dispatcher.cm", ChildOptions::new())
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(Ref::dictionary(&dispatcher, "output"))
                .to(Ref::parent()),
        )
        .await
        .unwrap();
    for (capability_name, capability_value) in [
        ("fuchsia.component.dispatcher.Name", fecho::EchoMarker::PROTOCOL_NAME.to_string()),
        ("fuchsia.component.dispatcher.Type", "protocol".to_string()),
        ("fuchsia.component.dispatcher.Target", worker_url),
    ] {
        builder
            .add_capability(cm_rust::CapabilityDecl::Config(cm_rust::ConfigurationDecl {
                name: Name::new(capability_name).unwrap(),
                value: cm_rust::ConfigValue::Single(cm_rust::ConfigSingleValue::String(
                    capability_value.into(),
                )),
            }))
            .await
            .unwrap();
        builder
            .add_route(
                Route::new()
                    .capability(Capability::configuration(capability_name))
                    .from(Ref::self_())
                    .to(&dispatcher),
            )
            .await
            .unwrap();
    }

    let (events_sender, mut events_receiver) = mpsc::unbounded();
    let events_sending_child = builder
        .add_local_child(
            "events_sending_child",
            move |handles| {
                let events_sender = events_sender.clone();
                async move {
                    let proxy: fcomponent::EventStreamProxy = handles.connect_to_protocol()?;
                    events_sender.unbounded_send(proxy).unwrap();
                    Ok(())
                }
                .boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::event_stream("started").path("/svc/fuchsia.component.EventStream"),
                )
                .capability(
                    Capability::event_stream("destroyed")
                        .path("/svc/fuchsia.component.EventStream"),
                )
                .from(Ref::parent())
                .to(&events_sending_child),
        )
        .await
        .unwrap();

    builder = extra_setup_fn(builder).await;

    let realm_instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let event_stream_proxy = events_receiver.next().await.unwrap();
    event_stream_proxy.wait_for_ready().await.unwrap();
    let event_stream = EventStream::new(event_stream_proxy);

    Test { _worker_task, realm_instance, event_stream }
}

/// Tests that connecting to the protocol the dispatcher is configured for results in a usable
/// protocol and causes a worker to start, and that when the worker stops (which happens on
/// protocol disconnect) the worker is destroyed.
#[fuchsia::test]
async fn dispatches_to_one_worker() {
    let Test { realm_instance, event_stream, _worker_task } = setup_test().await;

    let echo_proxy: fecho::EchoProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    assert_eq!(Some("hello".to_string()), echo_proxy.echo_string(Some("hello")).await.unwrap());

    let worker_started = EventMatcher::ok()
        .r#type(Started::TYPE)
        .moniker_regex("dispatcher/workers:worker-[0-9a-f]+");
    let event_stream = EventSequence::new()
        .has_subset(vec![worker_started], Ordering::Ordered)
        .expect_and_giveback(event_stream)
        .await
        .unwrap();

    drop(echo_proxy);

    let worker_destroyed = EventMatcher::ok()
        .r#type(Destroyed::TYPE)
        .moniker_regex("dispatcher/workers:worker-[0-9a-f]+");
    EventSequence::new()
        .has_subset(vec![worker_destroyed], Ordering::Ordered)
        .expect(event_stream)
        .await
        .unwrap();
}

/// Identical to dispatched_to_one_worker, but opens two protocols and observes that two separate
/// workers start and are later destroyed.
#[fuchsia::test]
async fn dispatches_to_two_workers() {
    let Test { realm_instance, event_stream, _worker_task } = setup_test().await;

    let echo_proxy_1: fecho::EchoProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    assert_eq!(Some("hello".to_string()), echo_proxy_1.echo_string(Some("hello")).await.unwrap());

    let echo_proxy_2: fecho::EchoProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    assert_eq!(Some("hello".to_string()), echo_proxy_2.echo_string(Some("hello")).await.unwrap());

    let worker_started = EventMatcher::ok()
        .r#type(Started::TYPE)
        .moniker_regex("dispatcher/workers:worker-[0-9a-f]+");
    let worker_destroyed = EventMatcher::ok()
        .r#type(Destroyed::TYPE)
        .moniker_regex("dispatcher/workers:worker-[0-9a-f]+");
    let event_stream = EventSequence::new()
        .has_subset(vec![worker_started.clone(), worker_started], Ordering::Ordered)
        .expect_and_giveback(event_stream)
        .await
        .unwrap();

    drop(echo_proxy_1);

    let event_stream = EventSequence::new()
        .has_subset(vec![worker_destroyed.clone()], Ordering::Ordered)
        .expect_and_giveback(event_stream)
        .await
        .unwrap();

    drop(echo_proxy_2);

    EventSequence::new()
        .has_subset(vec![worker_destroyed], Ordering::Ordered)
        .expect(event_stream)
        .await
        .unwrap();
}

/// Tests that worker components can access capabilities offered to the dispatcher. This is done by
/// adding a static child to provide the echo protocol, offering it to the dispatcher, and
/// confirming that the static child gets started when the dispatcher starts a worker (because the
/// worker connects to it).
#[fuchsia::test]
async fn offer_passthrough_works() {
    let Test { realm_instance, event_stream, _worker_task } =
        setup_test_with_extra(|builder: RealmBuilder| {
            async move {
                let static_echo_mock = builder
                    .add_local_child(
                        "static_echo_server",
                        |h| echo_server_mock(h).boxed(),
                        ChildOptions::new(),
                    )
                    .await
                    .unwrap();
                builder
                    .add_route(
                        Route::new()
                            .capability(Capability::protocol::<fecho::EchoMarker>())
                            .from(&static_echo_mock)
                            .to(Ref::child("dispatcher")),
                    )
                    .await
                    .unwrap();
                builder
            }
            .boxed()
        })
        .await;

    let echo_proxy: fecho::EchoProxy =
        realm_instance.root.connect_to_protocol_at_exposed_dir().unwrap();
    assert_eq!(Some("hello".to_string()), echo_proxy.echo_string(Some("hello")).await.unwrap());

    let static_mock_started =
        EventMatcher::ok().r#type(Started::TYPE).moniker_regex("static_echo_server");
    let worker_started = EventMatcher::ok()
        .r#type(Started::TYPE)
        .moniker_regex("dispatcher/workers:worker-[0-9a-f]+");
    let event_stream = EventSequence::new()
        .has_subset(vec![worker_started, static_mock_started], Ordering::Ordered)
        .expect_and_giveback(event_stream)
        .await
        .unwrap();

    drop(echo_proxy);

    let worker_destroyed = EventMatcher::ok()
        .r#type(Destroyed::TYPE)
        .moniker_regex("dispatcher/workers:worker-[0-9a-f]+");
    EventSequence::new()
        .has_subset(vec![worker_destroyed], Ordering::Ordered)
        .expect(event_stream)
        .await
        .unwrap();
}
