// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::pbm::{get_virtual_devices, make_configs};
use anyhow::{Context, anyhow};
use async_trait::async_trait;
use emulator_instance::{EmulatorConfiguration, EmulatorInstances, EngineType, NetworkingMode};
use ffx_config::EnvironmentContext;
use ffx_emulator_common::config::EMU_SERIAL_ENABLED;
use ffx_emulator_config::EmulatorEngine;
use ffx_emulator_engines::{EngineBuilder, process_flag_template, update_serial_number};
use ffx_emulator_start_args::StartCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{
    Error, FfxContext, FfxMain, FfxTool, Result, TryFromEnv, bug, return_bug, return_user_error,
};
use gcs::client::{Client, ProgressResponse};
use pbm::generate_mac_address;
use pbms::{AuthFlowChoice, LoadedProductBundle, handle_new_access_token};
use schemars::JsonSchema;
use sdk_metadata::CpuArchitecture;
use serde::Serialize;
use std::io::{stderr, stdin, stdout};
use std::path::PathBuf;
use std::str::FromStr;
use structured_ui::{Interface, TextUi};
use tempfile::TempDir;
use url::Url;

mod editor;
mod pbm;

pub(crate) const DEFAULT_NAME: &str = "fuchsia-emulator";

async fn download_from_gs(gs_url: &str) -> Result<PathBuf> {
    let dir = TempDir::new().context("create temp dir")?.keep();
    let file_name = gs_url.split('/').next_back().ok_or_else(|| anyhow!("Invalid GS URL"))?;
    let local_path = dir.join(file_name);

    let client = Client::initial()?;
    let url = Url::parse(gs_url).context("parsing gs url")?;
    let bucket = url.host_str().context("getting bucket from gs url")?;
    let object = &url.path()[1..];

    log::debug!("Downloading {}...", gs_url);
    let access_token =
        handle_new_access_token(&AuthFlowChoice::Pkce, &structured_ui::MockUi::new())
            .await
            .context("Getting new access token.")?;
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
        .await?;

    ui.clear_progress().map_err(|e| anyhow::Error::new(e))?;

    log::debug!("Downloaded to {}", local_path.display());

    if local_path.extension().unwrap_or_default() == "zip" {
        let file = std::fs::File::open(&local_path).context("opening zip file")?;
        let mut archive = zip::ZipArchive::new(file).context("reading zip archive")?;
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).context("getting file from archive")?;
            let outpath = file.mangled_name();
            let outpath = dir.join(outpath);

            if file.is_dir() {
                std::fs::create_dir_all(&outpath).context("creating directory")?;
                continue;
            }
            if let Some(p) = outpath.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(&p).context("creating parent directory")?;
                }
            }
            let mut outfile = std::fs::File::create(&outpath).context("creating output file")?;
            std::io::copy(&mut file, &mut outfile).context("copying file content")?;
        }

        let pb_path = dir.join("product_bundle");
        if pb_path.exists() {
            return Ok(pb_path);
        }

        return Ok(dir);
    }

    Ok(local_path)
}

/// EngineOperations trait is used to allow mocking of
/// these methods.
#[cfg_attr(test, mockall::automock)]
#[async_trait(?Send)]
pub trait EngineOperations: TryFromEnv + 'static {
    async fn get_engine_by_name(
        &self,
        name: &mut Option<String>,
    ) -> Result<Option<Box<dyn EmulatorEngine>>>;

    fn edit_configuration(&self, emu_config: &mut EmulatorConfiguration) -> Result<()>;

    async fn new_engine(
        &self,
        emulator_configuration: &EmulatorConfiguration,
        engine_type: EngineType,
        serial_enabled: bool,
    ) -> Result<Box<dyn EmulatorEngine>>;

    async fn load_product_bundle(
        &self,
        product_bundle: &Option<String>,
    ) -> Result<LoadedProductBundle>;

    async fn clean_up_instance_dir(&self, instance_name: &str) -> Result<()>;

    fn context(&self) -> EnvironmentContext;

    fn get_emu_instances(&self) -> EmulatorInstances;
}

pub struct EngineOperationsData {
    context: EnvironmentContext,
    emu_instances: EmulatorInstances,
}

#[async_trait(?Send)]
impl TryFromEnv for EngineOperationsData {
    type Error = fho::Error;
    async fn try_from_env(env: &fho::FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let context = env.environment_context();
        let instance_dir: PathBuf =
            context.get(ffx_config::keys::EMU_INSTANCE_ROOT_DIR).map_err(|e| bug!("{e}"))?;
        Ok(Self { context: context.clone(), emu_instances: EmulatorInstances::new(instance_dir) })
    }
}

#[async_trait(?Send)]
impl EngineOperations for EngineOperationsData {
    async fn get_engine_by_name(
        &self,
        name: &mut Option<String>,
    ) -> Result<Option<Box<dyn EmulatorEngine>>> {
        let builder = EngineBuilder::new(&self.context, self.emu_instances.clone());
        builder.get_engine_by_name(name).map_err(|e| e.into())
    }
    fn edit_configuration(&self, emu_config: &mut EmulatorConfiguration) -> Result<()> {
        crate::editor::edit_configuration(emu_config).map_err(|e| e.into())
    }

    async fn new_engine(
        &self,
        emulator_configuration: &EmulatorConfiguration,
        engine_type: EngineType,
        serial_enabled: bool,
    ) -> Result<Box<dyn EmulatorEngine>> {
        EngineBuilder::new(&self.context, self.emu_instances.clone())
            .config(emulator_configuration.clone())
            .engine_type(engine_type)
            .serial_enabled(serial_enabled)
            .build()
            .await
    }

    async fn load_product_bundle(
        &self,
        product_bundle: &Option<String>,
    ) -> Result<LoadedProductBundle> {
        let pb = match product_bundle {
            Some(b) => {
                if b.starts_with("gs://") {
                    let path = download_from_gs(b).await?;
                    path.to_string_lossy().to_string()
                } else {
                    b.clone()
                }
            }
            None => pbms::get_product_bundle_path(&self.context)
                .map_err(|e| anyhow::anyhow!("{:?}", e))?,
        };
        pbms::load_product_bundle(&self.context, &pb)
            .await
            .map_err(|e| fho::user_error!("Error loading product bundle: {e}"))
    }

    async fn clean_up_instance_dir(&self, instance_name: &str) -> Result<()> {
        self.emu_instances
            .clean_up_instance_dir(instance_name)
            .map_err(|e| anyhow::Error::from(e).into())
    }

    fn context(&self) -> EnvironmentContext {
        self.context.clone()
    }
    fn get_emu_instances(&self) -> EmulatorInstances {
        self.emu_instances.clone()
    }
}

