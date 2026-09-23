// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod test_server;

use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use log::{info, warn};
use std::fs;
use std::path::Path;
use test_runners_lib::elf;
use test_server::TestServer;
use thiserror::Error;
use {fidl_fuchsia_component_runner as fcrunner, fidl_fuchsia_io as fio, fuchsia_async as fasync};

#[cfg(feature = "gtest")]
#[fuchsia::main(logging_tags=["gtest_runner"])]
async fn main() -> Result<(), anyhow::Error> {
    main_impl().await
}

#[cfg(feature = "gunit")]
#[fuchsia::main(logging_tags=["gunit_runner"])]
async fn main() -> Result<(), anyhow::Error> {
    main_impl().await
}

#[derive(argh::FromArgs, Debug, Clone)]
/// runner for gtest binaries
struct Args {
    #[argh(switch)]
    /// if set, give each child test a duplicate of the incoming vDSO.
    /// this is necessary for tests that expect the "next vDSO"
    duplicate_vdso_for_children: bool,
}

async fn main_impl() -> Result<(), anyhow::Error> {
    info!("started");

    let args = argh::from_env::<Args>();
    fuchsia_trace_provider::trace_provider_create_with_fdio();
    fuchsia_trace_provider::trace_provider_wait_for_init();
    // We will divide this directory up and pass to  tests as /test_result so that they can write
    // their json output
    let path = Path::new("/data/test_data");
    // the directory might already be present so use create_dir_all.
    fs::create_dir_all(&path).expect("cannot create directory to store test results.");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(move |stream| {
        let args = args.clone();
        fasync::Task::local(async move {
            start_runner(stream, args).await.expect("failed to start runner.")
        })
        .detach();
    });
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

/// Error encountered by runner.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("Cannot read request: {:?}", _0)]
    RequestRead(fidl::Error),
}

async fn start_runner(
    mut stream: fcrunner::ComponentRunnerRequestStream,
    args: Args,
) -> Result<(), RunnerError> {
    let duplicate_vdso_for_children = args.duplicate_vdso_for_children;
    while let Some(event) = stream.try_next().await.map_err(RunnerError::RequestRead)? {
        match event {
            fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                let url = start_info.resolved_url.clone().unwrap_or_else(|| "".to_owned());
                if let Err(e) = elf::start_component(
                    start_info,
                    controller,
                    move || {
                        get_new_test_server()
                            .with_duplicate_vdso_for_children(duplicate_vdso_for_children)
                    },
                    TestServer::validate_args,
                )
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

fn get_new_test_server() -> TestServer {
    let test_data_name = format!("{}", rand::random::<u64>());
    let test_data_dir_parent = "/data/test_data".to_owned();
    let test_data_path = format!("{}/{}", test_data_dir_parent, test_data_name);

    // TODO(https://fxbug.dev/42122426): use async lib.
    fs::create_dir(&test_data_path).expect("cannot create test output directory.");
    let test_data_dir = fuchsia_fs::directory::open_in_namespace(
        &test_data_path,
        fio::PERM_READABLE | fio::PERM_WRITABLE,
    )
    .expect("Cannot open data directory");

    TestServer::new(test_data_dir, test_data_name, test_data_dir_parent)
}
