// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_test_powerelementrunner::{ControlRequest, ControlRequestStream, ControlStartResult};
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::*;
use {fidl_fuchsia_power_broker as fbroker, fuchsia_async as fasync};

#[fuchsia::main]
async fn main() -> Result<()> {
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(|stream: ControlRequestStream| stream);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, serve_power_element_runner).await;
    Ok(())
}

async fn serve_power_element_runner(mut stream: ControlRequestStream) {
    let result: Result<()> = async move {
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                ControlRequest::Start {
                    initial_current_level,
                    element_name,
                    element_runner,
                    responder,
                } => {
                    let result =
                        run_power_element(initial_current_level, element_name, element_runner);
                    responder.send(result)?;
                }
                ControlRequest::_UnknownMethod { .. } => unimplemented!(),
            }
        }

        Ok(())
    }
    .await;

    if let Err(err) = result {
        error!("{:?}", err);
    }
}

fn run_power_element(
    initial_current_level: u8,
    element_name: String,
    element_runner: fidl::endpoints::ServerEnd<fbroker::ElementRunnerMarker>,
) -> ControlStartResult {
    let mut stream = element_runner.into_stream();
    fasync::Task::local(async move {
        let mut last_required_level = initial_current_level;

        log::debug!(
            element_name:?,
            last_required_level:?;
            "run_power_element: waiting for new level"
        );
        while let Some(request) = {
            log::debug!(
                element_name:?,
                last_required_level:?;
                "run_power_element: waiting for new level"
            );
            stream.try_next().await.expect("run_power_element: ElementRunner stream failed")
        } {
            match request {
                fbroker::ElementRunnerRequest::SetLevel { level, responder } => {
                    log::debug!(
                        element_name:?,
                        level:?,
                        last_required_level:?;
                        "run_power_element: SetLevel received"
                    );
                    fuchsia_trace::counter!(
                        c"power-broker", c"element_runner.set_level.update", 0,
                        &element_name => level as u32
                    );
                    if level == last_required_level {
                        log::debug!(
                            element_name:?,
                            level:?,
                            last_required_level:?;
                            "run_power_element: required level has not changed"
                        );
                    }

                    if let Err(e) = responder.send() {
                        error!(
                            "run_power_element: failed to send SetLevel response for {}: {:?}",
                            element_name, e
                        );
                    }
                    last_required_level = level;
                }
                fbroker::ElementRunnerRequest::_UnknownMethod { ordinal, .. } => {
                    error!(element_name:?, ordinal:?; "run_power_element: Unknown ElementRunnerRequest method");
                }
            }
        }
    })
    .detach();
    Ok(())
}
