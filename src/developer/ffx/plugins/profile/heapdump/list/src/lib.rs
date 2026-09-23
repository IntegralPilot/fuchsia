// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use async_trait::async_trait;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_memory_heapdump_client as fheapdump_client;
use ffx_profile_heapdump_common::{
    build_process_selector, connect_to_collector, prettify_collector_error,
};
use ffx_profile_heapdump_list_args::ListCommand;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use prettytable::format::FormatBuilder;
use prettytable::{Table, row};
use serde::Serialize;
use target_holders::RemoteControlProxyHolder;

/// Representation of the [fheapdump_client::StoredSnapshot] FIDL type for machine output.
#[derive(Serialize)]
pub struct StoredSnapshot {
    snapshot_id: u32,
    snapshot_name: String,
    process_koid: u64,
    process_name: String,
}

async fn receive_list_of_stored_snapshots(
    iterator: fheapdump_client::StoredSnapshotIteratorProxy,
) -> Result<Vec<StoredSnapshot>> {
    let mut result = Vec::new();
    loop {
        let batch = iterator.get_next().await?.map_err(prettify_collector_error)?;
        if batch.is_empty() {
            break;
        }

        result.reserve(batch.len());
        for elem in batch {
            result.push(StoredSnapshot {
                snapshot_id: elem.snapshot_id.context("missing snapshot_id")?,
                snapshot_name: elem.snapshot_name.context("missing snapshot_name")?,
                process_koid: elem.process_koid.context("missing process_koid")?,
                process_name: elem.process_name.context("missing process_name")?,
            });
        }
    }

    result.sort_by_key(|snapshot| snapshot.snapshot_id);
    return Ok(result);
}

#[derive(FfxTool)]
#[check(AvailabilityFlag("ffx_profile_heapdump"))]
pub struct ListTool {
    #[command]
    cmd: ListCommand,
    remote_control: RemoteControlProxyHolder,
}

fho::embedded_plugin!(ListTool);

#[async_trait(?Send)]
impl FfxMain for ListTool {
    type Writer = MachineWriter<Vec<StoredSnapshot>>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        list(self.remote_control, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn list(
    remote_control: RemoteControlProxyHolder,
    cmd: ListCommand,
    writer: &mut MachineWriter<Vec<StoredSnapshot>>,
) -> Result<()> {
    let (iterator_proxy, iterator_server) =
        remote_control.domain().create_proxy::<fheapdump_client::StoredSnapshotIteratorMarker>();
    let request = fheapdump_client::CollectorListStoredSnapshotsRequest {
        iterator: Some(iterator_server),
        process_selector: build_process_selector(cmd.by_name, cmd.by_koid)?,
        ..Default::default()
    };

    let collector: fheapdump_client::CollectorProxy =
        connect_to_collector(&remote_control, cmd.collector).await?;
    collector.list_stored_snapshots(request)?;

    let stored_snapshots = receive_list_of_stored_snapshots(iterator_proxy).await?;
    if writer.is_machine() {
        writer.machine(&stored_snapshots)?;
    } else {
        let mut table = Table::new();

        // Make the table formatting standard with other ffx commands
        let padl = 0;
        let padr = 1;
        let table_format = FormatBuilder::new().padding(padl, padr).build();
        table.set_format(table_format);

        table.set_titles(row!["ID", "NAME", "PROCESS"]);

        for elem in stored_snapshots {
            table.add_row(row![
                format!("{}", elem.snapshot_id),
                elem.snapshot_name,
                format!("{}[{}]", elem.process_name, elem.process_koid),
            ]);
        }

        table.print(writer)?;
    }

    Ok(())
}
