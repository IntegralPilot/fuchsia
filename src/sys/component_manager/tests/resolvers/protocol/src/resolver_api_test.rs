// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This test validates that component URLs retain expected values when passed
//! to component resolvers. Scheme, host, path, query string, and fragment
//! values are generally passed through. (Note that some normalization may
//! still occur. The test URLs in this test should match the normalized values.)

use anyhow::{Context as _, Error};
use cm_rust::push_box;
use fidl::endpoints::DiscoverableProtocolMarker;
use fuchsia_component::server;
use fuchsia_component_test::{ChildOptions, LocalComponentHandles, RealmBuilder};
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use log::*;
use std::collections::HashSet;
use std::sync::Arc;
use {
    fidl_fuchsia_component_decl as fcdecl, fidl_fuchsia_component_resolution as fcresolution,
    fuchsia_async as fasync,
};

const ENVIRONMENT_NAME: &'static str = "resolver_env";
const RESOLVER_NAME: &'static str = "fake_resolver";
const RESOLVER_SCHEME: &'static str = "fake";

#[fuchsia::test]
async fn resolver_receives_expected_request_params() -> Result<(), Error> {
    let mut test_urls = HashSet::default();
    test_urls.insert(format!("{RESOLVER_SCHEME}://somerepo/somepackage#meta/somecomponent.cm"));
    test_urls.insert(format!(
        "{RESOLVER_SCHEME}://somerepo/somepackage?hash=1234#meta/somecomponent.cm"
    ));

    let builder = RealmBuilder::new().await?;

    let (send_complete, mut receive_complete) = mpsc::channel(1);
    let urls_to_resolve = Arc::new(Mutex::new(test_urls.clone()));
    let _resolver = builder
        .add_local_child(
            RESOLVER_NAME,
            move |handles| {
                Box::pin(local_resolver_impl(
                    handles,
                    urls_to_resolve.clone(),
                    send_complete.clone(),
                ))
            },
            ChildOptions::new(),
        )
        .await?;

    // Provide and expose the resolver capability from the resolver to the test realm
    let mut resolver_decl = builder.get_component_decl(RESOLVER_NAME).await?;
    push_box(
        &mut resolver_decl.exposes,
        cm_rust::ExposeDecl::Resolver(cm_rust::ExposeResolverDecl {
            source: cm_rust::ExposeSource::Self_,
            source_name: fcresolution::ResolverMarker::PROTOCOL_NAME.parse().unwrap(),
            source_dictionary: Default::default(),
            target: cm_rust::ExposeTarget::Parent,
            target_name: fcresolution::ResolverMarker::PROTOCOL_NAME.parse().unwrap(),
        }),
    );
    push_box(
        &mut resolver_decl.capabilities,
        cm_rust::CapabilityDecl::Resolver(cm_rust::ResolverDecl {
            name: fcresolution::ResolverMarker::PROTOCOL_NAME.parse().unwrap(),
            source_path: Some("/svc/fuchsia.component.resolution.Resolver".parse().unwrap()),
        }),
    );
    builder.replace_component_decl(RESOLVER_NAME, resolver_decl).await?;

    // Make sure all children to be resolved via this test resolver are added to
    // the environment that hosts the resolver: `ENVIRONMENT_NAME`.
    let child_opts_with_resolver = ChildOptions::new().environment(ENVIRONMENT_NAME).eager();

    // Add the resolver to the environment the child will be launched in.
    let mut realm_decl = builder.get_realm_decl().await?;
    push_box(
        &mut realm_decl.environments,
        cm_rust::EnvironmentDecl {
            name: ENVIRONMENT_NAME.parse().unwrap(),
            extends: fcdecl::EnvironmentExtends::Realm,
            resolvers: Box::from([cm_rust::ResolverRegistration {
                resolver: fcresolution::ResolverMarker::PROTOCOL_NAME.parse().unwrap(),
                source: cm_rust::RegistrationSource::Child(String::from(RESOLVER_NAME)),
                scheme: String::from(RESOLVER_SCHEME),
            }]),
            runners: Box::from([]),
            debug_capabilities: Box::from([]),
            stop_timeout_ms: None,
        },
    );
    builder.replace_realm_decl(realm_decl).await?;

    for (index, test_url) in test_urls.iter().enumerate() {
        builder
            .add_child(format!("test_comp_{index}"), test_url, child_opts_with_resolver.clone())
            .await?;
    }

    let _realm_instance = builder.build().await?;

    assert_eq!(receive_complete.next().await, Some(true));

    Ok(())
}

async fn local_resolver_impl(
    handles: LocalComponentHandles,
    urls_to_resolve: Arc<Mutex<HashSet<String>>>,
    send_complete: mpsc::Sender<bool>,
) -> Result<(), Error> {
    info!("fake_resolver is launching and waiting for ResolverRequestStream");
    let mut fs = server::ServiceFs::new();
    fs.dir("svc").add_fidl_service(|mut stream: fcresolution::ResolverRequestStream| {
        let mut send_complete = send_complete.clone();
        let urls_to_resolve = urls_to_resolve.clone();
        fasync::Task::local(async move {
            while let Some(request) = stream.try_next().await.unwrap() {
                match request {
                    fcresolution::ResolverRequest::Resolve { component_url, responder } => {
                        info!("Got Resolve request for {component_url}");
                        if !urls_to_resolve.lock().remove(&component_url) {
                            error!("received unexpected URL: {component_url}");
                            send_complete.send(false).await.expect("failed to send results");
                        }
                        if urls_to_resolve.lock().is_empty() {
                            // Success!
                            send_complete.send(true).await.expect("failed to send results");
                        }
                        // This isn't a real resolver so return an Internal error
                        responder
                            .send(Err(fcresolution::ResolverError::Internal))
                            .context("failed sending response")
                            .unwrap()
                    }
                    fcresolution::ResolverRequest::ResolveWithContext {
                        component_url: _,
                        context: _,
                        responder,
                    } => {
                        error!("ResolveWithContext not implemented in this test");
                        responder
                            .send(Err(fcresolution::ResolverError::Internal))
                            .context("failed sending response")
                            .unwrap()
                    }
                    fcresolution::ResolverRequest::_UnknownMethod { .. } => {
                        panic!("unknown resolver request");
                    }
                }
            }
        })
        .detach();
    });
    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;

    Ok(())
}
