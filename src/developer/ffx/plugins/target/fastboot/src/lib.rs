// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use assembly_partitions_config::UploadMethod;
use async_trait::async_trait;
use discovery::TargetState;
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_fastboot::common::vars::MAX_DOWNLOAD_SIZE_VAR;
use ffx_fastboot::common::{flash_partition_impl, upload_file};
use ffx_fastboot_connection_factory::{RetryLimit, get_fastboot_interface};
use ffx_fastboot_interface::fastboot_interface::{FastbootInterface, Variable};
use ffx_fastboot_tool_args::{FastbootCommand, FastbootSubcommand};
use ffx_flash_manifest::SSH_OEM_COMMAND;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxMain, FfxTool};
use futures::{TryFutureExt, try_join};
use product_bundle::ProductBundle;
use schemars::JsonSchema;
use serde::Serialize;
use sparse::reader::SparseReader;
use sparse::{build_sparse_files, resparse_sparse_img};
use std::fs::File;
use std::io::Write;

use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;

#[derive(FfxTool)]
#[target(None)]
pub struct FastbootTool {
    #[command]
    cmd: FastbootCommand,
    ctx: EnvironmentContext,
}

fho::embedded_plugin!(FastbootTool);

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FastbootMessage {
    Variable { name: String, value: String },
}

#[async_trait(?Send)]
impl FfxMain for FastbootTool {
    type Writer = VerifiedMachineWriter<FastbootMessage>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        cmd_impl(&self.ctx, &mut writer, &self.cmd).await
    }
}

async fn sink<T>(mut rx: Receiver<T>) -> Result<()> {
    while rx.recv().await.is_some() {}
    Ok(())
}

async fn handle_vars<W>(writer: &mut W, mut rx: Receiver<Variable>) -> Result<()>
where
    W: Write,
{
    loop {
        match rx.recv().await {
            Some(x) => {
                write!(writer, "{}:{}", x.name, x.value)?;
            }
            None => break,
        }
    }
    Ok(())
}

