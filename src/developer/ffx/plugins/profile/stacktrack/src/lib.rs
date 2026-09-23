// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_memory_stacktrack_client as fstacktrack_client;
use ffx_config::EnvironmentContext;
use ffx_profile_stacktrack_args::StackTrackCommand;
use ffx_profile_stacktrack_common::{
    ResolvedSnapshot, build_process_selector, connect_to_collector,
};
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool};
use stacktrack_snapshot_fdomain as stacktrack_snapshot;
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
pub struct StackTrackTool {
    #[command]
    cmd: StackTrackCommand,
    remote_control: RemoteControlProxyHolder,
    context: EnvironmentContext,
}

fho::embedded_plugin!(StackTrackTool);

#[async_trait(?Send)]
impl FfxMain for StackTrackTool {
    type Writer = MachineWriter<ResolvedSnapshot>;
    type Error = fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        stacktrack(self.context, self.remote_control, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn stacktrack(
    context: EnvironmentContext,
    remote_control: RemoteControlProxyHolder,
    cmd: StackTrackCommand,
    writer: &mut MachineWriter<ResolvedSnapshot>,
) -> Result<()> {
    // Connect to the collector and receive the snapshot.
    let selector = build_process_selector(cmd.by_name, cmd.by_koid)?;
    let proxy = connect_to_collector(&remote_control, cmd.collector).await?;
    let (receiver_client, receiver_stream) =
        proxy.domain().create_request_stream::<fstacktrack_client::SnapshotReceiverMarker>();
    proxy
        .get_stack_traces(fstacktrack_client::CollectorGetStackTracesRequest {
            process_selector: selector,
            receiver: Some(receiver_client),
            ..Default::default()
        })
        .map_err(|e| anyhow::anyhow!(e))?;
    let snapshot = stacktrack_snapshot::Snapshot::receive_from(receiver_stream).await?;

    let resolved_snapshot = ResolvedSnapshot::new(Some(&context), &snapshot)?;
    if writer.is_machine() {
        writer.machine(&resolved_snapshot)?;
    } else {
        resolved_snapshot.print_markdown(writer)?;
    }

    Ok(())
}