/// Sub-sub tool for `emu start`
#[derive(FfxTool)]
#[target(None)]
pub struct EmuStartTool<T: EngineOperations> {
    #[command]
    cmd: StartCommand,
    engine_operations: T,
    context: EnvironmentContext,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandStatus {
    /// Successful execution with informational strings.
    Ok { messages: Vec<String> },
    /// Unexpected error with string.
    UnexpectedError { message: String },
    /// A known kind of error that can be reported usefully to the user
    UserError { message: String },
}
// Since this is a part of a legacy plugin, add
// the legacy entry points. If and when this
// is migrated to a subcommand, this macro can be
// removed.
fho::embedded_plugin!(EmuStartTool<EngineOperationsData>);

#[async_trait(?Send)]
impl<T: EngineOperations> FfxMain for EmuStartTool<T> {
    type Writer = VerifiedMachineWriter<CommandStatus>;

    type Error = ::fho::Error;

    async fn main(mut self, mut writer: Self::Writer) -> fho::Result<()> {
        match self.do_start(&mut writer).await {
            Ok(messages) => writer.machine(&CommandStatus::Ok { messages }).map_err(|e| bug!(e)),
            Err(Error::User(e)) => {
                writer.machine(&CommandStatus::UserError { message: e.to_string() })?;
                Err(Error::User(e))
            }
            Err(e) => {
                writer.machine(&CommandStatus::UnexpectedError { message: e.to_string() })?;
                Err(e)
            }
        }
    }
}

impl<T: EngineOperations> EmuStartTool<T> {
    fn serial_enabled(&self) -> bool {
        !self.cmd.no_serial_number && self.context.get(EMU_SERIAL_ENABLED).unwrap_or(true)
    }

    async fn do_start(
        mut self,
        writer: &mut <EmuStartTool<T> as fho::FfxMain>::Writer,
    ) -> Result<Vec<String>> {
        let loaded_product_bundle = self.finalize_start_command(writer).await?;

        let product_bundle_path = loaded_product_bundle
            .as_ref()
            .map(|b| b.loaded_from_path().to_path_buf().into_std_path_buf());
        let product_bundle: Option<pbms::ProductBundle> = loaded_product_bundle.map(|b| b.into());

        // List the devices available in this product bundle
        if self.cmd.device_list {
            let virtual_devices = get_virtual_devices(&product_bundle.unwrap()).await?;
            let message = if virtual_devices.is_empty() {
                "There are no virtual devices configured for this product bundle".to_string()
            } else {
                format!("Valid virtual device specifications are: {:?}", virtual_devices)
            };
            writer.line(&message)?;
            return Ok(vec![message]);
        }

        let mut emulator_configuration = make_configs(
            &self.engine_operations.context(),
            &self.cmd,
            product_bundle.clone(),
            &self.engine_operations.get_emu_instances(),
        )
        .await?;
        // Plug through the flags needed during template processing and GPT image construction
        emulator_configuration.guest.is_gpt = self.cmd.uefi;
        emulator_configuration.guest.product_bundle_path = product_bundle_path;
        let engine_type = EngineType::from_str(
            &self.cmd.engine(&self.context).unwrap_or_else(|_| "femu".to_string()),
        )
        .context("Reading engine type from ffx config.")?;

        // Get the staged instance, if any
        let mut existing = self.engine_operations.get_engine_by_name(&mut self.cmd.name).await?;

        // Check that it is not running.
        if let Some(ref mut existing_instance) = existing {
            let name = self.cmd.name.as_ref().unwrap();
            if existing_instance.is_running().await {
                return_user_error!(
                    "An existing emulator instance named {name} is already running. \
                    Stop it with `ffx emu stop {name}` or use a different name."
                );
            } else if !self.cmd.reuse && !self.cmd.reuse_with_check {
                if let Some(cleanup_err) =
                    self.engine_operations.clean_up_instance_dir(&name).await.err()
                {
                    return_bug!(
                        "Cleanup of '{name}' failed with the following error: {cleanup_err}"
                    );
                }
                existing = None;
            }
        }

        let mut engine =
            self.get_engine(writer, &emulator_configuration, engine_type, existing).await?;

        if self.cmd.config.is_none() && !self.cmd.reuse {
            // We don't stage files for custom configurations, because the EmulatorConfiguration
            // doesn't hold valid paths to the system images.
            engine.stage().await?;

            let mut messages: Vec<String> = vec![];
            if self.cmd.stage {
                if self.cmd.verbose {
                    let emulator_cmd = engine.build_emulator_cmd();
                    let m = format!("[emulator] Command line after Staging: {:?}", emulator_cmd);
                    writer.line(&m)?;
                    messages.push(m);
                    let m2 = format!("[emulator] With ENV: {:?}", emulator_cmd.get_envs());
                    writer.line(&m2)?;
                    messages.push(m2);
                }
                return Ok(messages);
            }
        }

        if self.cmd.edit {
            if writer.is_machine() {
                return_user_error!("--edit cannot be used with --machine output.")
            }
            self.engine_operations.edit_configuration(engine.emu_config_mut())?;
        }

        let mut messages: Vec<String> = vec![];

        let emulator_cmd = engine.build_emulator_cmd();
        if self.cmd.verbose || self.cmd.dry_run {
            let m = format!("[emulator] Final Command line: {:?}", emulator_cmd);
            writer.line(&m)?;
            messages.push(m);
            let m2 = format!("[emulator] With ENV: {:?}\n", emulator_cmd.get_envs());
            writer.line(&m2)?;
            messages.push(m2);
        }

        // If we're just staging the instance, do not call start.
        if !self.cmd.stage && !self.cmd.dry_run {
            match engine.start(&self.engine_operations.context(), emulator_cmd).await {
                Ok(0) => Ok(messages),
                Ok(c) => return Err(Error::ExitWithCode(c)),
                Err(e) => return Err(e),
            }
        } else {
            Ok(messages)
        }
    }

    async fn get_engine(
        &mut self,
        writer: &mut <EmuStartTool<T> as fho::FfxMain>::Writer,
        emulator_configuration: &EmulatorConfiguration,
        engine_type: EngineType,
        existing_engine: Option<Box<dyn EmulatorEngine>>,
    ) -> Result<Box<dyn EmulatorEngine>> {
        let mut engine: Box<dyn EmulatorEngine>;
        // For reuse with check, we need to compare the existing hashes of the zbi and disk
        // to the product bundle. If they are the same, reuse the existing configuration and
        // data.
        //
        // If they are different, then restage from the product bundle.
        if self.cmd.reuse_with_check {
            if let Some(existing_engine) = existing_engine {
                let reused: bool;
                (reused, engine) =
                    self.check_if_reusable(&emulator_configuration, existing_engine).await?;

                if reused {
                    self.cmd.reuse = true;
                    let message = "[emulator] Reusing existing instance.";
                    log::info!("{message}");
                    writer.line(message)?;
                } else {
                    // They do not match, so don't reuse and reset the engine.
                    self.cmd.reuse = false;
                    let message =
                        "[emulator] Created new instance. Product bundle data has changed.";
                    log::info!("{message}");
                    writer.line(message)?;
                }
            } else {
                log::debug!("No existing instance to check as reusable.");
                engine = self
                    .engine_operations
                    .new_engine(&emulator_configuration, engine_type, self.serial_enabled())
                    .await?;
                let config = engine.emu_config_mut();
                Self::save_disk_hashes(config)?;
            }
        } else {
            if self.cmd.reuse && !emulator_configuration.runtime.config_override {
                if let Some(mut existing_instance) = existing_engine {
                    self.reuse_existing_engine(&mut existing_instance, emulator_configuration)
                        .await?;
                    engine = existing_instance;
                    let message = "[emulator] Reusing existing instance.";
                    log::info!("{message}");
                    writer.line(message)?;
                } else {
                    let message = format!(
                        "[emulator] Instance '{name}' not found with --reuse flag. \
                        Creating a new emulator named '{name}'.",
                        name = emulator_configuration.runtime.name
                    );
                    log::debug!("{message}");
                    writer.line(message)?;
                    self.cmd.reuse = false;
                    engine = self
                        .engine_operations
                        .new_engine(&emulator_configuration, engine_type, self.serial_enabled())
                        .await?
                }
            } else {
                engine = if !emulator_configuration.runtime.config_override
                    && existing_engine.is_some()
                {
                    existing_engine.expect("existing engine instance")
                } else {
                    self.engine_operations
                        .new_engine(&emulator_configuration, engine_type, self.serial_enabled())
                        .await?
                }
            }
        }
        Ok(engine)
    }