async fn fastboot_impl<F>(
    ctx: &EnvironmentContext,
    writer: &mut VerifiedMachineWriter<FastbootMessage>,
    command: &FastbootCommand,
    interface: &mut F,
) -> fho::Result<()>
where
    F: FastbootInterface,
{
    match &command.subcommand {
        FastbootSubcommand::Flash(cmd) => {
            let flash_timeout_rate_mb_per_second: f64 = 5000.0;
            let flash_min_timeout_seconds: u64 = 200;
            let (client, server) = mpsc::channel(1);
            try_join!(
                flash_partition_impl(
                    client,
                    &cmd.partition,
                    cmd.file.to_str().unwrap(),
                    interface,
                    flash_min_timeout_seconds,
                    flash_timeout_rate_mb_per_second,
                    cmd.max_download_size,
                )
                .map_err(anyhow::Error::from),
                sink(server)
            )
            .map_err(fho::Error::from)?;
        }
        FastbootSubcommand::GetVar(cmd) => match cmd.var_name.as_str() {
            "all" => {
                let (client, server) = mpsc::channel(1);
                try_join!(
                    async { interface.get_all_vars(client).await.map_err(|e| anyhow!(e)) },
                    handle_vars(writer, server)
                )
                .map_err(fho::Error::from)?;
            }
            v @ _ => {
                let value = interface
                    .get_var(&v)
                    .await
                    .map_err(|e| anyhow!(e))
                    .map_err(fho::Error::from)?;
                writeln!(writer, "{}: {}", v, value)
                    .map_err(|e| anyhow!(e))
                    .map_err(fho::Error::from)?;
            }
        },
        FastbootSubcommand::Stage(cmd) => {
            let (client, server) = mpsc::channel(1);
            try_join!(
                async {
                    interface
                        .stage(cmd.file.as_os_str().to_str().unwrap(), client)
                        .await
                        .map_err(|e| anyhow!(e))
                },
                sink(server)
            )
            .map_err(fho::Error::from)?;
        }
        FastbootSubcommand::Oem(cmd) => {
            interface
                .oem(&cmd.command.join(" ").to_string())
                .await
                .map_err(|e| anyhow!(e))
                .map_err(fho::Error::from)?;
        }
        FastbootSubcommand::Continue(_) => {
            interface.continue_boot().await.map_err(|e| anyhow!(e)).map_err(fho::Error::from)?;
        }
        FastbootSubcommand::Reboot(cmd) => match &cmd.bootloader {
            Some(b) => match b.as_str() {
                "bootloader" => {
                    let (client, server) = mpsc::channel(1);
                    try_join!(
                        async { interface.reboot_bootloader(client).await.map_err(|e| anyhow!(e)) },
                        sink(server)
                    )
                    .map_err(fho::Error::from)?;
                }
                mode => {
                    ffx_bail!("Unsupported mode: {}", mode);
                }
            },
            None => {
                interface.reboot().await.map_err(|e| anyhow!(e)).map_err(fho::Error::from)?;
            }
        },
        FastbootSubcommand::Sparse(cmd) => {
            let download_size = match cmd.size {
                Some(s) => s,
                None => {
                    let max_download_size_var = interface
                        .get_var(MAX_DOWNLOAD_SIZE_VAR)
                        .await
                        .map_err(|e| anyhow!("Communication error with the device: {:?}", e))?;

                    let trimmed_max_download_size_var =
                        max_download_size_var.trim_start_matches("0x");

                    let max_download_size: u64 =
                        u64::from_str_radix(trimmed_max_download_size_var, 16)
                            .expect("Fastboot max download size var was not a valid u32");
                    max_download_size
                }
            };

            let mut file_handle =
                File::open(&cmd.file).map_err(|e| anyhow!(e)).map_err(fho::Error::from)?;

            let sparse_files = match SparseReader::is_sparse_file(&mut file_handle) {
                Ok(true) => {
                    log::debug!("Is already a sparse file. Building Reader");
                    let mut reader = SparseReader::new(file_handle).map_err(|e| anyhow!(e))?;
                    log::debug!("Building sparse image");
                    resparse_sparse_img(&mut reader, &cmd.out_dir, download_size)
                        .map_err(|e| anyhow!(e))?
                }
                Err(_) | Ok(false) => {
                    log::debug!(
                        "About to build sparse files for: {:?}, and put them in: {:?}",
                        cmd.file,
                        cmd.out_dir
                    );

                    let filename = cmd.file.to_str().unwrap();
                    let files = build_sparse_files(filename, filename, &cmd.out_dir, download_size)
                        .map_err(|e| anyhow!(e))?;
                    files
                }
            };

            for (i, file) in sparse_files.into_iter().enumerate() {
                let out_path = cmd.out_dir.join(format!("{}-tmp.simg", i));
                file.persist(&out_path).map_err(|e| anyhow!(e)).map_err(fho::Error::from)?;
                log::debug!("Keeping file: {:?}", out_path);
            }
        }
        FastbootSubcommand::Authorize(cmd) => {
            let keys = ffx_ssh::SshKeyFiles::load(ctx).map_err(|e| anyhow!(e))?;

            // If a product bundle was given, look inside to see if there's a specified SSH key
            // upload method.
            let method = match &cmd.product_bundle {
                Some(path) => {
                    let pb = ProductBundle::try_load_from(path).map_err(|e| anyhow!(e))?;
                    match pb {
                        ProductBundle::V2(pb_v2) => pb_v2.partitions.ssh_key_upload_method,
                    }
                }
                None => None,
            };

            // If no upload method was specified, default to the OEM stage command.
            let method = match method {
                Some(m) => m,
                None => UploadMethod::Staged { command: SSH_OEM_COMMAND.to_string() },
            };

            let keys_path = keys.authorized_keys.to_string_lossy().to_string();

            let mut resolver = ffx_fastboot::file_resolver::resolvers::EmptyResolver::new()
                .map_err(|e| anyhow!(e))
                .map_err(fho::Error::from)?;

            let (messenger, rx) = mpsc::channel(1);
            try_join!(
                async {
                    let result = upload_file(
                        &messenger,
                        &mut resolver,
                        /*resolve=*/ false,
                        &keys_path,
                        &method,
                        interface,
                    )
                    .await;
                    // Drop `messenger` so `sink(rx)` running asynchronously doesn't hang forever.
                    drop(messenger);
                    result.map_err(|e| anyhow!(e))
                },
                sink(rx)
            )
            .map_err(fho::Error::from)?;

            interface.continue_boot().await.map_err(|e| anyhow!(e)).map_err(fho::Error::from)?;
            log::debug!("Sent continue boot command");
        }
    };
    Ok(())
}

