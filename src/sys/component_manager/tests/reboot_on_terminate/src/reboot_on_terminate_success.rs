// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::client;
use futures::stream::TryStreamExt as _;
use log::*;
use {fidl_fidl_test_components as ftest, fidl_fuchsia_process_lifecycle as flifecycle};

#[fuchsia::main(logging_tags = ["reboot-on-terminate-success"])]
async fn main() {
    info!("start");

    // The `on_terminate: "reboot"` child component `critical_child` will exit, causing CM to reboot
    // the system, which should result in this component being told to stop.
    let mut lifecycle_request_stream =
        fidl::endpoints::ServerEnd::<flifecycle::LifecycleMarker>::from(
            fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
                fuchsia_runtime::HandleType::Lifecycle,
                0,
            ))
            .unwrap(),
        )
        .into_stream();

    let Some(req) = lifecycle_request_stream.try_next().await.unwrap() else {
        panic!("lifecycle request stream closed before stop was called");
    };

    match req {
        flifecycle::LifecycleRequest::Stop { .. } => (),
    }

    let trigger = client::connect_to_protocol::<ftest::TriggerMarker>().unwrap();
    trigger.run().await.unwrap();
}
