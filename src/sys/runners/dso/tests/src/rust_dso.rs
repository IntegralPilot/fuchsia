// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fidl_test_dso as ftest;
use fuchsia_component::client;
use fuchsia_component::server::ServiceFs;
use fuchsia_dso::DsoAsyncArgs;
use futures::prelude::*;
use log::{info, warn};

enum IncomingService {
    TestHelper(ftest::TestHelperRequestStream),
}

async fn run_test_helper(mut stream: ftest::TestHelperRequestStream) -> Result<(), Error> {
    while let Some(request) = stream.try_next().await? {
        match request {
            ftest::TestHelperRequest::Ping { responder } => {
                info!("received Ping, replying pong");
                responder.send("pong")?;
            }
        }
    }
    Ok(())
}

#[fuchsia_dso::main(async, logging = true)]
pub async fn main(args: DsoAsyncArgs) {
    info!("main started");

    // Connect to TestHelper in namespace
    let svc_entry =
        args.incoming.into_iter().find(|entry| entry.path.to_string() == "/svc").unwrap();
    let svc_proxy = svc_entry.directory.into_proxy();
    let client =
        client::connect_to_protocol_at_dir_root::<ftest::TestHelperMarker>(&svc_proxy).unwrap();

    info!("sending ping to mock server");
    let response = client.ping().await.unwrap();
    assert_eq!(response, "pong");
    info!("ping succeeded");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingService::TestHelper);

    if let Some(outgoing_dir) = args.outgoing_dir {
        fs.serve_connection(outgoing_dir).unwrap();
    } else {
        panic!("outgoing_dir missing");
    }

    let lifecycle_fut = async {
        let mut stream = args.lifecycle.into_stream();
        info!("waiting for lifecycle events");
        if let Ok(Some(fidl_fuchsia_process_lifecycle::LifecycleRequest::Stop { .. })) =
            stream.try_next().await
        {
            info!("received Stop request");
        }
    };

    let fs_fut = fs.for_each_concurrent(None, |IncomingService::TestHelper(stream)| {
        run_test_helper(stream).unwrap_or_else(|e| warn!("error running test helper: {:?}", e))
    });

    futures::pin_mut!(lifecycle_fut);
    futures::pin_mut!(fs_fut);

    future::select(lifecycle_fut, fs_fut).await;
    info!("main exiting");
}
