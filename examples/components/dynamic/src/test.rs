// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints;
use fidl_fidl_examples_routing_echo::{EchoMarker, EchoRequest};
use fidl_fuchsia_component_sandbox as fsandbox;
use fuchsia_component::client;
use futures::TryStreamExt;

// This waits for a single Echo call, and then checks that the client exits afterwards.
async fn send_single_response_and_exit(
    mut stream: fsandbox::ReceiverRequestStream,
    response: String,
) {
    match stream.try_next().await.unwrap().unwrap() {
        fsandbox::ReceiverRequest::Receive { channel, control_handle: _ } => {
            let server_end = endpoints::ServerEnd::<EchoMarker>::new(channel.into());
            let event = server_end.into_stream().try_next().await.unwrap().unwrap();
            let EchoRequest::EchoString { value, responder } = event;
            match value {
                Some(_) => responder.send(Some(&response)),
                None => responder.send(None),
            }
            .unwrap();
        }
        fsandbox::ReceiverRequest::_UnknownMethod { ordinal, .. } => {
            panic!("Unknown Receiver request: {}", ordinal);
        }
    }
    let next = stream.try_next().await.unwrap();
    assert!(next.is_none(), "{:#?}", next);
}

async fn create_dictionary_with_receiver(
    store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    connector_id: u64,
) -> fsandbox::DictionaryRef {
    let dict_id = id_gen.next();

    store.dictionary_create(dict_id).await.unwrap().unwrap();
    store
        .dictionary_insert(
            dict_id,
            &fsandbox::DictionaryItem {
                key: "fidl.examples.routing.echo.Echo".into(),
                value: connector_id,
            },
        )
        .await
        .unwrap()
        .unwrap();

    let dict_ref = store.export(dict_id).await.unwrap().unwrap();
    match dict_ref {
        fidl_fuchsia_component_sandbox::Capability::Dictionary(d) => d,
        _ => panic!(),
    }
}

async fn create_child_with_specialized_name(
    store: &fsandbox::CapabilityStoreProxy,
    realm: &fidl_fuchsia_component::RealmProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    name: String,
) {
    // Create our connector.
    let (receiver, receiver_stream) =
        endpoints::create_request_stream::<fsandbox::ReceiverMarker>();
    let connector_id = id_gen.next();
    store.connector_create(connector_id, receiver).await.unwrap().unwrap();

    // Create our dictionary
    let dict_ref = create_dictionary_with_receiver(&store, id_gen, connector_id).await;
    log::info!("Populated the dictionary");

    // Create our child and supply it with the protocol in the form of a
    // `Connector` in its input dictionary.
    let collection_ref =
        fidl_fuchsia_component_decl::CollectionRef { name: "test_collection".to_string() };

    let args = fidl_fuchsia_component::CreateChildArgs {
        dictionary: Some(dict_ref),
        ..Default::default()
    };
    let child_decl = fidl_fuchsia_component_decl::Child {
        name: Some(name.clone()),
        url: Some("#meta/dynamic-create-child-child.cm".into()),
        startup: Some(fidl_fuchsia_component_decl::StartupMode::Eager),
        ..Default::default()
    };
    realm.create_child(&collection_ref, &child_decl, args).await.unwrap().unwrap();

    // Now that we created the child, receive our single request from it.
    send_single_response_and_exit(receiver_stream, format!("Hello {}", name)).await;
}

// This test creates a child that will call a single `fuchsia.examples.routing.echo.Echo" function.
// We route the protocol to that child by using CreateChildArgs::Dictionary.
// We then verify that we receive the one call to echo.
#[fuchsia::test]
async fn create_child() {
    let store = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>().unwrap();
    let realm = client::connect_to_protocol::<fidl_fuchsia_component::RealmMarker>().unwrap();
    let id_gen = sandbox::CapabilityIdGenerator::new();

    create_child_with_specialized_name(&store, &realm, &id_gen, "child-a".into()).await;
    create_child_with_specialized_name(&store, &realm, &id_gen, "child-b".into()).await;
}
