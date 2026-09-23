// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, anyhow};
use ffx_scrutiny_verify_args::{Command, SubCommand};
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool, Result, bug, return_user_error};
use scrutiny_utils::path::relativize_path;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

mod bootfs;
mod component_resolvers;
mod kernel_cmdline;
mod ota;
mod pre_signing;
mod route_sources;
mod routes;
mod static_pkgs;
mod structured_config;

#[derive(FfxTool)]
#[target(None)]
pub struct ScrutinyVerifyTool {
    #[command]
    pub cmd: Command,
}

fho::embedded_plugin!(ScrutinyVerifyTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ScrutinyVerifyTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        if self.cmd.depfile.is_some() && self.cmd.stamp.is_none() {
            return_user_error!("Cannot specify --depfile without --stamp");
        }

        let tmp_dir = self.cmd.tmp_dir.as_ref();
        if let Some(tmp_dir) = tmp_dir {
            fs::create_dir_all(tmp_dir).map_err(|err| {
                anyhow!(
                    "Failed to create temporary directory {:?} for `ffx scrutiny verify`: {}",
                    tmp_dir,
                    err
                )
            })?;
        }
        let recovery = self.cmd.recovery;

        let deps_set = match &self.cmd.subcommand {
            SubCommand::Bootfs(subcommand) => bootfs::verify(subcommand, recovery).await,
            SubCommand::ComponentResolvers(subcommand) => {
                component_resolvers::verify(subcommand, recovery).await
            }
            SubCommand::KernelCmdline(subcommand) => {
                kernel_cmdline::verify(subcommand, recovery).await
            }
            SubCommand::PreSigning(subcommand) => pre_signing::verify(subcommand, recovery).await,
            SubCommand::RouteSources(subcommand) => {
                route_sources::verify(subcommand, tmp_dir, recovery).await
            }
            SubCommand::Routes(subcommand) => routes::verify(subcommand, tmp_dir, recovery).await,
            SubCommand::StaticPkgs(subcommand) => static_pkgs::verify(subcommand, recovery).await,
            SubCommand::StructuredConfig(subcommand) => {
                structured_config::verify(subcommand, recovery).await
            }
            SubCommand::Ota(subcommand) => ota::verify(subcommand).await,
        }?;

        if let Some(depfile_path) = self.cmd.depfile.as_ref() {
            let stamp_path = self
                .cmd
                .stamp
                .as_ref()
                .ok_or_else(|| anyhow!("Cannot specify depfile without specifying stamp"))?;
            let stamp_path = stamp_path.to_str().ok_or_else(|| {
                anyhow!(
                    "Stamp path {:?} cannot be converted to string for writing to depfile",
                    stamp_path
                )
            })?;
            let mut depfile = fs::File::create(depfile_path).context("failed to create depfile")?;

            // Convert any absolute paths into paths relative to `cwd` to satisfy depfile format
            // requirements.
            let default_build_path = PathBuf::from(String::from("."));
            let relative_dep_paths: Vec<PathBuf> = deps_set
                .into_iter()
                .map(|dep_path| relativize_path(default_build_path.clone(), dep_path))
                .collect();

            let deps = relative_dep_paths
                .iter()
                .map(|dep_path| {
                    dep_path.to_str().ok_or_else(|| {
                        bug!("Failed to convert path for depfile to string: {:?}", dep_path)
                    })
                })
                .collect::<Result<Vec<&str>>>()?;
            write!(depfile, "{}: {}", stamp_path, deps.join(" "))
                .context("failed to write to depfile")?;
        }
        if let Some(stamp_path) = self.cmd.stamp.as_ref() {
            fs::write(stamp_path, "Verified\n").context("failed to write stamp file")?;
        }

        Ok(())
    }
}
