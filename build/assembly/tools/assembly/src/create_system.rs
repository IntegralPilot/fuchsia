// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use assembled_system::AssembledSystem;
use assembly_cli_args::CreateSystemArgs;
use assembly_container::AssemblyContainer;
use assembly_tool::{PlatformToolProvider, ToolProvider};
use assembly_util as util;
use camino::Utf8PathBuf;
use image_assembly_config::ImageAssemblyConfig;
use std::fs::File;

/// Create an image assembly config.
pub async fn create_system(args: CreateSystemArgs) -> Result<()> {
    let CreateSystemArgs {
        platform,
        image_assembly_config,
        include_account,
        outdir,
        gendir,
        base_package_name,
        mode,
    } = args;

    let image_assembly_config: ImageAssemblyConfig = util::read_config(image_assembly_config)
        .context("Failed to read the image assembly config")?;

    // Get the tool set.
    let tools = PlatformToolProvider::new(platform);

    // Construct the assembled system.
    let assembled_system = AssembledSystem::new(
        image_assembly_config,
        include_account.unwrap_or(false),
        &gendir,
        &tools,
        base_package_name,
        mode,
    )
    .await?;
    assembled_system
        .write_to_dir(&outdir, None::<Utf8PathBuf>)
        .context("Creating the assembly manifest")?;

    // Write the tool command log.
    let command_log_path = gendir.join("command_log.json");
    let command_log = File::create(command_log_path).context("Creating command log")?;
    serde_json::to_writer(&command_log, tools.log()).context("Writing command log")?;

    Ok(())
}
