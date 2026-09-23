// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framework::capabilities;
use crate::model::testing::out_dir::OutDir;
use crate::model::testing::routing_test_helpers::RoutingTestBuilder;
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::debug_route_sandbox_path;
use ::routing_test_helpers::dictionary::CommonDictionaryTest;
use ::routing_test_helpers::{CheckUse, ExpectedResult};
use assert_matches::assert_matches;
use capability_source::{CapabilitySource, InternalCapability, VoidSource};
use cm_rust::offer::*;
use cm_rust::*;
use cm_rust_testing::*;
use fidl::endpoints::{self, ServerEnd};
use fidl_fidl_examples_routing_echo::EchoMarker;
use fidl_fuchsia_component_runtime as fruntime;
use fuchsia_async as fasync;
use fuchsia_component::runtime;
use futures::{FutureExt, StreamExt};
use moniker::Moniker;
use routing_test_helpers::RoutingTestModel;
use zx_status::Status;

#[fuchsia::test]
async fn use_protocol_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_use_protocol_from_dictionary().await
}

#[fuchsia::test]
async fn use_protocol_from_dictionary_not_a_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_use_protocol_from_dictionary_not_a_dictionary()
        .await
}

#[fuchsia::test]
async fn use_protocol_from_dictionary_not_used() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_use_protocol_from_dictionary_not_used()
        .await
}

#[fuchsia::test]
async fn use_protocol_from_dictionary_not_found() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_use_protocol_from_dictionary_not_found()
        .await
}

#[fuchsia::test]
async fn use_directory_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_use_directory_from_dictionary().await
}

#[fuchsia::test]
async fn expose_directory_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_expose_directory_from_dictionary().await
}

#[fuchsia::test]
async fn use_protocol_from_nested_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_use_protocol_from_nested_dictionary()
        .await
}

#[fuchsia::test]
async fn offer_protocol_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_offer_protocol_from_dictionary().await
}

#[fuchsia::test]
async fn offer_protocol_from_dictionary_not_found() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_offer_protocol_from_dictionary_not_found()
        .await
}

#[fuchsia::test]
async fn offer_protocol_from_dictionary_to_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_offer_protocol_from_dictionary_to_dictionary()
        .await
}

#[fuchsia::test]
async fn offer_protocol_from_nested_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_offer_protocol_from_nested_dictionary()
        .await
}

#[fuchsia::test]
async fn expose_protocol_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_expose_protocol_from_dictionary().await
}

#[fuchsia::test]
async fn expose_protocol_from_dictionary_not_found() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_expose_protocol_from_dictionary_not_found()
        .await
}

#[fuchsia::test]
async fn expose_protocol_from_nested_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_expose_protocol_from_nested_dictionary()
        .await
}

#[fuchsia::test]
async fn dictionary_in_exposed_dir() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_dictionary_in_exposed_dir().await
}

#[fuchsia::test]
async fn offer_dictionary_to_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_offer_dictionary_to_dictionary().await
}

#[fuchsia::test]
async fn use_from_dictionary_availability_invalid() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_use_from_dictionary_availability_invalid()
        .await
}

#[fuchsia::test]
async fn offer_from_dictionary_availability_invalid() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_offer_from_dictionary_availability_invalid()
        .await
}

#[fuchsia::test]
async fn expose_from_dictionary_availability_attenuated() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_expose_from_dictionary_availability_attenuated()
        .await
}

#[fuchsia::test]
async fn expose_from_dictionary_availability_invalid() {
    CommonDictionaryTest::<RoutingTestBuilder>::new()
        .test_expose_from_dictionary_availability_invalid()
        .await
}

