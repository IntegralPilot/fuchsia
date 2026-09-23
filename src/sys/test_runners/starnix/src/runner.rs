// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::debian_guest::DebianGuest;
use crate::helpers::clone_start_info;
use crate::test_suite::handle_suite_requests;
use anyhow::{Error, anyhow};
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_test as ftest;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use futures::{StreamExt, TryStreamExt};
use std::sync::Arc;

/// Handles a `fcrunner::ComponentRunnerRequestStream`.

///
/// When a run request arrives, the test suite protocol is served in the test component's outgoing
/// namespace, and then the component is run in response to `ftest::SuiteRequest::Run` requests.
///
/// See `test_suite` for more on how the test suite requests are handled.
pub async fn handle_runner_requests(
    mut request_stream: fcrunner::ComponentRunnerRequestStream,
    debian_guest: Arc<DebianGuest>,
) -> Result<(), Error> {
    while let Some(event) = request_stream.try_next().await? {
        match event {
            fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. } => {
                let debian_guest = debian_guest.clone();
                fasync::Task::local(async move {
                    serve_test_suite(start_info, controller, debian_guest)
                        .await
                        .expect("Starnix test runner failed to serve suite");
                    log::info!("Finished serving test suite for component.");
                })
                .detach();
            }
            fcrunner::ComponentRunnerRequest::_UnknownMethod { ordinal, .. } => {
                log::warn!(ordinal:%; "Unknown ComponentRunner request");
            }
        }
    }

    Ok(())
}

enum TestSuiteServices {
    Suite(ftest::SuiteRequestStream),
}

/// Serves a `ftest::SuiteRequestStream` within the `outgoing_dir` of `start_info`.
///
/// This function is used to serve a `ftest::SuiteRequestStream` in the outgoing directory of a test
/// component. This is what the test framework connects to to run test cases.
///
/// When the returned future completes, the outgoing directory has finished serving requests.
///
/// # Parameters
///   - `start_info`: The start info associated with the test component, used to instantiate
///                   the test container.
///   - `controller`: The server end of the component controller for the test component.
async fn serve_test_suite(
    mut start_info: fcrunner::ComponentStartInfo,
    controller: ServerEnd<fcrunner::ComponentControllerMarker>,
    debian_guest: Arc<DebianGuest>,
) -> Result<(), Error> {
    // Drop the runtime_dir handle because it's not supported for now.
    start_info.runtime_dir.take();
    let outgoing_dir =
        start_info.outgoing_dir.take().ok_or_else(|| anyhow!("Missing outgoing_dir"))?;

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(TestSuiteServices::Suite);
    fs.serve_connection(outgoing_dir)?;

    let controller = Arc::new(Mutex::new(Some(controller)));
    let start_info = Arc::new(Mutex::new(start_info));

    fs.for_each_concurrent(None, |request| async {
        match request {
            TestSuiteServices::Suite(stream) => {
                let controller = controller.clone();
                let start_info = start_info.clone();
                let start_info =
                    clone_start_info(&mut start_info.lock()).expect("Failed to clone start info");
                handle_suite_requests(start_info, stream, debian_guest.clone())
                    .await
                    .expect("Starnix test runner failed to serve suite requests");
                log::info!("Finished serving test suite requests.");
                let _ = controller.lock().take().map(|c| c.close_with_epitaph(Ok(())));
            }
        }
    })
    .await;
    Ok(())
}
