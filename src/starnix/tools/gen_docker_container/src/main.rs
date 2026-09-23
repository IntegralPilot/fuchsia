// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;
use serde_json::json;
use std::fs::File;
use tar_img_extract::docker_archive::DockerArchive;

/// Generate a container manifest and default command manifest from a docker archive.
#[derive(FromArgs)]
struct Command {
    /// container features
    #[argh(option)]
    pub features: Vec<String>,

    /// container name
    #[argh(option)]
    pub container_name: String,

    /// input archive path
    #[argh(positional)]
    pub input_path: String,

    /// output container manifest
    #[argh(positional)]
    pub container_manifest: String,

    /// output default command manifest
    #[argh(positional)]
    pub default_command_manifest: String,
}

fn main() -> Result<()> {
    let cmd: Command = argh::from_env();

    let input_file = File::open(&cmd.input_path)?;
    let input_archive = DockerArchive::open(input_file)?;

    let mounts = vec![
        // Should we set nosuid for the root mount?
        "/:remote_bundle:data/system:nodev,relatime",
        "/dev:devtmpfs::nosuid,relatime",
        "/dev/pts:devpts::nosuid,noexec,relatime",
        "/dev/shm:tmpfs::nosuid,nodev",
        "/proc:proc::nosuid,nodev,noexec,relatime",
        "/sys:sysfs::nosuid,nodev,noexec,relatime",
        "/tmp:tmpfs",
    ];

    let mut features = cmd.features.clone();
    // TODO(https://fxbug.dev/356684424): Do we always want this feature in these containers?
    // We might because we use the starnix_container runner to run the default command.
    features.push("container".to_string());

    let container_manifest = generate_container_manifest(&cmd.container_name, &mounts, &features);
    let default_command_manifest =
        generate_default_command_cml(&input_archive.default_command(), &input_archive.environ());

    std::fs::write(&cmd.container_manifest, container_manifest)?;
    std::fs::write(&cmd.default_command_manifest, default_command_manifest)?;

    Ok(())
}

fn generate_default_command_cml(command: &[&str], environ: &[&str]) -> Vec<u8> {
    let binary = command[0];
    let args = &command[1..];

    let cml = json!({
        "program": {
            "runner": "starnix_container",
            "binary": binary,
            "args": args,
            "uid": "0",
            "environ": environ,
        },
    });

    serde_json::to_vec(&cml).expect("failed to serialize default command manifest")
}

fn generate_container_manifest(
    container_name: &str,
    mounts: &[&str],
    features: &[String],
) -> Vec<u8> {
    let cml = json!({
        "include": [
            "//src/starnix/containers/container.shard.cml",
        ],
        "program": {
            "runner": "starnix",
            "features": features,
            "init": [],
            "kernel_cmdline": "",
            "mounts": mounts,
            "name": container_name,
            "startup_file_path": "",
        },
    });

    serde_json::to_vec(&cml).expect("failed to serialize container manifest")
}
