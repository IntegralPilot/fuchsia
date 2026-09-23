// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result, anyhow};
use errors::ffx_bail;
use fdomain_fuchsia_tracing::{BufferingMode, KnownCategory};
use fdomain_fuchsia_tracing_controller::{
    CompressionType, ProviderInfo, ProviderStats, ProvisionerProxy, RecordingError,
    SessionManagerProxy, StopResult, TraceConfig, TraceOptions, Trigger,
};
use ffx_config::EnvironmentContext;
use ffx_target::get_target_specifier;
use ffx_trace_args::{Start, Stop, Symbolize, TraceCommand, TraceSubCommand, Upload};
use ffx_tracing::{self as ffx_trace, FidlLibraries};
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{Deferred, FfxMain, FfxTool, bug, deferred};
use futures::future::{BoxFuture, Future, FutureExt as _};
use prettytable::format::FormatBuilder;
use prettytable::{Table, row};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Stdin, stdin};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use target_holders::moniker;
use term_grid::Grid;
use termion::{color, style};

mod direct;
mod process;
mod progress_reader;
mod upload;
use crate::process::process_trace_file;

// LineWaiter abstracts waiting for the user to press enter.  It is needed
// to unit test interactive mode.
trait LineWaiter<'a> {
    type LineWaiterFut: 'a + Future<Output = ()>;
    fn wait(&'a mut self) -> Self::LineWaiterFut;
}

impl<'a> LineWaiter<'a> for Stdin {
    type LineWaiterFut = BoxFuture<'a, ()>;

    fn wait(&'a mut self) -> Self::LineWaiterFut {
        if cfg!(not(test)) {
            use std::io::BufRead;
            blocking::unblock(|| {
                let mut line = String::new();
                let stdin = stdin();
                let mut locked = stdin.lock();
                // Ignoring error, though maybe Ack would want to bubble up errors instead?
                let _ = locked.read_line(&mut line);
            })
            .boxed()
        } else {
            async move {}.boxed()
        }
    }
}

// This is to make the schema make sense as this plugin can output one of these based on the
// subcommand. An alternative is to break this one plugin into multiple plugins each with their own
// output type. That is probably preferred but either way works.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum TraceOutput {
    ListCategories(Vec<TraceKnownCategory>),
    ListProviders(Vec<TraceProviderInfo>),
    ListCategoryGroups(HashMap<String, Vec<String>>),
}

// These fields are arranged this way because deriving Ord uses field declaration order.
#[derive(Debug, Deserialize, Serialize, PartialOrd, Ord, PartialEq, Eq)]
pub struct TraceKnownCategory {
    /// The name of the category.
    name: String,
    /// A short, possibly empty description of this category.
    description: String,
}

impl From<KnownCategory> for TraceKnownCategory {
    fn from(category: KnownCategory) -> Self {
        Self { name: category.name, description: category.description }
    }
}

impl From<&'static str> for TraceKnownCategory {
    fn from(name: &'static str) -> Self {
        Self { name: name.to_string(), description: String::new() }
    }
}

// These fields are arranged this way because deriving Ord uses field declaration order.
#[derive(Debug, Deserialize, Serialize, PartialOrd, Ord, PartialEq, Eq)]
pub struct TraceProviderInfo {
    name: String,
    id: Option<u32>,
    pid: Option<u64>,
}

impl From<ProviderInfo> for TraceProviderInfo {
    fn from(info: ProviderInfo) -> Self {
        Self {
            id: info.id,
            pid: info.pid,
            name: info.name.as_ref().cloned().unwrap_or_else(|| "unknown".to_string()),
        }
    }
}

// The important data elements needed to finalize
// capturing the trace data.
#[derive(Debug)]
pub(crate) struct TraceData {
    pub(crate) output_file: String,
    pub(crate) categories: Vec<String>,
    pub(crate) stop_result: StopResult,
}

/// Enum to handle the FIDL proxy to get a trace session.
#[derive(Clone, Debug)]
pub(crate) enum SessionManagerProxyType {
    Provisioner(ProvisionerProxy),
    SessionManager(SessionManagerProxy),
}

impl From<ProvisionerProxy> for SessionManagerProxyType {
    fn from(p: ProvisionerProxy) -> Self {
        Self::Provisioner(p)
    }
}

impl From<SessionManagerProxy> for SessionManagerProxyType {
    fn from(p: SessionManagerProxy) -> Self {
        Self::SessionManager(p)
    }
}

fn handle_fidl_error<T>(res: Result<T, fidl::Error>) -> Result<T> {
    res.map_err(|e| anyhow!(handle_peer_closed(e)))
}

fn handle_peer_closed(err: fidl::Error) -> errors::FfxError {
    match err {
        fidl::Error::ClientChannelClosed { epitaph, protocol_name, reason, .. } => {
            errors::ffx_error!(
                "An attempt to access {protocol_name} resulted in a bad status: {epitaph:?} reason: {}. \
                 This can happen if tracing is not supported on the product configuration \
                 you are running or if it is missing from the base image.",
                reason.as_ref().map(String::as_str).unwrap_or("not given")
            )
        }
        _ => {
            errors::ffx_error!("Accessing the tracing controller failed: {:#?}", err)
        }
    }
}

fn more_than_init_record(
    non_durable_bytes_written: u64,
    durable_buffer_used: f32,
    buffering_mode: BufferingMode,
) -> bool {
    let init_record_size_in_bytes = 16;
    match buffering_mode {
        BufferingMode::Oneshot => non_durable_bytes_written > init_record_size_in_bytes,
        _ => durable_buffer_used > 0.0,
    }
}

// Scan through the resulting stats of a trace session and build up the output to inform or warn
// the user.
fn stats_to_output(provider_stats: Vec<ProviderStats>, verbose: bool) -> Vec<String> {
    let mut stats_output = Vec::new();
    let mut dropped_records_warnings = Vec::new();
    let mut providers_with_missing_stats = 0;
    for provider in provider_stats {
        let (
            Some(provider_name),
            Some(pid),
            Some(buffering_mode),
            Some(wrapped_count),
            Some(records_dropped),
            Some(durable_buffer_used),
            Some(non_durable_bytes_written),
        ) = (
            provider.name,
            provider.pid,
            provider.buffering_mode,
            provider.buffer_wrapped_count,
            provider.records_dropped,
            provider.percentage_durable_buffer_used,
            provider.non_durable_bytes_written,
        )
        else {
            providers_with_missing_stats += 1;
            continue;
        };

        // If we dropped records, we always want to warn the user, regardless of verbosity.
        if records_dropped != 0 {
            dropped_records_warnings.push(format!(
                "{}WARNING: {provider_name:?} dropped {records_dropped:?} records!{}",
                color::Fg(color::Yellow),
                color::Fg(color::Reset)
            ));
        }

        let provider_has_data =
            more_than_init_record(non_durable_bytes_written, durable_buffer_used, buffering_mode);
        if verbose && provider_has_data {
            stats_output.extend([
                format!("{provider_name:?} (pid: {pid:?}) trace stats"),
                format!("Buffer wrapped count: {wrapped_count:?}"),
                format!("# records dropped: {records_dropped:?}"),
                format!("Durable buffer used: {durable_buffer_used:.2}%"),
                format!("Bytes written to non-durable buffer: {non_durable_bytes_written:#X}\n"),
            ]);
        }
    }

    if !dropped_records_warnings.is_empty() {
        dropped_records_warnings
            .push(format!("{}TIP: One or more providers dropped records. Consider increasing the buffer size with `--buffer-size <MB>`.{}", style::Bold, style::Reset));
    }

    if verbose && providers_with_missing_stats != 0 {
        stats_output.push(format!(
            "{}WARNING: {} producers were missing stats. Perhaps a producer is misconfigured?{}",
            color::Fg(color::Yellow),
            providers_with_missing_stats,
            style::Reset
        ));
    }
    stats_output.extend(dropped_records_warnings);

    return stats_output;
}

fn symbolize_ordinal(ordinal: u64, ordinals: &FidlLibraries, mut writer: Writer) -> Result<()> {
    if let Some(name) = ordinals.get(ordinal) {
        // If the ordinal is present in the symbolization map print the name associated with it.
        writer.line(format!("{} -> {}", ordinal, name))?;
    } else {
        writer.line(format!(
            "Unable to symbolize ordinal {ordinal}. This could be because either:"
        ))?;
        writer.line("1. The ordinal is incorrect")?;
        writer.line("2. The ordinal is not found in IR files in $FUCHSIA_BUILD_DIR/all_fidl_json.txt or the input IR files")?;
    }
    Ok(())
}

// Print as a grid that fills the width of the terminal. Falls back to one value
// per line if any value is wider than the terminal.
fn print_grid(writer: &mut Writer, values: Vec<String>) -> Result<()> {
    let mut grid = Grid::new(term_grid::GridOptions {
        direction: term_grid::Direction::TopToBottom,
        filling: term_grid::Filling::Spaces(2),
    });
    for value in &values {
        grid.add(term_grid::Cell::from(value.as_str()));
    }

    #[cfg(not(test))]
    let terminal_width = termion::terminal_size().unwrap_or((80, 80)).0;
    #[cfg(test)]
    let terminal_width = 80usize;
    let formatted_values = match grid.fit_into_width(terminal_width.into()) {
        Some(grid_display) => grid_display.to_string(),
        None => values.join("\n"),
    };
    writer.line(formatted_values)?;
    Ok(())
}

