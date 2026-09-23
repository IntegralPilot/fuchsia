// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use errors::{ffx_bail, ffx_error};
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_memory_heapdump_client as fheapdump_client;
use ffx_config::EnvironmentContext;
use ffx_profile_heapdump_common::{
    LabelValue, PProfProfileBuilder, build_process_selector, check_snapshot_error,
    connect_to_collector,
};
use ffx_profile_heapdump_snapshot_args::SnapshotCommand;
use ffx_writer::VerifiedMachineWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use std::io::Write;
use target_holders::RemoteControlProxyHolder;

#[derive(FfxTool)]
#[check(AvailabilityFlag("ffx_profile_heapdump"))]
pub struct SnapshotTool {
    #[command]
    cmd: SnapshotCommand,
    remote_control: RemoteControlProxyHolder,
    context: EnvironmentContext,
}

fho::embedded_plugin!(SnapshotTool);

#[async_trait(?Send)]
impl FfxMain for SnapshotTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        snapshot(&self.context, self.remote_control, self.cmd).await?;
        writer.machine(&())?;
        Ok(())
    }
}

struct SnapshotWithMetadata {
    process_name: Option<String>,
    process_koid: Option<u64>,
    contents_prefix: String,
    snapshot: heapdump_snapshot_fdomain::Snapshot,
}

async fn snapshot(
    context: &EnvironmentContext,
    remote_control: RemoteControlProxyHolder,
    cmd: SnapshotCommand,
) -> Result<()> {
    let contents_dir = cmd.output_contents_dir.as_ref().map(std::path::Path::new);
    let multi_process = cmd.multi_process;

    let (receiver_client, receiver_stream) =
        remote_control.domain().create_request_stream::<fheapdump_client::SnapshotReceiverMarker>();
    let request = fheapdump_client::CollectorTakeLiveSnapshotRequest {
        process_selector: build_process_selector(cmd.by_name, cmd.by_koid)?,
        receiver: Some(receiver_client),
        with_contents: Some(contents_dir.is_some()),
        multi_process: Some(multi_process),
        ..Default::default()
    };

    let collector: fheapdump_client::CollectorProxy =
        connect_to_collector(&remote_control, cmd.collector).await?;
    collector.take_live_snapshot(request)?;

    // Receive the snapshot(s).
    let snapshots: Vec<SnapshotWithMetadata> = if multi_process {
        check_snapshot_error(
            heapdump_snapshot_fdomain::Snapshot::receive_multi_from(receiver_stream).await,
        )?
        .into_iter()
        .map(
            |heapdump_snapshot_fdomain::SnapshotWithHeader {
                 process_name,
                 process_koid,
                 snapshot,
             }| {
                SnapshotWithMetadata {
                    process_name: Some(process_name),
                    process_koid: Some(process_koid),
                    contents_prefix: format!("{process_koid}-"),
                    snapshot: snapshot,
                }
            },
        )
        .collect()
    } else {
        vec![SnapshotWithMetadata {
            process_name: None,
            process_koid: None,
            contents_prefix: "".to_string(),
            snapshot: check_snapshot_error(
                heapdump_snapshot_fdomain::Snapshot::receive_single_from(receiver_stream).await,
            )?,
        }]
    };

    // If the user has requested the blocks' contents, ensure that `contents_dir` is an empty
    // directory (creating it if necessary), then dump the contents of each allocated block to a
    // different file.
    if let Some(contents_dir) = contents_dir {
        if let Ok(mut iterator) = std::fs::read_dir(contents_dir) {
            // While not strictly necessary, requiring that the target directory is empty makes it
            // much harder to accidentally flood important directories.
            if iterator.next().is_some() {
                ffx_bail!("Output directory is not empty: {}", contents_dir.display());
            }
        } else {
            if let Err(err) = std::fs::create_dir(contents_dir) {
                ffx_bail!("Failed to create output directory: {}: {}", contents_dir.display(), err);
            }
        }

        for snapshot in &snapshots {
            for info in &snapshot.snapshot.allocations {
                if let Some(data) = &info.contents {
                    let address = info.address.ok_or(ffx_error!(
                        "Cannot to create an output file for an allocation without address."
                    ))?;
                    let path =
                        contents_dir.join(format!("{}0x{:x}", snapshot.contents_prefix, address));
                    match std::fs::File::create(&path) {
                        Ok(mut file) => file.write_all(&data)?,
                        Err(err) => {
                            ffx_bail!("Failed to create output file: {}: {}", path.display(), err)
                        }
                    };
                }
            }
        }
    }

    // Always emit full metadata if the user requested the blocks' contents, as it serves as an
    // index for the generated files.
    let with_tags = cmd.with_tags || contents_dir.is_some();

    let mut profile_builder = PProfProfileBuilder::new(context, with_tags, cmd.symbolize);
    for snapshot in &snapshots {
        let mut extra_labels = vec![];
        if let Some(process_name) = &snapshot.process_name {
            extra_labels.push(("process_name", LabelValue::String(process_name.as_str())));
        }
        if let Some(process_koid) = &snapshot.process_koid {
            extra_labels.push(("process_koid", LabelValue::Number(*process_koid as i64)));
        }

        profile_builder.add(&snapshot.snapshot, extra_labels.as_slice())?;
    }
    profile_builder.write_to_file(&mut std::fs::File::create(&cmd.output_file).map_err(
        |err| ffx_error!("Failed to create output file: {}: {}", cmd.output_file, err),
    )?)?;

    Ok(())
}
