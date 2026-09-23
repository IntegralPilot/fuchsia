// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![warn(clippy::all, clippy::pedantic, clippy::unwrap_used)]

use anyhow::Error;
use fidl_fuchsia_component as fcomponent;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::server::ServiceFs;
use log::info;
use session_manager_config::Config;
use session_manager_lib::session_manager::SessionManager;

// TODO(https://fxbug.dev/508683746): Remove tag once fixed.
#[fuchsia::main(logging_tags = ["session-manager"])]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new_local();
    let inspector = fuchsia_inspect::component::inspector();
    let _inspect_server_task =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default());

    let realm = connect_to_protocol::<fcomponent::RealmMarker>()?;

    // Start the startup session, if any, and serve services exposed by session manager.
    let Config { session_url, autolaunch, suspend_enabled, enable_debug_shortcut } =
        Config::take_from_startup_handle();
    let is_session_url_empty = session_url.is_empty();
    let mut session_manager = SessionManager::new(
        realm,
        inspector,
        (!is_session_url_empty).then_some(session_url),
        suspend_enabled,
        enable_debug_shortcut,
    );

    if is_session_url_empty {
        info!("Received an empty startup session URL. Waiting for a request.");
    } else if !autolaunch {
        info!(
            "Startup session URL set, but autolaunch config option was false. Waiting for a request."
        );
    } else {
        // TODO(https://fxbug.dev/42146741): Using ? here causes errors to not be logged.
        session_manager.start_default_session().await.expect("failed to start session");
    }

    session_manager.serve(&mut fs).await.expect("failed to serve protocols");

    Ok(())
}
