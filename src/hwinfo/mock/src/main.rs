// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail};
use fidl_fuchsia_hwinfo::{
    BoardInfo, BoardRequest, BoardRequestStream, DeviceInfo, DeviceRequest, DeviceRequestStream,
    ProductInfo, ProductRequest, ProductRequestStream,
};
use fidl_fuchsia_hwinfo_mock::{SetterRequest, SetterRequestStream};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use fuchsia_sync::Mutex;
use futures::prelude::*;
use std::sync::{Arc, LazyLock};

enum IncomingRequest {
    Product(ProductRequestStream),
    Board(BoardRequestStream),
    Device(DeviceRequestStream),
    Setter(SetterRequestStream),
}

struct ReturnValues {
    board: BoardInfo,
    product: ProductInfo,
    device: DeviceInfo,
}

static RETURN_VALUES: LazyLock<Arc<Mutex<Option<ReturnValues>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));

#[fuchsia::main(logging = true)]
async fn main() -> Result<()> {
    let mut service_fs = ServiceFs::new_local();

    // Initialize inspect
    component::health().set_starting_up();

    service_fs.dir("svc").add_fidl_service(IncomingRequest::Product);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Board);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Device);
    service_fs.dir("svc").add_fidl_service(IncomingRequest::Setter);

    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    component::health().set_ok();
    log::debug!("Initialized.");

    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );

    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| async move {
            match request {
                IncomingRequest::Product(stream) => fasync::Task::spawn(async move {
                    if let Err(e) = handle_product(stream).await {
                        log::error!("Product handling failed: {:?}", e);
                    }
                })
                .detach(),
                IncomingRequest::Board(stream) => fasync::Task::spawn(async move {
                    if let Err(e) = handle_board(stream).await {
                        log::error!("Board handling failed: {:?}", e);
                    }
                })
                .detach(),
                IncomingRequest::Device(stream) => fasync::Task::spawn(async move {
                    if let Err(e) = handle_device(stream).await {
                        log::error!("Device handling failed: {:?}", e);
                    }
                })
                .detach(),
                IncomingRequest::Setter(stream) => {
                    fasync::Task::spawn(handle_setter(stream)).detach()
                }
            }
            // match on `request` and handle each protocol.
        })
        .await;

    Ok(())
}

async fn handle_product(mut stream: ProductRequestStream) -> Result<()> {
    while let Some(Ok(req)) = stream.next().await {
        match req {
            ProductRequest::GetInfo { responder } => {
                let locked = RETURN_VALUES.lock();
                if locked.is_none() {
                    bail!("Return values have not been set in mock.");
                }
                responder.send(&locked.as_ref().unwrap().product)?;
            }
        }
    }
    Ok(())
}

async fn handle_board(mut stream: BoardRequestStream) -> Result<()> {
    while let Some(Ok(req)) = stream.next().await {
        match req {
            BoardRequest::GetInfo { responder } => {
                let locked = RETURN_VALUES.lock();
                if locked.is_none() {
                    bail!("Return values have not been set in mock.");
                }
                responder.send(&locked.as_ref().unwrap().board)?;
            }
        }
    }
    Ok(())
}

async fn handle_device(mut stream: DeviceRequestStream) -> Result<()> {
    while let Some(Ok(req)) = stream.next().await {
        match req {
            DeviceRequest::GetInfo { responder } => {
                let locked = RETURN_VALUES.lock();
                if locked.is_none() {
                    bail!("Return values have not been set in mock.");
                }
                responder.send(&locked.as_ref().unwrap().device)?;
            }
        }
    }
    Ok(())
}

async fn handle_setter(mut stream: SetterRequestStream) {
    while let Some(Ok(req)) = stream.next().await {
        match req {
            SetterRequest::SetResponses { device, product, board, responder } => {
                let mut locked = RETURN_VALUES.lock();
                *locked = Some(ReturnValues { board, product, device });
                responder.send().ok();
            }
        }
    }
}
