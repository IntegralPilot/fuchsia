// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{RequestStream, ServerEnd};
use fidl_fidl_test_components::TriggerRequestStream;
use fuchsia_component::server::{ServiceFs, ServiceFsDir};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::{StreamExt, TryStreamExt};
use {
    fidl_fuchsia_io as fio, fidl_fuchsia_process_lifecycle as flifecycle, fuchsia_async as fasync,
    zx,
};

/// See the `stop_with_pending_request` test case.
#[fuchsia::main]
pub async fn main() {
    // Check if the test wants us to stop right away. That is expressed by
    // the presence of the `USER_0` handle.
    match fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::User0, 0)) {
        Some(user_0) => escrow_outgoing_dir_then_stop(user_0).await,
        None => serve_outgoing_dir().await,
    }
}

async fn escrow_outgoing_dir_then_stop(user_0: zx::NullableHandle) {
    // Synchronize with test case first.
    fasync::OnSignals::new(&user_0, zx::Signals::OBJECT_PEER_CLOSED).await.unwrap();

    let outgoing_dir =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::DirectoryRequest, 0))
            .unwrap();
    let outgoing_dir = ServerEnd::<fio::DirectoryMarker>::from(outgoing_dir);

    // Also ensure the framework has processed the connection request, so that message is buffered
    // in the outgoing directory now.
    fasync::OnSignals::new(&outgoing_dir, zx::Signals::CHANNEL_READABLE).await.unwrap();

    let lifecycle =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0)).unwrap();
    let lifecycle = zx::Channel::from(lifecycle);
    let lifecycle = ServerEnd::<flifecycle::LifecycleMarker>::from(lifecycle);
    lifecycle
        .into_stream()
        .control_handle()
        .send_on_escrow(flifecycle::LifecycleOnEscrowRequest {
            outgoing_dir: Some(outgoing_dir),
            ..Default::default()
        })
        .unwrap();
}

async fn serve_outgoing_dir() {
    struct Trigger(TriggerRequestStream);

    let mut fs = ServiceFs::new();
    let _: &mut ServiceFsDir<'_, _> = fs.dir("svc").add_fidl_service(Trigger);
    let _: &mut ServiceFs<_> = fs.take_and_serve_directory_handle().unwrap();
    let () = fs
        .for_each_concurrent(None, |mut trigger| async move {
            while let Ok(Some(request)) = trigger.0.try_next().await {
                match request {
                    fidl_fidl_test_components::TriggerRequest::Run { responder } => {
                        responder.send("hello").unwrap()
                    }
                }
            }
        })
        .await;
}
