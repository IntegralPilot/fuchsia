// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The reachability monitor monitors reachability state and generates an event to signal
//! changes.

use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use futures::channel::mpsc::unbounded;
use futures::{FutureExt as _, StreamExt as _};
use log::info;
use reachability_core::{Monitor, NetworkCheckAction, NetworkCheckCookie};
use reachability_handler::ReachabilityHandler;
use std::pin::pin;

mod eventloop;

use crate::eventloop::EventLoop;

#[fuchsia::main(logging_tags = ["reachability"])]
pub fn main() {
    // TODO(dpradilla): use a `StructOpt` to pass in a log level option where the user can control
    // how verbose logs should be.
    info!("Starting reachability monitor!");
    let mut executor = fuchsia_async::LocalExecutorBuilder::new().build();

    let mut fs = ServiceFs::new_local();

    let mut handler = ReachabilityHandler::new();
    handler.publish_service(fs.dir("svc"));

    let inspector = component::inspector();
    // Report data on the size of the inspect VMO, and the number of allocation
    // failures encountered. (Allocation failures can lead to missing data.)
    component::serve_inspect_stats();

    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default())
            .expect("publish Inspect task");

    let fs = fs.take_and_serve_directory_handle().expect("failed to serve ServiceFS directory");

    let (sender, receiver) = unbounded::<(NetworkCheckAction, NetworkCheckCookie)>();
    let mut monitor = Monitor::new(sender).expect("failed to create reachability monitor");
    let () = monitor.set_inspector(inspector);

    info!("monitoring");
    let mut eventloop = EventLoop::new(monitor, handler, receiver, inspector);
    let mut eventloop_fut = pin!(eventloop.run().fuse());
    let mut serve_fut = pin!(fs.fuse().collect());

    executor.run_singlethreaded(async {
        futures::select! {
            r = eventloop_fut => {
                let r: Result<(), anyhow::Error> = r;
                panic!("unexpectedly exited event loop with result {:?}", r);
            },
            () = serve_fut => {
                panic!("unexpectedly stopped serving ServiceFS");
            },
        }
    })
}
