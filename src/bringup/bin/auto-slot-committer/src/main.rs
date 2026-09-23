// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The auto-slot-committer unconditionally marks the current slot as successful.
//!
//! The purpose here is for stability on bringup builds that do not contain the actual
//! `system-update-committer` component which normally does this. The bootloader decrements the
//! slot attempt count each time it boots, and if we don't mark a slot successful, the bootloader
//! will eventually decide it's a failed slot and will mark it unbootable. This results in
//! unexpectedly either flipping over to the other slot if it's bootable, or else failing to boot.
//!
//! To prevent this, we just mark the slot successful right away. Bringup builds don't need any
//! real system health checking, they are designed for early iteration and if we can boot far
//! enough to run this component that's enough of a signal that the slot is successful.

use anyhow::{Context, Error};
use fidl_fuchsia_paver::{BootManagerMarker, BootManagerProxy, Configuration, PaverMarker};
use fuchsia_component::client::connect_to_protocol;
use zx::Status;

macro_rules! log {
    ($fmt:expr $(, $arg:expr)* $(,)?) => {
        eprintln!(concat!("auto-slot-committer: ", $fmt) $(, $arg)*)
    };
}

async fn commit_slot(boot_manager: &BootManagerProxy) -> Result<(), Error> {
    let current = boot_manager
        .query_current_configuration()
        .await
        .context("querying current configuration")?
        .map_err(Status::err_from_raw)
        .context("failed to query current slot configuration")?;

    if current != Configuration::Recovery {
        // Don't bother checking if we're already healthy, the paver will do that internally.
        log!("Setting current configuration {:?} healthy", current);
        let status = boot_manager
            .set_configuration_healthy(current)
            .await
            .context("setting configuration healthy")?;
        Status::ok(status).context("failed to set configuration healthy")?;

        let status = boot_manager.flush().await.context("flushing configurations")?;
        Status::ok(status).context("failed to flush slot metadata")?;

        log!("Slot metadata successfully committed");
    } else {
        log!("Running in recovery, skipping slot commit");
    }

    Ok(())
}

#[fuchsia::main(logging = false)]
async fn main() -> Result<(), Error> {
    // Log to the kernel log. This component executes very early and we don't want to block waiting
    // for any logging systems to be ready. It's also useful to see the logs in context next to the
    // low-level boot-up/shutdown logs for debugging.
    //
    // However, these prints do sometimes get dropped, so failing to see a message in the log
    // does not necessarily indicate the component got stuck.
    if let Err(e) = stdout_to_debuglog::init().await {
        log!("Failed to redirect stdout/stderr to debuglog: {:?}", e);
    }
    log!("Started");

    let paver = connect_to_protocol::<PaverMarker>().context("connecting to paver")?;
    let (boot_manager, server) = fidl::endpoints::create_proxy::<BootManagerMarker>();
    paver.find_boot_manager(server).context("finding boot manager")?;

    commit_slot(&boot_manager).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_paver::BootManagerRequest;
    use futures::StreamExt;

    /// Tracks the calls we made into the boot manager.
    #[derive(Debug, Eq, PartialEq)]
    enum BootManagerCall {
        QueryCurrentConfiguration,
        SetConfigurationHealthy(Configuration),
        Flush,
    }

    /// Sets up a fake `BootManager` and calls `commit_slot()` on it.
    ///
    /// # Arguments
    ///
    /// * `current_slot`: the current slot that the fake boot manager should report
    ///
    /// # Returns
    ///
    /// The list of calls `commit_slot()` made against the paver.
    async fn run_commit_slot(current_slot: Configuration) -> Vec<BootManagerCall> {
        let mut calls = vec![];
        let (proxy, mut stream) = fidl::endpoints::create_proxy_and_stream::<BootManagerMarker>();

        let boot_manager_task = async {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    BootManagerRequest::QueryCurrentConfiguration { responder } => {
                        calls.push(BootManagerCall::QueryCurrentConfiguration);
                        responder.send(Ok(current_slot)).unwrap();
                    }
                    BootManagerRequest::SetConfigurationHealthy { configuration, responder } => {
                        calls.push(BootManagerCall::SetConfigurationHealthy(configuration));
                        responder.send(zx::sys::ZX_OK).unwrap();
                    }
                    BootManagerRequest::Flush { responder } => {
                        calls.push(BootManagerCall::Flush);
                        responder.send(zx::sys::ZX_OK).unwrap();
                    }
                    _ => panic!("unexpected request"),
                }
            }
        };

        let slot_committer_task = async {
            commit_slot(&proxy).await.unwrap();
            // Drop `proxy` to close the paver task `stream` or else it will wait forever.
            drop(proxy);
        };

        // Run both tasks on the current thread so that it can only borrow `calls` rather than
        // needing to move it, which lets us return it after they finish.
        futures::join!(boot_manager_task, slot_committer_task);

        calls
    }

    #[fuchsia::test]
    async fn test_successful_commit_a() {
        let calls = run_commit_slot(Configuration::A).await;
        assert_eq!(
            calls,
            [
                BootManagerCall::QueryCurrentConfiguration,
                BootManagerCall::SetConfigurationHealthy(Configuration::A),
                BootManagerCall::Flush
            ]
        );
    }

    #[fuchsia::test]
    async fn test_successful_commit_b() {
        let calls = run_commit_slot(Configuration::B).await;
        assert_eq!(
            calls,
            [
                BootManagerCall::QueryCurrentConfiguration,
                BootManagerCall::SetConfigurationHealthy(Configuration::B),
                BootManagerCall::Flush
            ]
        );
    }

    #[fuchsia::test]
    async fn test_skip_recovery() {
        let calls = run_commit_slot(Configuration::Recovery).await;
        // We should not attempt to set R successful since that's not a legal action.
        assert_eq!(calls, [BootManagerCall::QueryCurrentConfiguration,]);
    }
}
