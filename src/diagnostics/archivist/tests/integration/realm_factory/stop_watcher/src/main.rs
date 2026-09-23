// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use component_events::events::*;
use component_events::matcher::*;
use fidl_fuchsia_archivist_test::{
    ExitStatus, ExitStatusUnknown, StopWaiterRequest, StopWaiterRequestStream, StopWatcherRequest,
    StopWatcherRequestStream,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::{Future, StreamExt, TryStreamExt};
use log::error;

enum Incoming {
    StopWatcher(StopWatcherRequestStream),
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    let scope = fasync::Scope::new();

    fs.dir("svc").add_fidl_service(Incoming::StopWatcher);
    fs.take_and_serve_directory_handle()?;
    fs.for_each_concurrent(0, |connection| async {
        match connection {
            Incoming::StopWatcher(stream) => {
                scope.spawn(serve_stop_watcher(stream));
            }
        }
    })
    .await;

    scope.join().await;

    Ok(())
}

async fn serve_stop_watcher(mut stream: StopWatcherRequestStream) {
    let scope = fasync::Scope::new();
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            StopWatcherRequest::WatchComponent { moniker, expected_exit, responder } => {
                let (client_end, request_stream) = fidl::endpoints::create_request_stream();
                let checker = StopChecker::new().await;
                let fut = checker.wait_for_component(moniker, expected_exit);
                scope.spawn(serve_stop_waiter(request_stream, fut));
                let _ = responder.send(client_end);
            }
            StopWatcherRequest::_UnknownMethod { .. } => {
                error!("Unknown stop waiter request received. Ignoring");
            }
        }
    }
    scope.join().await
}

async fn serve_stop_waiter<F>(mut stream: StopWaiterRequestStream, fut: F)
where
    F: Future<Output = ()>,
{
    let mut fut = Some(fut);
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            StopWaiterRequest::Wait { responder } => {
                if let Some(fut) = fut.take() {
                    fut.await;
                }
                let _ = responder.send();
            }
            StopWaiterRequest::_UnknownMethod { .. } => {
                error!("Unknown stop waiter request received. Ignoring");
            }
        }
    }
}

pub struct StopChecker {
    event_stream: EventStream,
}

impl StopChecker {
    pub async fn new() -> Self {
        Self { event_stream: EventStream::open().await.unwrap() }
    }

    pub async fn wait_for_component(mut self, moniker: String, status: ExitStatus) {
        let matcher = match status {
            ExitStatus::Any => None,
            ExitStatus::Crash => Some(ExitStatusMatcher::AnyCrash),
            ExitStatus::Clean => Some(ExitStatusMatcher::Clean),
            ExitStatusUnknown!() => {
                error!("unexpected status received. ignoring it.");
                None
            }
        };
        EventMatcher::ok()
            .stop(matcher)
            .moniker(&moniker)
            .wait::<Stopped>(&mut self.event_stream)
            .await
            .unwrap();
    }
}
