// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use discovery::TargetState;
use discovery::query::TargetInfoQuery;
use errors::ffx_bail;
use fdomain_fuchsia_hardware_power_statecontrol::{
    AdminProxy, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fdomain_fuchsia_hwinfo::DeviceProxy;
use ffx_bootloader_args::SubCommand::{Boot, Info, Lock, Unlock};
use ffx_bootloader_args::{BootCommand, BootloaderCommand, UnlockCommand};
use ffx_config::EnvironmentContext;
use ffx_fastboot::boot::boot;
use ffx_fastboot::common::from_manifest;
use ffx_fastboot::file_resolver::resolvers::EmptyResolver;
use ffx_fastboot::info::info;
use ffx_fastboot::lock::lock;
use ffx_fastboot::unlock::unlock;
use ffx_fastboot::util::{Event, UnlockEvent};
use ffx_fastboot_connection_factory::{RetryLimit, get_fastboot_interface};
use ffx_fastboot_interface::fastboot_interface::{FastbootInterface, UploadProgress, Variable};
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool, deferred, return_bug, return_user_error};
use fidl::Error;
use futures::{TryFutureExt, try_join};
use schemars::JsonSchema;
use serde::Serialize;
use std::io::{Write, stdin};

use target_holders::moniker;
use termion::{color, style};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;

const MISSING_ZBI: &str = "Error: vbmeta parameter must be used with zbi parameter";

const WARNING: &str = "WARNING: ALL SETTINGS USER CONTENT WILL BE ERASED!\n\
                        Do you want to continue? [yN]";

#[derive(FfxTool)]
pub struct BootloaderTool {
    #[command]
    cmd: BootloaderCommand,
    ctx: EnvironmentContext,
    #[with(deferred(moniker("/bootstrap/shutdown_shim")))]
    power_proxy: fho::Deferred<AdminProxy>,
    #[with(deferred(moniker("/core/hwinfo")))]
    device_proxy: fho::Deferred<DeviceProxy>,
}

fho::embedded_plugin!(BootloaderTool);

#[derive(Default, Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BootloaderToolMessageType {
    #[default]
    Unknown,
    Info,
    Rebooting,
    Error,
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct VariableMessage {
    key: String,
    value: String,
}

#[derive(Default, Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct InfoMessage {
    variables: Vec<VariableMessage>,
}

#[derive(Default, Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct BootloaderToolMessage {
    message_type: BootloaderToolMessageType,
    info_message: InfoMessage,
}

#[async_trait(?Send)]
impl FfxMain for BootloaderTool {
    type Writer = VerifiedMachineWriter<BootloaderToolMessage>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let handle = ffx_target::discover_single_default_target(&self.ctx)
            .await
            .map_err(anyhow::Error::from)?;
        let handle = match &handle.state {
            TargetState::Fastboot { .. } => {
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
                // Wait to allow the Target to fully cycle to the bootloader
                writeln!(writer, "Rebooting Target to bootloader")
                    .user_message("Error writing user message")?;
                writer.flush().user_message("Error flushing writer buffer")?;

                // Should probably get the serial number of the target just in case
                let device = self.device_proxy.await.bug_context("Initializing device proxy")?;
                let info = device.get_info().await.bug_context("Getting target device info")?;

                // Tell the target to reboot to the bootloader
                log::debug!("Target in Product state. Rebooting to bootloader...",);

                let p_proxy = self.power_proxy.await?;

                // These calls erroring is fine...
                match p_proxy
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

                let query =
                    info.serial_number.map_or(TargetInfoQuery::First, |sn| TargetInfoQuery::Id(sn));
                ffx_target::discover_fastboot_target(&self.ctx, query, Some(100000)).await?
            }
            TargetState::Unknown => {
                ffx_bail!("Target is in an Unknown state.");
            }
            TargetState::Zedboot => {
                ffx_bail!("Bootloader operations not supported with Zedboot");
            }
        };

