// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod test;

use anyhow::*;
use fidl::endpoints::{ServerEnd, create_proxy};
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use log::{error, info};
use test::StressTest;
use {
    fidl_fuchsia_component_runner as fcrunner, fidl_fuchsia_io as fio, fidl_fuchsia_test as ftest,
    fuchsia_async as fasync,
};

#[fuchsia::main]
async fn main() -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(serve_runner(stream).map(|r| info!("Serving runner: {:?}", r)))
            .detach();
    });
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}

async fn serve_runner(mut stream: fcrunner::ComponentRunnerRequestStream) -> Result<()> {
    while let Some(fcrunner::ComponentRunnerRequest::Start { start_info, controller, .. }) =
        stream.try_next().await?
    {
        let controller_stream = controller.into_stream();
        fasync::Task::spawn(
            serve_controller(controller_stream).map(|r| info!("Serving controller: {:?}", r)),
        )
        .detach();

        let dictionary = start_info.program.ok_or_else(|| anyhow!("No program dictionary"))?;
        let dictionary =
            dictionary.entries.ok_or_else(|| anyhow!("No entries in program dictionary"))?;
        let namespace = start_info.ns.ok_or_else(|| anyhow!("No incoming namespace"))?;
        let test = StressTest::new(dictionary, namespace)?;
        let out_dir = start_info.outgoing_dir.ok_or_else(|| anyhow!("No outgoing directory"))?;
        fasync::Task::spawn(
            serve_out_dir(out_dir, test).map(|r| info!("Serving out dir: {:?}", r)),
        )
        .detach();
    }
    Ok(())
}

/// TODO(xbhatnag): All futures should be aborted when a Stop/Kill request is received.
async fn serve_controller(mut stream: fcrunner::ComponentControllerRequestStream) -> Result<()> {
    if let Some(request) = stream.try_next().await? {
        info!("Received controller request: {:?}", request);
    }
    Ok(())
}

async fn serve_out_dir(out_dir: ServerEnd<fio::DirectoryMarker>, test: StressTest) -> Result<()> {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(
            serve_test_suite(stream, test.clone()).map(|r| info!("Serving test suite: {:?}", r)),
        )
        .detach();
    });
    fs.serve_connection(out_dir)?;
    fs.collect::<()>().await;
    Ok(())
}

/// Implements `fuchsia.test.Suite` service and runs test.
async fn serve_test_suite(mut stream: ftest::SuiteRequestStream, test: StressTest) -> Result<()> {
    while let Some(event) = stream.try_next().await? {
        match event {
            ftest::SuiteRequest::GetTests { iterator, control_handle: _ } => {
                fasync::Task::spawn(
                    serve_case_iterator(iterator).map(|r| info!("Serving case iterator: {:?}", r)),
                )
                .detach();
            }
            ftest::SuiteRequest::Run { tests, listener, .. } => {
                let listener = listener.into_proxy();

                for invocation in tests {
                    let (case_listener, server_end) = create_proxy::<ftest::CaseListenerMarker>();

                    // TODO(84852): Use stderr to print status of actors
                    let (stderr_client, stderr_server) = zx::Socket::create_datagram();
                    let std_handles = ftest::StdHandles {
                        out: None,
                        err: Some(stderr_server),
                        ..Default::default()
                    };
                    listener.on_test_case_started(&invocation, std_handles, server_end)?;

                    let result = if let Err(e) = test.clone().start().await {
                        if let Err(status) = stderr_client.write(e.to_string().as_bytes()) {
                            error!(
                                "Failed to write error to stderr socket [write status={}][error={:?}]",
                                status, e
                            )
                        }
                        ftest::Result_ { status: Some(ftest::Status::Failed), ..Default::default() }
                    } else {
                        ftest::Result_ { status: Some(ftest::Status::Passed), ..Default::default() }
                    };

                    case_listener.finished(&result)?;
                }
                listener.on_finished()?;
            }
        }
    }
    Ok(())
}

async fn serve_case_iterator(iterator: ServerEnd<ftest::CaseIteratorMarker>) -> Result<()> {
    let mut stream = iterator.into_stream();
    let cases = &[ftest::Case {
        name: Some("stress_test".to_string()),
        enabled: Some(true),
        ..Default::default()
    }];

    // Send the `stress_test` case in the first call
    if let Some(ftest::CaseIteratorRequest::GetNext { responder }) = stream.try_next().await? {
        responder.send(cases)?;
    }

    // Send an empty response in the second call. This instructs the test_manager that there are
    // no more cases in this test suite.
    if let Some(ftest::CaseIteratorRequest::GetNext { responder }) = stream.try_next().await? {
        responder.send(&[])?;
    }
    Ok(())
}
