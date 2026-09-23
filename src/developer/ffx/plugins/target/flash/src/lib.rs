// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use discovery::query::TargetInfoQuery;
use discovery::{TargetHandle, TargetState};
use errors::ffx_bail;
use fdomain_fuchsia_hardware_power_statecontrol::{
    AdminProxy, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fdomain_fuchsia_hwinfo::DeviceProxy;
use ffx_config::EnvironmentContext;
use ffx_fastboot::common::from_manifest;
use ffx_fastboot::util::{Event, UnlockEvent};
use ffx_fastboot_connection_factory::{RetryLimit, get_fastboot_interface};
use ffx_fastboot_interface::fastboot_interface::UploadProgress;
use ffx_flash_args::FlashCommand;
use ffx_flash_manifest::SSH_OEM_COMMAND;
use ffx_ssh::SshKeyFiles;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxContext, FfxError, FfxMain, FfxTool, deferred, return_bug, return_user_error};
use fidl::Error;
use futures::channel::oneshot;
use futures::try_join;
use gcs::client::{Client, ProgressResponse};
use pbms::{AuthFlowChoice, handle_new_access_token};
use schemars::JsonSchema;
use serde::Serialize;
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::io::{Write, stderr, stdin, stdout};
use std::path::PathBuf;
use structured_ui::{Interface, TextUi};
use target_holders::moniker;
use tempfile::TempDir;
use termion::{color, style};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use url::Url;

#[derive(FfxTool)]
#[target(None)]
#[main_error(FlashError)]
pub struct FlashTool {
    #[command]
    cmd: FlashCommand,
    ctx: EnvironmentContext,
    #[with(deferred(moniker("/bootstrap/shutdown_shim")))]
    power_proxy: fho::Deferred<AdminProxy>,
    #[with(deferred(moniker("/core/hwinfo")))]
    device_proxy: fho::Deferred<DeviceProxy>,
}

