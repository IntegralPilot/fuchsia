// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::prelude::*;
use fidl::AsyncChannel;
use fidl_fuchsia_process_lifecycle::{LifecycleRequest, LifecycleRequestStream};
use fuchsia_async as fasync;
use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use fuchsia_runtime::{take_startup_handle, HandleInfo, HandleType};
use futures::TryStreamExt;
use log::{debug, info};

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    info!("Started diagnostics publisher");
    debug!("I'm a debug log from the publisher!");
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    component::health().set_ok();

    match take_startup_handle(HandleInfo::new(HandleType::Lifecycle, 0)) {
        Some(lifecycle_handle) => {
            let chan: zx::Channel = lifecycle_handle.into();
            let async_chan = AsyncChannel::from(fasync::Channel::from_channel(chan));
            let mut req_stream = LifecycleRequestStream::from_channel(async_chan);
            if let Some(LifecycleRequest::Stop { control_handle: c }) =
                req_stream.try_next().await.expect("Failure receiving lifecycle FIDL message")
            {
                info!("Finishing through Stop");
                c.shutdown();
                std::process::exit(0);
            }
            unreachable!("Unexpected closure of the lifecycle channel");
        }
        None => {
            info!("Finishing normally");
            std::process::abort();
        }
    }
}
