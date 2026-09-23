// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fuchsia_component::{client, server};
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt, TryStreamExt};
use vfs::execution_scope::ExecutionScope;
use vfs::file::vmo::read_only;
use {
    fidl_fidl_examples_routing_echo as fecho, fidl_fidl_test_components as ftest,
    fidl_fuchsia_io as fio, fuchsia_async as fasync,
};

#[fuchsia::test]
async fn protocols() {
    fn path(letter: &str) -> String {
        format!("/svc/fidl.test.components.Trigger-{letter}")
    }

    // See the test's cml to understand how this exercises dictionary routing.
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("a")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered a");
    }
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("b")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered b");
    }
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("c")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered c");
    }
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("d")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered d");
    }
}

#[fuchsia::test]
async fn protocols_use_dictionary() {
    fn path(letter: &str) -> String {
        format!("/svc2/fidl.test.components.Trigger-{letter}")
    }
    // See the test's cml to understand how this exercises dictionary routing.
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("a")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered a");
    }
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("b")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered b");
    }
    {
        let trigger =
            client::connect_to_protocol_at_path::<ftest::TriggerMarker>(&path("c")).unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered c");
    }
    {
        let trigger = client::connect_to_protocol_at_path::<ftest::TriggerMarker>(
            "/svc2/inner/fidl.test.components.Trigger-d",
        )
        .unwrap();
        let out = trigger.run().await.unwrap();
        assert_eq!(&out, "Triggered d");
    }
}

// TODO(https://fxbug.dev/383601465): Add tests for more capability types.

#[fuchsia::test]
async fn use_dictionary() {
    let builder = RealmBuilder::new().await.unwrap();
    let protocol_child = builder
        .add_local_child("protocol_child", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let directory_child = builder
        .add_local_child("directory_child", |h| directory_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
            name: cm_types::Name::new("my_dictionary").unwrap(),
            source_path: None,
        }))
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(&protocol_child)
                .to(Ref::capability("my_dictionary")),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("data").path("/data").rights(fio::R_STAR_DIR))
                .from(&directory_child)
                .to(Ref::capability("my_dictionary")),
        )
        .await
        .unwrap();

    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let handle_forwarder = builder
        .add_local_child(
            "handle_forwarder",
            move |h| {
                let _ = sender.lock().take().unwrap().send(h);
                futures::future::pending().boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("my_dictionary").path("/my_dictionary"))
                .from(Ref::self_())
                .to(&handle_forwarder),
        )
        .await
        .unwrap();

    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let local_handles = receiver.await.unwrap();
    let dictionary_directory = local_handles.clone_from_namespace("my_dictionary").unwrap();
    let echo_proxy =
        client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dictionary_directory)
            .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        echo_proxy.echo_string(Some("Hello, world!")).await.unwrap().unwrap()
    );
    let file_proxy = fuchsia_fs::directory::open_file(
        &dictionary_directory,
        "data/example_file",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap()
    );
}

#[fuchsia::test]
async fn use_dictionary_and_protocol_at_same_path() {
    let builder = RealmBuilder::new().await.unwrap();
    let protocol_child = builder
        .add_local_child("protocol_child", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let protocol_child_2 = builder
        .add_local_child("protocol_child_2", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let directory_child = builder
        .add_local_child("directory_child", |h| directory_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
            name: cm_types::Name::new("my_dictionary").unwrap(),
            source_path: None,
        }))
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(&protocol_child)
                .to(Ref::capability("my_dictionary")),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("data").path("/data").rights(fio::R_STAR_DIR))
                .from(&directory_child)
                .to(Ref::capability("my_dictionary")),
        )
        .await
        .unwrap();

    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let handle_forwarder = builder
        .add_local_child(
            "handle_forwarder",
            move |h| {
                let _ = sender.lock().take().unwrap().send(h);
                futures::future::pending().boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("my_dictionary").path("/svc"))
                .from(Ref::self_())
                .to(&handle_forwarder),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>().as_("fuchsia.echo2"))
                .from(&protocol_child_2)
                .to(&handle_forwarder),
        )
        .await
        .unwrap();

    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let local_handles = receiver.await.unwrap();
    let dictionary_directory = local_handles.clone_from_namespace("svc").unwrap();
    let echo_proxy =
        client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dictionary_directory)
            .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        echo_proxy.echo_string(Some("Hello, world!")).await.unwrap().unwrap()
    );
    let echo_proxy_2 = client::connect_to_named_protocol_at_dir_root::<fecho::EchoMarker>(
        &dictionary_directory,
        "fuchsia.echo2",
    )
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        echo_proxy_2.echo_string(Some("Hello, world!")).await.unwrap().unwrap()
    );
    let file_proxy = fuchsia_fs::directory::open_file(
        &dictionary_directory,
        "data/example_file",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap()
    );
}

