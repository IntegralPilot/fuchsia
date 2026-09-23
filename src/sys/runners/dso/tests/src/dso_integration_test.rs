// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fidl_test_dso as ftest;
use fidl_fuchsia_component_decl as fdecl;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, LocalComponentHandles, RealmBuilder, Ref, Route,
};
use futures::{FutureExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct TestParams {
    dso_name: &'static str,
    dso_url: &'static str,
}

async fn run_dso_test(params: TestParams) {
    let pinged = Arc::new(AtomicBool::new(false));
    let pinged2 = pinged.clone();

    let mock_server = move |handles: LocalComponentHandles| {
        let pinged = pinged2.clone();
        async move {
            let mut fs = ServiceFs::new();
            fs.dir("svc").add_fidl_service(|stream: ftest::TestHelperRequestStream| stream);
            fs.serve_connection(handles.outgoing_dir).unwrap();

            fs.for_each_concurrent(None, |mut stream| {
                let pinged = pinged.clone();
                async move {
                    while let Some(request) = stream.next().await {
                        let request = request.unwrap();
                        match request {
                            ftest::TestHelperRequest::Ping { responder } => {
                                println!("mock_server: received Ping");
                                pinged.store(true, Ordering::Relaxed);
                                responder.send("pong").unwrap();
                            }
                        }
                    }
                }
            })
            .await;
            Ok(())
        }
        .boxed()
    };

    let builder = RealmBuilder::new().await.unwrap();

    // Add dso_runner
    builder.add_child("dso_runner", "#meta/dso_runner.cm", ChildOptions::new()).await.unwrap();

    // Add mock_server
    builder.add_local_child("mock_server", mock_server, ChildOptions::new()).await.unwrap();

    // Add environment for DSO components
    builder
        .add_environment(cm_rust::EnvironmentDecl {
            name: "dso_env".parse().unwrap(),
            extends: fdecl::EnvironmentExtends::Realm,
            runners: Box::from([cm_rust::RunnerRegistration {
                source_name: "dso".parse().unwrap(),
                target_name: "dso".parse().unwrap(),
                source: cm_rust::RegistrationSource::Child("dso_runner".to_string()),
            }]),
            resolvers: Box::from([]),
            debug_capabilities: Box::from([]),
            stop_timeout_ms: None,
        })
        .await
        .unwrap();

    // Add DSO component
    builder
        .add_child(params.dso_name, params.dso_url, ChildOptions::new().environment("dso_env"))
        .await
        .unwrap();

    // Route LogSink
    builder
        .add_route(
            Route::new()
                .from(Ref::parent())
                .to(Ref::child("dso_runner"))
                .to(Ref::child("mock_server"))
                .to(Ref::child(params.dso_name))
                .capability(Capability::protocol_by_name("fuchsia.logger.LogSink")),
        )
        .await
        .unwrap();

    // Route TestHelper from mock_server to DSO
    builder
        .add_route(
            Route::new()
                .from(Ref::child("mock_server"))
                .to(Ref::child(params.dso_name))
                .capability(Capability::protocol::<ftest::TestHelperMarker>()),
        )
        .await
        .unwrap();

    // Route TestHelper from DSO to parent
    builder
        .add_route(
            Route::new()
                .from(Ref::child(params.dso_name))
                .to(Ref::parent())
                .capability(Capability::protocol::<ftest::TestHelperMarker>()),
        )
        .await
        .unwrap();

    let instance = builder.build().await.unwrap();

    // Connect to helper (starts DSO)
    let helper = client::connect_to_protocol_at_dir_root::<ftest::TestHelperMarker>(
        instance.root.get_exposed_dir(),
    )
    .unwrap();
    let response = helper.ping().await.unwrap();
    assert_eq!(response, "pong");

    // Verify ping from DSO component to mock_server
    assert!(pinged.load(Ordering::Relaxed));

    // Drop instance to stop components
    drop(instance);
}

#[fuchsia::test]
async fn test_rust_dso() {
    run_dso_test(TestParams { dso_name: "rust_dso", dso_url: "#meta/rust_dso.cm" }).await;
}

#[fuchsia::test]
async fn test_cpp_dso() {
    run_dso_test(TestParams { dso_name: "cpp_dso", dso_url: "#meta/cpp_dso.cm" }).await;
}
