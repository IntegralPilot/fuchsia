// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Library that obtains and prints memory digests of a running fuchsia device.

mod bucket;
mod digest;
mod plugin_output;
mod write_csv_output;
mod write_human_readable_output;

use crate::plugin_output::filter_digest_by_process;
use crate::write_csv_output::write_csv_output;
use crate::write_human_readable_output::write_human_readable_output;
use anyhow::Result;
use async_trait::async_trait;
use digest::{processed, raw};
use errors::ffx_bail;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_memory_attribution_plugin as attribution_plugin;
use fdomain_fuchsia_memory_inspection::CollectorProxy;
use ffx_profile_memory_args::{Backend, MemoryCommand};
use ffx_profile_memory_components::{ComponentProfileResult, MemoryComponentsTool, PluginOutput};
use ffx_profile_memory_components_args::ComponentsCommand;
use ffx_writer::{MachineWriter, ToolIO};
use fho::{FfxMain, FfxTool};
use futures::AsyncReadExt;
use plugin_output::ProfileMemoryOutput;
use std::io::Write;
use std::time::Duration;
use target_holders::optional_moniker;

/// Adapts the output of ffx profile component `ComponentProfileResult` with this plugin's output
/// writer.
struct ComponentProfileResultWriter {
    writer: MachineWriter<ProfileMemoryOutput>,
}

impl PluginOutput<ComponentProfileResult> for ComponentProfileResultWriter {
    fn machine(&mut self, output: ComponentProfileResult) -> Result<()> {
        match output {
            ComponentProfileResult::Summary(summary) => {
                self.writer.machine(&ProfileMemoryOutput::ComponentDigest(Box::new(summary)))?;
                Ok(())
            }
            _ => unimplemented!(),
        }
    }

    fn stderr(&mut self) -> &mut dyn Write {
        self.writer.stderr()
    }

    fn stdout(&mut self) -> &mut dyn Write {
        &mut self.writer
    }

    fn is_machine(&self) -> bool {
        self.writer.is_machine()
    }
}

#[derive(FfxTool)]
pub struct MemoryTool {
    #[command]
    cmd: MemoryCommand,
    #[with(optional_moniker("/core/memory_monitor"))]
    memory_monitor1: Option<CollectorProxy>,
    #[with(optional_moniker("/core/memory_monitor2"))]
    memory_monitor2: Option<attribution_plugin::MemoryMonitorProxy>,
}

fho::embedded_plugin!(MemoryTool);

#[async_trait(?Send)]
impl FfxMain for MemoryTool {
    type Writer = MachineWriter<ProfileMemoryOutput>;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        match (&self.cmd.backend, self.memory_monitor1, self.memory_monitor2) {
            (Backend::MemoryMonitor1, Some(mm1), _) | (Backend::Default, Some(mm1), _) => {
                plugin_entrypoint(&mm1, self.cmd, writer).await?;
                Ok(())
            }
            (Backend::MemoryMonitor2, _, Some(mm2)) | (Backend::Default, _, Some(mm2)) => {
                if !self.cmd.process_koids.is_empty() {
                    ffx_bail!(
                        "`--process-koids` argument not supported by memory_monitor_2 backend."
                    );
                }
                if !self.cmd.process_names.is_empty() {
                    ffx_bail!(
                        "`--process-names` argument not supported by memory_monitor_2 backend."
                    );
                }
                if let Some(_) = self.cmd.interval {
                    ffx_bail!("`--interval` argument not supported by memory_monitor_2 backend.");
                }

                if self.cmd.undigested {
                    ffx_bail!("`--undigested` argument not supported by memory_monitor_2 backend.");
                }
                if self.cmd.exact_sizes {
                    ffx_bail!(
                        "`--exact-sizes` argument not supported by memory_monitor_2 backend."
                    );
                }
                let tool = MemoryComponentsTool {
                    cmd: ComponentsCommand {
                        stdin_input: self.cmd.stdin_input,
                        debug_json: self.cmd.debug_json,
                        csv: self.cmd.csv,
                        buckets: self.cmd.buckets,
                        list_vmos: false,
                        detailed: false,
                        stats_only: None,
                        assembly_manifest: None,
                        abridged: false,
                    },
                    monitor_proxy: fho::Deferred::from_output(Ok(mm2)),
                };
                tool.run(ComponentProfileResultWriter { writer }).await
            }
            _ => ffx_bail!("Unable to connect to memory_monitor"),
        }
    }
}

/// Prints a memory digest to stdout.
pub async fn plugin_entrypoint(
    collector: &CollectorProxy,
    cmd: MemoryCommand,
    mut writer: MachineWriter<ProfileMemoryOutput>,
) -> Result<()> {
    // Either call `print_output` once, or call `print_output` repeatedly every `interval` seconds
    // until the user presses ctrl-C.
    match cmd.interval {
        None => print_output(collector, &cmd, &mut writer).await?,
        Some(interval) => loop {
            print_output(collector, &cmd, &mut writer).await?;
            fuchsia_async::Timer::new(Duration::from_secs_f64(interval)).await;
        },
    }
    Ok(())
}

