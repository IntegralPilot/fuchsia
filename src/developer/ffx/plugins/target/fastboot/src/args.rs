// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use core::num::NonZeroU64;
use ffx_core::ffx_command;
use std::path::PathBuf;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "fastboot",
    description = "Perform fastboot operations on a target device",
    example = "To flash a specific image:

    $ ffx target fastboot flash super foo.zbi"
)]
pub struct FastbootCommand {
    #[argh(subcommand)]
    pub subcommand: FastbootSubcommand,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
#[argh(subcommand)]
pub enum FastbootSubcommand {
    Flash(FlashSubcommand),
    GetVar(GetVarSubcommand),
    Stage(StageSubcommand),
    Oem(OemSubcommand),
    Reboot(RebootSubcommand),
    Continue(ContinueSubcommand),
    Sparse(SparseSubcommand),
    Authorize(AuthorizeSubcommand),
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Flash subcommand
#[argh(subcommand, name = "flash")]
pub struct FlashSubcommand {
    #[argh(positional)]
    /// which partition
    pub partition: String,

    #[argh(positional)]
    /// what file to flash
    pub file: PathBuf,

    #[argh(option, short = 'S')]
    /// overriding the maximum download size
    pub max_download_size: Option<NonZeroU64>,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Sparse subcommand.
#[argh(
    subcommand,
    name = "sparse",
    note = "Takes the provided file and out_dir and breaks the input file into
a set of files named in format n-tmp.img, each of which is no larger than
`size` (defaulting to Target's max download size) and is in the Android
Sparse Image Format."
)]
pub struct SparseSubcommand {
    #[argh(option)]
    /// size to split the image into. If unset defaults to target's max
    /// download size
    pub size: Option<u64>,

    #[argh(positional)]
    /// what file to flash
    pub file: PathBuf,

    #[argh(positional)]
    /// where to put the sparse images
    pub out_dir: PathBuf,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Authorize subcommand.
#[argh(
    subcommand,
    name = "authorize",
    note = "Sends a Fuchsia-specific OEM command to the target to upload the
ssh authorized keys and continue boot for the Target."
)]
pub struct AuthorizeSubcommand {
    #[argh(option, short = 'p')]
    /// path to a product bundle to extract SSH key upload method from
    pub product_bundle: Option<String>,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Get Variable subcommand
#[argh(subcommand, name = "getvar")]
pub struct GetVarSubcommand {
    #[argh(positional)]
    /// variable to get (special case for `all`)
    pub var_name: String,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Stage subcommand
#[argh(subcommand, name = "stage")]
pub struct StageSubcommand {
    #[argh(positional)]
    /// what file to upload
    pub file: PathBuf,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Oem subcommand
#[argh(subcommand, name = "oem")]
pub struct OemSubcommand {
    #[argh(positional, greedy)]
    /// oem-specific command to run
    pub command: Vec<String>,
}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Continue subcommand
#[argh(subcommand, name = "continue")]
pub struct ContinueSubcommand {}

#[derive(ArgsInfo, FromArgs, Eq, PartialEq, Clone, Debug)]
/// Reboot subcommand
#[argh(subcommand, name = "reboot")]
pub struct RebootSubcommand {
    #[argh(positional)]
    /// state to reboot to (e.g. bootloader)
    pub bootloader: Option<String>,
}

#[cfg(test)]
mod test {}
