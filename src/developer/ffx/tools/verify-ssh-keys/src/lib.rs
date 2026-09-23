// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use argh::{ArgsInfo, FromArgs};
use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_ssh::keys::{MatchingKeysInfo, SshKey};
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxContext, FfxMain, FfxTool, Result};
use std::io::Write;
use target_holders::SshAddrHolder;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "verify-ssh-keys",
    description = "Check target access keys in $FUCHSIA_DIR, ~/.ssh, or agent.",
    note = "Checks the device's authorized keys to determine whether we have any of the available
public keys locally.

Also, checks for public keys in the fuchsia directory (if set in $FUCHSIA_DIR), the home directory
under `.ssh` and also checks the ssh agent."
)]
pub struct VerifyCommand {}

#[derive(FfxTool)]
pub struct VerifyTool {
    #[command]
    _cmd: VerifyCommand,
    env: EnvironmentContext,
    device_addr: SshAddrHolder,
}

#[async_trait(?Send)]
impl FfxMain for VerifyTool {
    type Writer = MachineWriter<SshKey>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> Result<()> {
        let MatchingKeysInfo { keys: found_keys, dirs_searched, io_errors } =
            ffx_ssh::keys::find_matching_ssh_keys(&self.env, *self.device_addr).await?;
        if !writer.is_machine() {
            writeln!(&mut writer, "Found the following public keys matching your device:\n")
                .bug()?;
        }
        for key in found_keys.into_iter() {
            if writer.is_machine() {
                writer.item(&key).bug()?;
            } else {
                writeln!(&mut writer, "-- {key}").bug()?;
            }
        }
        if !writer.is_machine() {
            writeln!(
                &mut writer,
                "\nSearched the ssh agent and the following directories for keys:\n\n{}\n",
                dirs_searched
                    .into_iter()
                    .map(|d| format!("-- {}", d.display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
            .bug()?;
            if !io_errors.is_empty() {
                writeln!(
                    &mut writer,
                    "Encountered errors reading the following files and/or directories:\n\n{}",
                    io_errors
                        .into_iter()
                        .map(|(path, err)| format!("-- Reading {}: {}", path.display(), err))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
                .bug()?;
            }
            writeln!(
                &mut writer,
                "\nIf you are still not able to connect to your device, you may need to move one of your public/private key pairs to a different directory.
Please consult https://fuchsia.dev/fuchsia-src/development/tools/ffx/workflows/create-ssh-keys-for-devices for more details")
                .bug()?;
        }
        Ok(())
    }
}
