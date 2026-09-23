// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::RecoveryMessages;
use anyhow::{Context as _, Error};
use fidl::endpoints::{DiscoverableProtocolMarker as _, create_proxy};
use fidl_fuchsia_fxfs as ffxfs;
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol;
use futures::TryStreamExt as _;
use isolated_swd::updater::Updater;
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable as _;

pub async fn apply_update(
    url: &str,
    view_sender: &crate::view_sender::ViewSender,
    exposed_dir: Arc<vfs::directory::simple::Simple>,
    svc_dir: Arc<vfs::directory::simple::Simple>,
) -> Result<(), Error> {
    view_sender.queue_message(RecoveryMessages::Log(format!("Mounting system blob volume...")));
    let fshost_recovery = connect_to_protocol::<fidl_fuchsia_fshost::RecoveryMarker>()
        .context("connecting to fshost Recovery")?;
    let (blob_exposed_dir, blob_exposed_dir_server) = create_proxy::<fio::DirectoryMarker>();

    fshost_recovery
        .mount_system_blob_volume(blob_exposed_dir_server)
        .await
        .context("calling MountSystemBlobVolume")?
        .map_err(zx::Status::err_from_raw)
        .context("mounting system blob volume")?;

    let blob_root = fuchsia_fs::directory::open_directory(
        &blob_exposed_dir,
        "root",
        fio::PERM_READABLE | fio::PERM_WRITABLE | fio::PERM_EXECUTABLE,
    )
    .await
    .context("opening blob root")?;
    exposed_dir
        .add_entry_may_overwrite("blob", vfs::remote::remote_dir(blob_root), true)
        .context("adding blob dir entry")?;

    let blob_svc =
        fuchsia_fs::directory::open_directory(&blob_exposed_dir, "svc", fio::PERM_READABLE)
            .await
            .context("opening blob svc")?;
    for protocol_name in
        [ffxfs::BlobCreatorMarker::PROTOCOL_NAME, ffxfs::BlobReaderMarker::PROTOCOL_NAME]
    {
        let blob_svc = Clone::clone(&blob_svc);
        svc_dir
            .add_entry_may_overwrite(
                protocol_name,
                vfs::service::endpoint(move |_scope, channel| {
                    if let Err(e) = blob_svc.open(
                        protocol_name,
                        fio::Flags::PROTOCOL_SERVICE,
                        &Default::default(),
                        channel.into(),
                    ) {
                        log::error!("Failed to call open on blob svc: {e}");
                    }
                }),
                true,
            )
            .with_context(|| format!("adding {protocol_name} entry"))?;
    }

    view_sender.queue_message(RecoveryMessages::Log(format!("Installing update...")));
    let res = install_update(url, view_sender).await;
    // Explicitly closing the `blob_exposed_dir` to let fshost shutdown the filesystem and destroy
    // the fxblob component, if not closed, this should still happen when `blob_exposed_dir` goes
    // out of scope. The `blob_root` and `blob_svc` handles are connected directly to the fxblob
    // component, and will be invalidated.
    blob_exposed_dir
        .close()
        .await
        .context("calling close")?
        .map_err(zx::Status::err_from_raw)
        .context("closing blob exposed dir")?;
    if let Err(e) = stop_pkg_recovery().await {
        log::error!("Failed to stop pkg-recovery: {e:#}");
    }
    res
}

async fn install_update(
    url: &str,
    view_sender: &crate::view_sender::ViewSender,
) -> Result<(), Error> {
    let mut updater = Updater::new().context("Failed to create updater")?;
    let mut attempt =
        updater.start_update(Some(&url.parse()?)).await.context("Failed to start update")?;
    view_sender.queue_message(RecoveryMessages::Log(format!("update started...")));
    while let Some(state) = attempt.try_next().await.context("fetching next update state")? {
        log::info!("Install: {:?}", state);
        let progress_str = if let Some(progress) = state.progress() {
            format!("{}: {:.2}%", state.name(), progress.fraction_completed() * 100.0)
        } else {
            state.name().into()
        };
        view_sender.queue_message(RecoveryMessages::ReplaceLastLog(progress_str));
        if state.is_success() {
            return Ok(());
        }
        if state.is_failure() {
            anyhow::bail!("update attempt failed in state {:?}", state);
        }
    }
    Err(anyhow::anyhow!("unexpected end of update attempt"))
}

async fn stop_pkg_recovery() -> Result<(), Error> {
    let lifecycle_controller =
        connect_to_protocol::<fidl_fuchsia_sys2::LifecycleControllerMarker>()
            .context("connecting to lifecycle controller")?;
    for moniker in
        ["./pkg-recovery/system-updater", "./pkg-recovery/pkg-resolver", "./pkg-recovery/pkg-cache"]
    {
        lifecycle_controller
            .stop_instance(moniker)
            .await
            .context("calling lifecycle controller")?
            .map_err(|e| anyhow::anyhow!("failed to stop {moniker}: {e:?}"))?;
    }
    Ok(())
}
