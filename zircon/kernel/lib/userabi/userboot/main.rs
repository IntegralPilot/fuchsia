// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use bootfs::{get_bootfs_vmo, get_zbi_container};
use cmdline::{ProgramInfo, parse_cmdline};
use program::{SystemHandles, launch_program};
use std::fmt::Write as _;
use userboot::{Log, take_system_handles};
use zbi::{ZbiContainer, ZbiType};
use zerocopy::IntoBytes as _;

const DEFAULT_NEXT_BOOT: &str = "bin/component_manager+--boot";

/// Program configuration options parsed from the command line.
#[derive(Default)]
struct Options {
    /// Information about the primary boot program to launch.
    boot: ProgramInfo,
    /// Whether to check the CRC of bootfs files.
    bootfs_crc_check: bool,
}

fn parse_option(key: &str, value: &str, opts: &mut Options) -> bool {
    match key {
        "userboot.next" => {
            opts.boot.next = value.to_string();
            true
        }
        "userboot.root" => {
            opts.boot.root = value.strip_suffix('/').unwrap_or(value).to_string();
            true
        }
        "userboot.crc" => {
            opts.bootfs_crc_check = value == "true";
            true
        }
        _ => false,
    }
}

fn get_options(container: &ZbiContainer<&[u8]>, log: &mut Log) -> Result<Options, Error> {
    let mut options = Options::default();
    for item in container.iter() {
        if item.header.type_ == ZbiType::CmdLine as u32 {
            if let Ok(cmdline) = str::from_utf8(item.payload.as_bytes()) {
                writeln!(log, "CMDLINE {}", cmdline)?;
                parse_cmdline(cmdline, log, &mut options, parse_option)?;
            }
        }
    }

    if options.boot.next.is_empty() {
        options.boot.next = DEFAULT_NEXT_BOOT.to_string();
    }

    if !options.boot.root.is_empty() && options.boot.root.starts_with('/') {
        anyhow::bail!("`userboot.root` (\"{}\") must not begin with a '/'", options.boot.root);
    }

    Ok(options)
}

async fn run(log: &mut Log) -> Result<(), Error> {
    let handles = SystemHandles::from_handles(take_system_handles()?)?;
    let root_vmar = fuchsia_runtime::vmar_root_self();

    let container = get_zbi_container(&handles.zbi_vmo, &root_vmar)?;
    let options = get_options(&container, log)?;
    let bootfs_vmo = get_bootfs_vmo(&container, &root_vmar, options.bootfs_crc_check, log)?;

    let (program_name, target_path) = options.boot.filename();

    let _process = launch_program(
        program_name,
        &target_path,
        options.boot.next.split('+'),
        bootfs_vmo,
        handles,
        None,
        log,
    )
    .await
    .map_err(|e| anyhow!("launch_program failed: {e:?}"))?;

    Ok(())
}

#[fuchsia_async::run_singlethreaded]
async fn main() {
    let mut log = Log::new();
    if let Err(e) = run(&mut log).await {
        let _ = writeln!(log, "userboot error: {e:?}");
    }
}
