// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![allow(clippy::too_many_arguments)]

use crate::fidl::{FidlServer, UpdateStateNotifier};
use crate::install_manager::start_install_manager;
use crate::update::{NamespaceEnvironmentConnector, RealUpdater, UpdateHistory};
use anyhow::anyhow;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_sync::Mutex;
use log::{error, info};
use std::sync::Arc;

mod fidl;
mod install_manager;
pub(crate) mod update;

#[fuchsia::main(logging_tags = ["system-updater"])]
async fn main() {
    info!("starting system updater");
    let structured_config = system_updater_config::Config::take_from_startup_handle();

    let inspector = fuchsia_inspect::Inspector::default();
    let _inspect_server_task =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let history_node = inspector.root().create_child("history");
    let history = Arc::new(Mutex::new(UpdateHistory::load(history_node).await));

    let mut fs = ServiceFs::new_local();
    if let Err(e) = fs.take_and_serve_directory_handle() {
        error!("error encountered serving directory handle: {:#}", anyhow!(e));
        std::process::exit(1);
    }

    // The install manager task will run the update attempt task,
    // listen for FIDL events, and notify monitors of update attempt progress.
    let updater = RealUpdater::new(history, structured_config);
    let attempt_node = inspector.root().create_child("install_manager");
    let (install_manager_ch, install_manager_fut) = start_install_manager::<
        UpdateStateNotifier,
        RealUpdater,
        NamespaceEnvironmentConnector,
    >(updater, attempt_node)
    .await;

    // The FIDL server will forward requests to the install manager task via the control handle.
    let server_fut = FidlServer::new(install_manager_ch).run(fs);

    // Start the tasks.
    futures::join!(fasync::Task::local(install_manager_fut), fasync::Task::local(server_fut));
}