        match handle.state {
            TargetState::Fastboot(fastboot_state) => {
                let proxy = get_fastboot_interface(
                    &fastboot_state,
                    handle.node_name,
                    &self.ctx,
                    RetryLimit::Default,
                )
                .await
                .map_err(|e| anyhow::Error::from(e))?;
                bootloader_impl(&self.ctx, proxy, self.cmd, &mut writer).await
            }
            _ => {
                ffx_bail!("This is unsupported")
            }
        }
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

async fn handle_upload(
    writer: &mut VerifiedMachineWriter<BootloaderToolMessage>,
    mut prog_server: Receiver<UploadProgress>,
) -> anyhow::Result<()> {
    let mut start_time: Option<DateTime<Utc>> = None;
    loop {
        match prog_server.recv().await {
            Some(UploadProgress::OnReady { partition, files }) => {
                log::info!("Uploading partition {} ({} files)", partition, files);
            }
            Some(UploadProgress::OnStarted { size, .. }) => {
                start_time.replace(Utc::now());
                log::debug!("Upload started: {}", size);
                write!(writer, "Uploading... ")?;
                if size > (1 << 24) {
                    write!(writer, "Large file")?;
                }
                writer.flush()?;
            }
            Some(UploadProgress::OnFinished { .. }) => {
                if let Some(st) = start_time {
                    let d = Utc::now().signed_duration_since(st);
                    log::debug!("Upload duration: {:.2}s", (d.num_milliseconds() / 1000));
                } else {
                    writeln!(writer, "{}Done{}", color::Fg(color::Green), style::Reset)?;
                    writer.flush()?;
                }
                log::debug!("Upload finished");
            }
            Some(UploadProgress::OnError { error, .. }) => {
                log::error!("{}", error);
                ffx_bail!("{}", error)
            }
            Some(UploadProgress::OnProgress { bytes_written, .. }) => {
                log::trace!("Upload progress: {}", bytes_written);
            }
            None => return Ok(()),
        }
    }
}

fn done_time(duration: Duration) -> String {
    format!(
        "{}Done{} [{}{:.2}s{}]",
        color::Fg(color::Green),
        style::Reset,
        color::Fg(color::Blue),
        (duration.num_milliseconds() as f32) / (1000 as f32),
        style::Reset
    )
}
async fn handle_events(
    writer: &mut VerifiedMachineWriter<BootloaderToolMessage>,
    mut var_server: Receiver<Event>,
) -> anyhow::Result<()> {
    let mut start_time: Option<DateTime<Utc>> = None;
    loop {
        match var_server.recv().await {
            Some(Event::Locked) => {
                writeln!(writer, "Locked")?;
            }
            Some(Event::Unlock(unlock_event)) => {
                let message = match unlock_event {
                    UnlockEvent::SearchingForCredentials => {
                        "Looking for unlock credentials... ".to_string()
                    }
                    UnlockEvent::FoundCredentials(delta) => format!("{}\n", done_time(delta)),
                    UnlockEvent::GeneratingToken => "Generating unlock token... ".to_string(),
                    UnlockEvent::FinishedGeneratingToken(delta) => {
                        format!("{}\n", done_time(delta))
                    }
                    UnlockEvent::BeginningUploadOfToken => {
                        "Preparing to upload unlock token\n".to_string()
                    }
                    UnlockEvent::Done => "Done\n".to_string(),
                };
                write!(writer, "{}", message)?;
            }
            Some(Event::RebootStarted) => {
                writeln!(writer, "Reboot started")?;
            }
            Some(Event::Rebooted(_)) => {
                writeln!(writer, "Rebooted")?;
            }
            Some(Event::Oem { oem_command }) => {
                writeln!(writer, "Sending oem command: {}", oem_command)?;
            }
            Some(Event::Upload(upload)) => match upload {
                UploadProgress::OnReady { partition, files } => {
                    log::info!("Uploading partition {} ({} files)", partition, files);
                }
                UploadProgress::OnStarted { size, .. } => {
                    start_time.replace(Utc::now());
                    log::debug!("Upload started: {}", size);
                    write!(writer, "Uploading... ")?;
                    if size > (1 << 24) {
                        write!(writer, "Large file")?;
                    }
                    writer.flush()?;
                }
                UploadProgress::OnFinished { .. } => {
                    if let Some(st) = start_time {
                        let d = Utc::now().signed_duration_since(st);
                        log::debug!("Upload duration: {:.2}s", (d.num_milliseconds() / 1000));
                    } else {
                        writeln!(writer, "{}Done{}", color::Fg(color::Green), style::Reset)?;
                        writer.flush()?;
                    }
                    log::debug!("Upload finished");
                }
                UploadProgress::OnError { error, .. } => {
                    log::error!("{}", error);
                    ffx_bail!("{}", error)
                }
                UploadProgress::OnProgress { bytes_written, .. } => {
                    log::trace!("Upload progress: {}", bytes_written);
                }
            },
            Some(Event::FlashProduct { .. })
            | Some(Event::FlashPartition { .. })
            | Some(Event::FlashPartitionFinished { .. }) => {
                ffx_bail!("Should not get flash partition events in this bootloader command.");
            }
            Some(Event::Variable(_)) => {
                ffx_bail!("Should not get variable event in this bootloader command.");
            }
            None => break,
        }
    }
    return Ok(());
}

async fn handle_variables_for_fastboot(
    writer: &mut VerifiedMachineWriter<BootloaderToolMessage>,
    mut var_server: Receiver<Variable>,
) -> anyhow::Result<()> {
    let mut variables = vec![];
    loop {
        match var_server.recv().await {
            Some(Variable { name, value, .. }) => {
                variables.push(VariableMessage { key: name, value });
            }
            None => break,
        }
    }
    let message = variables
        .iter()
        .map(|x| format!("{}: {}", x.key, x.value))
        .collect::<Vec<String>>()
        .join("\n");
    writer
        .machine_or(
            &BootloaderToolMessage {
                message_type: BootloaderToolMessageType::Info,
                info_message: InfoMessage { variables },
            },
            message,
        )
        .map_err(|e| anyhow!(e))
}

pub async fn bootloader_impl(
    ctx: &EnvironmentContext,
    mut fastboot_proxy: impl FastbootInterface,
    mut cmd: BootloaderCommand,
    writer: &mut VerifiedMachineWriter<BootloaderToolMessage>,
) -> fho::Result<()> {
    if cmd.product_bundle.is_none() && cmd.manifest.is_none() {
        let product_path = ctx.get("product.path").ok();
        if let Some(product_path) = product_path {
            writeln!(
                writer,
                "No product bundle or manifest passed. Inferring product bundle path from config: {}",
                product_path
            )
            .user_message("Error writing user message")?;
            cmd.product_bundle = Some(product_path);
        }
    }
    // SubCommands can overwrite the manifest with their own parameters, so check for those
    // conditions before continuing through to check the flash manifest.
    match &cmd.subcommand {
        Info(_) => {
            let (client, server) = mpsc::channel(1);
            try_join!(
                info(client, &mut fastboot_proxy).map_err(anyhow::Error::from),
                handle_variables_for_fastboot(writer, server)
            )
            .map_err(fho::Error::from)?;
            return Ok(());
        }
        Lock(_) => {
            lock(&mut fastboot_proxy)
                .await
                .map_err(anyhow::Error::from)
                .map_err(fho::Error::from)?;
            writeln!(writer, "Target is now locked.").bug_context("failed to write")?;
            return Ok(());
        }
        Unlock(UnlockCommand { cred, force }) => {
            if !force {
                writeln!(writer, "{}", WARNING).bug_context("failed to write")?;
                let answer = blocking::unblock(|| {
                    use std::io::BufRead;
                    let mut line = String::new();
                    let stdin = stdin();
                    let mut locked = stdin.lock();
                    let _ = locked.read_line(&mut line);
                    line
                })
                .await;
                if answer.trim() != "y" {
                    ffx_bail!("User aborted");
                }
            }
            match cred {
                Some(cred_file) => {
                    let (client, server) = mpsc::channel(1);
                    let credentials = vec![cred_file.to_string()];
                    let mut resolver = EmptyResolver::new().map_err(anyhow::Error::from)?;
                    try_join!(
                        unlock(client, &mut resolver, &credentials, &mut fastboot_proxy,)
                            .map_err(anyhow::Error::from),
                        handle_events(writer, server)
                    )
                    .map_err(fho::Error::from)?;
                    return Ok(());
                }
                _ => {}
            }
        }
        Boot(BootCommand { zbi, vbmeta, .. }) => {
            if vbmeta.is_some() && zbi.is_none() {
                ffx_bail!("{}", MISSING_ZBI)
            }
            match zbi {
                Some(z) => {
                    let (client, server) = mpsc::channel(1);
                    let mut resolver = EmptyResolver::new().map_err(anyhow::Error::from)?;
                    try_join!(
                        boot(
                            client,
                            &mut resolver,
                            z.to_owned(),
                            vbmeta.to_owned(),
                            &mut fastboot_proxy,
                        )
                        .map_err(anyhow::Error::from),
                        handle_upload(writer, server)
                    )
                    .map_err(fho::Error::from)?;
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    let (client, server) = mpsc::channel(1);
    try_join!(
        from_manifest(ctx, client, cmd, &mut fastboot_proxy).map_err(anyhow::Error::from),
        handle_events(writer, server)
    )
    .map_err(fho::Error::from)?;
    return Ok(());
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use ffx_bootloader_args::LockCommand;
    use ffx_config::environment::test_init;
    use ffx_fastboot::common::vars::LOCKED_VAR;
    use ffx_fastboot_interface::test::setup;
    use ffx_writer::Format;
    use tempfile::NamedTempFile;

    #[fuchsia::test]
    async fn test_boot_stages_file_and_calls_boot() -> fho::Result<()> {
        let test_env = test_init()?;
        let zbi_file = NamedTempFile::new().expect("tmp access failed");
        let zbi_file_name = zbi_file.path().to_string_lossy().to_string();
        let vbmeta_file = NamedTempFile::new().expect("tmp access failed");
        let vbmeta_file_name = vbmeta_file.path().to_string_lossy().to_string();
        let (state, proxy) = setup();
        let mut w = VerifiedMachineWriter::<BootloaderToolMessage>::new(Some(Format::Json));
        bootloader_impl(
            &test_env.context,
            proxy,
            BootloaderCommand {
                manifest: None,
                product: "Fuchsia".to_string(),
                product_bundle: None,
                skip_verify: false,
                subcommand: Boot(BootCommand {
                    zbi: Some(zbi_file_name),
                    vbmeta: Some(vbmeta_file_name),
                    slot: "a".to_string(),
                }),
            },
            &mut w,
        )
        .await?;
        let state = state.lock().unwrap();
        assert_eq!(1, state.staged_files.len());
        assert_eq!(1, state.boots);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_boot_stages_file_and_calls_boot_with_just_zbi() -> fho::Result<()> {
        let test_env = test_init()?;
        let zbi_file = NamedTempFile::new().expect("tmp access failed");
        let zbi_file_name = zbi_file.path().to_string_lossy().to_string();
        let (state, proxy) = setup();
        let mut w = VerifiedMachineWriter::<BootloaderToolMessage>::new(Some(Format::Json));
        bootloader_impl(
            &test_env.context,
            proxy,
            BootloaderCommand {
                manifest: None,
                product: "Fuchsia".to_string(),
                product_bundle: None,
                skip_verify: false,
                subcommand: Boot(BootCommand {
                    zbi: Some(zbi_file_name),
                    vbmeta: None,
                    slot: "a".to_string(),
                }),
            },
            &mut w,
        )
        .await?;
        let state = state.lock().unwrap();
        assert_eq!(1, state.staged_files.len());
        assert_eq!(1, state.boots);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_boot_fails_with_just_vbmeta() {
        let test_env = test_init().expect("creating test env");
        let vbmeta_file = NamedTempFile::new().expect("tmp access failed");
        let vbmeta_file_name = vbmeta_file.path().to_string_lossy().to_string();
        let (_, proxy) = setup();
        let mut w = VerifiedMachineWriter::<BootloaderToolMessage>::new(Some(Format::Json));
        assert!(
            bootloader_impl(
                &test_env.context,
                proxy,
                BootloaderCommand {
                    manifest: None,
                    product: "Fuchsia".to_string(),
                    product_bundle: None,
                    skip_verify: false,
                    subcommand: Boot(BootCommand {
                        zbi: None,
                        vbmeta: Some(vbmeta_file_name),
                        slot: "a".to_string(),
                    }),
                },
                &mut w,
            )
            .await
            .is_err()
        );
    }

    #[fuchsia::test]
    async fn test_lock_calls_oem_command() -> fho::Result<()> {
        let test_env = test_init()?;
        let (state, proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            // is_locked
            state.set_var(LOCKED_VAR.to_string(), "no".to_string());
            state.set_var("vx-unlockable".to_string(), "no".to_string());
        }
        let mut w = VerifiedMachineWriter::<BootloaderToolMessage>::new(Some(Format::Json));
        bootloader_impl(
            &test_env.context,
            proxy,
            BootloaderCommand {
                manifest: None,
                product: "Fuchsia".to_string(),
                product_bundle: None,
                skip_verify: false,
                subcommand: Lock(LockCommand {}),
            },
            &mut w,
        )
        .await?;
        let state = state.lock().unwrap();
        assert_eq!(1, state.oem_commands.len());
        assert_eq!("oem vx-lock".to_string(), state.oem_commands[0]);
        Ok(())
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