    async fn finalize_start_command(
        &mut self,
        writer: &mut <EmuStartTool<T> as fho::FfxMain>::Writer,
    ) -> Result<Option<LoadedProductBundle>> {
        // name is important to not be empty since it is used to
        // create a directory path.
        let mut name = self.cmd.name(&self.context)?;
        if self.cmd.name.is_none() || name == "" {
            if name == "" {
                name = DEFAULT_NAME.into();
            }

            self.cmd.name = Some(name.into());
        }

        // TODO(https://fxbug.dev/382694675): Remove when we can persist `zircon.nodename`.
        // If the emulator starts GPT full disk images and performs it will not respect the
        // runtime.name because we can't guarantee that `zircon.nodename` is persisted across
        // running `fx ota` on such a configuration. Thus, we force the emulator name to be
        // consistent with the nodename that the running instance will choose when booted up
        // with an unset `zircon.nodename`.
        if self.cmd.uefi {
            let mac = generate_mac_address(&self.cmd.name.as_ref().unwrap());
            let m: Vec<_> = mac.split(":").collect();
            let name = format!("fuchsia-{}{}-{}{}-{}{}", m[0], m[1], m[2], m[3], m[4], m[5]);
            log::info!("Setting emulator name to {} for --uefi", name);
            self.cmd.name = Some(name);
        }

        // if a custom config is used, skip the product bundle checks.
        if self.cmd.config.is_none() {
            let loaded_product_bundle =
                self.engine_operations.load_product_bundle(&self.cmd.product_bundle).await?;

            // if we're just printing a device list, return
            if self.cmd.device_list {
                return Ok(Some(loaded_product_bundle));
            }

            let gpu = self.cmd.gpu(&self.context)?;
            if self.cmd.gpu.is_none() && gpu != "" {
                self.cmd.gpu = Some(gpu);
            }

            let net = self.cmd.net(&self.context)?;
            if self.cmd.net.is_none() && net != "" {
                self.cmd.net = Some(net);
            }

            let startup_timeout = self.cmd.startup_timeout(&self.context)?;
            if self.cmd.startup_timeout.is_none() && startup_timeout > 0 {
                self.cmd.startup_timeout = Some(startup_timeout);
            }

            if self.cmd.product_bundle.is_none() {
                self.cmd.product_bundle = Some(loaded_product_bundle.loaded_from_path().to_string())
            }

            if self.cmd.device.is_none() {
                let virtual_devices = get_virtual_devices(&loaded_product_bundle).await?;
                if virtual_devices.is_empty() {
                    return_user_error!(
                        "There are no virtual devices configured for this product bundle"
                    )
                }
                // Virtual device spec name
                if let Some(device_name) = self.cmd.device(&self.context)? {
                    if self.cmd.device.is_none() && device_name != "" {
                        self.cmd.device = Some(device_name);
                    } else {
                        self.cmd.device = Some(
                            loaded_product_bundle
                                .device_refs()
                                .context("Getting device references")?[0]
                                .clone(),
                        );
                    }
                } else {
                    self.cmd.device = Some(
                        loaded_product_bundle.device_refs().context("Getting device references")?
                            [0]
                        .clone(),
                    );
                }
            }

            let target_arch = loaded_product_bundle
                .get_device(&self.cmd.device)
                .context("Getting virtual device specification")?
                .hardware
                .cpu
                .arch;

            match target_arch {
                CpuArchitecture::X64 | CpuArchitecture::Arm64 => (),
                _ if self.cmd.uefi => return_user_error!(
                    "The --uefi flag is currently only supported for x64 and arm64 target cpus."
                ),
                _ => (),
            }

            if self.cmd.engine.is_none() {
                self.cmd.engine =
                    Some(self.pick_compatible_default_engine_type(writer, target_arch)?);
            }

            return Ok(Some(loaded_product_bundle));
        }
        Ok(None)
    }

    /// Checks whether the default engine type is compatible with possible uefi
    /// and cross-cpu virtualization use-cases.
    ///
    /// Attempts to fix cross-cpu and uefi engine incompatibility by overriding
    /// to the "qemu" engine type for those cases.
    fn pick_compatible_default_engine_type(
        &mut self,
        writer: &mut <EmuStartTool<T> as fho::FfxMain>::Writer,
        target_arch: CpuArchitecture,
    ) -> Result<String> {
        let config_default_engine = self.cmd.engine(&self.context)?;
        let is_uefi = self.cmd.uefi;

        // The android emulator (femu) doesn't support cross-cpu virtualization,
        // so check for cases where the host and target cpus don't match.
        let is_cross_cpu = CpuArchitecture::current() != target_arch;

        if let Some(unsupported_feature) =
            match (config_default_engine == "qemu", is_cross_cpu, is_uefi) {
                (false, false, true) => Some("uefi"),
                (false, true, false) => Some("cross-cpu virtualization"),
                (false, true, true) => Some("uefi and cross-cpu virtualization"),
                _ => None,
            }
        {
            let msg = format!(
                "[emulator] defaulting to qemu engine to support {unsupported_feature}.\n\
                Use `--engine` to explicitly set the engine type if needed."
            );
            log::warn!("{}", msg);
            writer.line(msg)?;
            return Ok("qemu".into());
        }

        Ok(config_default_engine)
    }

