// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::controller::serve_controller;
use crate::test::Test;
use crate::util::create_task;
use anyhow::{Context as _, Result};
use flex_fuchsia_fuzzer as fuzz;
use futures::StreamExt;
use zx_status as zx;

/// Serves `fuchsia.fuzzer.Manager` on the given `server_end` of a FIDL channel.
pub async fn serve_manager(
    server_end: flex_client::fidl::ServerEnd<fuzz::ManagerMarker>,
    mut test: Test,
) -> Result<()> {
    let mut stream = server_end.into_stream();
    let mut task = None;
    let url = test.url();
    let fake = test.controller();
    let writer = test.writer().clone();
    while let Some(request) = stream.next().await {
        let request = request.context("fuchsia.fuzzer/Manager")?;
        match request {
            fuzz::ManagerRequest::Connect { fuzzer_url, controller, responder } => {
                test.record(format!("fuchsia.fuzzer/Manager.Connect({})", fuzzer_url));
                let stream = controller.into_stream();
                {
                    let mut url_mut = url.borrow_mut();
                    *url_mut = Some(fuzzer_url);
                }
                responder.send(Ok(()))?;
                task = Some(create_task(serve_controller(stream, test.clone()), &writer));
            }
            fuzz::ManagerRequest::GetOutput { fuzzer_url, output, socket, responder } => {
                test.record(format!(
                    "fuchsia.fuzzer/Manager.GetOutput({}, {:?})",
                    fuzzer_url, output
                ));
                let running = {
                    let url = url.borrow();
                    url.clone().unwrap_or_default()
                };
                if fuzzer_url == running {
                    let response = match fake.set_output(output, socket) {
                        Ok(()) => Ok(()),
                        Err(status) => Err(status.into_raw()),
                    };
                    responder.send(response)?;
                } else {
                    responder.send(Err(zx::Status::NOT_FOUND.into_raw()))?;
                }
            }
            fuzz::ManagerRequest::Stop { fuzzer_url, responder } => {
                test.record(format!("fuchsia.fuzzer/Manager.Stop({})", fuzzer_url));
                let running = {
                    let mut url_mut = url.borrow_mut();
                    let running =
                        url_mut.as_ref().map_or_else(|| String::default(), |url| url.to_string());
                    *url_mut = Some(fuzzer_url.to_string());
                    running
                };
                if fuzzer_url == running {
                    task = None;
                    responder.send(Ok(()))?;
                } else {
                    responder.send(Err(zx::Status::NOT_FOUND.into_raw()))?;
                }
            }
        }
    }
    if let Some(task) = task.take() {
        task.await;
    }
    Ok(())
}
