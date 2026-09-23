// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod binder_latency;
mod debian_guest;
mod gbenchmark;
mod gtest;
mod helpers;
mod results_parser;
mod runner;
mod selinux;
mod syscalls_gtest;
mod syscalls_libtest;
mod test_suite;
mod vts_binary;

use crate::debian_guest::DebianGuest;

use anyhow::Error;
use fidl_fuchsia_component_runner as frunner;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt;
use log::debug;
use std::sync::Arc;

enum Services {
    ComponentRunner(frunner::ComponentRunnerRequestStream),
}

#[fuchsia::main(logging_tags=["starnix_test_runner"])]
async fn main() -> Result<(), Error> {
    debug!("starnix test runner started");
    let scope_dispatcher = libasync_scope_dispatcher::ScopeDispatcher::new();
    let trace_provider =
        fuchsia_trace_provider::TraceProvider::new_with_fdio(&scope_dispatcher, None);
    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(Services::ComponentRunner);
    fs.take_and_serve_directory_handle()?;

    // Some tests execute against both a Starnix environment and Linux environment via a virtualized
    // guest environment. The guest lifecycle needs to match that of the overall runner, so it's
    // instantiated here. The overhead of actually starting the virtualization process is done
    // lazily, only when someone attempts to interact with this guest object.
    let guest_name = String::from("linux_guest");
    let debian_guest = Arc::new(DebianGuest::new(guest_name));

    fs.for_each_concurrent(None, |request| async {
        match request {
            Services::ComponentRunner(stream) => {
                runner::handle_runner_requests(stream, debian_guest.clone())
                    .await
                    .expect("Error serving runner requests.")
            }
        }
    })
    .await;

    if let Err(e) = debian_guest.shutdown().await {
        log::warn!("Failed to gracefully shutdown the Debian guest: {}", e);
    }

    drop(trace_provider);
    scope_dispatcher.shutdown().await;

    Ok(())
}
