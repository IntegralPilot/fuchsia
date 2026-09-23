// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints;
use fidl_fidl_test_components as ftest;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_runtime::{HandleInfo, HandleType};

#[fuchsia::test]
async fn collections() {
    let realm = client::connect_to_protocol::<fcomponent::RealmMarker>()
        .expect("could not connect to Realm service");

    // Create a couple child components.
    for name in vec!["a", "b"] {
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        let child_decl = fdecl::Child {
            name: Some(name.to_string()),
            url: Some(format!("#meta/trigger_{}.cm", name)),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            ..Default::default()
        };
        realm
            .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
            .await
            .unwrap_or_else(|e| panic!("create_child {} failed: {:?}", name, e))
            .unwrap_or_else(|e| panic!("failed to create child {}: {:?}", name, e));
    }

    let children = list_children(&realm).await.expect("failed to list children");
    assert_eq!("coll:a,coll:b", &children);

    // Start the children.
    for name in vec!["a", "b"] {
        let child_ref = new_child_ref(name, "coll");
        let (dir, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
        realm
            .open_exposed_dir(&child_ref, server_end)
            .await
            .unwrap_or_else(|e| panic!("open_exposed_dir {} failed: {:?}", name, e))
            .unwrap_or_else(|e| panic!("failed to open exposed dir of child {}: {:?}", name, e));
        let trigger = client::connect_to_protocol_at_dir_root::<ftest::TriggerMarker>(&dir)
            .expect("failed to open trigger service");

        let out =
            trigger.run().await.unwrap_or_else(|e| panic!("trigger {} failed: {:?}", name, e));
        assert_eq!(out, format!("Triggered {}", name));
    }

    // Destroy one.
    {
        let child_ref = new_child_ref("a", "coll");
        realm
            .destroy_child(&child_ref)
            .await
            .expect("destroy_child a failed")
            .expect("failed to destroy child");
    }

    // Binding to destroyed child should fail.
    {
        let (_, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
        let child_ref = new_child_ref("a", "coll");
        let res = realm
            .open_exposed_dir(&child_ref, server_end)
            .await
            .expect("second open_exposed_dir a failed");
        let err = res.expect_err("expected open_exposed_dir a to fail");
        assert_eq!(err, fcomponent::Error::InstanceNotFound);
    }

    let children = list_children(&realm).await.expect("failed to list children");
    assert_eq!("coll:b", &children);

    // Recreate child (with different URL), and start it. Should work.
    {
        let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
        let child_decl = fdecl::Child {
            name: Some("a".to_string()),
            url: Some("#meta/trigger_realm.cm".to_string()),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            ..Default::default()
        };
        realm
            .create_child(&collection_ref, &child_decl, fcomponent::CreateChildArgs::default())
            .await
            .expect("second create_child a failed")
            .expect("failed to create second child a");
    }
    {
        let (dir, server_end) = endpoints::create_proxy::<fio::DirectoryMarker>();
        let child_ref = new_child_ref("a", "coll");
        realm
            .open_exposed_dir(&child_ref, server_end)
            .await
            .expect("open_exposed_dir a failed")
            .expect("failed to open exposed dir of child a");
        let trigger = client::connect_to_protocol_at_dir_root::<ftest::TriggerMarker>(&dir)
            .expect("failed to open trigger service");
        let out = trigger.run().await.expect("second trigger a failed");
        assert_eq!(&out, "Triggered a");
    }

    let children = list_children(&realm).await.expect("failed to list children");
    assert_eq!("coll:a,coll:b", &children);
}

#[fuchsia::test]
async fn child_args() {
    let realm = client::connect_to_protocol::<fcomponent::RealmMarker>()
        .expect("could not connect to Realm service");

    // Providing numbered handles to a component that is not in a single run collection should fail.
    {
        let name = "a";
        let collection_ref = fdecl::CollectionRef { name: "not_single_run".to_string() };
        let child_decl = fdecl::Child {
            name: Some(name.to_string()),
            url: Some(format!("#meta/trigger_{}.cm", name)),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            ..Default::default()
        };
        let (_, socket) = zx::Socket::create_stream();
        let numbered_handles = vec![fprocess::HandleInfo { handle: socket.into_handle(), id: 0 }];
        let child_args = fcomponent::CreateChildArgs {
            numbered_handles: Some(numbered_handles),
            ..Default::default()
        };
        let res = realm
            .create_child(&collection_ref, &child_decl, child_args)
            .await
            .unwrap_or_else(|e| panic!("create_child {} failed: {:?}", name, e));
        let err = res.expect_err("expected create_child a to fail");
        assert_eq!(err, fcomponent::Error::Unsupported);
    }
    // Providing numbered handles to a component that is in a single run collection should succeed.
    {
        let collection_ref = fdecl::CollectionRef { name: "single_run".to_owned() };
        let child_decl = fdecl::Child {
            name: Some("write_startup_socket".to_owned()),
            url: Some("#meta/write_startup_socket.cm".to_owned()),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            ..Default::default()
        };
        let (their_socket, our_socket) = zx::Socket::create_stream();
        let numbered_handles = vec![fprocess::HandleInfo {
            handle: their_socket.into_handle(),
            // Must correspond to the same Handle Id in write_startup_socket.rs.
            id: HandleInfo::new(HandleType::User0, 0).as_raw(),
        }];
        let child_args = fcomponent::CreateChildArgs {
            numbered_handles: Some(numbered_handles),
            ..Default::default()
        };
        realm
            .create_child(&collection_ref, &child_decl, child_args)
            .await
            .expect("fidl error in create_child")
            .expect("failed to create_child");

        // NOTE: Although we specified StartupMode::Lazy above, |create_child| will actually start
        // the component immediately without the need to manually bind it, because "the instances
        // in a single run collection are started when they are created" (see
        // //docs/concepts/components/v2/realms.md). https://fxbug.dev/42171498 discusses the idea
        // to remove the startup parameter.

        fasync::OnSignals::new(&our_socket, zx::Signals::SOCKET_READABLE)
            .await
            .expect("SOCKET_READABLE should be signaled after the child starts");
        let mut bytes = [0u8; 32];
        let length = our_socket.read(&mut bytes).expect("read should succeed");
        assert_eq!(bytes[..length], b"Hello, World!"[..]);

        // Checking OBJECT_PEER_CLOSED ensures that |their_socket| is not leaked.
        fasync::OnSignals::new(&our_socket, zx::Signals::OBJECT_PEER_CLOSED)
            .await
            .expect("OBJECT_PEER_CLOSED should be signaled after the child exits");
    }
    // Providing numbered handles with invalid id should fail.
    {
        let collection_ref = fdecl::CollectionRef { name: "single_run".to_owned() };
        let child_decl = fdecl::Child {
            name: Some("write_startup_socket_2".to_owned()),
            url: Some("#meta/write_startup_socket.cm".to_owned()),
            startup: Some(fdecl::StartupMode::Lazy),
            environment: None,
            ..Default::default()
        };
        let (their_socket, our_socket) = zx::Socket::create_stream();
        let job =
            fuchsia_runtime::job_default().create_child_job().expect("fail to create_child_job");
        job.set_name(&zx::Name::new("new-job-new-name").unwrap()).expect("fail to set_name");
        let numbered_handles = vec![
            fprocess::HandleInfo {
                handle: job.into_handle(),
                // Only PA_FD and PA_USER* handles are valid arguments (//sdk/fidl/fuchsia.component/realm.fidl).
                id: HandleInfo::new(HandleType::DefaultJob, 0).as_raw(),
            },
            fprocess::HandleInfo {
                handle: their_socket.into_handle(),
                // Must correspond to the same Handle Id in write_startup_socket.rs.
                id: HandleInfo::new(HandleType::User0, 0).as_raw(),
            },
        ];
        let child_args = fcomponent::CreateChildArgs {
            numbered_handles: Some(numbered_handles),
            ..Default::default()
        };
        let result = realm
            .create_child(&collection_ref, &child_decl, child_args)
            .await
            .expect("fidl error in create_child");

        // TODO(https://fxbug.dev/42181105): it should fail to create_child, but it succeeds instead. Also
        // the default_job received in write_startup_socket.rs is NOT the job we created above.
        result.unwrap();

        // Wait for the component to exit (to mute the log).
        fasync::OnSignals::new(&our_socket, zx::Signals::OBJECT_PEER_CLOSED)
            .await
            .expect("OBJECT_PEER_CLOSED should be signaled after the child exits");
    }
}

#[fuchsia::test]
async fn eager() {
    let realm = client::connect_to_protocol::<fcomponent::RealmMarker>().unwrap();

    let collection_ref = fdecl::CollectionRef { name: "eager".to_string() };
    let child_decl = fdecl::Child {
        name: Some("a".into()),
        url: Some("#meta/trigger_a.cm".into()),
        startup: Some(fdecl::StartupMode::Eager),
        ..Default::default()
    };
    let (controller_proxy, server_end) = endpoints::create_proxy::<fcomponent::ControllerMarker>();
    let child_args =
        fcomponent::CreateChildArgs { controller: Some(server_end), ..Default::default() };
    realm.create_child(&collection_ref, &child_decl, child_args).await.unwrap().unwrap();

    // [create_child] blocks for eager children to start so it should be started already.
    assert_eq!(controller_proxy.is_started().await.unwrap().unwrap(), true);

    let child_ref = new_child_ref("a", "eager");
    realm.destroy_child(&child_ref).await.unwrap().unwrap();
}

fn new_child_ref(name: &str, collection: &str) -> fdecl::ChildRef {
    fdecl::ChildRef { name: name.to_string(), collection: Some(collection.to_string()) }
}

async fn list_children(realm: &fcomponent::RealmProxy) -> Result<String, Error> {
    let (iterator_proxy, server_end) = endpoints::create_proxy();
    let collection_ref = fdecl::CollectionRef { name: "coll".to_string() };
    realm
        .list_children(&collection_ref, server_end)
        .await
        .expect("list_children failed")
        .expect("failed to list children");
    let res = iterator_proxy.next().await;
    let children = res.expect("failed to iterate over children");
    let children: Vec<_> = children
        .iter()
        .map(|c| format!("{}:{}", c.collection.as_ref().expect("no collection"), &c.name))
        .collect();
    Ok(children.join(","))
}
