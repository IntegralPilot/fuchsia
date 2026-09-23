// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use fidl_fuchsia_update_installer::{InstallerMarker, InstallerProxy, RebootControllerMarker};
use fidl_fuchsia_update_installer_ext::{self as installer, Options, StateId, start_update};
use fuchsia_component::client::connect_to_protocol;
use futures::prelude::*;

pub async fn handle_force_install(
    update_pkg_url: String,
    reboot: bool,
    service_initiated: bool,
) -> Result<(), Error> {
    let installer = connect_to_protocol::<InstallerMarker>()
        .context("connecting to fuchsia.update.installer")?;
    handle_force_install_impl(update_pkg_url, reboot, service_initiated, &installer).await
}

async fn handle_force_install_impl(
    update_url: String,
    reboot: bool,
    service_initiated: bool,
    installer: &InstallerProxy,
) -> Result<(), Error> {
    let update_url = update_url.parse().context("parsing update url")?;

    let options = Options {
        initiator: if service_initiated {
            installer::Initiator::Service
        } else {
            installer::Initiator::User
        },
        should_write_recovery: true,
        allow_attach_to_existing_attempt: true,
        manifest_range: None,
        manifest_headers: vec![],
    };

    let (reboot_controller, reboot_controller_server_end) =
        fidl::endpoints::create_proxy::<RebootControllerMarker>();

    let mut update_attempt =
        start_update(&update_url, options, installer, Some(reboot_controller_server_end))
            .await
            .context("starting update")?;

    println!(
        "Installing an update.
Progress reporting is based on the fraction of packages resolved, so if one package is much
larger than the others, then the reported progress could appear to stall near the end.
Until the update process is improved to have more granular reporting, try using
    ffx inspect show 'core/pkg-resolver'
for more detail on the progress of update-related downloads.
"
    );
    if !reboot {
        reboot_controller.detach().context("notify installer do not reboot")?;
    }
    while let Some(state) = update_attempt.try_next().await.context("getting next state")? {
        println!("State: {state:?}");
        if state.id() == StateId::WaitToReboot {
            if reboot {
                return Ok(());
            }
        } else if state.is_success() {
            return Ok(());
        } else if state.is_failure() {
            anyhow::bail!("Encountered failure state");
        }
    }

    Err(anyhow!("Installation ended unexpectedly"))
}
