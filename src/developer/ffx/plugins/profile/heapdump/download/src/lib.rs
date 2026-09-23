// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use errors::ffx_error;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_memory_heapdump_client as fheapdump_client;
use ffx_config::EnvironmentContext;
use ffx_profile_heapdump_common::{
    PProfProfileBuilder, check_snapshot_error, connect_to_collector,
};
use ffx_profile_heapdump_download_args::DownloadCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
#[check(AvailabilityFlag("ffx_profile_heapdump"))]
pub struct DownloadTool {
    #[command]
    cmd: DownloadCommand,
    remote_control: RemoteControlProxyHolder,
    context: EnvironmentContext,
}

fho::embedded_plugin!(DownloadTool);

#[async_trait(?Send)]
impl FfxMain for DownloadTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        download(&self.context, self.remote_control, self.cmd).await?;
        writer.machine(&())?;
        Ok(())
    }
}

async fn download(
    context: &EnvironmentContext,
    remote_control: RemoteControlProxyHolder,
    cmd: DownloadCommand,
) -> Result<()> {
    let (receiver_client, receiver_stream) =
        remote_control.domain().create_request_stream::<fheapdump_client::SnapshotReceiverMarker>();
    let request = fheapdump_client::CollectorDownloadStoredSnapshotRequest {
        snapshot_id: Some(cmd.snapshot_id),
        receiver: Some(receiver_client),
        ..Default::default()
    };

    let collector: fheapdump_client::CollectorProxy =
        connect_to_collector(&remote_control, cmd.collector).await?;
    collector.download_stored_snapshot(request)?;
    let snapshot = check_snapshot_error(
        heapdump_snapshot_fdomain::Snapshot::receive_single_from(receiver_stream).await,
    )?;

    let mut builder = PProfProfileBuilder::new(context, cmd.with_tags, cmd.symbolize);
    builder.add(&snapshot, &[])?;
    builder.write_to_file(&mut std::fs::File::create(&cmd.output_file).map_err(|err| {
        ffx_error!("Failed to create output file: {}: {}", cmd.output_file, err)
    })?)?;

    Ok(())
}
