// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use erofs::ErofsError;
use erofs_component::{ErofsPager, ErofsVolume};
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_erofs::{ErofsMarker, ErofsRequest, ErofsRequestStream};
use fidl_fuchsia_io as fio;
use futures::TryStreamExt;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

fn map_to_status(error: anyhow::Error) -> zx::Status {
    if let Some(status) = error.root_cause().downcast_ref::<zx::Status>() {
        status.clone()
    } else if let Some(erofs_error) = error.root_cause().downcast_ref::<ErofsError>() {
        erofs_error.clone().to_status()
    } else {
        // The expectation is that places that map to status will have already printed out more
        // contextual errors if appropriate.
        zx::Status::INTERNAL
    }
}

#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    log::info!("Starting EROFS component");

    let outgoing =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::DirectoryRequest.into())
            .context("missing DirectoryRequest startup handle")?;

    let pager = Arc::new(ErofsPager::new().context("Failed to create ErofsPager")?);

    let export = vfs::pseudo_directory! {
        "svc" => vfs::pseudo_directory! {
            ErofsMarker::PROTOCOL_NAME => {
                let pager = pager.clone();
                vfs::service::host(move |stream: ErofsRequestStream| {
                    let pager = pager.clone();
                    async move {
                        if let Err(e) = handle_request_stream(stream, pager).await {
                            log::error!("Error handling stream: {:?}", e);
                        }
                    }
                })
            }
        }
    };

    let scope = ExecutionScope::new();
    vfs::directory::serve_on(
        export,
        fio::PERM_READABLE,
        scope.clone(),
        fidl::endpoints::ServerEnd::new(outgoing.into()),
    );

    scope.wait().await;
    Ok(())
}

async fn handle_request_stream(
    mut stream: ErofsRequestStream,
    pager: Arc<ErofsPager>,
) -> Result<(), anyhow::Error> {
    while let Some(request) = stream.try_next().await? {
        match request {
            ErofsRequest::Serve { payload, responder } => {
                log::debug!("Received Serve request");
                match serve_erofs(payload, pager.clone()) {
                    Ok(()) => {
                        responder.send(Ok(()))?;
                    }
                    Err(e) => {
                        log::error!("Failed to serve EROFS: {:?}", e);
                        responder.send(Err(map_to_status(e).into_raw()))?;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn serve_erofs(
    payload: fidl_fuchsia_erofs::ErofsServeRequest,
    pager: Arc<ErofsPager>,
) -> Result<(), anyhow::Error> {
    let backing_vmo =
        payload.backing_vmo.ok_or(zx::Status::INVALID_ARGS).context("Missing backing_vmo")?;
    let root = payload.root.ok_or(zx::Status::INVALID_ARGS).context("Missing root")?;

    log::info!("Serving new EROFS instance");
    ErofsVolume::serve(backing_vmo, pager, fio::PERM_READABLE, root)?;

    Ok(())
}