pub async fn print_output(
    collector: &CollectorProxy,
    cmd: &MemoryCommand,
    writer: &mut MachineWriter<ProfileMemoryOutput>,
) -> Result<()> {
    if cmd.debug_json {
        let raw_data = get_raw_data(collector).await?;
        writeln!(writer, "{}", String::from_utf8(raw_data)?).map_err(|err| match err.kind() {
            std::io::ErrorKind::BrokenPipe => fho::Error::ExitWithCode(141),
            _ => fho::Error::Unexpected(err.into()),
        })?;
        Ok(())
    } else {
        let memory_monitor_output = get_output(collector).await?;
        let processed_digest = processed::digest_from_memory_monitor_output(
            memory_monitor_output,
            cmd.buckets,
            cmd.undigested,
        );
        let output = if cmd.process_koids.is_empty() && cmd.process_names.is_empty() {
            ProfileMemoryOutput::CompleteDigest(Box::new(processed_digest))
        } else {
            let process_koids = cmd
                .process_koids
                .iter()
                .copied()
                .map(processed::ProcessKoid::new)
                .collect::<Vec<processed::ProcessKoid>>();
            filter_digest_by_process(processed_digest, &process_koids, &cmd.process_names)
        };
        if cmd.csv {
            write_csv_output(writer, output, cmd.buckets)
        } else {
            if writer.is_machine() {
                writer.machine(&output)?;
                Ok(())
            } else {
                write_human_readable_output(writer, output, cmd.exact_sizes)
            }
        }
    }
}

/// Returns a buffer containing the data that `CollectorProxy` wrote.
async fn get_raw_data(collector: &CollectorProxy) -> Result<Vec<u8>> {
    // Create a socket.
    let (mut rx_async, tx) = collector.domain().create_stream_socket();

    // Ask the collector to fill the socket with the data.
    collector.collect_json_stats(tx)?;

    // Read all the bytes sent from the other end of the socket.
    let mut buffer = Vec::new();
    rx_async.read_to_end(&mut buffer).await?;

    Ok(buffer)
}

/// Returns the `MemoryMonitorOutput` obtained via the `CollectorProxy`. Performs basic schema validation.
async fn get_output(collector: &CollectorProxy) -> anyhow::Result<raw::MemoryMonitorOutput> {
    let buffer = get_raw_data(collector).await?;
    Ok(serde_json::from_slice(&buffer)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncWriteExt;
    use target_holders::fake_proxy;

    use std::sync::LazyLock;
    static EXPECTED_CAPTURE: LazyLock<raw::Capture> = LazyLock::new(|| raw::Capture {
        time: 0,
        kernel: raw::Kernel {
            total: 0,
            free: 0,
            wired: 0,
            total_heap: 0,
            free_heap: 0,
            vmo: 0,
            vmo_pager_total: 0,
            vmo_pager_newest: 0,
            vmo_pager_oldest: 0,
            vmo_discardable_locked: 0,
            vmo_discardable_unlocked: 0,
            mmu: 0,
            ipc: 0,
            other: 0,
            zram_compressed_total: None,
            zram_fragmentation: None,
            zram_uncompressed: None,
        },
        kmem_stats_compression: Default::default(),
        processes: vec![
            raw::Process::Headers(raw::ProcessHeaders::default()),
            raw::Process::Data(raw::ProcessData {
                koid: 2,
                name: "process1".to_string(),
                vmos: vec![1, 2, 3],
            }),
            raw::Process::Data(raw::ProcessData {
                koid: 3,
                name: "process2".to_string(),
                vmos: vec![2, 3, 4],
            }),
        ],
        vmo_names: vec!["name1".to_string(), "name2".to_string()],
        vmos: vec![],
    });
    static EXPECTED_OUTPUT: LazyLock<raw::MemoryMonitorOutput> = LazyLock::new(|| {
        raw::MemoryMonitorOutput { capture: EXPECTED_CAPTURE.clone(), buckets_definitions: vec![] }
    });
    static DATA_WRITTEN_BY_MEMORY_MONITOR: LazyLock<Vec<u8>> =
        LazyLock::new(|| serde_json::to_vec(&*EXPECTED_OUTPUT).unwrap());

    fn create_fake_collector_proxy(
        client: std::sync::Arc<fdomain_client::Client>,
    ) -> CollectorProxy {
        fake_proxy(client, move |req| match req {
            fdomain_fuchsia_memory_inspection::CollectorRequest::CollectJsonStats {
                socket,
                ..
            } => {
                let mut s = socket;
                fuchsia_async::Task::local(async move {
                    s.write_all(&DATA_WRITTEN_BY_MEMORY_MONITOR).await.unwrap();
                })
                .detach();
            }
            _ => panic!("Expected CollectorRequest/CollectJsonStats; got {:?}", req),
        })
    }

    /// Tests that `get_raw_data` properly reads data from the memory monitor service.
    #[fuchsia::test]
    async fn get_raw_data_test() {
        let client = fdomain_local::local_client_empty();
        let collector = create_fake_collector_proxy(client);
        let raw_data = get_raw_data(&collector).await.expect("failed to get raw data");
        assert_eq!(raw_data, *DATA_WRITTEN_BY_MEMORY_MONITOR);
    }

    /// Tests that `get_output` properly reads and parses data from the memory monitor service.
    #[fuchsia::test]
    async fn get_output_test() {
        let client = fdomain_local::local_client_empty();
        let collector = create_fake_collector_proxy(client);
        let output = get_output(&collector).await.expect("failed to get output");
        assert_eq!(output, *EXPECTED_OUTPUT);
    }
}