#[fuchsia::test]
async fn use_2_dictionaries_and_protocol_at_same_path() {
    let builder = RealmBuilder::new().await.unwrap();
    let protocol_child = builder
        .add_local_child("protocol_child", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let protocol_child_2 = builder
        .add_local_child("protocol_child_2", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let protocol_child_3 = builder
        .add_local_child("protocol_child_3", |h| echo_server_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let directory_child = builder
        .add_local_child("directory_child", |h| directory_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();
    let directory_child_2 = builder
        .add_local_child("directory_child_2", |h| directory_mock(h).boxed(), ChildOptions::new())
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
            name: cm_types::Name::new("my_dictionary").unwrap(),
            source_path: None,
        }))
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>())
                .from(&protocol_child)
                .to(Ref::capability("my_dictionary")),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::directory("data").path("/data").rights(fio::R_STAR_DIR))
                .from(&directory_child)
                .to(Ref::capability("my_dictionary")),
        )
        .await
        .unwrap();

    builder
        .add_capability(cm_rust::CapabilityDecl::Dictionary(cm_rust::DictionaryDecl {
            name: cm_types::Name::new("my_dictionary2").unwrap(),
            source_path: None,
        }))
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>().as_("fuchsia.echo3"))
                .from(&protocol_child_3)
                .to(Ref::capability("my_dictionary2")),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(
                    Capability::directory("data")
                        .path("/data")
                        .rights(fio::R_STAR_DIR)
                        .as_("data2"),
                )
                .from(&directory_child_2)
                .to(Ref::capability("my_dictionary2")),
        )
        .await
        .unwrap();

    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    let handle_forwarder = builder
        .add_local_child(
            "handle_forwarder",
            move |h| {
                let _ = sender.lock().take().unwrap().send(h);
                futures::future::pending().boxed()
            },
            ChildOptions::new().eager(),
        )
        .await
        .unwrap();

    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("my_dictionary").path("/svc"))
                .from(Ref::self_())
                .to(&handle_forwarder),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::dictionary("my_dictionary2").path("/svc"))
                .from(Ref::self_())
                .to(&handle_forwarder),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<fecho::EchoMarker>().as_("fuchsia.echo2"))
                .from(&protocol_child_2)
                .to(&handle_forwarder),
        )
        .await
        .unwrap();

    let _instance =
        builder.build_in_nested_component_manager("#meta/component_manager.cm").await.unwrap();
    let local_handles = receiver.await.unwrap();
    let dictionary_directory = local_handles.clone_from_namespace("svc").unwrap();
    log::warn!(
        "dictionary directory contents: {:?}",
        fuchsia_fs::directory::readdir(&dictionary_directory).await
    );
    let echo_proxy =
        client::connect_to_protocol_at_dir_root::<fecho::EchoMarker>(&dictionary_directory)
            .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        echo_proxy.echo_string(Some("Hello, world!")).await.unwrap().unwrap()
    );
    let echo_proxy_2 = client::connect_to_named_protocol_at_dir_root::<fecho::EchoMarker>(
        &dictionary_directory,
        "fuchsia.echo2",
    )
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        echo_proxy_2.echo_string(Some("Hello, world!")).await.unwrap().unwrap()
    );
    let echo_proxy_3 = client::connect_to_named_protocol_at_dir_root::<fecho::EchoMarker>(
        &dictionary_directory,
        "fuchsia.echo3",
    )
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        echo_proxy_3.echo_string(Some("Hello, world!")).await.unwrap().unwrap()
    );
    let file_proxy = fuchsia_fs::directory::open_file(
        &dictionary_directory,
        "data/example_file",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        fuchsia_fs::file::read_to_string(&file_proxy).await.unwrap()
    );
    let file_proxy_2 = fuchsia_fs::directory::open_file(
        &dictionary_directory,
        "data2/example_file",
        fio::PERM_READABLE,
    )
    .await
    .unwrap();
    assert_eq!(
        "Hello, world!".to_string(),
        fuchsia_fs::file::read_to_string(&file_proxy_2).await.unwrap()
    );
}

async fn echo_server_mock(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = server::ServiceFs::new();
    let mut tasks = vec![];
    fs.dir("svc").add_fidl_service(move |mut stream: fecho::EchoRequestStream| {
        tasks.push(fasync::Task::local(async move {
            while let Some(fecho::EchoRequest::EchoString { value, responder }) =
                stream.try_next().await.expect("failed to serve echo service")
            {
                responder.send(value.as_ref().map(|s| &**s)).expect("failed to send echo response");
            }
        }));
    });
    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

async fn directory_mock(handles: LocalComponentHandles) -> Result<(), Error> {
    let out_dir = vfs::pseudo_directory! {
        "data" => vfs::pseudo_directory! {
            "example_file" => read_only(b"Hello, world!")
        },
    };
    vfs::directory::serve_on(
        out_dir,
        fio::PERM_READABLE,
        ExecutionScope::new(),
        handles.outgoing_dir,
    );
    Ok(())
}

#[fuchsia::test]
async fn use_directory_from_dictionary_static() {
    let realm =
        fuchsia_component::client::connect_to_protocol::<fidl_fuchsia_component::RealmMarker>()
            .expect("failed to connect to Realm");

    let child_ref = fidl_fuchsia_component_decl::ChildRef {
        name: "directory_test".to_string(),
        collection: None,
    };

    let (client, server) = fidl::endpoints::create_endpoints();
    realm.open_exposed_dir(&child_ref, server).await.unwrap().unwrap();

    let trigger = client::connect_to_protocol_at_dir_root::<ftest::TriggerMarker>(&client).unwrap();
    let out = trigger.run().await.unwrap();
    assert_eq!(&out, "Directory verified");
}
