// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for embedding SSH keys, kernel arguments, and serial numbers into ZBI images.

use ffx_config::{EnvironmentContext, get_host_tool};
use ffx_ssh::SshKeyFiles;
use fho::{Result, bug, return_bug};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::NamedTempFile;

/// Bootloader file name used by bootsvc to persist authorized keys into `/data/ssh/authorized_keys`.
pub const SSH_BOOTLOADER_FILE_NAME: &str = "ssh.authorized_keys";

/// Prepare the SSH key as boot loader file.
pub fn authorized_keys_to_boot_loader_file(src: &Path, dst: &Path) -> Result<()> {
    let mut v = Vec::new();
    let name = SSH_BOOTLOADER_FILE_NAME;
    let authorized_keys = fs::read(src).map_err(|e| bug!("{e}"))?;

    // The format for the boot loader files is described in
    // https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/zbi-format/include/lib/zbi-format/zbi.h;l=229-237;drc=64cdcbf06860ab1f19b85b3c221debcadcae3b5d
    v.push(name.len().try_into().map_err(|_| {
        bug!("Invalid length for boot file name: {} cannot be converted to u8", name.len())
    })?);
    v.extend(name.as_bytes());
    v.extend(authorized_keys);

    fs::write(dst, v).map_err(|e| bug!("{e}"))
}

/// Embeds boot data into a Zircon Boot Image (ZBI) using host tools and SSH keys from `ctx`:
/// - Authorized keys as both bootfs (`data/ssh/authorized_keys`) and bootloader file (`ssh.authorized_keys`).
/// - Kernel command line parameters if provided.
/// - Serial number (`SERIAL_NUMBER` boot item) if provided.
/// - System entropy from `/dev/urandom` if available.
pub fn embed_boot_data(
    ctx: &EnvironmentContext,
    src: &Path,
    dest: &Path,
    cmdline: Option<&str>,
    serial_number: Option<&str>,
) -> Result<()> {
    let zbi_tool = get_host_tool(ctx, "zbi").map_err(|e| bug!("ZBI tool is missing: {e}"))?;
    let ssh_keys =
        SshKeyFiles::load(ctx).map_err(|e| bug!("Error finding ssh authorized_keys file: {e}"))?;
    ssh_keys
        .create_keys_if_needed(false)
        .map_err(|e| bug!("Error creating ssh keys if needed: {e}"))?;
    let auth_keys = ssh_keys.authorized_keys.display().to_string();
    if !ssh_keys.authorized_keys.exists() {
        return_bug!("No authorized_keys found. {} does not exist.", auth_keys);
    }
    if src == dest {
        return_bug!("source and dest zbi paths cannot be the same.");
    }

    let replace_str = format!("data/ssh/authorized_keys={}", auth_keys);

    let mut zbi_command = Command::new(zbi_tool);
    zbi_command.arg("-o").arg(dest).arg("--replace").arg(src).arg("-e").arg(replace_str);

    // Embed the authorized_keys as bootloader file. This ensures that the key file will be
    // persisted in /data/ssh, and after an `fx ota` of a GPT image the ssh connection
    // continues to work in subsequent boots.
    let bootloader_file = NamedTempFile::new().map_err(|e| bug!("{e}"))?;
    authorized_keys_to_boot_loader_file(&ssh_keys.authorized_keys, bootloader_file.path())?;
    zbi_command.arg("--type=bootloader_file").arg(bootloader_file.path());

    // Lazy tempfile creation to avoid unnecessary disk I/O when features are disabled/unused.
    // We bind the NamedTempFiles to variables prefixed with "_" in the outer scope to indicate
    // they are used solely for their RAII side-effects (file deletion upon dropping), while
    // extending their lifetimes to persist until the end of the function.
    let _cmdline_file = if let Some(c) = cmdline {
        let file = NamedTempFile::new().map_err(|e| bug!("{e}"))?;
        fs::write(&file, c).map_err(|e| bug!("{e}"))?;
        zbi_command.arg("--type=cmdline").arg(file.path());
        Some(file)
    } else {
        None
    };

    // Note: For ZBI boots, the emulator injects the serial number here as a ZBI boot item.
    // If the serial number is explicitly disabled or legacy, we do not inject it.
    let _serial_file = if let Some(serial) = serial_number {
        let file = NamedTempFile::new().map_err(|e| bug!("{e}"))?;
        fs::write(&file, serial).map_err(|e| bug!("{e}"))?;
        zbi_command.arg("--type=SERIAL_NUMBER").arg(file.path());
        Some(file)
    } else {
        None
    };

    // added last.
    zbi_command.arg("--type=entropy:64").arg("/dev/urandom");

    let zbi_command_output = zbi_command.output().map_err(|e| bug!("{e}"))?;

    if !zbi_command_output.status.success() {
        return_bug!(
            "Error embedding boot data: {}",
            String::from_utf8_lossy(&zbi_command_output.stderr)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn test_authorized_keys_to_boot_loader_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("authorized_keys");
        let dst = dir.path().join("bootloader_file");

        std::fs::write(&src, b"ssh-ed25519 AAAAC3... test key").unwrap();
        authorized_keys_to_boot_loader_file(&src, &dst).unwrap();

        let bytes = std::fs::read(&dst).unwrap();
        assert_eq!(bytes[0], SSH_BOOTLOADER_FILE_NAME.len() as u8);
        assert_eq!(
            &bytes[1..1 + SSH_BOOTLOADER_FILE_NAME.len()],
            SSH_BOOTLOADER_FILE_NAME.as_bytes()
        );
        assert_eq!(&bytes[1 + SSH_BOOTLOADER_FILE_NAME.len()..], b"ssh-ed25519 AAAAC3... test key");
    }

    #[fuchsia::test]
    fn test_embed_boot_data_same_src_dest() {
        let env = ffx_config::test_env().build().unwrap();
        let path = Path::new("/fake/path.zbi");
        let result = embed_boot_data(&env.context, path, path, None, None);
        assert!(result.is_err());
    }
}
