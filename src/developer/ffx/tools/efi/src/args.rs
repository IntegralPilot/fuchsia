// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use sdk_metadata::CpuArchitecture;

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "efi", description = "Manipulate efi partition")]
pub struct EfiCommand {
    #[argh(subcommand)]
    pub subcommand: EfiSubCommand,
}

#[derive(ArgsInfo, FromArgs, Clone, PartialEq, Debug)]
#[argh(subcommand)]
pub enum EfiSubCommand {
    Create(CreateCommand),
}

#[derive(ArgsInfo, FromArgs, Clone, PartialEq, Debug)]
/// Creates efi partition, copies zircon.bin, bootdata.bin, the efi bootloader file, zedboot.bin,
/// etc...
#[argh(subcommand, name = "create")]
pub struct CreateCommand {
    #[argh(option, short = 'o')]
    /// target file/disk to write EFI partition to
    pub output: String,
    /// optional path to source file for zircon.bin
    #[argh(option)]
    pub zircon: Option<String>,
    /// optional path to source file for bootdata.bin
    #[argh(option)]
    pub bootdata: Option<String>,
    /// cpu architecture of the target
    #[argh(option, default = "CpuArchitecture::X64")]
    pub arch: CpuArchitecture,
    /// optional path to source file for the efi bootloader file (e.g. EFI/BOOT/BOOTX64.EFI for X64)
    #[argh(option)]
    pub efi_bootloader: Option<String>,
    /// optional path to a source file for zedboot.bin
    #[argh(option)]
    pub zedboot: Option<String>,
    /// optional bootloader cmdline file
    #[argh(option)]
    pub cmdline: Option<String>,
}
