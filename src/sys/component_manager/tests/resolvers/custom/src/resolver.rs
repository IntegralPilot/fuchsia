// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use log::*;

/// Wraps all hosted protocols into a single type that can be matched against
/// and dispatched.
enum IncomingRequest {
    /// A request to the fuchsia.component.resolution.Resolver protocol.
    ResolverProtocol(fresolution::ResolverRequestStream),
}

async fn serve_resolver(mut stream: fresolution::ResolverRequestStream) -> Result<(), Error> {
    while let Some(request) =
        stream.try_next().await.context("failed to read request from stream")?
    {
        match request {
            fresolution::ResolverRequest::Resolve { component_url, responder } => {
                if component_url == "test://trigger" {
                    let (client, server) = fidl::endpoints::create_endpoints();
                    fdio::open(
                        "/pkg",
                        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                        server.into_channel(),
                    )
                    .context("failed to open /pkg")?;
                    responder.send( Ok(fresolution::Component {
                        url: Some("fuchsia-pkg://fuchsia.com/component-manager-test-resolver#meta/component.cm".to_string()),
                        decl: Some(build_decl()),
                        package: Some(fresolution::Package {
                            url: Some("fuchsia-pkg://fuchsia.com/component-manager-test-resolver".to_string()),
                            directory: Some(client),
                            ..Default::default()
                        }),
                        // component-manager-test-resolver only resolves test://trigger
                        resolution_context: None,
                        abi_revision: Some(version_history_data::HISTORY
                            .get_example_supported_version_for_tests()
                            .abi_revision
                            .into()),
                        ..Default::default()
                    })).with_context(|| format!("failed to send response to resolve request for component URL {}", component_url))?;
                } else {
                    responder
                        .send(Err(fresolution::ResolverError::ManifestNotFound))
                        .with_context(|| {
                            format!(
                                "failed to send response to resolve request for component URL {}",
                                component_url
                            )
                        })?;
                }
            }
            fresolution::ResolverRequest::ResolveWithContext {
                component_url,
                context,
                responder,
            } => {
                error!(
                    url:? = component_url, context:?;
                    "custom resolver does not support ResolveWithContext, and could not resolve",
                );
                responder.send(Err(fresolution::ResolverError::InvalidArgs)).with_context(
                    || {
                        format!(
                            "failed to send response to resolve request for component URL {}",
                            component_url
                        )
                    },
                )?;
            }
            fresolution::ResolverRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown Resolver request");
            }
        }
    }
    Ok(())
}

fn build_decl() -> fmem::Data {
    let mut component_decl = fdecl::Component::default();
    component_decl.program = Some(fdecl::Program {
        runner: Some("elf".to_string()),
        info: Some(fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "binary".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str(
                    "bin/component_manager_test_trigger_bin".to_string(),
                ))),
            }]),
            ..Default::default()
        }),
        ..Default::default()
    });
    component_decl.capabilities = Some(vec![fdecl::Capability::Protocol(fdecl::Protocol {
        name: Some("fidl.test.components.Trigger".to_string()),
        source_path: Some("/svc/fidl.test.components.Trigger".to_string()),
        ..Default::default()
    })]);
    component_decl.exposes = Some(vec![fdecl::Expose::Protocol(fdecl::ExposeProtocol {
        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
        target: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
        source_name: Some("fidl.test.components.Trigger".to_string()),
        target_name: Some("fidl.test.components.Trigger".to_string()),
        ..Default::default()
    })]);
    fmem::Data::Bytes(fidl::persist(&component_decl).expect("encoded"))
}

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    let mut service_fs = ServiceFs::new_local();
    service_fs.dir("svc").add_fidl_service(IncomingRequest::ResolverProtocol);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;
    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| async move {
            match request {
                IncomingRequest::ResolverProtocol(stream) => match serve_resolver(stream).await {
                    Ok(()) => {}
                    Err(error) => error!(error:%; "resolver failed"),
                },
            }
        })
        .await;

    Ok(())
}
