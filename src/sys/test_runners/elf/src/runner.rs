// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use log::{info, warn};
use test_runners_lib::elf;
use test_runners_lib::elf::SuiteServer;
use test_runners_lib::errors::*;
use thiserror::Error;
use {fidl_fuchsia_component_runner as fcrunner, fuchsia_async as fasync};

pub fn add_runner_service<F, U, S>(
    get_test_server: F,
    validate_args: U,
) -> Result<(), anyhow::Error>
where
    F: 'static + Fn() -> S + Send + Copy,
    U: 'static + Fn(&Vec<String>) -> Result<(), ArgumentError> + Copy,
    S: SuiteServer,
{
    info!("started");
    let mut executor = fasync::LocalExecutorBuilder::new().build();
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    fuchsia_trace_provider::trace_provider_wait_for_init();

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::local(async move {
            start_runner(stream, get_test_server, validate_args)
                .await
                .expect("failed to start runner.")
        })
        .detach();
    });
    fs.take_and_serve_directory_handle()?;
    executor.run_singlethreaded(fs.collect::<()>());
    Ok(())
}

/// Error encountered by runner.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("Cannot read request: {:?}", _0)]
    RequestRead(fidl::Error),
}

async fn start_runner<F, U, S>(
    mut stream: fcrunner::ComponentRunnerRequestStream,
    get_test_server: F,
    validate_args: U,
) -> Result<(), RunnerError>
where
    F: 'static + Fn() -> S + Send + Copy,
    U: 'static + Fn(&Vec<String>) -> Result<(), ArgumentError> + Copy,
    S: SuiteServer,
{
    while let Some(event) = stream.try_next().await.map_err(RunnerError::RequestRead)? {
        match event {
            fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                let url = start_info.resolved_url.clone().unwrap_or_else(|| "".to_owned());
                if let Err(e) =
                    elf::start_component(start_info, controller, get_test_server, validate_args)
                        .await
                {
                    warn!("Cannot start component '{}': {:?}", url, e)
                };
            }
            fcrunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown ComponentRunner request");
            }
        }
    }
    Ok(())
}