#[fuchsia::test]
async fn use_from_void_dictionary() {
    let use_decl = UseBuilder::protocol()
        .name("A")
        .from_dictionary("dict")
        .availability(Availability::Optional)
        .build();
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .offer(
                    OfferBuilder::dictionary()
                        .name("dict")
                        .source(OfferSource::Void)
                        .target_static_child("leaf")
                        .availability(Availability::Optional),
                )
                .child_default("leaf")
                .build(),
        ),
        ("leaf", ComponentDeclBuilder::new().use_(use_decl.clone()).build()),
    ];

    let test = RoutingTestBuilder::new("root", components).build().await;
    let leaf = test.model.root().find_and_maybe_resolve(&"leaf".parse().unwrap()).await.unwrap();

    let res = debug_route_sandbox_path(&leaf, &use_decl).await;
    assert_matches!(
        res,
        Ok(CapabilitySource::Void(VoidSource {
            moniker,
            capability: InternalCapability::Dictionary(name)
        })) if moniker == Moniker::root() && name == "dict"
    );
}

#[fuchsia::test]
async fn use_from_void_nested_dictionary() {
    let use_decl = UseBuilder::protocol()
        .name("A")
        .from_dictionary("outer/inner")
        .availability(Availability::Optional)
        .build();
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .dictionary_default("outer")
                .offer(
                    OfferBuilder::dictionary()
                        .name("outer")
                        .source(OfferSource::Self_)
                        .target_static_child("leaf")
                        .availability(Availability::Optional),
                )
                .offer(
                    OfferBuilder::dictionary()
                        .name("inner")
                        .source_static_child("child")
                        .target(OfferTarget::Capability("outer".parse().unwrap()))
                        .availability(Availability::Optional),
                )
                .child_default("leaf")
                .child_default("child")
                .build(),
        ),
        (
            "child",
            ComponentDeclBuilder::new()
                .expose(
                    ExposeBuilder::dictionary()
                        .name("inner")
                        .source(ExposeSource::Void)
                        .availability(Availability::Optional),
                )
                .build(),
        ),
        ("leaf", ComponentDeclBuilder::new().use_(use_decl.clone()).build()),
    ];

    let test = RoutingTestBuilder::new("root", components).build().await;
    let leaf = test.model.root().find_and_maybe_resolve(&"leaf".parse().unwrap()).await.unwrap();

    let res = debug_route_sandbox_path(&leaf, &use_decl).await;
    assert_matches!(
        res,
        Ok(CapabilitySource::Void(VoidSource {
            moniker,
            capability: InternalCapability::Dictionary(name)
        })) if moniker == "child".parse().unwrap() && name == "inner"
    );
}

#[fuchsia::test]
async fn test_dictionary_from_program() {
    // Tests a dictionary that is backed by the program.

    const ROUTER_PATH: &str = "/svc/fuchsia.component.runtime.DictionaryRouter";
    let components = vec![
        (
            "root",
            ComponentDeclBuilder::new()
                .capability(CapabilityBuilder::dictionary().name("dict").path(ROUTER_PATH))
                .offer(
                    OfferBuilder::dictionary()
                        .name("dict")
                        .source(OfferSource::Self_)
                        .target_static_child("leaf"),
                )
                .child_default("leaf")
                .build(),
        ),
        (
            "leaf",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::protocol().name("A").from_dictionary("dict"))
                .build(),
        ),
    ];
    let test = RoutingTestBuilder::new("root", components).build().await;

    let (capabilities_proxy, server) = endpoints::create_proxy::<fruntime::CapabilitiesMarker>();
    let weak_root = test.model.root().as_weak();
    let _capabilities_task = fasync::Task::spawn(async move {
        capabilities::serve(server.into_channel(), weak_root.clone(), weak_root.clone())
            .await
            .unwrap();
    });

    // Create a dictionary with a Sender at "A" for the Echo protocol.
    let dictionary = runtime::Dictionary::new_with_proxy(capabilities_proxy.clone()).await;
    let (connector, mut receiver) =
        runtime::Connector::new_with_proxy(capabilities_proxy.clone()).await;
    dictionary.insert("A", connector).await;

    // Serve the Echo protocol from the Receiver.
    let _receiver_task = fasync::Task::spawn(async move {
        let scope = fasync::Scope::new();
        while let Some(channel) = receiver.next().await {
            let channel: ServerEnd<EchoMarker> = channel.into();
            scope.spawn(OutDir::echo_protocol_fn(channel.into_stream()));
        }
        scope.join().await
    });

    // Serve the Router protocol from the root's out dir. Its implementation calls Dictionary/Clone
    // and returns the handle.
    let mut root_out_dir = OutDir::new();
    let dictionary_clone = dictionary.clone();
    root_out_dir.add_entry(
        ROUTER_PATH.parse().unwrap(),
        vfs::service::endpoint(move |scope, channel| {
            let server_end: ServerEnd<fruntime::DictionaryRouterMarker> =
                channel.into_zx_channel().into();
            let router_receiver =
                runtime::DictionaryRouterReceiver { stream: server_end.into_stream() };
            let dictionary = dictionary_clone.clone();
            scope.spawn(router_receiver.handle_with(move |_request, _instance_token| {
                futures::future::ready(Ok(Some(dictionary.clone()))).boxed()
            }));
        }),
    );
    test.mock_runner.add_host_fn("test:///root", root_out_dir.host_fn());

    // Using "A" from the dictionary should succeed.
    for _ in 0..3 {
        test.check_use(
            "leaf".try_into().unwrap(),
            CheckUse::Protocol {
                path: "/svc/A".parse().unwrap(),
                expected_res: ExpectedResult::Ok,
            },
        )
        .await;
    }

    // Now, remove "A" from the dictionary. Using "A" this time should fail.
    dictionary.remove("A").await;
    test.check_use(
        "leaf".try_into().unwrap(),
        CheckUse::Protocol {
            path: "/svc/A".parse().unwrap(),
            expected_res: ExpectedResult::Err(Status::NOT_FOUND),
        },
    )
    .await;
}