type Writer = MachineWriter<TraceOutput>;
#[derive(FfxTool)]
#[target(direct)]
pub struct TraceTool {
    #[with(deferred(moniker("/core/trace_manager")))]
    provisioner: Deferred<ProvisionerProxy>,
    #[with(deferred(moniker("/core/trace_manager/trace_session_manager")))]
    session_manager: Deferred<SessionManagerProxy>,
    #[command]
    cmd: TraceCommand,
    context: EnvironmentContext,
}

#[async_trait::async_trait(?Send)]
impl FfxMain for TraceTool {
    type Writer = Writer;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        match self.cmd.sub_cmd.clone() {
            TraceSubCommand::ListCategories(_) => self.list_categories(writer).await,
            TraceSubCommand::ListProviders(_) => self.list_providers(writer).await,
            TraceSubCommand::ListCategoryGroups(_) => self.list_category_groups(writer).await,
            TraceSubCommand::Symbolize(ref opts) => self.symbolize(opts, writer).await,
            TraceSubCommand::Start(ref opts) => self.trace_start(opts, writer).await,
            TraceSubCommand::Stop(ref opts) => self.trace_stop(opts, writer).await,
            TraceSubCommand::Status(_) => self.trace_status(writer).await,
            TraceSubCommand::Upload(ref opts) => self.trace_upload(opts, writer).await,
        }
    }
}

impl TraceTool {
    async fn get_trace_proxy(self) -> Result<SessionManagerProxyType> {
        match self.session_manager.await {
            Ok(p) => Ok(p.into()),
            Err(e) => {
                eprintln!(
                    "Cannot connect to SessionManagerProxy, falling back to basic trace manager."
                );
                log::warn!(
                    "Cannot connect to SessionManagerProxy, falling back to basic trace manager: {e:?}."
                );
                Ok(self.provisioner.await?.into())
            }
        }
    }

    async fn list_categories(self, mut writer: Writer) -> fho::Result<()> {
        let trace_proxy = self.get_trace_proxy().await?;

        let result = match trace_proxy {
            SessionManagerProxyType::Provisioner(p) => p.get_known_categories().await,
            SessionManagerProxyType::SessionManager(p) => p.get_known_categories().await,
        };
        let mut categories = handle_fidl_error(result)?;
        categories.sort_unstable();
        if writer.is_machine() {
            let categories = categories
                .into_iter()
                .map(TraceKnownCategory::from)
                .collect::<Vec<TraceKnownCategory>>();

            writer.machine(&TraceOutput::ListCategories(categories))?;
        } else {
            print_grid(
                &mut writer,
                categories
                    .into_iter()
                    .map(|category| {
                        if !category.description.is_empty() {
                            format!("{} ({})", category.name, category.description)
                        } else {
                            category.name
                        }
                    })
                    .collect(),
            )?;
        }
        Ok(())
    }

    async fn list_providers(self, mut writer: Writer) -> fho::Result<()> {
        let trace_proxy = self.get_trace_proxy().await?;

        let result = match trace_proxy {
            SessionManagerProxyType::Provisioner(p) => p.get_providers().await,
            SessionManagerProxyType::SessionManager(p) => p.get_providers().await,
        };

        let mut providers = handle_fidl_error(result)?
            .into_iter()
            .map(TraceProviderInfo::from)
            .collect::<Vec<TraceProviderInfo>>();
        providers.sort_unstable();
        if writer.is_machine() {
            writer.machine(&TraceOutput::ListProviders(providers))?;
        } else {
            writer.line("Trace providers:")?;
            print_grid(&mut writer, providers.into_iter().map(|provider| provider.name).collect())?;
        }
        Ok(())
    }

    async fn list_category_groups(&self, mut writer: Writer) -> fho::Result<()> {
        let category_groups =
            ffx_trace::get_category_groups(&self.context).map_err(anyhow::Error::from)?;

        if writer.is_machine() {
            writer.machine(&TraceOutput::ListCategoryGroups(category_groups))?;
            return Ok(());
        }

        let mut table = Table::new();
        let table_format = FormatBuilder::new().padding(/*left*/ 0, /*right*/ 1).build();
        table.set_format(table_format);
        table.set_titles(row!("Name", "Categories"));

        // Sort the names with #default being last.
        let mut names = category_groups.keys().cloned().collect::<Vec<String>>();
        if names.contains(&"default".to_string()) {
            names.retain(|name| *name != "default");
            names.sort_unstable();
            names.push("default".to_string());
        } else {
            names.sort_unstable();
        }

        for name in names {
            let values = category_groups
                .get(&name)
                .unwrap()
                .chunks(7)
                .map(|chunk| chunk.join(", "))
                .collect::<Vec<String>>()
                .join("\n");
            table.add_row(row![format!("#{name}"), values]);
        }

        table.print(&mut writer).map_err(|e| bug!(e))?;
        Ok(())
    }

    async fn trace_start(self, opts: &Start, mut writer: Writer) -> fho::Result<()> {
        match (
            opts.background,
            opts.on_boot,
            opts.output.is_some(),
            opts.upload,
            opts.bucket.is_some(),
        ) {
            (_, _, _, false, true) => {
                ffx_bail!("The option '--bucket' can only be used with '--upload'.");
            }
            (true, _, true, _, _) => {
                ffx_bail!(
                    "The option '--output' cannot be used with background tracing. Use `ffx trace stop --output` instead."
                );
            }
            (true, _, _, true, _) => {
                ffx_bail!(
                    "The switch '--upload' cannot be used with background tracing. Use `ffx trace stop --upload` instead."
                );
            }
            (_, true, _, true, _) => {
                ffx_bail!(
                    "The switch '--upload' cannot be used with on-boot tracing. Use `ffx trace stop --upload` instead."
                );
            }
            (_, true, true, _, _) => {
                ffx_bail!(
                    "The option '--output' cannot be used with on-boot tracing. Use `ffx trace stop --output` instead."
                );
            }
            _ => {}
        }
        let triggers = if opts.trigger.is_empty() { None } else { Some(opts.trigger.clone()) };
        if triggers.is_some() && !opts.background {
            ffx_bail!(
                "Triggers can only be set on a background trace. \
                     Trace should be run with the --background flag."
            );
        }
        let context = self.context.clone();
        let trace_proxy = self.get_trace_proxy().await?;

        let expanded_categories = ffx_trace::expand_categories(&context, opts.categories.clone())
            .map_err(anyhow::Error::from)?;
        let defer_transfer = match opts.buffering_mode {
            BufferingMode::Oneshot | BufferingMode::Circular => false,
            BufferingMode::Streaming => true,
        };
        let trace_config = TraceConfig {
            buffer_size_megabytes_hint: Some(opts.buffer_size),
            categories: Some(expanded_categories.clone()),
            buffering_mode: Some(opts.buffering_mode),
            defer_transfer: Some(defer_transfer),
            ..ffx_trace::map_categories_to_providers(&expanded_categories)
        };
        let output = canonical_path(opts.output.clone().unwrap_or_else(|| "trace.fxt".to_owned()))?;

        let compression =
            if opts.nocompress { CompressionType::None } else { CompressionType::Zstd };

        let options = TraceOptions {
            duration_ns: opts.duration.map(|d| Duration::from_secs(d.into()).as_nanos() as i64),
            triggers,
            requested_categories: Some(opts.categories.clone()),
            compression: Some(compression),
            ..Default::default()
        };
        writer.line(format!("Tracing categories: [{}]...", expanded_categories.join(","),))?;

        if opts.on_boot {
            return match trace_proxy {
                SessionManagerProxyType::Provisioner(_) => {
                    ffx_bail!(
                        "Trace on boot is not supported with devices that do not support SessionManagerProxy"
                    );
                }
                SessionManagerProxyType::SessionManager(_) => {
                    let trace_config = TraceConfig {
                        categories: Some(expanded_categories.clone()),
                        ..trace_config
                    };
                    configure_on_boot_trace(trace_proxy, options, trace_config, &mut writer)
                        .await
                        .map_err(Into::into)
                }
            };
        }

        // For the background we need a background task, so still use the daemon.
        // Otherwise use a direct connection.
        if opts.background {
            return match trace_proxy {
                SessionManagerProxyType::Provisioner(_) => {
                    ffx_bail!(
                        "Background tracing is not supported with devices that do not support SessionManagerProxy"
                    );
                }
                SessionManagerProxyType::SessionManager(_) => {
                    let trace_config = TraceConfig {
                        categories: Some(expanded_categories.clone()),
                        ..trace_config
                    };
                    background_trace(trace_proxy, options, trace_config, &mut writer)
                        .await
                        .map_err(Into::into)
                }
            };
        }

        let trace_config =
            TraceConfig { categories: Some(expanded_categories.clone()), ..trace_config };
        let task = direct::trace(trace_proxy.clone(), options, trace_config.clone(), false).await?;

        if opts.duration.is_none() {
            let waiter = &mut stdin();
            writer.line("Press <enter> to stop trace.")?;
            waiter.wait().await;
        }

        writer.line("Trace completed! Copying trace from device...")?;
        let trace_data = direct::stop_tracing(&context, task, trace_proxy, &output).await?;

        finalize_trace(
            &context,
            trace_data,
            &Stop {
                output: Some(output.clone()),
                verbose: opts.verbose,
                no_symbolize: opts.no_symbolize,
                no_verify_trace: opts.no_verify_trace,
                retain_raw_fidl: opts.retain_raw_fidl,
                upload: opts.upload,
                bucket: opts.bucket.clone(),
                ..Default::default()
            },
            writer,
        )
        .await?;
        Ok(())
    }

