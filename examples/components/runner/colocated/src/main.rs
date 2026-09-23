// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use attribution_server::{AttributionServer, AttributionServerHandle, Observer, Publisher};
use fidl::endpoints::{ControlHandle, RequestStream};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_memory_attribution as fattribution;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::{StreamExt, TryStreamExt};
use log::{info, warn};
use runner::component::{Controllable, Controller, StopInfo};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use zx::Koid;

mod program;

use crate::program::ColocatedProgram;

enum IncomingRequest {
    Runner(fcrunner::ComponentRunnerRequestStream),
    Memory(fattribution::ProviderRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let resource_tracker = Arc::new(ResourceTracker { resources: Mutex::new(Default::default()) });

    let mut service_fs = ServiceFs::new_local();
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Runner);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Memory);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    let memory_server_handle = get_memory_server(resource_tracker.clone());

    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| async {
            match request {
                IncomingRequest::Runner(stream) => {
                    if let Err(err) = handle_runner_request(
                        stream,
                        resource_tracker.clone(),
                        memory_server_handle.clone(),
                    )
                    .await
                    {
                        warn!("Error while serving ComponentRunner: {err}");
                    }
                }
                IncomingRequest::Memory(stream) => {
                    let observer = memory_server_handle.new_observer(stream.control_handle());
                    if let Err(err) = handle_memory_request(stream, observer).await {
                        warn!("Error while serving AttributionProvider: {err}");
                    }
                }
            }
        })
        .await;

    Ok(())
}

fn get_memory_server(resource_tracker: Arc<ResourceTracker>) -> AttributionServerHandle {
    let state_fn = Box::new(move || get_attribution(resource_tracker.clone()));
    AttributionServer::new(state_fn)
}

/// Handles `fuchsia.component.runner/ComponentRunner` requests over a FIDL connection.
async fn handle_runner_request(
    mut stream: fcrunner::ComponentRunnerRequestStream,
    resource_tracker: Arc<ResourceTracker>,
    memory_server_handle: AttributionServerHandle,
) -> Result<()> {
    while let Some(request) =
        stream.try_next().await.context("failed to serve ComponentRunner protocol")?
    {
        let fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } = request else {
            bail!("Unexpected runner request");
        };
        let url = start_info.resolved_url.clone().unwrap_or_else(|| "unknown url".to_string());
        info!("Colocated runner is going to start component {url}");
        let (stream, control) = controller.into_stream_and_control_handle();
        match start(start_info, resource_tracker.clone(), memory_server_handle.new_publisher()) {
            Ok((program, on_exit)) => {
                let controller = Controller::new(program, stream, control);
                fasync::Task::spawn(controller.serve(on_exit)).detach();
            }
            Err(err) => {
                warn!("Colocated runner failed to start component {url}: {err}");
                let _ = control.send_on_stop(fcrunner::ComponentStopInfo {
                    termination_status: Some(fcomponent::Error::Internal.into_primitive() as i32),
                    ..Default::default()
                });
                control.shutdown();
            }
        }
    }

    Ok(())
}

async fn handle_memory_request(
    mut stream: fattribution::ProviderRequestStream,
    subscriber: Observer,
) -> Result<()> {
    while let Some(request) =
        stream.try_next().await.context("failed to serve AttributionProvider protocol")?
    {
        match request {
            fattribution::ProviderRequest::Get { responder } => {
                subscriber.next(responder);
            }
            fattribution::ProviderRequest::_UnknownMethod { ordinal, control_handle, .. } => {
                warn!("Invalid request to AttributionProvider: {ordinal}");
                control_handle.shutdown_with_epitaph(zx::Status::INVALID_ARGS);
            }
        }
    }

    Ok(())
}

fn get_attribution(resource_tracker: Arc<ResourceTracker>) -> Vec<fattribution::AttributionUpdate> {
    let mut children = vec![];
    for (_, (token, koid)) in resource_tracker.resources.lock().iter() {
        children.push(fattribution::AttributionUpdate::Add(fattribution::NewPrincipal {
            identifier: Some(token.koid().unwrap().raw_koid()),
            description: Some(fattribution::Description::Component(
                token.duplicate_handle(fidl::Rights::SAME_RIGHTS).unwrap(),
            )),
            detailed_attribution: None,
            ..Default::default()
        }));
        children.push(fattribution::AttributionUpdate::Update(fattribution::UpdatedPrincipal {
            identifier: Some(token.koid().unwrap().raw_koid()),
            resources: Some(fattribution::Resources::Data(fattribution::Data {
                resources: vec![fattribution::Resource::KernelObject(koid.raw_koid())],
            })),
            ..Default::default()
        }));
    }
    children
}