    /// Checks the configuration of the given engine against the
    /// product bundle based on the command line. If they match,
    /// the given engine is reusable, and returned, otherwise a
    /// new engine instance is returned.
    async fn check_if_reusable(
        &self,
        new_config: &EmulatorConfiguration,
        mut engine: Box<dyn EmulatorEngine>,
    ) -> Result<(bool, Box<dyn EmulatorEngine>)> {
        log::debug!(
            "New config image files kernel: {kernel:?} ramdisk:{ramdisk:?} disk:{fvm:?}",
            ramdisk = new_config.guest.ramdisk.as_ref().map(|ramdisk| ramdisk.path.clone()),
            fvm = new_config.guest.disk_image,
            kernel = new_config.guest.kernel_image
        );
        // Note: For images that use EFI, the zbi_hash is actually the hash of the efi image, since
        // there is no zbi image.
        let (new_zbi_hash, new_disk_hash) =
            new_config.guest.get_image_hashes().bug_context("could not calculate image hashes")?;

        let new_zbi = format!("{new_zbi_hash:x}");
        let new_disk = format!("{new_disk_hash:x}");
        let config = engine.emu_config();
        let zbi_hash = &config.guest.zbi_hash;
        let disk_hash = &config.guest.disk_hash;

        // If the hashes match, reuse the instance. Reset the config properties that are
        // dynamic and should be set from the command line.
        if &new_zbi == zbi_hash && &new_disk == disk_hash {
            self.reuse_existing_engine(&mut engine, new_config).await?;
            return Ok((true, engine));
        } else {
            let engine_type = EngineType::from_str(
                &self.cmd.engine(&self.context).unwrap_or_else(|_| "femu".to_string()),
            )
            .context("Reading engine type from ffx config.")?;
            engine = self
                .engine_operations
                .new_engine(&new_config, engine_type, self.serial_enabled())
                .await?;
            let config = engine.emu_config_mut();
            config.guest.zbi_hash = new_zbi.clone();
            config.guest.disk_hash = new_disk.clone();
            return Ok((false, engine));
        }
    }

    fn save_disk_hashes(config: &mut EmulatorConfiguration) -> Result<()> {
        config.guest.save_disk_hashes().bug_context("could not save disk hashes")?;
        Ok(())
    }