    async fn trace_stop(self, opts: &Stop, mut writer: Writer) -> fho::Result<()> {
        let context = self.context.clone();

        match (
            opts.abort,
            opts.upload,
            opts.output.is_some(),
            opts.bucket.is_some(),
            opts.no_symbolize,
            opts.no_verify_trace,
            opts.retain_raw_fidl,
        ) {
            (true, true, _, _, _, _, _) => {
                ffx_bail!("The switch '--upload' cannot be used with '--abort'.");
            }
            (true, _, true, _, _, _, _) => {
                ffx_bail!("The option '--output' cannot be used with '--abort'.");
            }
            (true, _, _, true, _, _, _) => {
                ffx_bail!("The option '--bucket' cannot be used with '--abort'.");
            }
            (true, _, _, _, true, _, _) => {
                ffx_bail!("The switch '--no-symbolize' cannot be used with '--abort'.");
            }
            (true, _, _, _, _, true, _) => {
                ffx_bail!("The switch '--no-verify-trace' cannot be used with '--abort'.");
            }
            (true, _, _, _, _, _, true) => {
                ffx_bail!("The switch '--retain-raw-fidl' cannot be used with '--abort'.");
            }
            (_, false, _, true, _, _, _) => {
                ffx_bail!("The option '--bucket' can only be used with '--upload'.");
            }
            _ => {}
        }

        let trace_proxy = self.get_trace_proxy().await?;
        let output = canonical_path(opts.output.clone().unwrap_or_else(|| "trace.fxt".to_owned()))?;

        let trace_data = match trace_proxy {
            SessionManagerProxyType::Provisioner(_) => {
                ffx_bail!(
                    "Stop is not supported with devices that do not expose SessionManagerProxy"
                );
            }
            SessionManagerProxyType::SessionManager(_) => {
                if opts.abort {
                    direct::abort_tracing(&context, None, trace_proxy).await?;
                    writer.line("Trace aborted.")?;
                    return Ok(());
                } else {
                    direct::stop_tracing(&context, None, trace_proxy, &output).await?
                }
            }
        };

        finalize_trace(&context, trace_data, opts, writer).await.map_err(Into::into)
    }

    async fn trace_upload(self, opts: &Upload, writer: Writer) -> fho::Result<()> {
        let trace_file = opts.trace_file.as_deref().unwrap_or("trace.fxt");
        trace_upload_impl(
            &self.context,
            trace_file,
            opts.bucket.as_deref(),
            writer,
            |path, bucket, viewer| async move {
                upload::upload_trace(&path, &bucket, &viewer).await
            },
        )
        .await
        .map_err(Into::into)
    }

    async fn trace_status(self, mut writer: Writer) -> fho::Result<()> {
        let trace_proxy = self.get_trace_proxy().await?;

        match trace_proxy {
            SessionManagerProxyType::Provisioner(_) => {
                ffx_bail!(
                    "Status is unavailable with devices that do not support SessionManagerProxy"
                )
            }
            SessionManagerProxyType::SessionManager(session_manager_proxy) => {
                status(session_manager_proxy, &mut writer).await.map_err(Into::into)
            }
        }
    }
    async fn symbolize(self, opts: &Symbolize, mut writer: Writer) -> fho::Result<()> {
        match (
            opts.fxt.is_some(),
            opts.ordinal.is_some(),
            opts.outfile.is_some(),
            !opts.ir_path.is_empty(),
        ) {
            (true, true, _, _) => {
                ffx_bail!("The options '--ordinal' and '--fxt' cannot be used together.");
            }
            (_, true, true, _) => {
                ffx_bail!("The option '--outfile' cannot be used with '--ordinal'.");
            }
            (true, _, _, true) => {
                ffx_bail!("The option '--ir-path' cannot be used with '--fxt'.");
            }
            _ => {}
        }

        if let Some(ref trace_file) = opts.fxt {
            let outfile = opts.outfile.as_ref().unwrap_or(trace_file);
            for warning in process_trace_file(
                trace_file,
                &outfile,
                true,
                opts.retain_raw_fidl,
                None,
                &self.context,
            )? {
                writer.line(warning)?;
            }
            writer.line(format!("Symbolized traces written to {outfile}"))?;
        } else if let Some(ordinal) = opts.ordinal {
            let mut ordinals = match FidlLibraries::from_context(&self.context) {
                Ok(ordinals) => ordinals,
                Err(err) => {
                    writer.line(format!("Unable to load FIDL symbolization map: {err}"))?;
                    FidlLibraries::default()
                }
            };
            for ir_file in &opts.ir_path {
                ordinals.add_ir_file(ir_file).map_err(anyhow::Error::from)?;
            }

            symbolize_ordinal(ordinal, &ordinals, writer)?;
        } else {
            ffx_bail!("Either ordinal or trace file must be provided to symbolize");
        }
        Ok(())
    }
}

/// Does the final steps of capturing the trace such as, dumping the provider stats, post_processing
/// and letting the user know the file name of the trace data.
async fn finalize_trace(
    context: &EnvironmentContext,
    trace_data: TraceData,
    opts: &Stop,
    writer: Writer,
) -> Result<()> {
    finalize_trace_impl(context, trace_data, opts, writer, |path, bucket, viewer| async move {
        upload::upload_trace(&path, &bucket, &viewer).await
    })
    .await
}

/// Implementation of [`finalize_trace`] parameterized over an `uploader` closure to allow
/// dependency injection in tests.
///
/// This decouples trace finalization (stats formatting, post-processing, verification, configuration
/// resolution, and error reporting) from network I/O and interactive GCS authentication, enabling
/// hermetic unit tests to verify behavior without external cloud dependencies.
async fn finalize_trace_impl<F, Fut>(
    context: &EnvironmentContext,
    trace_data: TraceData,
    opts: &Stop,
    mut writer: Writer,
    uploader: F,
) -> Result<()>
where
    F: FnOnce(PathBuf, String, String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let verify_trace = !opts.no_verify_trace;

    for line in
        stats_to_output(trace_data.stop_result.provider_stats.unwrap_or(vec![]), opts.verbose)
    {
        writer.line(line)?;
    }
    if verify_trace {
        let categories = if verify_trace { Some(trace_data.categories) } else { None };
        post_process(
            &context,
            &trace_data.output_file,
            categories,
            opts.no_symbolize,
            opts.retain_raw_fidl,
            &mut writer,
        )?;
    }
    writer.line(format!("Results written to {}", trace_data.output_file))?;
    if opts.upload {
        upload_and_print(
            context,
            Path::new(&trace_data.output_file),
            opts.bucket.as_deref(),
            &mut writer,
            uploader,
        )
        .await?;
    } else {
        writer.line("Upload to https://ui.perfetto.dev/#!/ to view.")?;
    }
    Ok(())
}

async fn upload_and_print<F, Fut>(
    context: &EnvironmentContext,
    file_path: &Path,
    cli_bucket: Option<&str>,
    writer: &mut Writer,
    uploader: F,
) -> Result<()>
where
    F: FnOnce(PathBuf, String, String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let bucket = upload::resolve_bucket(cli_bucket, context);
    let viewer_base = upload::resolve_viewer_url(context);
    writer.line(format!("Uploading trace to gs://{}...", bucket))?;
    match uploader(file_path.to_path_buf(), bucket, viewer_base).await {
        Ok(url) => {
            writer.line("Trace uploaded successfully!")?;
            writer.line("")?;
            writer.line("View in Perfetto Trace Viewer:")?;
            writer.line(format!("  {}", url))?;
        }
        Err(e) => {
            writer.line(format!("Warning: Failed to upload trace to GCS: {e:#}"))?;
        }
    }
    Ok(())
}

async fn trace_upload_impl<F, Fut>(
    context: &EnvironmentContext,
    fxt_path: &str,
    cli_bucket: Option<&str>,
    mut writer: Writer,
    uploader: F,
) -> Result<()>
where
    F: FnOnce(PathBuf, String, String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let path = Path::new(fxt_path);
    if !path.exists() {
        ffx_bail!("Trace file does not exist: {}", fxt_path);
    }
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for trace file: {:?}", path))?;
    if metadata.len() == 0 {
        ffx_bail!("cannot upload zero length traces");
    }
    let bucket = upload::resolve_bucket(cli_bucket, context);
    let viewer_base = upload::resolve_viewer_url(context);
    writer.line(format!("Uploading trace to gs://{}...", bucket))?;
    match uploader(path.to_path_buf(), bucket, viewer_base).await {
        Ok(url) => {
            writer.line("Trace uploaded successfully!")?;
            writer.line("")?;
            writer.line("View in Perfetto Trace Viewer:")?;
            writer.line(format!("  {}", url))?;
            Ok(())
        }
        Err(e) => {
            ffx_bail!("Failed to upload trace to GCS: {e:#}");
        }
    }
}

