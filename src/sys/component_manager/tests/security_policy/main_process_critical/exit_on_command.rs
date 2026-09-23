// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_test_policy::{ExitControllerRequest, ExitControllerRequestStream};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use std::process;

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(
            run_service(stream).unwrap_or_else(|e| panic!("error running service: {:?}", e)),
        )
        .detach();
    });
    fs.take_and_serve_directory_handle().expect("failed to serve outgoing dir");
    fs.collect::<()>().await;
}

async fn run_service(mut stream: ExitControllerRequestStream) -> Result<(), Error> {
    if let Some(request) = stream.try_next().await? {
        match request {
            ExitControllerRequest::Exit { code, control_handle: _ } => {
                process::exit(code);
            }
        }
    }
    Ok(())
}