/// Tracks resources used by each [`ColocatedProgram`]. Since each program just allocates
/// one VMO, we only need to track one KOID here.
struct ResourceTracker {
    resources: Mutex<HashMap<ProgramId, (zx::Event, Koid)>>,
}

type ProgramId = u64;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Starts a colocated component.
fn start(
    start_info: fcrunner::ComponentStartInfo,
    resource_tracker: Arc<ResourceTracker>,
    publisher: Publisher,
) -> Result<(impl Controllable, impl Future<Output = StopInfo> + Unpin)> {
    let numbered_handles = start_info.numbered_handles.unwrap_or(vec![]);
    let program = ColocatedProgram::new(numbered_handles)?;
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let vmo_koid = program.get_vmo_koid().raw_koid();
    // Register this VMO.
    let instance_token = start_info.component_instance.as_ref().unwrap();

    let mut resources = resource_tracker.resources.lock();
    resources.insert(
        id,
        (
            instance_token.duplicate_handle(fidl::Rights::SAME_RIGHTS).unwrap(),
            program.get_vmo_koid(),
        ),
    );
    drop(resources);

    let updates = vec![
        fattribution::AttributionUpdate::Add(fattribution::NewPrincipal {
            identifier: Some(instance_token.koid().unwrap().raw_koid()),
            description: Some(fattribution::Description::Component(
                instance_token.duplicate_handle(fidl::Rights::SAME_RIGHTS).unwrap(),
            )),
            detailed_attribution: None,
            ..Default::default()
        }),
        fattribution::AttributionUpdate::Update(fattribution::UpdatedPrincipal {
            identifier: Some(instance_token.koid().unwrap().raw_koid()),
            resources: Some(fattribution::Resources::Data(fattribution::Data {
                resources: vec![fattribution::Resource::KernelObject(vmo_koid)],
            })),
            ..Default::default()
        }),
    ];
    publisher.on_update(updates);
    let termination = program.wait_for_termination();
    let termination_clone = program.wait_for_termination();
    // Remove this VMO when the program has terminated.
    let tracker = resource_tracker.clone();
    fasync::Task::spawn(async move {
        termination_clone.await;
        if let Some((token, _)) = tracker.resources.lock().remove(&id) {
            publisher.on_update(vec![fattribution::AttributionUpdate::Remove(
                token.koid().unwrap().raw_koid(),
            )]);
        }
    })
    .detach();
    Ok((program, termination))
}

/// Unit test the `ComponentRunner` protocol server implementation.
#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_component_decl as fdecl;
    use fidl_fuchsia_examples_colocated as fcolocated;
    use fidl_fuchsia_process::HandleInfo;
    use fuchsia_runtime::HandleType;

    #[fuchsia::test]
    async fn test_start_stop_component() {
        let resource_tracker =
            Arc::new(ResourceTracker { resources: Mutex::new(Default::default()) });
        let (runner, runner_stream) =
            fidl::endpoints::create_proxy_and_stream::<fcrunner::ComponentRunnerMarker>();
        let memory_server = get_memory_server(resource_tracker.clone());
        let server = fasync::Task::spawn(handle_runner_request(
            runner_stream,
            resource_tracker,
            memory_server.clone(),
        ));

        // Start a colocated component.
        let decl = fuchsia_fs::file::read_in_namespace_to_fidl::<fdecl::Component>(
            "/pkg/meta/colocated-component.cm",
        )
        .await
        .unwrap();
        let (controller, controller_server_end) = fidl::endpoints::create_endpoints();
        let (user0, user0_peer) = zx::Channel::create();
        let start_info = fcrunner::ComponentStartInfo {
            program: decl.program.unwrap().info,
            numbered_handles: Some(vec![HandleInfo {
                handle: user0_peer.into(),
                id: fuchsia_runtime::HandleInfo::new(HandleType::User0, 0).as_raw(),
            }]),
            component_instance: Some(zx::Event::create()),
            ..Default::default()
        };
        runner.start(start_info, controller_server_end).unwrap();

        // Wait until the program has allocated 64 MiB worth of pages.
        let colocated_component_vmos =
            fcolocated::ColocatedProxy::new(fasync::Channel::from_channel(user0))
                .get_vmos()
                .await
                .unwrap();

        // Measure our private memory usage again. It should increase by roughly that much more.
        assert!(!colocated_component_vmos.is_empty());

        // Stop the component.
        let controller = controller.into_proxy();
        controller.stop().unwrap();
        controller.on_closed().await.unwrap();

        // Close the connection and verify the server task ends successfully.
        drop(runner);
        server.await.unwrap();
    }
}