fho::embedded_plugin!(FlashTool, FlashError);

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FlashMessage {
    Preflight { message: String },
    Progress(FlashProgress),
    Finished { success: bool, error_message: String },
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FlashProgress {
    FlashPartitionStarted { partition_name: String },
    FlashPartitionFinished { partition_name: String },
    Unlock,
    RebootToBootloaderStarted,
    RebootToBootloaderFinished,
    GotVariable,
    OemCommand { oem_command: String },
    UploadStarted,
    UploadFinished,
    UploadError,
}

#[derive(Debug)]
pub struct ProgressIndicator {
    title: String,
    is_finished: bool,

    // Progress layer 1.
    partition_subtitle: String,
    partition_progress: u64,
    partition_total: u64,

    // Progress layer 2.
    file_progress: u64,
    file_subtotal: u64,

    // Progress layer 3.
    bytes_progress: u64,
    bytes_subtotal: u64,
}

impl ProgressIndicator {
    fn new() -> Self {
        Self {
            title: "Flashing unknown product".into(),
            is_finished: false,
            partition_subtitle: "Processing next partition...".into(),
            partition_progress: 0,
            partition_total: 0,
            file_progress: 0,
            file_subtotal: 0,
            bytes_progress: 0,
            bytes_subtotal: 0,
        }
    }

    fn init(&mut self, product_name: &str, total_partitions: u64) {
        self.title = format!("Flashing {}", product_name);
        self.partition_total = total_partitions;
    }

    fn start_next_partition(&mut self, partition_name: &str, files: u64) {
        self.partition_subtitle = format!("Flash partition: {}", partition_name);
        self.partition_progress += 1;
        self.file_subtotal = files;
    }

    fn finish_partition(&mut self) {
        self.partition_subtitle = "Processing next partition...".to_owned();
        self.is_finished = self.partition_progress == self.partition_total;
        self.file_progress = 0;
        self.file_subtotal = 0;
        self.bytes_subtotal = 0;
    }

    fn start_next_file(&mut self, file_size: u64) {
        self.file_progress += 1;
        self.bytes_progress = 0;
        self.bytes_subtotal = file_size;
    }

    fn wrote_bytes(&mut self, bytes_written: u64) {
        self.bytes_progress += bytes_written;
    }

    fn present<I: Interface>(&mut self, ui: &mut I) -> anyhow::Result<()> {
        // Don't rerender the progress bar if we're done flashing.
        // Since the TUI is cleared before each `present()` in
        // `handle_event_text()`.
        // This avoids having a lingering progress bar after the flash command
        // is complete.
        if self.is_finished {
            return Ok(());
        }

        let mut progress = structured_ui::Progress::builder();
        progress.title(&self.title);
        progress.entry(
            "Flash partitions",
            self.partition_progress,
            self.partition_total,
            "partitions",
        );
        progress.entry(&self.partition_subtitle, self.file_progress, self.file_subtotal, "files");
        if self.bytes_subtotal > 0 {
            progress.entry("Upload file", self.bytes_progress, self.bytes_subtotal, "bytes");
        }
        let result = structured_ui::Presentation::Progress(progress);
        ui.present(&result)?;
        Ok(())
    }
}

#[derive(FfxError, Error, Debug)]
pub enum PreflightError {
    #[exit_with_code(1)]
    #[error("Writer error: {0}")]
    Writer(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("the manifest must be specified either by positional argument or the --manifest flag")]
    ManifestSpecifiedTwice,
}

#[derive(FfxError, Error, Debug)]
pub enum ProductBundleError {
    #[exit_with_code(1)]
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[exit_with_code(1)]
    #[error("Invalid GS URL: {0}")]
    UrlParse(#[from] url::ParseError),

    #[exit_with_code(1)]
    #[error("GS URL \"{url}\" is missing a host (bucket name)")]
    GcsUrlMissingHost { url: String },

    #[exit_with_code(1)]
    #[error("GS URL \"{url}\" is missing a filename")]
    GcsUrlMissingFilename { url: String },

    #[exit_with_code(1)]
    #[error("Failed to initialize GCS download client: {0}")]
    GcsClientInitialization(#[source] anyhow::Error),

    #[exit_with_code(1)]
    #[error("Failed to fetch GCS access token: {0}")]
    GcsAccessTokenFetch(#[source] anyhow::Error),

    #[exit_with_code(1)]
    #[error("Failed to download product bundle from GCS: {0}")]
    GcsDownloadFailed(#[source] anyhow::Error),

    #[exit_with_code(1)]
    #[error("Failed to clear progress indicators from UI: {0}")]
    UiClearFailed(#[from] structured_ui::StructuredUiError),

    #[exit_with_code(1)]
    #[error("SSH key setup failed: {0}")]
    SshKey(#[from] ffx_ssh::SshKeyError),

    #[exit_with_code(1)]
    #[error("Config resolution failed: {0}")]
    Config(#[from] ffx_config::ConfigError),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),

    #[exit_with_code(1)]
    #[error("Cannot find SSH key \"{path}\": {error}")]
    SshKeyNotFound { path: String, error: std::io::Error },

    #[exit_with_code(1)]
    #[error("Both the SSH key and the SSH OEM Stage flags were set. Only use one.")]
    SshKeyAndOemStageSet,

    #[exit_with_code(1)]
    #[error("Both the SSH key and Skip Uploading Authorized Keys flags were set. Only use one.")]
    SshKeyAndSkipUploadSet,

    #[exit_with_code(1)]
    #[error("We requested ssh keys to be created but they were not")]
    SshKeysNotCreated,

    #[exit_with_code(1)]
    #[error(
        "Both the SSH OEM Stage and Skip Uploading Authorized Keys flags were set. Only use one."
    )]
    OemStageAndSkipUploadSet,

    #[exit_with_code(1)]
    #[error("Manifest path: {path} does not exist")]
    ManifestPathDoesNotExist { path: PathBuf },

    #[exit_with_code(1)]
    #[error("SSH key path \"{path:?}\" contains invalid UTF-8")]
    SshKeyPathInvalidUtf8 { path: std::ffi::OsString },
}

#[derive(FfxError, Error, Debug)]
pub enum FlashError {
    #[exit_with_code(1)]
    #[error("Preflight validation failed: {0}")]
    Preflight(#[from] PreflightError),

    #[exit_with_code(1)]
    #[error("Failed to resolve or download product bundle: {0}")]
    ProductBundleResolution(#[from] ProductBundleError),

    #[exit_with_code(1)]
    #[error("Fastboot target discovery failed: {0}")]
    FastbootDiscovery(#[from] ffx_target::FfxTargetCrateError),

    #[exit_with_code(1)]
    #[error("Failed to reboot target to bootloader: {0}")]
    TargetReboot(#[source] fho::Error),

    #[exit_with_code(1)]
    #[error("Fastboot flashing execution failed: {0}")]
    FlashExecution(#[from] ffx_fastboot::error::FfxFastbootError),

    #[exit_with_code(1)]
    #[error("FFX Writer error: {0}")]
    Writer(#[from] ffx_writer::Error),

    #[transparent]
    #[error(transparent)]
    Fho(#[from] fho::Error),
}

#[async_trait(?Send)]
impl FfxMain for FlashTool {
    type Writer = VerifiedMachineWriter<FlashMessage>;
    type Error = FlashError;

    async fn main(self, mut writer: Self::Writer) -> Result<(), Self::Error> {
        // Checks
        preflight_checks(&self.cmd, &mut writer)?;

        // Massage FlashCommand
        let cmd = preprocess_flash_cmd(&self.ctx, &mut writer, &self.cmd).await?;

        self.flash_plugin_impl(cmd, &mut writer).await.map_err(FlashError::from)
    }
}

fn preflight_checks<W: Write>(cmd: &FlashCommand, mut writer: W) -> Result<(), PreflightError> {
    if cmd.manifest_path.is_some() {
        // TODO(https://fxbug.dev/42076631)
        writeln!(writer, "{}WARNING:{} specifying the flash manifest via a positional argument is deprecated. Use the --manifest flag instead (https://fxbug.dev/42076631)", color::Fg(color::Red), style::Reset)
            .map_err(PreflightError::Writer)?;
    }
    if cmd.manifest_path.is_some() && cmd.manifest.is_some() {
        return Err(PreflightError::ManifestSpecifiedTwice);
    }
    Ok(())
}

async fn preprocess_flash_cmd(
    ctx: &EnvironmentContext,
    writer: &mut VerifiedMachineWriter<FlashMessage>,
    i_cmd: &FlashCommand,
) -> Result<FlashCommand, ProductBundleError> {
    let cmd: &mut FlashCommand = &mut i_cmd.clone();

    if cmd.product_bundle.is_some()
        && cmd.product_bundle.clone().unwrap().starts_with("\"")
        && cmd.product_bundle.clone().unwrap().ends_with("\"")
    {
        let cleaned_product_bundle = cmd
            .product_bundle
            .clone()
            .unwrap()
            .strip_prefix('"')
            .unwrap()
            .strip_suffix('"')
            .unwrap()
            .to_string();
        log::debug!(
            "Passed product bundle was wrapped in quotes, trimming it to: {}",
            cleaned_product_bundle
        );
        cmd.product_bundle = Some(cleaned_product_bundle);
    }

    // Download product bundle from gs:// if necessary.
    if let Some(product_bundle) = &cmd.product_bundle {
        if product_bundle.starts_with("gs://") {
            let dir = TempDir::new().map_err(ProductBundleError::Io)?.keep();
            let url = Url::parse(product_bundle)?;
            let bucket = url.host_str().filter(|h| !h.is_empty()).ok_or_else(|| {
                ProductBundleError::GcsUrlMissingHost { url: product_bundle.clone() }
            })?;
            let file_name = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| ProductBundleError::GcsUrlMissingFilename {
                    url: product_bundle.clone(),
                })?;
            let local_path = dir.join(file_name);

            let client = Client::initial().map_err(ProductBundleError::GcsClientInitialization)?;
            let object = &url.path()[1..]; // Strip leading slash

            log::debug!("Downloading {}...", product_bundle);
            let access_token =
                handle_new_access_token(&AuthFlowChoice::Pkce, &structured_ui::MockUi::new())
                    .await
                    .map_err(|e| ProductBundleError::GcsAccessTokenFetch(e.into()))?;
            client.set_access_token(access_token).await;
            let mut input = stdin();
            let mut output = stdout();
            let mut err_out = stderr();
            let ui = TextUi::new(&mut input, &mut output, &mut err_out);
            client
                .fetch_with_progress(bucket, object, &local_path, &|progress| {
                    let mut p = structured_ui::Progress::builder();
                    p.title("Downloading product bundle from GCS");
                    p.entry(progress.name, progress.at, progress.of, progress.units);
                    ui.present(&structured_ui::Presentation::Progress(p))?;
                    Ok(ProgressResponse::Continue)
                })
                .await
                .map_err(ProductBundleError::GcsDownloadFailed)?;
            ui.clear_progress()?;

            log::debug!("Downloaded to {}", local_path.display());

            cmd.product_bundle = Some(local_path.to_string_lossy().to_string());
        }
    }

    match cmd.authorized_keys.as_ref() {
        Some(ssh) => {
            let ssh_file = match std::fs::canonicalize(ssh) {
                Ok(path) => path,
                Err(err) => {
                    return Err(ProductBundleError::SshKeyNotFound {
                        path: ssh.to_string(),
                        error: err,
                    });
                }
            };
            if cmd.oem_stage.iter().any(|f| f.command() == SSH_OEM_COMMAND) {
                return Err(ProductBundleError::SshKeyAndOemStageSet);
            }
            if cmd.skip_authorized_keys {
                return Err(ProductBundleError::SshKeyAndSkipUploadSet);
            }
            cmd.authorized_keys = Some(
                ssh_file
                    .into_os_string()
                    .into_string()
                    .map_err(|s| ProductBundleError::SshKeyPathInvalidUtf8 { path: s })?,
            );
        }
        None => {
            if !cmd.oem_stage.iter().any(|f| f.command() == SSH_OEM_COMMAND) {
                if cmd.skip_authorized_keys {
                    log::warn!("Skipping uploading authorized keys");
                } else {
                    let ssh_keys = SshKeyFiles::load(ctx).map_err(ProductBundleError::SshKey)?;
                    ssh_keys.create_keys_if_needed(false).map_err(ProductBundleError::SshKey)?;
                    if ssh_keys.authorized_keys.exists() {
                        let k = ssh_keys.authorized_keys.display().to_string();
                        log::debug!("No `--authorized-keys` flag, using {}", k);
                        cmd.authorized_keys = Some(k);
                    } else {
                        // Since the key will be initialized, this should never happen.
                        return Err(ProductBundleError::SshKeysNotCreated);
                    }
                }
            } else if cmd.skip_authorized_keys {
                // We have both skip authorized-keys and the OEM command including
                // the authorized keys... this is a problem.
                return Err(ProductBundleError::OemStageAndSkipUploadSet);
            }
        }
    };

    if cmd.manifest_path.is_some() {
        if !std::path::Path::exists(cmd.manifest_path.clone().unwrap().as_path()) {
            return Err(ProductBundleError::ManifestPathDoesNotExist {
                path: cmd.manifest_path.clone().unwrap(),
            });
        }
    }

    if cmd.product_bundle.is_none() && cmd.manifest_path.is_none() && cmd.manifest.is_none() {
        let product_path: String = ctx.get("product.path").map_err(ProductBundleError::Config)?;
        let message = format!(
            "No product bundle or manifest passed. Inferring product bundle path from config: {}",
            product_path
        );
        log::debug!("{}", message);
        writer
            .machine_or(&FlashMessage::Preflight { message: message.clone() }, message)
            .map_err(ProductBundleError::Writer)?;

        cmd.product_bundle = Some(product_path);
    }

    Ok(cmd.to_owned())
}

async fn reboot_target_to_bootloader_and_rediscover(
    writer: &mut VerifiedMachineWriter<FlashMessage>,
    ctx: &EnvironmentContext,
    device_proxy: DeviceProxy,
    power_proxy: AdminProxy,
    node_name: &Option<String>,
) -> fho::Result<TargetHandle> {
    // Wait to allow the Target to fully cycle to the bootloader
    writeln!(writer, "Waiting for Target to reboot...")
        .user_message("Error writing user message")?;
    writer.flush().user_message("Error flushing writer buffer")?;

    // Tell the target to reboot to the bootloader
    // Should probably get the serial number of the target just in case
    // let device_proxy = device_proxy.await.bug_context("Initializing device proxy")?;
    let info = device_proxy.get_info().await.bug_context("Getting target device info")?;

    // Tell the target to reboot to the bootloader
    log::debug!("Target in Product state. Rebooting to bootloader...",);

    // These calls erroring is fine...
    match power_proxy
        .shutdown(&ShutdownOptions {
            action: Some(ShutdownAction::RebootToBootloader),
            reasons: Some(vec![ShutdownReason::DeveloperRequest]),
            ..Default::default()
        })
        .await
    {
        Ok(_) => {}
        Err(e) => handle_fidl_connection_err(e)?,
    };

    let query = match (info.serial_number, node_name) {
        (Some(sn), _) => TargetInfoQuery::Id(sn),
        (None, Some(nn)) => TargetInfoQuery::NodenameOrId(nn.clone()),
        (None, None) => TargetInfoQuery::First,
    };
    ffx_target::discover_fastboot_target(&ctx, query, Some(100000)).await.map_err(|e| e.into())
}

impl FlashTool {
    const EVENT_CHANNEL_SIZE: usize = 128;

    async fn flash_plugin_impl(
        self,
        cmd: FlashCommand,
        writer: &mut VerifiedMachineWriter<FlashMessage>,
    ) -> fho::Result<()> {
        let handle = ffx_target::discover_single_default_target(&self.ctx)
            .await
            .map_err(anyhow::Error::from)?;

        let handle = match &handle.state {
            TargetState::Fastboot(_) => {
                // Nothing to do
                log::debug!("Target already in Fastboot state");
                handle
            }
            TargetState::Product { .. } => {
                if self.ctx.is_strict() {
                    return_user_error!(
                        r"
When running in strict mode, this tool does not support Targets in Product mode.
Reboot the Target to the bootloader and re-run this command."
                    );
                }
                let device_proxy =
                    self.device_proxy.await.bug_context("Initializing device proxy")?;
                let power_proxy = self.power_proxy.await?;

                reboot_target_to_bootloader_and_rediscover(
                    writer,
                    &self.ctx,
                    device_proxy,
                    power_proxy,
                    &handle.node_name,
                )
                .await?
            }
            TargetState::Unknown => {
                ffx_bail!("Target is in an Unknown state.");
            }
            TargetState::Zedboot => {
                ffx_bail!("Bootloader operations not supported with Zedboot");
            }
        };

        let start_time = Utc::now();

        let (cancel_sender, cancel_receiver) = oneshot::channel::<()>();
        let mut signals = Signals::new(&[SIGINT, SIGTERM]).unwrap();
        let _signal_handle_thread = std::thread::spawn(move || {
            if let Some(signal) = signals.forever().next() {
                match signal {
                    SIGINT | SIGTERM => {
                        let _ = cancel_sender.send(());
                    }
                    _ => unreachable!(),
                }
            }
        });

        let flash_fut = async {
            match handle.state {
                TargetState::Fastboot(fastboot_state) => {
                    let mut proxy = get_fastboot_interface(
                        &fastboot_state,
                        handle.node_name,
                        &self.ctx,
                        RetryLimit::Forever,
                    )
                    .await
                    .map_err(|e| anyhow::Error::from(e))?;

                    let (event_tx, event_rx) = mpsc::channel(Self::EVENT_CHANNEL_SIZE);
                    let handler_fut = if writer.is_machine() {
                        futures::future::Either::Left(handle_event_machine(writer, event_rx))
                    } else {
                        futures::future::Either::Right(handle_event_text(writer, event_rx))
                    };
                    try_join!(
                        async {
                            from_manifest(
                                &self.ctx,
                                event_tx,
                                cmd.to_manifest(&self.ctx),
                                &mut proxy,
                            )
                            .await
                            .map_err(anyhow::Error::from)
                        },
                        handler_fut
                    )
                    .map_err(fho::Error::from)?;
                    Ok(())
                }
                _ => {
                    ffx_bail!("Could not connect. Target not in fastboot: {handle}");
                }
            }
        };

        let res = match futures::future::select(Box::pin(flash_fut), cancel_receiver).await {
            futures::future::Either::Left((r, _)) => r,
            futures::future::Either::Right((_, _)) => {
                log::warn!("Received signal, cancelling flash operation");
                return_user_error!("Flash operation cancelled by signal")
            }
        };

        match res {
            Ok(()) => {
                let duration = Utc::now().signed_duration_since(start_time);
                let finished_message = format!(
                    "Continuing to boot - this could take a while\n{}Done{}. {}Total Time{} [{}{:.2}s{}]",
                    color::Fg(color::Green),
                    style::Reset,
                    color::Fg(color::Green),
                    style::Reset,
                    color::Fg(color::Blue),
                    (duration.num_milliseconds() as f32) / (1000 as f32),
                    style::Reset,
                );

                writer.machine_or(
                    &FlashMessage::Finished { success: true, error_message: "".to_string() },
                    finished_message,
                )?;
            }
            Err(e) => {
                writer.machine_or(
                    &FlashMessage::Finished { success: false, error_message: format!("{}", e) },
                    format!("Error: {:?}", e),
                )?;
                return Err(e);
            }
        }
        Ok(())
    }
}

fn handle_fidl_connection_err(e: Error) -> fho::Result<()> {
    match e {
        Error::ClientChannelClosed { protocol_name, .. } => {
            // Changing this to an info from warn since reboot has succeeded The assumption that
            // reboot has succeeded is correct since we received a ClientChannelClosed
            // successfully. So let's just make the message clearer to the user.
            //
            // Check the 'protocol_name' and if it is 'fuchsia.hardware.power.statecontrol.Admin'
            // then we can be more confident that target reboot/shutdown has succeeded.
            if protocol_name == "fuchsia.hardware.power.statecontrol.Admin" {
                log::info!("Target reboot succeeded.");
            } else {
                log::info!(
                    "Assuming target reboot succeeded. Client received a PEER_CLOSED from '{protocol_name}'"
                );
            }
            log::debug!("{:?}", e);
            Ok(())
        }
        Error::ClientRead(_) => {
            // Under FDomain (direct target connections over SSH or VSOCK), when the
            // target initiates reboot the remote transport drops abruptly without
            // sending an in-band PEER_CLOSED epitaph. The resulting transport error
            // (e.g. Broken Pipe / StreamingAborted) surfaces as a ClientRead error
            // while awaiting the FIDL response, indicating the target shut down as
            // expected.
            log::info!("Target reboot succeeded (transport closed).");
            log::debug!("{:?}", e);
            Ok(())
        }
        _ => {
            log::error!("Target communication error: {:?}", e);
            return_bug!("Target communication error: {:?}", e)
        }
    }
}

fn done() -> String {
    format!("{}Done{}", color::Fg(color::Green), style::Reset)
}

fn time(duration: Duration) -> String {
    format!(
        "[{}{:.2}s{}]",
        color::Fg(color::Blue),
        (duration.num_milliseconds() as f32) / (1000 as f32),
        style::Reset
    )
}

async fn handle_event_text(
    writer: &mut VerifiedMachineWriter<FlashMessage>,
    mut rec: Receiver<Event>,
) -> anyhow::Result<()> {
    let mut input = stdin();
    // TUI progress presentations are noop in non-TTY cases, so we won't need to
    // worry about noisy output in non-interactive flash use-cases like CI/CQ.
    let mut output = stdout();
    let mut err_out = stderr();
    let mut ui = TextUi::new(&mut input, &mut output, &mut err_out);
    let mut progress_indicator = ProgressIndicator::new();
    loop {
        // Clear TUI so normal stdout/stderr doesn't instantly get overwritten
        // by the progress indicator.
        ui.clear_progress().map_err(|e| anyhow::Error::new(e))?;
        match rec.recv().await {
            Some(event) => match event {
                Event::Upload(upload) => match upload {
                    UploadProgress::OnReady { partition, files } => {
                        log::info!("Uploading partition {} ({} files)", partition, files);
                        progress_indicator.start_next_partition(&partition, files);
                    }
                    UploadProgress::OnStarted { size } => {
                        progress_indicator.start_next_file(size);
                    }
                    UploadProgress::OnProgress { bytes_written } => {
                        log::trace!("Made progress, wrote: {}", bytes_written);
                        progress_indicator.wrote_bytes(bytes_written);
                    }
                    UploadProgress::OnFinished => {}
                    UploadProgress::OnError { error } => {
                        writeln!(writer, "Error {}", error)?;
                    }
                },
                Event::FlashProduct { product_name, partition_count } => {
                    log::info!("Flashing {} ({} partitions)", product_name, partition_count);
                    progress_indicator.init(&product_name, partition_count.try_into()?);
                }
                Event::FlashPartition { .. } => {}
                Event::FlashPartitionFinished { partition_name, duration } => {
                    progress_indicator.finish_partition();
                    writeln!(writer, "Flashed {} in {}", partition_name, time(duration))?;
                }
                Event::Unlock(unlock_event) => match unlock_event {
                    UnlockEvent::SearchingForCredentials => {
                        writeln!(writer, "Looking for unlock credentials...")?
                    }
                    UnlockEvent::GeneratingToken => writeln!(writer, "Generating unlock token...")?,
                    UnlockEvent::FoundCredentials(delta)
                    | UnlockEvent::FinishedGeneratingToken(delta) => {
                        writeln!(writer, "{} {}", done(), time(delta))?
                    }
                    UnlockEvent::BeginningUploadOfToken => {
                        writeln!(writer, "Preparing to upload unlock token...")?
                    }
                    UnlockEvent::Done => writeln!(writer, "{}", done())?,
                },
                Event::Oem { oem_command } => {
                    writeln!(writer, "Sending command: \"{}\"", oem_command)?;
                }
                Event::RebootStarted => {
                    writeln!(writer, "Rebooting to bootloader... ")?;
                }
                Event::Rebooted(delta) => {
                    writeln!(writer, "{} {}", done(), time(delta))?;
                }
                Event::Variable(variable) => {
                    log::trace!("got variable {:#?}", variable);
                }
                Event::Locked => {
                    let msg = "The flashing library should not lock the device...";
                    writeln!(writer, "Error: {}", msg)?;
                    return Err(anyhow!(msg));
                }
            },
            None => return Ok(()),
        }
        progress_indicator.present(&mut ui)?;
    }
}

async fn handle_event_machine(
    writer: &mut VerifiedMachineWriter<FlashMessage>,
    mut rec: Receiver<Event>,
) -> anyhow::Result<()> {
    loop {
        match rec.recv().await {
            Some(event) => match event {
                Event::Upload(upload) => match upload {
                    UploadProgress::OnReady { partition, files } => {
                        log::info!("Uploading partition {} ({} files)", partition, files);
                    }
                    UploadProgress::OnStarted { size: _ } => {
                        let output = FlashMessage::Progress(FlashProgress::UploadStarted);
                        writer.machine(&output)?;
                    }
                    UploadProgress::OnProgress { bytes_written } => {
                        log::trace!("Made progress, wrote: {}", bytes_written);
                    }
                    UploadProgress::OnFinished => {
                        let output = FlashMessage::Progress(FlashProgress::UploadFinished);
                        writer.machine(&output)?;
                    }
                    UploadProgress::OnError { error: _ } => {
                        let output = FlashMessage::Progress(FlashProgress::UploadError);
                        writer.machine(&output)?;
                    }
                },
                Event::FlashProduct { product_name, partition_count } => {
                    log::info!("Flashing {} ({} partitions)", product_name, partition_count);
                }
                Event::FlashPartition { partition_name } => {
                    let output = FlashMessage::Progress(FlashProgress::FlashPartitionStarted {
                        partition_name: partition_name,
                    });
                    writer.machine(&output)?;
                }
                Event::FlashPartitionFinished { partition_name, duration: _ } => {
                    let output = FlashMessage::Progress(FlashProgress::FlashPartitionFinished {
                        partition_name: partition_name,
                    });
                    writer.machine(&output)?;
                }
                Event::Unlock(unlock_event) => {
                    let output = match unlock_event {
                        UnlockEvent::SearchingForCredentials => {
                            FlashMessage::Progress(FlashProgress::Unlock)
                        }
                        UnlockEvent::FoundCredentials(_) => {
                            FlashMessage::Progress(FlashProgress::Unlock)
                        }
                        UnlockEvent::GeneratingToken => {
                            FlashMessage::Progress(FlashProgress::Unlock)
                        }
                        UnlockEvent::FinishedGeneratingToken(_) => {
                            FlashMessage::Progress(FlashProgress::Unlock)
                        }
                        UnlockEvent::BeginningUploadOfToken => {
                            FlashMessage::Progress(FlashProgress::Unlock)
                        }
                        UnlockEvent::Done => FlashMessage::Progress(FlashProgress::Unlock),
                    };
                    writer.machine(&output)?;
                }
                Event::Oem { oem_command } => {
                    let output = FlashMessage::Progress(FlashProgress::OemCommand {
                        oem_command: oem_command,
                    });
                    writer.machine(&output)?;
                }
                Event::RebootStarted => {
                    let output = FlashMessage::Progress(FlashProgress::RebootToBootloaderStarted);
                    writer.machine(&output)?;
                }
                Event::Rebooted(_) => {
                    let output = FlashMessage::Progress(FlashProgress::RebootToBootloaderFinished);
                    writer.machine(&output)?;
                }
                Event::Variable(variable) => {
                    log::trace!("got variable {:#?}", variable);
                    let output = FlashMessage::Progress(FlashProgress::GotVariable);
                    writer.machine(&output)?;
                }
                Event::Locked => {
                    let msg = format!("The flashing library should not Lock the device...");
                    let output =
                        FlashMessage::Finished { success: false, error_message: msg.clone() };
                    writer.machine(&output)?;
                    return Err(anyhow!(msg));
                }
            },
            None => return Ok(()),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use ffx_config::environment::TestEnvBuilder;
    use ffx_flash_manifest::OemFile;
    use ffx_writer::{Format, TestBuffers};
    use pretty_assertions::assert_eq;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn setup_ssh_paths(builder: TestEnvBuilder) -> (TestEnvBuilder, NamedTempFile, NamedTempFile) {
        let temp_ssh_priv = NamedTempFile::new().expect("creating temp file for ssh.priv");
        let temp_ssh_pub = NamedTempFile::new().expect("creating temp file for ssh.pub");
        let builder = builder
            .user_config("ssh.priv", temp_ssh_priv.path().to_string_lossy())
            .user_config("ssh.pub", temp_ssh_pub.path().to_string_lossy());
        (builder, temp_ssh_priv, temp_ssh_pub)
    }

    #[fuchsia::test]
    async fn test_preprocess_flash_command_infers_product_bundle() {
        let builder = ffx_config::test_env().user_config("product.path", "foo");
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(builder);
        let env = builder.build().expect("Failed to initialize test env");
        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);
        let cmd = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand { skip_authorized_keys: true, ..Default::default() },
        )
        .await
        .unwrap();
        assert_eq!(cmd.product_bundle, Some("foo".to_string()));
    }

    #[fuchsia::test]
    async fn test_nonexistent_file_throws_err() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);
        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                manifest_path: Some(PathBuf::from("ffx_test_does_not_exist")),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            res,
            Err(ProductBundleError::ManifestPathDoesNotExist { path }) if path == PathBuf::from("ffx_test_does_not_exist")
        ));
    }

    #[fuchsia::test]
    async fn test_clean_quotes() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let pb_tmp_file = NamedTempFile::new().expect("tmp access failed");
        let pb_tmp_file_name = pb_tmp_file.path().to_string_lossy().to_string();
        let wrapped_pb_tmp_file_name = format!("\"{}\"", pb_tmp_file_name);

        let ssh_tmp_file = NamedTempFile::new().expect("tmp access failed");
        let ssh_tmp_file_name = ssh_tmp_file.path().to_string_lossy().to_string();

        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);

        let cmd = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                product_bundle: Some(wrapped_pb_tmp_file_name),
                authorized_keys: Some(ssh_tmp_file_name),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(Some(pb_tmp_file_name), cmd.product_bundle);
    }

    #[fuchsia::test]
    async fn test_nonexistent_ssh_file_throws_err() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);
        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                manifest_path: Some(PathBuf::from(tmp_file_name)),
                authorized_keys: Some("ssh_does_not_exist".to_string()),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            res,
            Err(ProductBundleError::SshKeyNotFound { path, .. }) if path == "ssh_does_not_exist"
        ));
    }

    #[fuchsia::test]
    async fn test_gcs_url_invalid_syntax_throws_err() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);
        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                product_bundle: Some("gs://bucket:999999/file".to_string()),
                skip_authorized_keys: true,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(res, Err(ProductBundleError::UrlParse(_))));
    }

    #[fuchsia::test]
    async fn test_gcs_url_missing_host_throws_err() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);
        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                product_bundle: Some("gs:///filename".to_string()),
                skip_authorized_keys: true,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            res,
            Err(ProductBundleError::GcsUrlMissingHost { url }) if url == "gs:///filename"
        ));
    }

    #[fuchsia::test]
    async fn test_gcs_url_missing_filename_throws_err() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);

        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                product_bundle: Some("gs://bucket".to_string()),
                skip_authorized_keys: true,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            res,
            Err(ProductBundleError::GcsUrlMissingFilename { url }) if url == "gs://bucket"
        ));

        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                product_bundle: Some("gs://bucket/".to_string()),
                skip_authorized_keys: true,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(
            res,
            Err(ProductBundleError::GcsUrlMissingFilename { url }) if url == "gs://bucket/"
        ));
    }

    #[fuchsia::test]
    async fn test_specify_manifest_twice_throws_error() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let _env = builder.build().expect("Failed to initialize test env");
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);
        let res = preflight_checks(
            &FlashCommand {
                manifest: Some(PathBuf::from(tmp_file_name.clone())),
                manifest_path: Some(PathBuf::from(tmp_file_name)),
                ..Default::default()
            },
            &mut writer,
        );
        assert!(matches!(res, Err(PreflightError::ManifestSpecifiedTwice)));
    }

    #[fuchsia::test]
    async fn test_both_ssh_key_and_oem_stage_throws_error() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);

        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                manifest_path: Some(PathBuf::from(&tmp_file_name)),
                authorized_keys: Some(tmp_file_name),
                oem_stage: vec![OemFile::new(
                    SSH_OEM_COMMAND.to_string(),
                    "some_key_path".to_string(),
                )],
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(res, Err(ProductBundleError::SshKeyAndOemStageSet)));
    }

    #[fuchsia::test]
    async fn test_ssh_key_and_skip_upload_keys_throws_error() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);

        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                manifest_path: Some(PathBuf::from(&tmp_file_name)),
                authorized_keys: Some(tmp_file_name),
                skip_authorized_keys: true,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(res, Err(ProductBundleError::SshKeyAndSkipUploadSet)));
    }

    #[fuchsia::test]
    async fn test_oem_stage_and_skip_upload_keys_throws_error() {
        let (builder, _tmp_ssh_priv, _tmp_ssh_pub) = setup_ssh_paths(ffx_config::test_env());
        let env = builder.build().expect("Failed to initialize test env");
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();

        let buffers = TestBuffers::default();
        let mut writer =
            VerifiedMachineWriter::<FlashMessage>::new_test(Some(Format::Json), &buffers);

        let res = preprocess_flash_cmd(
            &env.context,
            &mut writer,
            &FlashCommand {
                manifest_path: Some(PathBuf::from(&tmp_file_name)),
                authorized_keys: None,
                oem_stage: vec![OemFile::new(
                    SSH_OEM_COMMAND.to_string(),
                    "some_key_path".to_string(),
                )],
                skip_authorized_keys: true,
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(res, Err(ProductBundleError::OemStageAndSkipUploadSet)));
    }

    // Refreshes the progress indicator and returns whether it contains required
    // substrings and excludes disallowed substrings in the output text.
    fn progress_message_contents(
        indicator: &mut ProgressIndicator,
        required_substrings: &[&str],
        disallowed_substrings: &[&str],
    ) -> bool {
        let mut input = "".as_bytes();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let mut ui = TextUi::new_for_test(&mut input, &mut stdout, &mut stderr, true);
        indicator.present(&mut ui).unwrap();
        let output = format!(
            "{}\n{}",
            String::from_utf8(stdout.clone()).expect("decode stdout"),
            String::from_utf8(stderr.clone()).expect("decode stderr")
        );
        for substring in required_substrings {
            if !output.contains(substring) {
                eprintln!(
                    "Progress message does not contain required substring!\n\
                    Actual output: \"{}\"\n\
                    Required substring: \"{}\"",
                    output, substring,
                );
                return false;
            }
        }
        for substring in disallowed_substrings {
            if output.contains(substring) {
                eprintln!(
                    "Progress message contains disallowed substring!\n\
                    Actual output: \"{}\"\n\
                    Disallowed substring: \"{}\"",
                    output, substring,
                );
                return false;
            }
        }
        true
    }

    #[test]
    fn test_progress_indicator() {
        let mut indicator = ProgressIndicator::new();

        indicator.init("a_product", 2);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "0 of 2 partitions",
                "Processing next partition...",
                "0 of 0 files",
            ],
            &["Upload", "byte"],
        ));

        indicator.start_next_partition("foo_partition", 1);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "1 of 2 partitions",
                "Flash partition: foo_partition",
                "0 of 1 files",
            ],
            &["Upload", "byte"],
        ));

        indicator.start_next_file(64);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "1 of 2 partitions",
                "Flash partition: foo_partition",
                "1 of 1 files",
                "Upload file",
                "0 of 64 bytes",
            ],
            &[],
        ));

        indicator.wrote_bytes(32);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "1 of 2 partitions",
                "Flash partition: foo_partition",
                "1 of 1 files",
                "Upload file",
                "32 of 64 bytes",
            ],
            &[],
        ));

        indicator.wrote_bytes(32);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "1 of 2 partitions",
                "Flash partition: foo_partition",
                "1 of 1 files",
                "Upload file",
                "64 of 64 bytes",
            ],
            &[],
        ));

        indicator.finish_partition();
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "1 of 2 partitions",
                "Processing next partition...",
                "0 of 0 files",
            ],
            &["Upload", "byte"],
        ));

        indicator.start_next_partition("bar_partition", 2);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "2 of 2 partitions",
                "Flash partition: bar_partition",
                "0 of 2 files",
            ],
            &["Upload", "byte"],
        ));

        indicator.start_next_file(128);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "2 of 2 partitions",
                "Flash partition: bar_partition",
                "1 of 2 files",
                "Upload file",
                "0 of 128 bytes",
            ],
            &[],
        ));

        indicator.wrote_bytes(128);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "2 of 2 partitions",
                "Flash partition: bar_partition",
                "1 of 2 files",
                "Upload file",
                "128 of 128 bytes",
            ],
            &[],
        ));

        indicator.start_next_file(16);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "2 of 2 partitions",
                "Flash partition: bar_partition",
                "2 of 2 files",
                "Upload file",
                "0 of 16 bytes",
            ],
            &[],
        ));

        indicator.wrote_bytes(16);
        assert!(progress_message_contents(
            &mut indicator,
            &[
                "Flashing a_product",
                "2 of 2 partitions",
                "Flash partition: bar_partition",
                "2 of 2 files",
                "Upload file",
                "16 of 16 bytes",
            ],
            &[],
        ));

        indicator.finish_partition();
        assert!(progress_message_contents(
            &mut indicator,
            &[],
            &["Flash", "partition", "file", "Upload", "byte"],
        ));
    }

    #[fuchsia::test]
    async fn test_flash_schema() {
        let messages = vec![
            FlashMessage::Preflight { message: "Starting preflight checks".to_string() },
            FlashMessage::Progress(FlashProgress::FlashPartitionStarted {
                partition_name: "boot".to_string(),
            }),
            FlashMessage::Progress(FlashProgress::FlashPartitionFinished {
                partition_name: "boot".to_string(),
            }),
            FlashMessage::Progress(FlashProgress::Unlock),
            FlashMessage::Progress(FlashProgress::RebootToBootloaderStarted),
            FlashMessage::Progress(FlashProgress::RebootToBootloaderFinished),
            FlashMessage::Progress(FlashProgress::GotVariable),
            FlashMessage::Progress(FlashProgress::OemCommand {
                oem_command: "oem-cmd".to_string(),
            }),
            FlashMessage::Progress(FlashProgress::UploadStarted),
            FlashMessage::Progress(FlashProgress::UploadFinished),
            FlashMessage::Progress(FlashProgress::UploadError),
            FlashMessage::Finished { success: true, error_message: "".to_string() },
            FlashMessage::Finished { success: false, error_message: "Flashing failed".to_string() },
        ];

        for msg in messages {
            let json = serde_json::to_value(&msg).expect("serialize FlashMessage");
            <FlashTool as FfxMain>::Writer::verify_schema(&json)
                .unwrap_or_else(|e| panic!("Schema verification failed for {json:?}: {e}"));
        }
    }

    #[fuchsia::test]
    fn test_handle_fidl_connection_err() {
        assert!(
            handle_fidl_connection_err(Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                protocol_name: "fuchsia.hardware.power.statecontrol.Admin",
                reason: None,
            })
            .is_ok()
        );

        assert!(
            handle_fidl_connection_err(Error::ClientRead(fidl::TransportError::Status(
                fidl::Status::PEER_CLOSED
            )))
            .is_ok()
        );

        assert!(handle_fidl_connection_err(Error::InvalidHeader).is_err());
    }
}
