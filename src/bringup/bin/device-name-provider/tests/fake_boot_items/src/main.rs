// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use fidl_fuchsia_boot::{ItemsRequest, ItemsRequestStream};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;

enum IncomingRequest {
    Items(ItemsRequestStream),
}

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), anyhow::Error> {
    let mut service_fs = ServiceFs::new_local();

    service_fs.dir("svc").add_fidl_service(IncomingRequest::Items);
    service_fs.take_and_serve_directory_handle().context("failed to serve outgoing namespace")?;

    service_fs
        .for_each_concurrent(None, |request: IncomingRequest| async move {
            match request {
                IncomingRequest::Items(stream) => handle_items_request(stream).await,
            }
        })
        .await;

    Ok(())
}

async fn handle_items_request(mut stream: ItemsRequestStream) {
    while let Some(event) = stream.try_next().await.expect("failed to serve items service") {
        match event {
            ItemsRequest::Get { type_, extra, responder } => {
                let (result, len) = if type_ != zbi::Type::DrvMacAddress as u32 || extra != 0 {
                    (None, 0)
                } else {
                    let vmo = zx::Vmo::create(6).unwrap();
                    vmo.write(&[0x45, 0x67, 0x89, 0xab, 0xcd, 0xef], 0).unwrap();
                    (Some(vmo), 6)
                };
                responder.send(result, len).unwrap();
            }
            ItemsRequest::Get2 { responder, .. } => {
                responder.send(Err(zx::Status::NOT_SUPPORTED.into_raw())).unwrap()
            }
            ItemsRequest::GetBootloaderFile { responder, .. } => responder.send(None).unwrap(),
        }
    }
}