/// Do some quick verification that the trace file
/// contains the categories specified. The categories passed in here should be what was on the
/// command line, not the expanded list categories after processing the groups, otherwise there
/// is a bunch or warning messages about categories not being found.
fn post_process(
    context: &EnvironmentContext,
    output_file: &str,
    categories: Option<Vec<String>>,
    skip_symbolization: bool,
    retain_raw_fidl: bool,
    writer: &mut Writer,
) -> Result<()> {
    let expanded_categories =
        ffx_trace::expand_categories(context, categories.clone().unwrap_or(vec![]))
            .map_err(anyhow::Error::from)?;
    let skip_symbolization = skip_symbolization
        || !expanded_categories.contains(&"kernel:ipc".to_string())
            && !expanded_categories.contains(&"kernel:*".to_string());
    writer.line("Post Processing Trace...")?;
    let warnings = process_trace_file(
        &output_file,
        &output_file,
        !skip_symbolization,
        retain_raw_fidl,
        categories,
        context,
    )?;
    for warning in warnings {
        writer.line(format!("{warning}"))?;
    }
    Ok(())
}

async fn status(session_manager_proxy: SessionManagerProxy, writer: &mut Writer) -> Result<()> {
    let res = session_manager_proxy.status().await?;
    let data = match res {
        Err(RecordingError::NoSuchTraceFile) => "No active traces running".to_string(),
        Ok(status) => {
            format!(
                "Task Id: {}\nTotal Duration: {}\nRemaining Duration: {}\nCategories: {}\nTriggers:\n{}",
                status.task_id.map(|id| id.to_string()).unwrap_or_else(|| "unknown".to_string()),
                status
                    .duration
                    .map(|d| Duration::from_nanos(d as u64))
                    .map(|d| format!("{} seconds", d.as_secs()))
                    .unwrap_or_else(|| "infinite".to_string()),
                status
                    .remaining_runtime
                    .map(|d| Duration::from_nanos(d as u64))
                    .map(|d| format!("{} seconds", d.as_secs()))
                    .unwrap_or_else(|| "infinite".to_string()),
                status
                    .config
                    .map(|c| c
                        .categories
                        .map(|cat| cat.join(","))
                        .unwrap_or_else(|| "None".to_string()))
                    .unwrap_or_else(|| "None".to_string()),
                trigger_status(status.triggers)
            )
        }
        Err(e) => format!("Unexpected error {e:?}"),
    };
    writer.line(data)?;
    Ok(())
}