    /// Syncs configuration updates and validates the environment when reusing an existing engine.
    async fn reuse_existing_engine(
        &self,
        engine: &mut Box<dyn EmulatorEngine>,
        new_config: &EmulatorConfiguration,
    ) -> Result<()> {
        // Dynamic options set from the command line (like host port mappings or logging configurations)
        // must be updated in the reused engine instance to match the current invocation parameters.
        if engine.emu_config().host.networking == NetworkingMode::User {
            engine.emu_config_mut().host.port_map = new_config.host.port_map.clone();
        }
        let serial_enabled = self.serial_enabled();
        {
            let config = engine.emu_config_mut();
            config.host.log = new_config.host.log.clone();
            config.runtime.startup_timeout = new_config.runtime.startup_timeout.clone();
            config.runtime.log_level = new_config.runtime.log_level.clone();

            update_serial_number(config, serial_enabled);
        }

        // Before starting a reused instance, we must validate its configuration and verify that
        // any external host-level dependencies (such as bridged network tap interfaces) are still
        // configured. If the host machine rebooted or the interface was deleted, this will return
        // a user-friendly error instead of failing later with generic process execution errors.
        engine.configure()?;

        // Resolve and load the path to the correct emulator binary. Calling this here ensures
        // that if the host SDK environment or toolpaths changed since the instance was first staged,
        // we resolve to the correct path.
        engine.load_emulator_binary()?;

        {
            let config = engine.emu_config_mut();
            // Process the flag template to regenerate the command line parameters for the emulator
            // binary (QEMU/Femu), incorporating the updated port mappings and configuration parameters.
            config.flags = process_flag_template(config)
                .context("Failed to process the flags template file.")?;
        }

        // Save the updated configuration to disk to ensure subsequent starts or status queries
        // read the correct state.
        engine.save_to_disk().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembled_system::Image;
    use assembly_partitions_config::PartitionsConfig;
    use camino::{Utf8Path, Utf8PathBuf};
    use emulator_instance::{LogLevel, RuntimeConfig, SerialMode};
    use ffx_config::environment::TestEnvBuilder;
    use ffx_writer::TestBuffers;
    use pbms::ProductBundle;
    use product_bundle::ProductBundleV2;
    use sdk_metadata::VirtualDevice;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    const VIRTUAL_DEVICE_VALID: &str = include_str!("../test_data/virtual_device.json");
    const VIRTUAL_DEVICE_TEMPLATE_VALID: &str = include_str!("../test_data/device_1.json.template");

    /// TestEngine is a test struct for implementing the EmulatorEngine trait. This version
    /// just captures when the stage and start functions are called, and asserts that they were
    /// supposed to be. On tear-down, if they were supposed to be and weren't, it will detect this
    /// in the Drop implementation and fail the test accordingly.
    pub struct TestEngine {
        do_stage: bool,
        do_start: bool,
        do_configure: bool,
        did_stage: bool,
        did_start: bool,
        did_configure: bool,
        stage_test_fn: fn(&mut EmulatorConfiguration) -> Result<()>,
        start_test_fn: fn(Command) -> Result<()>,
        config: EmulatorConfiguration,
        running: bool,
    }

    impl Default for TestEngine {
        fn default() -> Self {
            Self {
                stage_test_fn: |_| Ok(()),
                start_test_fn: |_| Ok(()),
                do_stage: false,
                do_start: false,
                do_configure: false,
                did_stage: false,
                did_start: false,
                did_configure: false,
                config: EmulatorConfiguration::default(),
                running: false,
            }
        }
    }

    #[async_trait(?Send)]
    impl EmulatorEngine for TestEngine {
        async fn save_to_disk(&self) -> fho::Result<()> {
            Ok(())
        }
        fn build_emulator_cmd(&self) -> Command {
            Command::new(self.config.runtime.name.clone())
        }
        async fn stage(&mut self) -> fho::Result<()> {
            self.did_stage = true;
            (self.stage_test_fn)(&mut self.config)?;
            if !self.do_stage {
                fho::return_bug!("Test called stage() when it wasn't supposed to.")
            }
            Ok(())
        }
        async fn start(
            &mut self,
            _context: &EnvironmentContext,
            emulator_cmd: Command,
        ) -> fho::Result<i32> {
            self.did_start = true;
            (self.start_test_fn)(emulator_cmd)?;
            if !self.do_start {
                fho::return_bug!("Test called start() when it wasn't supposed to.")
            }
            Ok(0)
        }
        fn configure(&mut self) -> fho::Result<()> {
            self.did_configure = true;
            if !self.do_configure {
                fho::return_bug!("Test called configure() when it wasn't supposed to.")
            }
            Ok(())
        }
        fn load_emulator_binary(&mut self) -> fho::Result<()> {
            Ok(())
        }

        fn emu_config_mut(&mut self) -> &mut EmulatorConfiguration {
            &mut self.config
        }

        fn emu_config(&self) -> &EmulatorConfiguration {
            &self.config
        }

        async fn is_running(&mut self) -> bool {
            self.running
        }
    }

    impl Drop for TestEngine {
        fn drop(&mut self) {
            if self.do_stage {
                assert!(
                    self.did_stage,
                    "The stage() function was supposed to be called but never was."
                );
            }
            if self.do_start {
                assert!(
                    self.did_start,
                    "The start() function was supposed to be called but never was."
                );
            }
            if self.do_configure {
                assert!(
                    self.did_configure,
                    "The configure() function was supposed to be called but never was."
                );
            }
        }
    }

    #[async_trait(?Send)]
    impl TryFromEnv for MockEngineOperations {
        type Error = fho::Error;
        async fn try_from_env(
            _env: &fho::FhoEnvironment,
        ) -> std::result::Result<Self, Self::Error> {
            Ok(Self::default())
        }
    }

    fn set_virtual_device_arch(dir: &Path, target_arch: CpuArchitecture) {
        let manifest_path_binding = dir.join("virtual_device_1.json");
        let manifest_path = Utf8Path::from_path(manifest_path_binding.as_path())
            .expect("construct virtual device manifest path");
        let mut manifest =
            VirtualDevice::try_load_from(manifest_path).expect("parse virtual device");
        match manifest {
            VirtualDevice::V1(ref mut device) => {
                device.hardware.cpu.arch = target_arch;
            }
        }
        manifest.write(manifest_path).expect("write virtual device manifest");
    }

    fn make_test_product_bundle(dir: &Path) -> Result<ProductBundleV2> {
        let dev_manifest = dir.join("virtual_device_manifest.json");
        fs::write(&dev_manifest,r#"
        {"recommended":"virtual_device_1","device_paths":{"virtual_device_1":"virtual_device_1.json","virtual_device_2":"virtual_device_2.json"}}
        "#).unwrap();

        let kernel_path = dir.join("kernel.dat");
        fs::write(&kernel_path, "this is kernel file").bug_context("writing test kernel")?;

        let zbi_path = dir.join("zbi-file.zbi");
        fs::write(&zbi_path, "this is zbi file").bug_context("writing test zbi")?;

        let fvm_path = dir.join("fvm-file.fvm");
        fs::write(&fvm_path, "this is fvm file").bug_context("writing test fvm")?;

        fs::write(dir.join("virtual_device_1.json"), VIRTUAL_DEVICE_VALID)
            .expect("writing device json");

        fs::write(dir.join("device_1.json.template"), VIRTUAL_DEVICE_TEMPLATE_VALID)
            .expect("writing template json");

        Ok(ProductBundleV2 {
            product_name: "".to_string(),
            product_version: "".to_string(),
            partitions: PartitionsConfig {
                bootstrap_partitions: vec![],
                bootloader_partitions: vec![],
                partitions: vec![],
                hardware_revision: "board".into(),
                unlock_credentials: vec![],
                product_matches: vec![],
                ssh_key_upload_method: None,
            },
            sdk_version: "".to_string(),
            system_a: Some(vec![
                Image::QemuKernel(
                    Utf8PathBuf::from_path_buf(kernel_path).expect("utf path from buf"),
                ),
                Image::ZBI {
                    path: Utf8PathBuf::from_path_buf(zbi_path).expect("utf path from buf"),
                    signed: false,
                },
                Image::FVM(Utf8PathBuf::from_path_buf(fvm_path).expect("utf path from buf")),
            ]),
            system_b: None,
            system_r: None,
            platform_tools_a: vec![],
            platform_tools_b: vec![],
            platform_tools_r: vec![],
            repositories: vec![],
            update_package_hash: None,
            virtual_devices_path: Some(dev_manifest.to_str().unwrap().into()),
            release_info: None,
        })
    }

    async fn make_test_emu_start_tool(
        context: &EnvironmentContext,
        cmd: StartCommand,
    ) -> EmuStartTool<MockEngineOperations> {
        EmuStartTool {
            context: context.clone(),
            cmd,
            engine_operations: MockEngineOperations::new(),
        }
    }

    async fn make_fake_sdk(mut builder: TestEnvBuilder) -> TestEnvBuilder {
        let isolate_root = builder.isolate_root();
        let manifest_path = isolate_root.join("meta/manifest.json");
        fs::create_dir_all(manifest_path.parent().unwrap()).expect("temp sdk dir");
        fs::write(
            &manifest_path,
            r#"{ "arch": {  "host": "x86_64-linux-gnu",  "target": ["x64" ] },
            "id": "9999",
            "parts": [
                {
      "meta": "qemu_uefi_internal_x64-meta.json",
      "type": "companion_host_tool"
    }],  "root": "..",
  "schema_version": "1"}"#,
        )
        .expect("sdk manifest");

        let ovmf_code = isolate_root.join("ovmf_code.fd");
        fs::write(&ovmf_code, "ovmf").expect("fake ovmf");

        builder
            .user_config("sdk.root", isolate_root.to_string_lossy())
            .user_config("sdk.overrides.uefi_internal_x64", ovmf_code.to_string_lossy())
    }

    // Check that a running instance is an error
    #[fuchsia::test]
    async fn test_start_with_running_instance() {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand::default();
        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(|_| {
                Ok(Some(Box::new(TestEngine {
                    do_stage: false,
                    do_start: false,
                    running: true,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);

        let pb =
            ProductBundle::V2(make_test_product_bundle(env.isolate_root.path()).expect("test pb"));
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(1);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        let result = tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await;
        assert!(result.is_err())
    }

    // Check that an existing instance that is not running is cleaned up.
    #[fuchsia::test]
    async fn test_start_with_instance_dir() {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand::default();
        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations.expect_get_emu_instances().times(0);
        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(|_| {
                Ok(Some(Box::new(TestEngine {
                    do_stage: false,
                    do_start: false,
                    running: false,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);

        tool.engine_operations.expect_new_engine().returning(|_, _, _| {
            Ok(Box::new(TestEngine {
                do_stage: true,
                do_start: true,
                config: EmulatorConfiguration::default(),
                ..Default::default()
            }) as Box<dyn EmulatorEngine>)
        });

        tool.engine_operations.expect_clean_up_instance_dir().returning(|_| Ok(())).times(1);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        let pb =
            ProductBundle::V2(make_test_product_bundle(env.isolate_root.path()).expect("test pb"));
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);

        let result = tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await;
        assert!(result.is_ok())
    }

    // Check that new_engine gets called by default and get_engine_by_name doesn't
    #[fuchsia::test]
    async fn test_get_engine_no_reuse_makes_new() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand::default();
        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: true,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);
        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main in test_get_engine_no_reuse_makes_new");
        Ok(())
    }

    // Check that reuse and config together is still new_engine (i.e. config overrides reuse)
    #[fuchsia::test]
    async fn test_get_engine_with_config_doesnt_reuse() -> Result<()> {
        let env = ffx_config::test_init().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand {
            reuse: true,
            net: Some("user".into()),
            config: Some("config.file".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: false,
                    do_start: true,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);
        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(|_| {
                Ok(Some(Box::new(TestEngine {
                    do_stage: false,
                    do_start: false,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);

        tool.engine_operations.expect_load_product_bundle().times(0);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);
        tool.engine_operations.expect_context().returning_st(move || env.context.clone()).times(2);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main in test_get_engine_with_config_doesnt_reuse");

        Ok(())
    }

    // Check that reuse and config.is_none calls get_engine_by_name
    #[fuchsia::test]
    async fn test_get_engine_without_config_does_reuse() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let cmd = StartCommand { reuse: true, net: Some("user".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let reused_config = make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
            .await
            .expect("reused_config config");

        tool.engine_operations.expect_new_engine().times(0);
        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(move |_| {
                Ok(Some(Box::new(TestEngine {
                    do_stage: false,
                    do_start: true,
                    do_configure: true,
                    config: reused_config.clone(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main in test_get_engine_without_config_does_reuse");

        Ok(())
    }

    // Check that if get_engine_by_name returns DoesNotExist, new_engine still gets called and reuse is reset
    // reuse is checked to be false, by watching do_stage. stage() is only called on non-reused instances.
    #[fuchsia::test]
    async fn test_get_engine_doesnotexist_creates_new() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand { reuse: true, net: Some("user".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_new_engine()
            .returning(|config, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: true,
                    config: config.clone(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);
        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main in test_get_engine_doesnotexist_creates_new");

        Ok(())
    }

    // Check that if DoesExist, then cmd.name is updated too
    #[fuchsia::test]
    async fn test_get_engine_updates_cmd_name_when_blank() -> Result<()> {
        let mut builder = ffx_config::test_env();
        builder = make_fake_sdk(builder).await;
        builder = builder.user_config("emu.name", "");
        let env = builder.build().expect("test env");

        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand { name: None, reuse: true, config: None, ..Default::default() };

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let reused_config = make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
            .await
            .expect("reused_config config");

        tool.engine_operations.expect_new_engine().times(0);
        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(move |name| {
                assert_eq!(name, &Some(DEFAULT_NAME.to_string()));
                Ok(Some(Box::new(TestEngine {
                    do_stage: false,
                    do_start: true,
                    do_configure: true,
                    config: reused_config.clone(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);
        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main in test_get_engine_updates_cmd_name_when_blank");
        Ok(())
    }

    // Ensure dry-run stops after building command, doesn't run
    #[fuchsia::test]
    async fn test_dry_run() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand {
            dry_run: true,
            verbose: true,
            net: Some("user".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: false,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);
        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(1);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await?;

        Ok(())
    }

    // Ensure stage stops after staging the files, doesn't run
    #[fuchsia::test]
    async fn test_stage() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand { stage: true, net: Some("user".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: false,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);

        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None));

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");
        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);

        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(1);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);
        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await?;
        Ok(())
    }

    // Ensure start goes through config and staging by default and calls start
    #[fuchsia::test]
    async fn test_start() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand::default();

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;
        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: true,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);

        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None));

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        let result = tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await;
        assert!(result.is_ok(), "{:?}", result.err());
        Ok(())
    }

    // Ensure start() skips the stage() call if the reuse flag is true
    #[fuchsia::test]
    async fn test_reuse_doesnt_stage() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let cmd = StartCommand { reuse: true, net: Some("user".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let reused_config = make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
            .await
            .expect("reused_config config");

        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(move |_| {
                Ok(Some(Box::new(TestEngine {
                    do_stage: false,
                    do_start: true,
                    do_configure: true,
                    config: reused_config.clone(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);
        tool.engine_operations.expect_new_engine().times(0);

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await?;
        Ok(())
    }

    // Ensure start() skips the stage() call is a custom config is provided
    #[fuchsia::test]
    async fn test_custom_config_doesnt_stage() -> Result<()> {
        let env = ffx_config::test_init().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand { config: Some("filename".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);

        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: false,
                    do_start: true,
                    config: EmulatorConfiguration::default(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);

        tool.engine_operations.expect_context().returning_st(move || env.context.clone()).times(2);
        tool.engine_operations.expect_load_product_bundle().times(0);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await?;
        Ok(())
    }

    // Check that the final command reflects changes from the edit stage
    #[fuchsia::test]
    async fn test_edit() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand { edit: true, ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: true,
                    start_test_fn: |command| {
                        assert_eq!(command.get_program(), "EditedValue");
                        Ok(())
                    },
                    config: EmulatorConfiguration {
                        runtime: RuntimeConfig { name: "name".to_string(), ..Default::default() },
                        ..Default::default()
                    },
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);
        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);

        tool.engine_operations
            .expect_edit_configuration()
            .returning(|config| {
                config.runtime.name = "EditedValue".to_string();
                Ok(())
            })
            .times(1);

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");
        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);

        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await?;
        Ok(())
    }

    // Check that the final command reflects changes from staging
    #[fuchsia::test]
    async fn test_staging_edits() -> Result<()> {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let env_context = env.context.clone();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let cmd = StartCommand::default();

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations.expect_context().returning(move || env_context.clone()).times(2);
        tool.engine_operations
            .expect_new_engine()
            .returning(|_, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: true,
                    stage_test_fn: |config| {
                        config.runtime.name = "EditedValue".to_string();
                        Ok(())
                    },
                    start_test_fn: |command| {
                        assert_eq!(command.get_program(), "EditedValue");
                        Ok(())
                    },
                    config: EmulatorConfiguration {
                        runtime: RuntimeConfig { name: "name".to_string(), ..Default::default() },
                        ..Default::default()
                    },
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);

        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        let pb = ProductBundle::V2(make_test_product_bundle(env.isolate_root.path())?);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None)).await?;
        Ok(())
    }

    // Tests that if check_if_reusable is set, but there is no existing instance, it starts a new engine
    // and sets the disk hashes in the config.
    #[fuchsia::test]
    async fn test_check_if_reusable_new() {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let cmd = StartCommand {
            name: Some("reuse-test".into()),
            reuse_with_check: true,
            net: Some("user".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        // Only load the product bundle once.
        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        // Only look for the existing engine once.
        tool.engine_operations.expect_get_engine_by_name().returning(|_| Ok(None)).times(1);

        // Only make the engine once.
        tool.engine_operations
            .expect_new_engine()
            .returning(|config, _, _| {
                Ok(Box::new(TestEngine {
                    do_stage: true,
                    do_start: true,
                    config: config.clone(),
                    stage_test_fn: |config| {
                        assert_eq!("d53f0b8a19b29d74", config.guest.zbi_hash);
                        assert_eq!("d547336219b6b160", config.guest.disk_hash);
                        Ok(())
                    },
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>)
            })
            .times(1);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main with stage");
    }

    // Checks that if the existing instance has matching disk hashes, it is reused.
    #[fuchsia::test]
    async fn test_check_if_reusable_matching() {
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));
        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let cmd = StartCommand {
            name: Some("reuse-test".into()),
            reuse_with_check: true,
            net: Some("user".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let mut matching_config =
            make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
                .await
                .expect("matching config");
        matching_config.guest.zbi_hash = "d53f0b8a19b29d74".into();
        matching_config.guest.disk_hash = "d547336219b6b160".into();

        // Only load the product bundle once.
        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);

        // Only look for the existing engine once, and find it.
        tool.engine_operations
            .expect_get_engine_by_name()
            .returning(move |_| {
                Ok(Some(Box::new(TestEngine {
                    // no staging should happen since we're reusing an instance.
                    do_stage: false,
                    do_start: true,
                    do_configure: true,
                    config: matching_config.clone(),
                    ..Default::default()
                }) as Box<dyn EmulatorEngine>))
            })
            .times(1);
        let context = env.context.clone();
        tool.engine_operations.expect_context().returning(move || context.clone()).times(2);
        tool.engine_operations
            .expect_get_emu_instances()
            .returning(move || emu_instances.clone())
            .times(1);

        // New engine should not be made.
        tool.engine_operations.expect_new_engine().times(0);

        tool.main(VerifiedMachineWriter::<CommandStatus>::new(None))
            .await
            .expect("main with stage");
    }

    // Checks that the command line options are applied to the existing instance when it is reused.
    #[fuchsia::test]
    async fn test_check_if_reusable_applies_args() {
        // Setup the test environment and SDK. This is boilerplate for
        // any test running ffx.
        let builder = ffx_config::test_env();
        let builder = make_fake_sdk(builder).await;
        let env = builder.build().unwrap();
        // Create a test product bundle. This is boilerplate for
        // any test that needs to use a product bundle. See the
        // make_test_product_bundle function to get the specific contents
        // that are staged in the product bundle.
        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );

        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        // Create the command line arguments for this test.
        let cmd = StartCommand {
            name: Some("reuse-test".into()),
            reuse_with_check: true,
            net: Some("user".into()),
            verbose: true,
            log: Some(env.isolate_root.path().join("emu.log")),
            ..Default::default()
        };

        // Create a configuration based on the product bundle, adding the
        // disk hashes so it appears that the product bundle has not changed since
        // when the instance was created.
        let mut default_config =
            make_configs(&env.context, &StartCommand::default(), Some(pb.clone()), &emu_instances)
                .await
                .expect("default config");
        default_config.guest.zbi_hash = "d53f0b8a19b29d74".into();
        default_config.guest.disk_hash = "d547336219b6b160".into();

        // Create the fake test engine instance. In this case since we are only testing if the engine
        // is reusable, set do_stage and do_start to false.
        let existing_engine = Box::new(TestEngine {
            // no staging should happen since we're reusing an instance.
            do_stage: false,
            do_start: false,
            do_configure: true,
            config: default_config.clone(),
            ..Default::default()
        });

        // Make the test instance of the tool, this uses mocks for the engine_operations
        // object.
        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        // Create the configuration that is based on the command line and the product bundle.
        let mut emulator_configuration =
            make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
                .await
                .expect("cmd configs");
        emulator_configuration.guest.zbi_hash = "d53f0b8a19b29d74".into();
        emulator_configuration.guest.disk_hash = "d547336219b6b160".into();

        // Set the mock expectation new_engine(). New engine should not be called.
        tool.engine_operations.expect_new_engine().times(0);

        // Call the function under test.
        let (reused, engine) = tool
            .check_if_reusable(&emulator_configuration, existing_engine)
            .await
            .expect("check_if_reusable");

        // assert that it was reused and the log path and verbose flag
        // were set to match the command line.
        assert!(reused, "Expected engine to be reused");
        assert_eq!(engine.emu_config().host.log, env.isolate_root.path().join("emu.log"));
        assert_eq!(engine.emu_config().runtime.log_level, LogLevel::Verbose);
    }

    #[fuchsia::test]
    async fn test_reuse_engine_serial_transitions() {
        async fn run_reuse(
            initial_serial: SerialMode,
            serial_enabled: bool,
        ) -> EmulatorConfiguration {
            let builder = ffx_config::test_env();
            let builder = make_fake_sdk(builder).await;
            let env = builder
                .user_config("emu.serial_number.enabled", serial_enabled)
                .build()
                .expect("test env");

            let cmd =
                StartCommand { name: Some("reuse-test".into()), reuse: true, ..Default::default() };

            let mut existing_config = EmulatorConfiguration::default();
            existing_config.runtime.serial_number = initial_serial;

            let mut existing_engine: Box<dyn EmulatorEngine> = Box::new(TestEngine {
                config: existing_config,
                do_configure: true,
                ..Default::default()
            });

            let tool = make_test_emu_start_tool(&env.context, cmd).await;
            let new_config = EmulatorConfiguration::default();

            tool.reuse_existing_engine(&mut existing_engine, &new_config)
                .await
                .expect("reuse_existing_engine");

            existing_engine.emu_config().clone()
        }

        // Test Cases:

        // 1. None (legacy) -> enabled=true => generates serial
        let config = run_reuse(SerialMode::Uninitialized, true).await;
        if let SerialMode::Enabled(serial) = &config.runtime.serial_number {
            assert!(serial.starts_with("EM-"));
        } else {
            panic!("expected serial to be generated, got {:?}", config.runtime.serial_number);
        }

        // 2. None (legacy) -> enabled=false => "none" (Disabled)
        let config = run_reuse(SerialMode::Uninitialized, false).await;
        assert_eq!(config.runtime.serial_number, SerialMode::Disabled);

        // 3. Existing serial -> enabled=true => preserved (stable!)
        let config = run_reuse(SerialMode::Enabled("EM-CUSTOM123".to_string()), true).await;
        assert_eq!(config.runtime.serial_number, SerialMode::Enabled("EM-CUSTOM123".to_string()));

        // 4. Existing serial -> enabled=false => "none" (Disabled)
        let config = run_reuse(SerialMode::Enabled("EM-CUSTOM123".to_string()), false).await;
        assert_eq!(config.runtime.serial_number, SerialMode::Disabled);

        // 5. "none" (Disabled) -> enabled=true => generates new serial (re-enabled!)
        let config = run_reuse(SerialMode::Disabled, true).await;
        if let SerialMode::Enabled(serial) = &config.runtime.serial_number {
            assert!(serial.starts_with("EM-"));
        } else {
            panic!("expected serial to be generated, got {:?}", config.runtime.serial_number);
        }

        // 6. "none" (Disabled) -> enabled=false => preserved as "none" (Disabled)
        let config = run_reuse(SerialMode::Disabled, false).await;
        assert_eq!(config.runtime.serial_number, SerialMode::Disabled);
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_named() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand { name: Some("test-instance-name".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        assert_eq!(tool.cmd.name, Some("test-instance-name".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_unsupported_arch() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );
        set_virtual_device_arch(env.isolate_root.path(), CpuArchitecture::Riscv64);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand { uefi: true, ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        assert_eq!(
            tool.finalize_start_command(&mut writer)
                .await
                .err()
                .expect("non-x64/arm64 product bundle is expected to fail")
                .to_string(),
            "The --uefi flag is currently only supported for x64 and arm64 target cpus."
        );
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_x64_pb() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "product.arch".into();
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand {
            uefi: true,
            name: Some("test-instance-name".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // TODO(https://fxbug.dev/382694675): When zircon.nodename can be persisted across reboots
        // from a running instance, rewriting the name won't be necessary anymore.
        assert_eq!(tool.cmd.name, Some("fuchsia-5254-ea06-13fe".into()));
        assert_eq!(tool.cmd.engine, Some("qemu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_x64_pb_femu() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "product.arch".into();
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand {
            uefi: true,
            engine: Some("femu".into()),
            name: Some("test-instance-name".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // TODO(https://fxbug.dev/382694675): When zircon.nodename can be persisted across reboots
        // from a running instance, rewriting the name won't be necessary anymore.
        assert_eq!(tool.cmd.name, Some("fuchsia-5254-ea06-13fe".into()));
        assert_eq!(tool.cmd.engine, Some("femu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_arm64_pb() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "product.arch".into();
        set_virtual_device_arch(env.isolate_root.path(), CpuArchitecture::Arm64);
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand {
            uefi: true,
            name: Some("test-instance-name".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // TODO(https://fxbug.dev/382694675): When zircon.nodename can be persisted across reboots
        // from a running instance, rewriting the name won't be necessary anymore.
        assert_eq!(tool.cmd.name, Some("fuchsia-5254-ea06-13fe".into()));
        assert_eq!(tool.cmd.engine, Some("qemu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_same_arch_pb_default_femu() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "product.arch".into();
        set_virtual_device_arch(env.isolate_root.path(), CpuArchitecture::current());
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand::default();

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // If `--engine` is not specified, the default value should be "femu".
        assert_eq!(tool.cmd.engine, Some("femu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_cross_arch_pb_default_femu() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "product.arch".into();
        set_virtual_device_arch(
            env.isolate_root.path(),
            match CpuArchitecture::current() {
                CpuArchitecture::X64 => CpuArchitecture::Arm64,
                _ => CpuArchitecture::X64,
            },
        );
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand::default();

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // If `--engine` is not specified and we're running cross-arch, the
        // default value should be overridden to "qemu".
        assert_eq!(tool.cmd.engine, Some("qemu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_cross_arch_pb_explicit_femu() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "product.arch".into();
        set_virtual_device_arch(
            env.isolate_root.path(),
            match CpuArchitecture::current() {
                CpuArchitecture::X64 => CpuArchitecture::Arm64,
                _ => CpuArchitecture::X64,
            },
        );
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand { engine: Some("femu".into()), ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // If `--engine` is explicitly specified try using that engine
        // unconditionally, even when we're running cross-arch and "femu" is
        // presumed incompatible.
        assert_eq!(tool.cmd.engine, Some("femu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_noname() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand { name: None, ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        assert_eq!(tool.cmd.name, Some(DEFAULT_NAME.into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_nodevice() {
        let env = ffx_config::test_init().unwrap();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let pb = ProductBundle::V2(
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle"),
        );
        let loaded_pb = LoadedProductBundle::new(pb, "some/path/to_bundle");

        let cmd = StartCommand { device: None, ..Default::default() };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);

        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // This is set as the recommended device in the product bundle.
        assert_eq!(tool.cmd.device, Some("virtual_device_1".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_vbmeta_from_ffx_config() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmpfile = tmp.path().display().to_string();

        let mut builder = ffx_config::test_env();
        builder = make_fake_sdk(builder).await;
        builder = builder
            .user_config("emu.vbmeta.key", tmpfile.clone())
            .user_config("emu.vbmeta.metadata", tmpfile.clone());
        let env = builder.build().expect("test env");

        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "pb.x64".into();
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let cmd = StartCommand {
            uefi: true,
            name: Some("test-instance-name".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let emu_config = make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
            .await
            .expect("emu config");

        assert_eq!(emu_config.guest.vbmeta_key_file, Some(tmp.path().to_path_buf()));
        assert_eq!(emu_config.guest.vbmeta_key_metadata_file, Some(tmp.path().to_path_buf()));

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // TODO(https://fxbug.dev/382694675): When zircon.nodename can be persisted across reboots
        // from a running instance, rewriting the name won't be necessary anymore.
        assert_eq!(tool.cmd.name, Some("fuchsia-5254-ea06-13fe".into()));
        assert_eq!(tool.cmd.engine, Some("qemu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_vbmeta_from_cmd_line() {
        let mut builder = ffx_config::test_env();
        builder = make_fake_sdk(builder).await;
        builder = builder
            .user_config("emu.vbmeta.key", "/some/vbmeta/key")
            .user_config("emu.vbmeta.metadata", "/some/vbmeta/metadata");
        let env = builder.build().expect("test env");

        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "pb.x64".into();
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmpfile = tmp.path().display().to_string();

        let cmd = StartCommand {
            uefi: true,
            name: Some("test-instance-name".into()),
            vbmeta_key: Some(tmpfile.clone()),
            vbmeta_key_metadata: Some(tmpfile.clone()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let emu_config = make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
            .await
            .expect("emu_config");

        assert_eq!(emu_config.guest.vbmeta_key_file, Some(tmp.path().to_path_buf()));
        assert_eq!(emu_config.guest.vbmeta_key_metadata_file, Some(tmp.path().to_path_buf()));

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // TODO(https://fxbug.dev/382694675): When zircon.nodename can be persisted across reboots
        // from a running instance, rewriting the name won't be necessary anymore.
        assert_eq!(tool.cmd.name, Some("fuchsia-5254-ea06-13fe".into()));
        assert_eq!(tool.cmd.engine, Some("qemu".into()));
    }

    #[fuchsia::test]
    async fn test_finalize_start_command_uefi_vbmeta_non_existing_files() {
        let mut builder = ffx_config::test_env();
        builder = make_fake_sdk(builder).await;
        builder = builder
            .user_config("emu.vbmeta.key", "/some/vbmeta/key")
            .user_config("emu.vbmeta.metadata", "/some/vbmeta/metadata");
        let env = builder.build().expect("test env");

        let emu_instances = EmulatorInstances::new(PathBuf::from(env.isolate_root.path()));

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(None, &test_buffers);

        let mut pb =
            make_test_product_bundle(env.isolate_root.path()).expect("test product bundle");
        pb.product_name = "pb.x64".into();
        let pb = ProductBundle::V2(pb);
        let loaded_pb = LoadedProductBundle::new(pb.clone(), "some/path/to_bundle");

        let cmd = StartCommand {
            uefi: true,
            name: Some("test-instance-name".into()),
            ..Default::default()
        };

        let mut tool = make_test_emu_start_tool(&env.context, cmd).await;

        let emu_config = make_configs(&env.context, &tool.cmd, Some(pb.clone()), &emu_instances)
            .await
            .expect("emu_config");

        assert_eq!(emu_config.guest.vbmeta_key_file, None);
        assert_eq!(emu_config.guest.vbmeta_key_metadata_file, None);

        tool.engine_operations
            .expect_load_product_bundle()
            .returning(move |_| Ok(loaded_pb.clone()))
            .times(1);
        tool.engine_operations.expect_context().times(0);

        tool.finalize_start_command(&mut writer).await.unwrap();

        // TODO(https://fxbug.dev/382694675): When zircon.nodename can be persisted across reboots
        // from a running instance, rewriting the name won't be necessary anymore.
        assert_eq!(tool.cmd.name, Some("fuchsia-5254-ea06-13fe".into()));
        assert_eq!(tool.cmd.engine, Some("qemu".into()));
    }
}
