// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_settings as fsettings;
use fidl_fuchsia_starnix_runner as fstarnixrunner;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use kernel_manager::kernels::Kernels;
use kernel_manager::proxy::run_proxy_thread;
use kernel_manager::serve_starnix_manager;
use kernel_manager::suspend::SuspendContext;
use log::{error, info, warn};
use std::sync::Arc;
use zx;

enum Services {
    ComponentRunner(frunner::ComponentRunnerRequestStream),
    StarnixManager(fstarnixrunner::ManagerRequestStream),
}

const MEMORY_ROLE_NAME: &str = "fuchsia.starnix.runner";

#[fuchsia::main(logging_tags = ["starnix_runner"])]
async fn main() -> Result<(), Error> {
    if let Err(e) = fuchsia_scheduler::set_role_for_root_vmar(MEMORY_ROLE_NAME) {
        warn!(e:%; "failed to set memory role");
    }
    let scope_dispatcher = libasync_scope_dispatcher::ScopeDispatcher::new();
    let trace_provider =
        fuchsia_trace_provider::TraceProvider::new_with_fdio(&scope_dispatcher, None);
    let _inspect_server_task = inspect_runtime::publish(
        fuchsia_inspect::component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    let config = starnix_runner_config::Config::take_from_startup_handle();
    if config.enable_data_collection {
        info!("Attempting to set user data sharing consent.");
        if let Ok(privacy) = connect_to_protocol_sync::<fsettings::PrivacyMarker>() {
            let privacy_settings = fsettings::PrivacySettings {
                user_data_sharing_consent: Some(true),
                ..Default::default()
            };
            match privacy.set(&privacy_settings, zx::MonotonicInstant::INFINITE) {
                Ok(Ok(())) => info!("Successfully set user data sharing consent."),
                Ok(Err(err)) => warn!("Could not set user data sharing consent: {err:?}"),
                Err(err) => warn!("Could not set user data sharing consent: {err:?}"),
            }
        } else {
            warn!("failed to connect to fuchsia.settings.Privacy");
        }
    }

    let kernels = Kernels::new();
    let mut fs = ServiceFs::new_local();

    let (proxy_sender, proxy_receiver) = async_channel::unbounded();
    run_proxy_thread(proxy_receiver);

    fs.dir("svc").add_fidl_service(Services::ComponentRunner);
    fs.dir("svc").add_fidl_service(Services::StarnixManager);
    fs.take_and_serve_directory_handle()?;
    let suspend_context = Arc::new(SuspendContext::default());
    let (suspend_sender, suspend_receiver) = async_channel::unbounded();
    let pager = Arc::new(kernel_manager::pager::Pager::new()?);
    pager.start_threads();

    let suspend_context_for_loop = suspend_context.clone();
    let kernels_ref = &kernels;
    let fs_loop = fs.for_each_concurrent(None, move |request: Services| {
        let proxy_sender = proxy_sender.clone();
        let suspend_sender = suspend_sender.clone();
        let pager = pager.clone();
        let suspend_context = suspend_context_for_loop.clone();
        let kernels = kernels_ref;
        async move {
            match request {
                Services::ComponentRunner(stream) => {
                    if let Err(e) = serve_component_runner(stream, kernels).await {
                        error!(e:%; "failed to serve component runner");
                    }
                }
                Services::StarnixManager(stream) => {
                    if let Err(e) = serve_starnix_manager(
                        stream,
                        suspend_context,
                        &proxy_sender,
                        pager.clone(),
                        &suspend_sender,
                    )
                    .await
                    {
                        error!(e:%; "failed to serve starnix manager");
                    }
                }
            }
        }
    });

    let suspend_worker =
        kernel_manager::run_suspend_worker(suspend_receiver, suspend_context.clone(), &kernels);

    futures::future::join(fs_loop, suspend_worker).await;

    drop(trace_provider);
    scope_dispatcher.shutdown().await;

    Ok(())
}

async fn serve_component_runner(
    mut stream: frunner::ComponentRunnerRequestStream,
    kernels: &Kernels,
) -> Result<(), Error> {
    while let Some(event) = stream.next().await {
        match event.context("serving component runner")? {
            frunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                kernels.start(start_info, controller).await?;
            }
            frunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown ComponentRunner request");
            }
        }
    }
    Ok(())
}