fn trigger_status(triggers: Option<Vec<Trigger>>) -> String {
    if let Some(trigger_list) = triggers {
        trigger_list
            .iter()
            .map(|t| {
                format!(
                    "- {} : {}",
                    t.alert.clone().unwrap_or_else(|| "".to_string()),
                    t.action.map(|a| format!("{a:?}")).unwrap_or_else(|| "".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        "None".into()
    }
}

async fn background_trace(
    trace_proxy: SessionManagerProxyType,
    options: TraceOptions,
    trace_config: TraceConfig,
    writer: &mut Writer,
) -> Result<()> {
    let _r = direct::trace(trace_proxy.clone(), options, trace_config.clone(), true).await?;
    writer.line("To manually stop the trace, use `ffx trace stop`")?;
    writer.line("Current tracing status:")?;
    if let SessionManagerProxyType::SessionManager(session_manager_proxy) = trace_proxy {
        status(session_manager_proxy, writer).await
    } else {
        anyhow::bail!("Missing SessionManagerProxy")
    }
}

async fn configure_on_boot_trace(
    trace_proxy: SessionManagerProxyType,
    options: TraceOptions,
    trace_config: TraceConfig,
    writer: &mut Writer,
) -> Result<()> {
    direct::trace_on_reboot(trace_proxy.clone(), options, trace_config.clone()).await?;
    writer.line("Once the device is rebooted, stop the trace using `ffx trace stop`")?;
    Ok(())
}

pub(crate) async fn handle_recording_error(
    context: &EnvironmentContext,
    err: RecordingError,
    output: &str,
) -> String {
    let target_spec = get_target_specifier(context).unwrap_or(None);
    match err {
        RecordingError::TargetProxyOpen => {
            "Error: ffx trace was unable to connect to trace_manager on the device.

Note that tracing is available for eng and core products, but not user or userdebug.
To fix general connection issues, you could also try:

$ ffx doctor

For a tutorial on getting started with tracing, visit:
https://fuchsia.dev/fuchsia-src/development/sdk/ffx/record-traces"
                .to_owned()
        }
        RecordingError::RecordingAlreadyStarted => {
            format!(
                "Trace already started for target {}.\n\
                If you want to stop the trace and discard the data, run `ffx trace stop --abort`.",
                target_spec.unwrap_or_else(|| "".to_owned())
            )
        }
        RecordingError::DuplicateTraceFile => {
            format!("Trace already running for file {output}")
        }
        RecordingError::RecordingStart => {
            let log_file: String = context.get("log.dir").unwrap();
            format!(
                "Error starting Fuchsia trace. See {log_file}/ffx.daemon.log\n
Search for lines tagged with `ffx_daemon_service_tracing`. A common issue is a
peer closed error from `fuchsia.tracing.controller.Controller`. If this is the
case either tracing is not supported in the product configuration or the tracing
package is missing from the device's system image."
            )
        }
        RecordingError::RecordingStop => {
            let log_file: String = context.get("log.dir").unwrap();
            format!(
                "Error stopping Fuchsia trace. See {log_file}/ffx.daemon.log\n
Search for lines tagged with `ffx_daemon_service_tracing`. A common issue is a
peer closed error from `fuchsia.tracing.controller.Controller`. If this is the
case either tracing is not supported in the product configuration or the tracing
package is missing from the device's system image."
            )
        }
        RecordingError::NoSuchTraceFile => {
            format!("Could not stop trace. No active traces for {output}.")
        }
        RecordingError::NoSuchTarget => {
            format!(
                "The string '{}' didn't match a trace output file, or any valid targets.",
                target_spec.as_deref().unwrap_or("")
            )
        }
        RecordingError::DisconnectedTarget => {
            format!(
                "The string '{}' didn't match a valid target.",
                target_spec.as_deref().unwrap_or("")
            )
        }
        unknown_error => format!("Unknown error: {unknown_error:?}"),
    }
}

fn canonical_path<T: ToString>(output_path: T) -> Result<String> {
    let output_path = PathBuf::from(output_path.to_string());
    let mut path = PathBuf::new();
    if !output_path.has_root() {
        path.push(std::env::current_dir()?);
    }
    path.push(output_path);
    let mut components = path.components().peekable();
    let mut res = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };
    for component in components {
        match component {
            Component::Prefix(..) => return Err(anyhow!("prefix unreachable")),
            Component::RootDir => {
                res.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                res.pop();
            }
            Component::Normal(c) => {
                res.push(c);
            }
        }
    }
    res.into_os_string()
        .into_string()
        .map_err(|e| anyhow!("unable to convert OsString to string {e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use errors::ResultExt as _;
    use fdomain_client::Client as FDomainClient;
    use fdomain_client::fidl::{ControlHandle as _, Responder};
    use fdomain_fuchsia_tracing as tracing;
    use fdomain_fuchsia_tracing_controller::{
        self as tracing_controller, Action, TraceStatus, Trigger,
    };
    use ffx_trace_args::{ListCategories, ListProviders, Start, Status, Stop, Symbolize};
    use ffx_writer::{Format, TestBuffers};
    use pretty_assertions::assert_eq;
    use regex::Regex;
    use serde_json::json;
    use std::io::Write;
    use std::matches;
    use std::sync::Arc;
    use target_holders::fake_proxy;
    use tempfile::{Builder, NamedTempFile, TempDir};

    #[test]
    fn test_canonical_path_has_root() {
        let p = canonical_path("what".to_string()).unwrap();
        let got = PathBuf::from(p);
        let got = got.components().next().unwrap();
        assert!(matches!(got, Component::RootDir));
    }

    #[test]
    fn test_canonical_path_cleans_dots() {
        let mut path = PathBuf::new();
        path.push(Component::RootDir);
        path.push("this");
        path.push(Component::ParentDir);
        path.push("that");
        path.push("these");
        path.push(Component::CurDir);
        path.push("what.txt");
        let got = canonical_path(path.into_os_string().into_string().unwrap()).unwrap();
        let mut want = PathBuf::new();
        want.push(Component::RootDir);
        want.push("that");
        want.push("these");
        want.push("what.txt");
        let want = want.into_os_string().into_string().unwrap();
        assert_eq!(want, got);
    }

    #[test]
    fn test_print_grid_too_wide() {
        let test_buffers = TestBuffers::default();
        let mut writer = Writer::new_test(None, &test_buffers);
        print_grid(
            &mut writer,
            vec![
                "really_really_really_really\
                _really_really_really_really\
                _really_really_long_category"
                    .to_string(),
                "short_category".to_string(),
                "another_short_category".to_string(),
            ],
        )
        .unwrap();
        let output = test_buffers.into_stdout_str();
        let want = "really_really_really_really\
                          _really_really_really_really\
                          _really_really_long_category\n\
                          short_category\n\
                          another_short_category\n";
        assert_eq!(want, output);
    }

    fn _generate_stop_result() -> tracing_controller::StopResult {
        let mut stats = tracing_controller::ProviderStats::default();
        stats.name = Some("provider_bar".to_string());
        stats.pid = Some(1234);
        stats.buffering_mode = Some(BufferingMode::Oneshot);
        stats.buffer_wrapped_count = Some(10);
        stats.records_dropped = Some(0);
        stats.percentage_durable_buffer_used = Some(30.0);
        stats.non_durable_bytes_written = Some(40);
        let mut result = tracing_controller::StopResult::default();
        result.provider_stats = Some(vec![stats]);
        return result;
    }

    fn setup_fake_session_manager(client: Arc<FDomainClient>) -> Deferred<SessionManagerProxy> {
        Deferred::from_output(Ok(fake_proxy(client, |req| match req {
            tracing_controller::SessionManagerRequest::GetKnownCategories { responder, .. } => {
                responder.send(&fake_known_categories()).expect("should respond");
            }
            tracing_controller::SessionManagerRequest::GetProviders { responder, .. } => {
                responder.send(&fake_provider_infos()).expect("should respond");
            }
            tracing_controller::SessionManagerRequest::StartTraceSession {
                config: _,
                options: _,
                responder,
            } => {
                responder.send(Ok(123)).expect("should respond");
            }
            tracing_controller::SessionManagerRequest::StartTraceSessionOnBoot {
                config: _,
                options: _,
                responder,
            } => {
                responder.send(Ok(())).expect("should respond");
            }
            tracing_controller::SessionManagerRequest::EndTraceSession {
                task_id: _,
                output: _,
                responder,
            } => {
                responder
                    .send(Ok((&TraceOptions { ..Default::default() }, &_generate_stop_result())))
                    .expect("should respond");
            }
            tracing_controller::SessionManagerRequest::AbortTraceSession {
                task_id: _,
                responder,
            } => {
                responder.send(Ok(())).expect("should respond");
            }
            tracing_controller::SessionManagerRequest::Status { responder } => {
                responder
                    .send(Ok(&TraceStatus {
                        task_id: Some(2468),
                        duration: None,
                        remaining_runtime: None,
                        config: Some(TraceConfig {
                            categories: Some(vec!["beaver".into(), "platypus".into()]),
                            ..Default::default() //buffer_size_megabytes_hint: (), start_timeout_milliseconds: (), buffering_mode: (), provider_specs: (), version: (), defer_transfer: (), __source_breaking: ()
                        }),
                        triggers: Some(vec![
                            Trigger {
                                alert: Some("foo".into()),
                                action: Some(Action::Terminate),
                                ..Default::default()
                            },
                            Trigger {
                                alert: Some("bar".into()),
                                action: Some(Action::Terminate),
                                ..Default::default()
                            },
                        ]),
                        ..Default::default()
                    }))
                    .expect("should respond");
            }
            r => panic!("unsupported req: {r:?}"),
        })))
    }

    fn fake_known_categories() -> Vec<tracing::KnownCategory> {
        vec![
            tracing::KnownCategory {
                name: String::from("input"),
                description: String::from("Input system"),
            },
            tracing::KnownCategory {
                name: String::from("kernel"),
                description: String::from("All kernel trace events"),
            },
            tracing::KnownCategory {
                name: String::from("kernel:arch"),
                description: String::from("Kernel arch events"),
            },
            tracing::KnownCategory {
                name: String::from("kernel:ipc"),
                description: String::from("Kernel ipc events"),
            },
        ]
    }

    fn fake_provider_infos() -> Vec<tracing_controller::ProviderInfo> {
        vec![
            tracing_controller::ProviderInfo {
                id: Some(42),
                name: Some("foo".to_string()),
                ..Default::default()
            },
            tracing_controller::ProviderInfo {
                id: Some(99),
                pid: Some(1234567),
                name: Some("bar".to_string()),
                ..Default::default()
            },
            tracing_controller::ProviderInfo { id: Some(2), ..Default::default() },
        ]
    }

    fn fake_trace_provider_infos() -> Vec<TraceProviderInfo> {
        let mut infos: Vec<TraceProviderInfo> =
            fake_provider_infos().into_iter().map(TraceProviderInfo::from).collect();
        infos.sort_unstable();
        infos
    }

    fn setup_closed_fake_controller_proxy(
        client: &Arc<FDomainClient>,
    ) -> Deferred<ProvisionerProxy> {
        Deferred::from_output(Ok(fake_proxy(Arc::clone(client), |req| match req {
            tracing_controller::ProvisionerRequest::GetKnownCategories { responder, .. } => {
                responder.control_handle().shutdown();
            }
            tracing_controller::ProvisionerRequest::GetProviders { responder, .. } => {
                responder.control_handle().shutdown();
            }
            r => panic!("unsupported req: {r:?}"),
        })))
    }

    #[fuchsia::test]
    async fn test_list_categories() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::ListCategories(ListCategories {}) },
            context: env.context.clone(),
        };

        tool.list_categories(writer).await.expect("list categories failed");

        let output = test_buffers.into_stdout_str();
        let want = "input (Input system)\nkernel (All kernel trace events)\nkernel:arch (Kernel arch events)\nkernel:ipc (Kernel ipc events)\n";
        assert_eq!(want, output);
    }

    #[fuchsia::test]
    async fn test_symbolize_success() {
        let env = ffx_config::test_init().unwrap();
        let fake_ir_json = json!({
            "name": "fake.library",
            "platform": "",
            "available": {},
            "experiments": [],
            "library_dependencies": [],
            "bits_declarations": [],
            "const_declarations": [],
            "enum_declarations": [],
            "experimental_resource_declarations": [],
            "service_declarations": [],
            "struct_declarations": [],
            "external_struct_declarations": [],
            "table_declarations": [],
            "union_declarations": [],
            "alias_declarations": [],
            "new_type_declarations": [],
            "declaration_order": [],
            "declarations": {
                "fake_protocol_name": "protocol",
            },
            "protocol_declarations": [
                {
                    "name": "fake_protocol_name",
                    "methods": [
                        {
                            "ordinal": 12345678,
                            "name": "fake_method_name",
                            "is_composed": false,
                            "strict": false,
                            "has_request": true,
                            "has_response": true,
                        },
                    ],
                    "composed_protocols": [],
                    "deprecated": [],
                },
            ],
        });
        let mut temp_file = NamedTempFile::new().expect("Failed to create temp IR file");
        temp_file
            .write_all(fake_ir_json.to_string().as_bytes())
            .expect("Failed to write IR string to file");
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let fake_ir_path =
            temp_file.path().to_str().expect("Unable to convert fake IR path to string");

        let symbolize_opts = Symbolize {
            ordinal: Some(12345678),
            ir_path: vec![fake_ir_path.to_string()],
            fxt: None,
            outfile: None,
            retain_raw_fidl: false,
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: Deferred::from_output(Err(fho::user_error!("not found"))),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Symbolize(symbolize_opts.clone()) },
        };

        tool.symbolize(&symbolize_opts, writer).await.expect("symbolize failed");

        let output = test_buffers.into_stdout_str();
        let want = "12345678 -> fake_protocol_name.fake_method_name\n";
        assert!(output.contains(want));
    }

    #[fuchsia::test]
    async fn test_empty_trace_data() {
        let fake_temp_file =
            Builder::new().suffix("foo.fxt").tempfile().expect("Failed to create a temp file");
        let fake_trace_file_name = fake_temp_file.path().to_str().unwrap().to_string();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let start_opts = Start {
            buffer_size: 2,
            categories: vec!["invalid_categories".to_string()],
            duration: Some(1),
            buffering_mode: tracing::BufferingMode::Oneshot,
            output: Some(fake_trace_file_name),
            background: false,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: false,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: Deferred::from_output(Err(fho::user_error!("not found"))),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        assert!(tool.trace_start(&start_opts, writer).await.is_err());
    }

    #[fuchsia::test]
    async fn test_symbolize_fail() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let symbolize_opts = Symbolize {
            ordinal: Some(12345678),
            ir_path: vec![],
            fxt: None,
            outfile: None,
            retain_raw_fidl: false,
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: Deferred::from_output(Err(fho::user_error!("not found"))),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Symbolize(symbolize_opts.clone()) },
        };

        tool.symbolize(&symbolize_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let want = "Unable to symbolize ordinal 12345678. This could be because either:\n\
                    1. The ordinal is incorrect\n\
                    2. The ordinal is not found in IR files in $FUCHSIA_BUILD_DIR/all_fidl_json.txt or the input IR files\n";
        assert!(output.contains(want));
    }

    #[fuchsia::test]
    async fn test_list_categories_machine() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(Some(Format::Json), &test_buffers);

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::ListCategories(ListCategories {}) },
        };

        tool.list_categories(writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let want = serde_json::to_string(
            &fake_known_categories()
                .into_iter()
                .map(TraceKnownCategory::from)
                .collect::<Vec<TraceKnownCategory>>(),
        )
        .unwrap();
        assert_eq!(want, output.trim_end());
    }

    #[fuchsia::test]
    async fn test_list_categories_peer_closed() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let tool = TraceTool {
            provisioner: setup_closed_fake_controller_proxy(&client),
            session_manager: Deferred::from_output(Err(fho::user_error!("not found"))),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::ListCategories(ListCategories {}) },
        };

        let res = tool.list_categories(writer).await.unwrap_err();
        assert!(res.ffx_error().is_some());
        assert!(res.to_string().contains("This can happen if tracing is not"));
        assert!(test_buffers.into_stdout_str().is_empty());
    }

    #[fuchsia::test]
    async fn test_list_providers() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::ListProviders(ListProviders {}) },
        };

        tool.list_providers(writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let want = "Trace providers:\n\
                   bar  foo  unknown\n\n"
            .to_string();
        assert_eq!(want, output);
    }

    #[fuchsia::test]
    async fn test_list_providers_peer_closed() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let cmd = TraceCommand { sub_cmd: TraceSubCommand::ListProviders(ListProviders {}) };

        let tool = TraceTool {
            provisioner: setup_closed_fake_controller_proxy(&client),
            session_manager: Deferred::from_output(Err(fho::user_error!("not found"))),
            context: env.context.clone(),
            cmd,
        };

        let res = tool.list_providers(writer).await.unwrap_err();
        assert!(res.ffx_error().is_some());
        assert!(res.to_string().contains("This can happen if tracing is not"));
        assert!(test_buffers.into_stdout_str().is_empty());
    }

    #[fuchsia::test]
    async fn test_list_providers_machine() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(Some(Format::Json), &test_buffers);
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::ListProviders(ListProviders {}) },
        };

        tool.list_providers(writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let want = serde_json::to_string(&fake_trace_provider_infos()).unwrap();
        assert_eq!(want, output.trim_end());
    }

    #[fuchsia::test]
    async fn test_start() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start {
            buffer_size: 2,
            categories: vec!["platypus".to_string(), "beaver".to_string()],
            duration: None,
            buffering_mode: tracing::BufferingMode::Oneshot,
            output: None,
            background: true,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        // This doesn't find `/.../foo.txt` for the tracing status, since the faked
        // proxy has no state.
        let regex_str = "Tracing categories: \\[beaver,platypus\\]...
To manually stop the trace, use `ffx trace stop`
Current tracing status:
Task Id: 2468
Total Duration: infinite
Remaining Duration: infinite
Categories: beaver,platypus
Triggers:
- foo : Terminate
- bar : Terminate\n";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_status() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Status(Status {}) },
        };

        tool.trace_status(writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let want = "Task Id: 2468
Total Duration: infinite
Remaining Duration: infinite
Categories: beaver,platypus
Triggers:
- foo : Terminate
- bar : Terminate\n";
        assert_eq!(want, output);
    }

    #[fuchsia::test]
    async fn test_stop() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            output: Some("foo.txt".to_string()),
            verbose: false,
            no_symbolize: false,
            no_verify_trace: true,
            retain_raw_fidl: false,
            abort: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(stop_opts.clone()) },
        };

        tool.trace_stop(&stop_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str =
            "Results written to /([^/]+/)+?foo.txt\nUpload to https://ui.perfetto.dev/#!/ to view.";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_stop_upload() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            output: Some("foo_upload.txt".to_string()),
            verbose: false,
            no_symbolize: false,
            no_verify_trace: true,
            retain_raw_fidl: false,
            abort: false,
            upload: true,
            bucket: Some("test-upload-bucket".to_string()),
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(stop_opts.clone()) },
        };

        // Even though upload to GCS fails in a test environment (no network/credentials),
        // trace_stop must return Ok(()) (non-blocking failure as per requirement R3).
        tool.trace_stop(&stop_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Results written to /([^/]+/)+?foo_upload.txt\nUploading trace to gs://test-upload-bucket\\.\\.\\.\nWarning: Failed to upload trace to GCS: .*";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_stop_upload_with_config() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_env()
            .user_config("trace.gcs_bucket", "my-config-bucket")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            output: Some("foo_upload_cfg.txt".to_string()),
            upload: true,
            bucket: None,
            no_verify_trace: true,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(stop_opts.clone()) },
        };

        tool.trace_stop(&stop_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Results written to /([^/]+/)+?foo_upload_cfg.txt\nUploading trace to gs://my-config-bucket\\.\\.\\.\nWarning: Failed to upload trace to GCS: .*";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_finalize_trace_upload_success() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            upload: true,
            bucket: Some("my-bucket".to_string()),
            no_verify_trace: true,
            ..Default::default()
        };

        let trace_data = TraceData {
            output_file: "test_output.fxt".to_string(),
            categories: vec![],
            stop_result: StopResult::default(),
        };

        let expected_url = "https://fuchsia-trace-viewer.corp.goog/trace/b94d27b9";
        finalize_trace_impl(
            &env.context,
            trace_data,
            &stop_opts,
            writer,
            |_path, _bucket, _viewer| async { Ok(expected_url.to_string()) },
        )
        .await
        .unwrap();

        let output = test_buffers.into_stdout_str();
        let expected_output = format!(
            "Results written to test_output.fxt\nUploading trace to gs://my-bucket...\nTrace uploaded successfully!\n\nView in Perfetto Trace Viewer:\n  {}\n",
            expected_url
        );
        assert_eq!(output, expected_output);
    }

    #[fuchsia::test]
    async fn test_finalize_trace_upload_failure_non_blocking() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            upload: true,
            bucket: Some("my-bucket".to_string()),
            no_verify_trace: true,
            ..Default::default()
        };

        let trace_data = TraceData {
            output_file: "test_output.fxt".to_string(),
            categories: vec![],
            stop_result: StopResult::default(),
        };

        let res = finalize_trace_impl(
            &env.context,
            trace_data,
            &stop_opts,
            writer,
            |_path, _bucket, _viewer| async { Err(anyhow!("simulated upload failure")) },
        )
        .await;

        assert!(res.is_ok());
        let output = test_buffers.into_stdout_str();
        let expected_output = "Results written to test_output.fxt\nUploading trace to gs://my-bucket...\nWarning: Failed to upload trace to GCS: simulated upload failure\n";
        assert_eq!(output, expected_output);
    }

    #[fuchsia::test]
    async fn test_finalize_trace_upload_cli_override_config() {
        let env = ffx_config::test_env()
            .user_config("trace.gcs_bucket", "config-default-bucket")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            upload: true,
            bucket: Some("cli-override-bucket".to_string()),
            no_verify_trace: true,
            ..Default::default()
        };

        let trace_data = TraceData {
            output_file: "test_output.fxt".to_string(),
            categories: vec![],
            stop_result: StopResult::default(),
        };

        let expected_url = "https://fuchsia-trace-viewer.corp.goog/trace/12345678";
        finalize_trace_impl(
            &env.context,
            trace_data,
            &stop_opts,
            writer,
            |_path, bucket, _viewer| async move {
                assert_eq!(bucket, "cli-override-bucket");
                Ok(expected_url.to_string())
            },
        )
        .await
        .unwrap();

        let output = test_buffers.into_stdout_str();
        let expected_output = format!(
            "Results written to test_output.fxt\nUploading trace to gs://cli-override-bucket...\nTrace uploaded successfully!\n\nView in Perfetto Trace Viewer:\n  {}\n",
            expected_url
        );
        assert_eq!(output, expected_output);
    }

    #[fuchsia::test]
    async fn test_finalize_trace_upload_viewer_url_config() {
        let env = ffx_config::test_env()
            .user_config("trace.viewer_url", "https://custom-viewer.corp.goog/")
            .build()
            .unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop { upload: true, no_verify_trace: true, ..Default::default() };

        let trace_data = TraceData {
            output_file: "test_output.fxt".to_string(),
            categories: vec![],
            stop_result: StopResult::default(),
        };

        let expected_url = "https://custom-viewer.corp.goog/trace/12345678";
        finalize_trace_impl(
            &env.context,
            trace_data,
            &stop_opts,
            writer,
            |_path, _bucket, viewer| async move {
                assert_eq!(viewer, "https://custom-viewer.corp.goog");
                Ok(expected_url.to_string())
            },
        )
        .await
        .unwrap();

        let output = test_buffers.into_stdout_str();
        let expected_output = format!(
            "Results written to test_output.fxt\nUploading trace to gs://fuchsia-trace-viewer-traces...\nTrace uploaded successfully!\n\nView in Perfetto Trace Viewer:\n  {}\n",
            expected_url
        );
        assert_eq!(output, expected_output);
    }

    #[fuchsia::test]
    async fn test_upload_trace_success() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let temp_dir = tempfile::tempdir().unwrap();
        let trace_path = temp_dir.path().join("existing_trace.fxt");
        std::fs::write(&trace_path, b"placeholder trace data").unwrap();

        let expected_url = "https://fuchsia-trace-viewer.corp.goog/trace/abcdef12";
        let trace_path_str = trace_path.to_str().unwrap().to_string();
        trace_upload_impl(
            &env.context,
            &trace_path_str,
            Some("custom-bucket"),
            writer,
            |path, bucket, _viewer| async move {
                assert_eq!(path, trace_path);
                assert_eq!(bucket, "custom-bucket");
                Ok(expected_url.to_string())
            },
        )
        .await
        .unwrap();

        let output = test_buffers.into_stdout_str();
        let expected_output = format!(
            "Uploading trace to gs://custom-bucket...\nTrace uploaded successfully!\n\nView in Perfetto Trace Viewer:\n  {}\n",
            expected_url
        );
        assert_eq!(output, expected_output);
    }

    #[fuchsia::test]
    async fn test_upload_trace_failure_error() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let temp_dir = tempfile::tempdir().unwrap();
        let trace_path = temp_dir.path().join("existing_trace.fxt");
        std::fs::write(&trace_path, b"placeholder trace data").unwrap();

        let res = trace_upload_impl(
            &env.context,
            trace_path.to_str().unwrap(),
            Some("custom-bucket"),
            writer,
            |_path, _bucket, _viewer| async { Err(anyhow!("simulated upload failure")) },
        )
        .await;

        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("Failed to upload trace to GCS: simulated upload failure"),
            "unexpected error message: {err_msg}"
        );
    }

    #[fuchsia::test]
    async fn test_upload_trace_missing_file_error() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let res = trace_upload_impl(
            &env.context,
            "non_existent_trace.fxt",
            None,
            writer,
            |_path, _bucket, _viewer| async { Ok("https://example.com".to_string()) },
        )
        .await;

        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(
            err_msg.contains("Trace file does not exist: non_existent_trace.fxt"),
            "unexpected error message: {err_msg}"
        );
    }

    #[fuchsia::test]
    async fn test_upload_trace_zero_length_error() {
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let temp_dir = tempfile::tempdir().unwrap();
        let trace_path = temp_dir.path().join("empty_trace.fxt");
        std::fs::write(&trace_path, b"").unwrap();

        let trace_path_str = trace_path.to_str().unwrap().to_string();
        let res = trace_upload_impl(
            &env.context,
            &trace_path_str,
            None,
            writer,
            |_path, _bucket, _viewer| async { Ok("https://example.com".to_string()) },
        )
        .await;

        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert_eq!(err_msg, "cannot upload zero length traces");
    }

    #[fuchsia::test]
    async fn test_upload_trace_tool_dispatch() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let temp_dir = tempfile::tempdir().unwrap();
        let trace_path = temp_dir.path().join("trace.fxt");
        std::fs::write(&trace_path, b"placeholder trace data").unwrap();

        let upload_opts = Upload {
            trace_file: Some(trace_path.to_str().unwrap().to_string()),
            bucket: Some("my-bucket".to_string()),
        };
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Upload(upload_opts) },
        };

        let res = tool.main(writer).await;
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to upload trace to GCS"), "unexpected error: {err_msg}");
    }

    #[fuchsia::test]
    async fn test_stop_abort() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        // Custom mock to verify write_results
        let session_manager =
            Deferred::from_output(Ok(fake_proxy(client.clone(), |req| match req {
                tracing_controller::SessionManagerRequest::AbortTraceSession {
                    task_id: _,
                    responder,
                } => {
                    responder.send(Ok(())).expect("should respond");
                }
                _ => panic!("unsupported req"),
            })));

        let stop_opts = Stop { abort: true, ..Default::default() };
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager,
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(stop_opts.clone()) },
        };

        tool.trace_stop(&stop_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        assert!(output.contains("Trace aborted."));
    }

    #[fuchsia::test]
    async fn test_stop_with_long_path() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let long_dirname = "long_directory_name_0123456789abcdef_1123456789abcdef_2123456789abcdef_3123456789abcdef_4123456789abcdef_5123456789abcdef_6123456789abcdef_7123456789abcdef_8123456789abcdef";

        let tmp_dir = TempDir::new().expect("tmp");
        let dir_path = tmp_dir.path().join(long_dirname);
        std::fs::create_dir_all(&dir_path).expect("temp directory");

        let stop_opts = Stop {
            output: Some(dir_path.join("trace.fxt").to_string_lossy().to_string()),
            verbose: false,
            no_symbolize: false,
            no_verify_trace: true,
            retain_raw_fidl: false,
            abort: false,
            ..Default::default()
        };
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(stop_opts.clone()) },
        };

        tool.trace_stop(&stop_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Results written to /([^/]+/)+?trace.fxt\nUpload to https://ui.perfetto.dev/#!/ to view.";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_start_verbose() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let start_opts = Start {
            buffer_size: 2,
            categories: vec!["platypus".to_string(), "beaver".to_string()],
            duration: None,
            buffering_mode: tracing::BufferingMode::Oneshot,
            output: None,
            background: true,
            verbose: true,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        // This doesn't find `/.../foo.txt` for the tracing status, since the faked
        // proxy has no state.
        let regex_str = "Tracing categories: \\[beaver,platypus\\]...
To manually stop the trace, use `ffx trace stop`
Current tracing status:
Task Id: 2468
Total Duration: infinite
Remaining Duration: infinite
Categories: beaver,platypus
Triggers:
- foo : Terminate
- bar : Terminate\n";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_stop_verbose() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let stop_opts = Stop {
            output: Some("foo.txt".to_string()),
            verbose: true,
            no_symbolize: false,
            no_verify_trace: true,
            retain_raw_fidl: false,
            abort: false,
            ..Default::default()
        };
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(stop_opts.clone()) },
        };

        tool.trace_stop(&stop_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "\"provider_bar\" \\(pid: 1234\\) trace stats\n\
            Buffer wrapped count: 10\n\
            # records dropped: 0\n\
            Durable buffer used: 30.00%\n\
            Bytes written to non-durable buffer: 0x28\n\n\
            Results written to /([^/]+/)+?foo.txt\n\
            Upload to https://ui.perfetto.dev/#!/ to view.";
        let want = Regex::new(regex_str).unwrap();
        assert!(
            want.is_match(&output),
            "Actual ----------\n{}\n didn't match regex \n{}\n----------",
            output,
            regex_str
        );
    }

    #[fuchsia::test]
    async fn test_start_with_duration() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let start_opts = Start {
            buffer_size: 2,
            categories: vec![],
            duration: Some(5),
            buffering_mode: tracing::BufferingMode::Oneshot,
            output: Some("foober.fxt".to_owned()),
            background: false,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Tracing categories: \\[\\]...\n";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_start_with_duration_foreground() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);

        let start_opts = Start {
            buffer_size: 2,
            categories: vec![],
            duration: Some(1),
            buffering_mode: tracing::BufferingMode::Oneshot,
            output: Some("foober.fxt".to_owned()),
            background: false,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Tracing categories: \\[\\]...\n\
            Trace completed! Copying trace from device...\n\
            Results written to /([^/]+/)+?foober.fxt\n\
            Upload to https://ui.perfetto.dev/#!/ to view.";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_start_foreground() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start {
            buffer_size: 2,
            categories: vec![],
            buffering_mode: tracing::BufferingMode::Oneshot,
            duration: None,
            output: Some("foober.fxt".to_owned()),
            background: false,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Tracing categories: \\[\\]...\n\
            Press <enter> to stop trace.\n\
            Trace completed! Copying trace from device...\n\
            Results written to /([^/]+/)+?foober.fxt\n\
            Upload to https://ui.perfetto.dev/#!/ to view.";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_start_upload() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start {
            buffer_size: 2,
            categories: vec![],
            duration: Some(1),
            buffering_mode: tracing::BufferingMode::Oneshot,
            output: Some("foo_upload.fxt".to_owned()),
            background: false,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            upload: true,
            bucket: Some("test-upload-bucket".to_string()),
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Tracing categories: \\[\\]...\n\
            Trace completed! Copying trace from device...\n\
            Results written to /([^/]+/)+?foo_upload.fxt\n\
            Uploading trace to gs://test-upload-bucket\\.\\.\\.\n\
            Warning: Failed to upload trace to GCS: .*";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[fuchsia::test]
    async fn test_start_background_upload_error() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start { background: true, upload: true, ..Default::default() };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        let res = tool.trace_start(&start_opts, writer).await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("The switch '--upload' cannot be used with background tracing")
        );
    }

    #[fuchsia::test]
    async fn test_start_on_boot_upload_error() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start { on_boot: true, upload: true, ..Default::default() };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        let res = tool.trace_start(&start_opts, writer).await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("The switch '--upload' cannot be used with on-boot tracing")
        );
    }

    #[fuchsia::test]
    async fn test_start_on_boot_with_background() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start { on_boot: true, background: true, ..Default::default() };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        let res = tool.trace_start(&start_opts, writer).await;
        assert!(res.is_ok());
        let output = test_buffers.into_stdout_str();
        assert!(
            output.contains("Once the device is rebooted, stop the trace using `ffx trace stop`")
        );
    }

    #[fuchsia::test]
    async fn test_stop_abort_exclusive_options() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();

        let cases = vec![
            (
                Stop { abort: true, upload: true, ..Default::default() },
                "The switch '--upload' cannot be used with '--abort'",
            ),
            (
                Stop { abort: true, output: Some("trace.fxt".to_string()), ..Default::default() },
                "The option '--output' cannot be used with '--abort'",
            ),
            (
                Stop { abort: true, bucket: Some("bucket".to_string()), ..Default::default() },
                "The option '--bucket' cannot be used with '--abort'",
            ),
            (
                Stop { abort: true, no_symbolize: true, ..Default::default() },
                "The switch '--no-symbolize' cannot be used with '--abort'",
            ),
            (
                Stop { abort: true, no_verify_trace: true, ..Default::default() },
                "The switch '--no-verify-trace' cannot be used with '--abort'",
            ),
            (
                Stop { abort: true, retain_raw_fidl: true, ..Default::default() },
                "The switch '--retain-raw-fidl' cannot be used with '--abort'",
            ),
        ];

        for (opts, expected_err) in cases {
            let test_buffers = TestBuffers::default();
            let writer = Writer::new_test(None, &test_buffers);
            let tool = TraceTool {
                provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
                session_manager: setup_fake_session_manager(client.clone()),
                context: env.context.clone(),
                cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(opts.clone()) },
            };
            let res = tool.trace_stop(&opts, writer).await;
            assert!(res.is_err(), "Expected error for opts: {:?}", opts);
            let err_msg = res.unwrap_err().to_string();
            assert!(
                err_msg.contains(expected_err),
                "Error \"{}\" did not contain \"{}\"",
                err_msg,
                expected_err
            );
        }
    }

    #[fuchsia::test]
    async fn test_stop_bucket_exclusive_options() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let opts = Stop { bucket: Some("my-bucket".to_string()), ..Default::default() };
        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Stop(opts.clone()) },
        };
        let res = tool.trace_stop(&opts, writer).await;
        assert!(res.is_err());
        assert!(
            res.unwrap_err()
                .to_string()
                .contains("The option '--bucket' can only be used with '--upload'")
        );
    }

    #[fuchsia::test]
    async fn test_start_exclusive_options() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();

        let cases = vec![
            (
                Start {
                    on_boot: true,
                    output: Some("trace.fxt".to_string()),
                    ..Default::default()
                },
                "The option '--output' cannot be used with on-boot tracing",
            ),
            (
                Start {
                    background: true,
                    bucket: Some("bucket".to_string()),
                    ..Default::default()
                },
                "The option '--bucket' can only be used with '--upload'",
            ),
            (
                Start { on_boot: true, bucket: Some("bucket".to_string()), ..Default::default() },
                "The option '--bucket' can only be used with '--upload'",
            ),
            (
                Start { bucket: Some("bucket".to_string()), upload: false, ..Default::default() },
                "The option '--bucket' can only be used with '--upload'",
            ),
        ];

        for (opts, expected_err) in cases {
            let test_buffers = TestBuffers::default();
            let writer = Writer::new_test(None, &test_buffers);
            let tool = TraceTool {
                provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
                session_manager: setup_fake_session_manager(client.clone()),
                context: env.context.clone(),
                cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(opts.clone()) },
            };
            let res = tool.trace_start(&opts, writer).await;
            assert!(res.is_err(), "Expected error for opts: {:?}", opts);
            let err_msg = res.unwrap_err().to_string();
            assert!(
                err_msg.contains(expected_err),
                "Error \"{}\" did not contain \"{}\"",
                err_msg,
                expected_err
            );
        }
    }

    #[fuchsia::test]
    async fn test_symbolize_exclusive_options() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();

        let cases = vec![
            (
                Symbolize {
                    ordinal: Some(12345),
                    fxt: Some("trace.fxt".to_string()),
                    outfile: None,
                    ir_path: vec![],
                    retain_raw_fidl: false,
                },
                "The options '--ordinal' and '--fxt' cannot be used together",
            ),
            (
                Symbolize {
                    ordinal: Some(12345),
                    fxt: None,
                    outfile: Some("out.txt".to_string()),
                    ir_path: vec![],
                    retain_raw_fidl: false,
                },
                "The option '--outfile' cannot be used with '--ordinal'",
            ),
            (
                Symbolize {
                    ordinal: None,
                    fxt: Some("trace.fxt".to_string()),
                    outfile: None,
                    ir_path: vec!["some/ir.json".to_string()],
                    retain_raw_fidl: false,
                },
                "The option '--ir-path' cannot be used with '--fxt'",
            ),
        ];

        for (opts, expected_err) in cases {
            let test_buffers = TestBuffers::default();
            let writer = Writer::new_test(None, &test_buffers);
            let tool = TraceTool {
                provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
                session_manager: setup_fake_session_manager(client.clone()),
                context: env.context.clone(),
                cmd: TraceCommand { sub_cmd: TraceSubCommand::Symbolize(opts.clone()) },
            };
            let res = tool.symbolize(&opts, writer).await;
            assert!(res.is_err(), "Expected error for opts: {:?}", opts);
            let err_msg = res.unwrap_err().to_string();
            assert!(
                err_msg.contains(expected_err),
                "Error \"{}\" did not contain \"{}\"",
                err_msg,
                expected_err
            );
        }
    }

    #[fuchsia::test]
    async fn test_large_buffer() {
        let client = fdomain_local::local_client_empty();
        let env = ffx_config::test_init().unwrap();
        let test_buffers = TestBuffers::default();
        let writer = Writer::new_test(None, &test_buffers);
        let start_opts = Start {
            buffer_size: 1024,
            categories: vec![],
            buffering_mode: tracing::BufferingMode::Oneshot,
            duration: None,
            output: Some("foober.fxt".to_owned()),
            background: false,
            verbose: false,
            trigger: vec![],
            no_symbolize: false,
            no_verify_trace: true,
            on_boot: false,
            retain_raw_fidl: false,
            nocompress: false,
            ..Default::default()
        };

        let tool = TraceTool {
            provisioner: Deferred::from_output(Err(fho::user_error!("not found"))),
            session_manager: setup_fake_session_manager(client),
            context: env.context.clone(),
            cmd: TraceCommand { sub_cmd: TraceSubCommand::Start(start_opts.clone()) },
        };

        tool.trace_start(&start_opts, writer).await.unwrap();
        let output = test_buffers.into_stdout_str();
        let regex_str = "Tracing categories: \\[\\]...\n\
            Press <enter> to stop trace.\n\
            Trace completed! Copying trace from device...\n\
            Results written to /([^/]+/)+?foober.fxt\n\
            Upload to https://ui.perfetto.dev/#!/ to view.";
        let want = Regex::new(regex_str).unwrap();
        assert!(want.is_match(&output), "\"{}\" didn't match regex /{}/", output, regex_str);
    }

    #[test]
    fn test_stats_to_print() {
        // Verbose output with dropped records
        let mut stats = tracing_controller::ProviderStats::default();
        stats.name = Some("provider_foo".to_string());
        stats.pid = Some(1234);
        stats.buffering_mode = Some(BufferingMode::Oneshot);
        stats.buffer_wrapped_count = Some(10);
        stats.records_dropped = Some(10);
        stats.percentage_durable_buffer_used = Some(30.0);
        stats.non_durable_bytes_written = Some(40);
        let warn_str = format!(
            "{}WARNING: \"provider_foo\" dropped 10 records!{}",
            color::Fg(color::Yellow),
            color::Fg(color::Reset)
        );
        let tip_str = format!(
            "{}TIP: One or more providers dropped records. Consider increasing the buffer size with `--buffer-size <MB>`.{}",
            style::Bold,
            style::Reset
        );
        let mut expected_output: Vec<String> = vec![
            "\"provider_foo\" (pid: 1234) trace stats".into(),
            "Buffer wrapped count: 10".into(),
            "# records dropped: 10".into(),
            "Durable buffer used: 30.00%".into(),
            "Bytes written to non-durable buffer: 0x28\n".into(),
            warn_str.clone(),
            tip_str.clone(),
        ];

        let mut actual_output = stats_to_output(vec![stats.clone()], true);
        assert_eq!(expected_output, actual_output);

        // Verify that dropped records warning is printed even if not verbose
        expected_output = vec![warn_str, tip_str];
        actual_output = stats_to_output(vec![stats.clone()], false);
        assert_eq!(expected_output, actual_output);

        // Verbose output with missing stats
        stats.buffer_wrapped_count = None;
        expected_output = vec![format!(
            "{}WARNING: 1 producers were missing stats. Perhaps a producer is misconfigured?{}",
            color::Fg(color::Yellow),
            style::Reset
        )];
        actual_output = stats_to_output(vec![stats.clone()], true);
        assert_eq!(expected_output, actual_output);

        // No output on missing stats if not verbose
        expected_output = vec![];
        actual_output = stats_to_output(vec![stats.clone()], false);
        assert_eq!(expected_output, actual_output);
    }

    #[fuchsia::test]
    async fn test_handle_recording_error() {
        let target = "fuchsia-device";
        let output_file = "foo_bar_bazzle_wazzle.fxt";
        let log_dir = "important_log_file.log";
        let env = ffx_config::test_env()
            .env_var("FUCHSIA_NODENAME", target.into())
            .user_config("log.dir", log_dir)
            .build()
            .unwrap();
        let context = &env.context;

        struct Test {
            error: RecordingError,
            matches: Vec<&'static str>,
        }

        // Avoid being overly prescriptive about the actual contents of the errors. Just make sure
        // the basics are included and the thing we care about is inside.
        use fdomain_fuchsia_tracing_controller::RecordingError::*;
        let tests = vec![
            Test { error: TargetProxyOpen, matches: vec!["unable to connect", "ffx doctor"] },
            Test { error: RecordingAlreadyStarted, matches: vec!["already", target] },
            Test { error: DuplicateTraceFile, matches: vec!["already", output_file] },
            Test { error: RecordingStart, matches: vec![log_dir, "starting"] },
            Test { error: RecordingStop, matches: vec![log_dir, "stopping"] },
            Test { error: NoSuchTraceFile, matches: vec!["stop trace", output_file] },
            Test { error: NoSuchTarget, matches: vec![target] },
            Test { error: DisconnectedTarget, matches: vec![target] },
        ];

        for test in tests.into_iter() {
            let error_string = format!("{:?}", test.error);
            let result =
                handle_recording_error(&context, test.error, &output_file.to_owned()).await;
            for matching_string in test.matches.into_iter() {
                assert!(
                    result.contains(matching_string),
                    "Unable to find string '{}' when handling error '{}'. Error string: \"{}\"",
                    matching_string,
                    error_string,
                    result
                );
            }
        }
    }
}
