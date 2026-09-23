// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_config::{EnvironmentContext, FfxConfigBacked};
use ffx_core::ffx_command;
use ffx_flash_manifest::{Command, ManifestParams, OemFile};
use std::default::Default;
use std::path::PathBuf;

#[ffx_command()]
#[derive(FfxConfigBacked, ArgsInfo, FromArgs, Default, Clone, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "flash",
    description = "Flash an image to a target device",
    example = "To flash a specific image:

    $ ffx target flash --manifest $(fx get-build-dir)/flash.json --product fuchsia

To include SSH keys as well:

    $ ffx target flash
    --authorized-keys ~/fuchsia/.ssh/authorized_keys
    $(fx get-build-dir)/flash.json
    --product fuchsia",
    note = "Flashes an image to a target device using the fastboot protocol.
Requires a specific <manifest> file and <product> name as an input.

This is only applicable to a physical device and not an emulator target.
The target device is typically connected via a micro-USB connection to
the host system.

The <manifest> format is a JSON file generated when building a fuchsia
<product> and can be found in the build output directory.

The `--oem-stage` option can be supplied multiple times for several OEM
files. The format expects a single OEM command to execute after staging
the given file.

The format for the `--oem-stage` parameter is a comma separated pair:
'<OEM_COMMAND>,<FILE_TO_STAGE>'"
)]
pub struct FlashCommand {
    #[argh(
        positional,
        description = "path to flashing manifest or zip file containing images and manifest"
    )]
    pub manifest_path: Option<PathBuf>,

    #[argh(
        option,
        short = 'p',
        description = "product entry in manifest - defaults to `fuchsia`",
        default = "String::from(\"fuchsia\")"
    )]
    pub product: String,

    #[argh(option, short = 'b', description = "optional product bundle name")]
    pub product_bundle: Option<String>,

    #[argh(option, short = 'm', description = "optional manifest path")]
    pub manifest: Option<PathBuf>,

    #[argh(option, description = "oem staged file - can be supplied multiple times")]
    pub oem_stage: Vec<OemFile>,

    #[argh(
        option,
        description = "path to authorized keys file - will default to the value configured for \
           `ssh.pub` key in ffx config. If the file does not exist, it will be created."
    )]
    pub authorized_keys: Option<String>,

    #[argh(
        switch,
        description = "the device should not reboot after bootloader images are flashed"
    )]
    pub no_bootloader_reboot: bool,

    #[argh(
        switch,
        description = "skip hardware verification. This is dangerous, please be sure the images you are flashing match the device"
    )]
    pub skip_verify: bool,

    #[argh(option, description = "flash timeout rate in mb/second.")]
    #[ffx_config_default(key = "fastboot.flash.timeout_rate", default = "2")]
    pub timeout_rate: Option<f64>,

    #[argh(
        option,
        description = "minimum timeout in seconds to wait while flashing per-partition"
    )]
    #[ffx_config_default(key = "fastboot.flash.min_timeout_secs", default = "60")]
    pub min_timeout_secs: Option<u64>,

    #[argh(
        switch,
        description = "skip uploading ssh authorized keys. This is dangerous, you will be unable to communicate with the target via ffx.",
        hidden_help
    )]
    pub skip_authorized_keys: bool,
}

impl FlashCommand {
    pub fn to_manifest(self, context: &EnvironmentContext) -> ManifestParams {
        let flash_min_timeout_seconds = self.min_timeout_secs(context).ok().unwrap();
        let flash_timeout_rate_mb_per_second = self.timeout_rate(context).ok().unwrap();
        let manifest = self.manifest.or(self.manifest_path);
        ManifestParams {
            manifest,
            product: self.product.clone(),
            product_bundle: self.product_bundle,
            oem_stage: self.oem_stage,
            skip_verify: self.skip_verify,
            no_bootloader_reboot: self.no_bootloader_reboot,
            op: Command::Flash,
            flash_min_timeout_seconds,
            flash_timeout_rate_mb_per_second,
            ssh_key: self.authorized_keys,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::Result;
    use tempfile::NamedTempFile;

    #[test]
    fn test_oem_staged_file_from_str() -> Result<()> {
        let test_oem_cmd = "test-oem-cmd";
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let test_staged_file = format!("{},{}", test_oem_cmd, tmp_file_name).parse::<OemFile>()?;
        assert_eq!(test_oem_cmd, test_staged_file.command());
        assert_eq!(tmp_file_name, test_staged_file.file());
        Ok(())
    }

    #[test]
    fn test_oem_staged_file_from_str_fails_with_nonexistent_file() {
        let test_oem_cmd = "test-oem-cmd";
        let tmp_file_name = "/fake/test/for/testing/that/should/not/exist";
        let test_staged_file = format!("{},{}", test_oem_cmd, tmp_file_name).parse::<OemFile>();
        assert!(test_staged_file.is_err());
    }

    #[test]
    fn test_oem_staged_file_from_str_fails_with_malformed_string() {
        let test_oem_cmd = "test-oem-cmd";
        let tmp_file_name = "/fake/test/for/testing/that/should/not/exist";
        let test_staged_file = format!("{}..{}", test_oem_cmd, tmp_file_name).parse::<OemFile>();
        assert!(test_staged_file.is_err());
    }

    #[test]
    fn test_oem_staged_file_from_str_fails_with_empty_string() {
        let test_staged_file = "".parse::<OemFile>();
        assert!(test_staged_file.is_err());
    }

    #[test]
    fn test_oem_staged_files_are_in_manifest_params() -> Result<()> {
        let env = ffx_config::test_env().build()?;
        let test_oem_cmd = "test-oem-cmd";
        let tmp_file = NamedTempFile::new().expect("tmp access failed");
        let tmp_file_name = tmp_file.path().to_string_lossy().to_string();
        let test_staged_file = format!("{},{}", test_oem_cmd, tmp_file_name).parse::<OemFile>()?;
        let cmd = FlashCommand {
            manifest_path: None,
            manifest: None,
            product: "fuchsia".to_string(),
            product_bundle: None,
            authorized_keys: None,
            no_bootloader_reboot: false,
            skip_verify: false,
            oem_stage: vec![test_staged_file],
            min_timeout_secs: Some(100),
            timeout_rate: Some(1.0),
            skip_authorized_keys: false,
        };

        let params: ManifestParams = cmd.to_manifest(&env.context);
        assert_eq!(params.oem_stage[0].file(), tmp_file_name);
        assert_eq!(params.oem_stage[0].command(), test_oem_cmd);
        Ok(())
    }
}