async fn cmd_impl(
    ctx: &EnvironmentContext,
    writer: &mut VerifiedMachineWriter<FastbootMessage>,
    command: &FastbootCommand,
) -> fho::Result<()> {
    let handle =
        ffx_target::discover_single_default_target(ctx).await.map_err(anyhow::Error::from)?;

    if !matches!(handle.state, TargetState::Fastboot(_)) {
        ffx_bail!("This plugin only works when the target is in Fastboot mode");
    }

    let res = match handle.state {
        TargetState::Fastboot(fastboot_state) => {
            let mut proxy =
                get_fastboot_interface(&fastboot_state, handle.node_name, ctx, RetryLimit::Default)
                    .await
                    .map_err(|e| anyhow::Error::from(e))?;
            fastboot_impl(ctx, writer, command, &mut proxy).await
        }
        _ => {
            ffx_bail!("Could not connect. Target not in fastboot: {handle}");
        }
    };
    res
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use chrono::Duration;
    use ffx_fastboot_interface::fastboot_interface::*;
    use ffx_fastboot_interface::stream::StreamCommand;
    use ffx_fastboot_tool_args::AuthorizeSubcommand;
    use ffx_writer::{Format, TestBuffers};
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc::Sender;

    #[derive(Default, Debug, Clone)]
    struct MockInterface {
        staged_path: Arc<Mutex<Option<String>>>,
        /// All transmitted OEM commands; each will have the "oem " prefix automatically inserted
        /// to match real fastboot behavior.
        oem_commands: Arc<Mutex<Vec<String>>>,
        continue_boot_called: Arc<Mutex<bool>>,
    }

    impl FastbootInterface for MockInterface {}

    #[async_trait]
    impl Fastboot for MockInterface {
        async fn stage(
            &mut self,
            path: &str,
            _sender: mpsc::Sender<UploadProgress>,
        ) -> Result<(), FastbootError> {
            *self.staged_path.lock().unwrap() = Some(path.to_string());
            Ok(())
        }

        async fn oem(&mut self, command: &str) -> Result<(), FastbootError> {
            self.oem_commands.lock().unwrap().push(format!("oem {}", command));
            Ok(())
        }

        async fn continue_boot(&mut self) -> Result<(), FastbootError> {
            *self.continue_boot_called.lock().unwrap() = true;
            Ok(())
        }

        async fn flash(
            &mut self,
            _partition_name: &str,
            _path: &str,
            _listener: Sender<UploadProgress>,
            _timeout: Duration,
        ) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn flash_from_reader(
            &mut self,
            _partition_name: &str,
            _size: u32,
            _reader: &mut (dyn std::io::Read + Send),
            _listener: Sender<UploadProgress>,
            _timeout: Duration,
        ) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn erase(&mut self, _partition_name: &str) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn reboot(&mut self) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn reboot_bootloader(
            &mut self,
            _sender: mpsc::Sender<RebootEvent>,
        ) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn get_var(&mut self, _name: &str) -> Result<String, FastbootError> {
            unimplemented!()
        }
        async fn get_all_vars(
            &mut self,
            _sender: mpsc::Sender<Variable>,
        ) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn set_active(&mut self, _slot: &str) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn boot(&mut self) -> Result<(), FastbootError> {
            unimplemented!()
        }
        async fn get_staged(&mut self, _path: &str) -> Result<(), FastbootError> {
            unimplemented!()
        }

        async fn stream<'a>(
            &mut self,
            _partition_name: &str,
            _stream_command: StreamCommand,
            _listener: &Sender<UploadProgress>,
            _timeout: chrono::TimeDelta,
        ) -> Result<(), FastbootError> {
            unimplemented!()
        }
    }

    #[fuchsia::test]
    async fn test_authorize() -> Result<()> {
        let temp_ssh_priv = NamedTempFile::new().expect("creating temp file for ssh.priv");
        let temp_ssh_pub = NamedTempFile::new().expect("creating temp file for ssh.pub");
        let authorized_keys_path = temp_ssh_pub.path().to_string_lossy().to_string();

        let env = ffx_config::test_env()
            .user_config("ssh.priv", temp_ssh_priv.path().to_str().unwrap())
            .user_config("ssh.pub", temp_ssh_pub.path().to_str().unwrap())
            .build()
            .unwrap();

        let mut interface = MockInterface::default();
        let command = FastbootCommand {
            subcommand: FastbootSubcommand::Authorize(AuthorizeSubcommand { product_bundle: None }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(Format::Json), &buffers);

        fastboot_impl(&env.context, &mut writer, &command, &mut interface).await?;

        assert_eq!(
            interface.staged_path.lock().unwrap().as_deref(),
            Some(authorized_keys_path.as_str())
        );
        assert_eq!(
            *interface.oem_commands.lock().unwrap(),
            vec!["oem add-staged-bootloader-file ssh.authorized_keys"]
        );
        assert!(*interface.continue_boot_called.lock().unwrap());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_authorize_inline() -> Result<()> {
        let temp_ssh_priv = NamedTempFile::new().expect("creating temp file for ssh.priv");
        let temp_ssh_pub = NamedTempFile::new().expect("creating temp file for ssh.pub");
        std::fs::write(temp_ssh_pub.path(), vec![b'a'; 100])?;

        let env = ffx_config::test_env()
            .user_config("ssh.priv", temp_ssh_priv.path().to_str().unwrap())
            .user_config("ssh.pub", temp_ssh_pub.path().to_str().unwrap())
            .build()
            .unwrap();

        let pb_dir = tempfile::tempdir()?;
        let pb_path = pb_dir.path().to_string_lossy().to_string();
        let pb_json = json!({
            "version": "2",
            "product_name": "test",
            "product_version": "test",
            "sdk_version": "test",
            "partitions": {
                "hardware_revision": "board",
                "bootstrap_partitions": [],
                "bootloader_partitions": [],
                "partitions": [],
                "unlock_credentials": [],
                "ssh_key_upload_method": {
                    "type": "inline",
                    "command_prefix": "inline_command=",
                    "command_max_length": 64,
                    "init_command": "init_command"
                }
            }
        });
        std::fs::write(
            pb_dir.path().join("product_bundle.json"),
            serde_json::to_string_pretty(&pb_json)?,
        )?;

        let mut interface = MockInterface::default();
        let command = FastbootCommand {
            subcommand: FastbootSubcommand::Authorize(AuthorizeSubcommand {
                product_bundle: Some(pb_path),
            }),
        };
        let buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::new_test(Some(Format::Json), &buffers);

        fastboot_impl(&env.context, &mut writer, &command, &mut interface).await?;

        let cmds = interface.oem_commands.lock().unwrap();
        // Verify that the SSH parameters were passed through correctly.
        assert!(cmds.len() >= 2);
        assert_eq!(cmds[0], "oem init_command");
        assert!(cmds[1].starts_with("oem inline_command="));
        assert_eq!(cmds[1].len(), 64);
        assert!(*interface.continue_boot_called.lock().unwrap());
        Ok(())
    }
}
