// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod diagnostics;
mod events;
mod fuzzer;
mod manager;

#[cfg(test)]
mod test_support;

use crate::manager::Manager;
use anyhow::{Context as _, Error, Result};
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use futures::channel::mpsc;
use futures::{try_join, SinkExt, StreamExt, TryStreamExt};
use log::warn;
use {fidl_fuchsia_fuzzer as fuzz, fidl_fuchsia_test_manager as test_manager};

enum IncomingService {
    FuzzManager(fuzz::ManagerRequestStream),
}

struct SuiteRunnerEndpoint {}

impl manager::FidlEndpoint<test_manager::SuiteRunnerMarker> for SuiteRunnerEndpoint {
    fn create_proxy(&self) -> Result<test_manager::SuiteRunnerProxy, Error> {
        connect_to_protocol::<test_manager::SuiteRunnerMarker>()
    }
}

#[fuchsia::main(logging = true)]
async fn main() -> Result<()> {
    let (sender, receiver) = mpsc::unbounded::<fuzz::ManagerRequest>();
    let registry = connect_to_protocol::<fuzz::RegistryMarker>()
        .context("failed to connect to fuchsia.fuzzing.Registry")?;
    let suite_runner = SuiteRunnerEndpoint {};
    let manager = Manager::new(registry, suite_runner);
    let results = try_join!(multiplex_requests(sender), manager.serve(receiver));
    results.and(Ok(()))
}

// Concurrent calls to `connect` and `stop` can become complicated, especially given that the state
// of the fuzzer is replicated between the manager and registry. Moreover, fuzzing typically
// involves a small number of latency-tolerant clients perform testing. As a result, the simplest
// solution is to have the |fuzz::Manager| service multiple clients, but handle requests
// sequentially by multiplexing them into a single stream.
async fn multiplex_requests(sender: mpsc::UnboundedSender<fuzz::ManagerRequest>) -> Result<()> {
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingService::FuzzManager);
    fs.take_and_serve_directory_handle().context("failed to take and serve directory")?;
    const MAX_CONCURRENT: usize = 100;
    fs.for_each_concurrent(MAX_CONCURRENT, |IncomingService::FuzzManager(stream)| async {
        let sender = sender.clone();
        let result = stream.map_err(Error::msg).forward(sender.sink_map_err(Error::msg)).await;
        if let Err(e) = result {
            warn!("failed to forward fuzz-manager request: {:?}", e);
        }
    })
    .await;
    Ok(())
}