#[fuchsia::test]
async fn use_dictionary_with_service_from_collection() {
    // Tests using a dictionary through an aggregate router for an offered
    // service.
    //
    // This is a regression test for https://fxbug.dev/471268867.

    let components = vec![
        (
            "a",
            ComponentDeclBuilder::new()
                .dictionary_default("my_dictionary")
                .child_default("b")
                .offer(
                    OfferBuilder::dictionary()
                        .name("my_dictionary")
                        .source(OfferSource::Self_)
                        .target_static_child("b"),
                )
                .offer(
                    OfferBuilder::service()
                        .name("foo")
                        .source(OfferSource::Collection("coll".parse().unwrap()))
                        .target_capability("my_dictionary"),
                )
                .collection_default("coll")
                .use_(
                    UseBuilder::protocol()
                        .source(UseSource::Framework)
                        .name("fuchsia.component.Realm"),
                )
                .build(),
        ),
        (
            "b",
            ComponentDeclBuilder::new()
                .use_(UseBuilder::dictionary().name("my_dictionary").path("/my_dictionary"))
                .build(),
        ),
        (
            "child",
            ComponentDeclBuilder::new()
                .expose(ExposeBuilder::service().name("foo").source(ExposeSource::Self_))
                .capability(CapabilityBuilder::service().name("foo").path("/svc/foo.service"))
                .build(),
        ),
    ];
    let test = RoutingTestBuilder::new("a", components).build().await;

    test.create_dynamic_child(&Moniker::root(), "coll", ChildBuilder::new().name("child")).await;
    test.start_instance_and_wait_start(&["coll:child"].try_into().unwrap())
        .await
        .expect("failed to start `child`");

    test.check_open_node(["b"].try_into().unwrap(), "/my_dictionary/foo".parse().unwrap()).await;
}

#[fuchsia::test]
async fn use_runner_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_use_runner_from_dictionary().await
}

#[fuchsia::test]
async fn offer_runner_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_offer_runner_from_dictionary().await
}

#[fuchsia::test]
async fn offer_directory_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_offer_directory_from_dictionary().await
}

#[fuchsia::test]
async fn use_config_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_use_config_from_dictionary().await
}

#[fuchsia::test]
async fn offer_config_from_dictionary() {
    CommonDictionaryTest::<RoutingTestBuilder>::new().test_offer_config_from_dictionary().await
}
