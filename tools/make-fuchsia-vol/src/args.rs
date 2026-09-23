// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use camino::Utf8PathBuf;
use std::str::FromStr;

pub const ABR_SIZE: u64 = 256 * 1024 * 1024;
pub const EFI_SIZE: u64 = 63 * 1024 * 1024;
pub const SYSTEM_PART_SIZE: u64 = 16 * 1024 * 1024 * 1024;
pub const VBMETA_SIZE: u64 = 64 * 1024;

#[derive(Debug, PartialEq)]
pub enum Arch {
    X64,
    Arm64,
}

impl FromStr for Arch {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "x64" => Ok(Arch::X64),
            "arm64" => Ok(Arch::Arm64),
            _ => Err("Expected x64 or arm64".to_string()),
        }
    }
}

#[cfg(target_arch = "x86_64")]
impl Default for Arch {
    fn default() -> Self {
        Arch::X64
    }
}

#[cfg(target_arch = "aarch64")]
impl Default for Arch {
    fn default() -> Self {
        Arch::Arm64
    }
}

#[derive(Debug)]
pub enum BootPart {
    BootA,
    BootB,
    BootR,
}

impl Default for BootPart {
    fn default() -> Self {
        BootPart::BootA
    }
}

impl FromStr for BootPart {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "a" => Ok(BootPart::BootA),
            "b" => Ok(BootPart::BootB),
            "r" => Ok(BootPart::BootR),
            _ => Err("Expected a, b or r".to_string()),
        }
    }
}

/// make-fuchsia-vol command line arguments
#[derive(FromArgs, Debug, Default)]
pub struct TopLevel {
    /// disk-path
    #[argh(positional)]
    pub disk_path: Utf8PathBuf,

    /// enable verbose logging
    #[argh(switch)]
    pub verbose: bool,

    /// fuchsia build dir
    #[argh(option)]
    pub fuchsia_build_dir: Option<Utf8PathBuf>,

    /// use named product information from Product Bundle Metadata (PBM). If no
    /// product bundle is specified and there is an obvious choice, that will be
    /// used (e.g. if there is only one PBM available).
    #[argh(option)]
    pub product_bundle: Option<Utf8PathBuf>,

    /// the name of a particular product bundle to use, consulted if
    /// --product-bundle is unset.
    #[argh(option)]
    pub product_bundle_name: Option<String>,

    /// the path to the mkfs-msdosfs tool. If this is not specified, make-fuchsia-vol will try
    /// to derive it from fuchsia_build_dir and/or environment variables.
    #[argh(option)]
    pub mkfs_msdosfs: Option<Utf8PathBuf>,

    /// the architecture of the target CPU (x64|arm64)
    #[argh(option, default = "Arch::X64")]
    pub arch: Arch,

    /// the architecture of the host CPU (x64|arm64)
    #[argh(option, default = "Arch::default()")]
    pub host_arch: Arch,

    /// path to fuchsia-efi.efi
    #[argh(option)]
    pub bootloader: Option<Utf8PathBuf>,

    /// path to zbi (default: zircon-a from image manifests)
    #[argh(option)]
    pub zbi: Option<Utf8PathBuf>,

    /// path to command line file (if exists)
    #[argh(option)]
    pub cmdline: Option<Utf8PathBuf>,

    /// path to zedboot.zbi (default: zircon-r from image manifests)
    #[argh(option)]
    pub zedboot: Option<Utf8PathBuf>,

    /// ramdisk-only mode - only write an ESP partition
    #[argh(switch)]
    pub ramdisk_only: bool,

    /// path to blob partition image (not used with ramdisk)
    #[argh(option)]
    pub blob: Option<Utf8PathBuf>,

    /// if true, use sparse fvm instead of full fvm
    #[argh(switch)]
    pub use_sparse_fvm: bool,

    /// path to sparse FVM image (default: storage-sparse from image manifests)
    #[argh(option)]
    pub sparse_fvm: Option<Utf8PathBuf>,

    /// don't add Zircon-{{A,B,R}} partitions
    #[argh(switch)]
    pub no_abr: bool,

    /// path to partition image for Zircon-A (default: from --zbi)
    #[argh(option)]
    pub zircon_a: Option<Utf8PathBuf>,

    /// path to partition image for Vbmeta-A
    #[argh(option)]
    pub vbmeta_a: Option<Utf8PathBuf>,

    /// path to partition image for Zircon-B (default: from --zbi)
    #[argh(option)]
    pub zircon_b: Option<Utf8PathBuf>,

    /// path to partition image for Vbmeta-B
    #[argh(option)]
    pub vbmeta_b: Option<Utf8PathBuf>,

    /// path to partition image for Zircon-R (default: zircon-r from image manifests)
    #[argh(option)]
    pub zircon_r: Option<Utf8PathBuf>,

    /// path to partition image for Vbmeta-R
    #[argh(option)]
    pub vbmeta_r: Option<Utf8PathBuf>,

    /// kernel partition size for A/B/R
    #[argh(option, default = "ABR_SIZE")]
    pub abr_size: u64,

    /// partition size for vbmeta A/B/R
    #[argh(option, default = "VBMETA_SIZE")]
    pub vbmeta_size: u64,

    /// the A/B/R partition to boot by default
    #[argh(option, default = "BootPart::default()")]
    pub abr_boot: BootPart,

    /// the block size of the target disk
    #[argh(option)]
    pub block_size: Option<u64>,

    /// efi partition size in bytes
    #[argh(option, default = "EFI_SIZE")]
    pub efi_size: u64,

    /// system (i.e. FVM or Fxfs) disk partition size in bytes (unspecified means `fill`)
    #[argh(option)]
    pub system_disk_size: Option<u64>,

    /// create or resize the image to this size in bytes
    #[argh(option)]
    pub resize: Option<u64>,

    /// a seed from which the UUIDs are derived; this will also set any timestamps used to fixed
    /// values, so the resulting image should be reproducible (only suitable for TESTING).
    #[argh(option)]
    pub seed: Option<String>,

    /// if true, write a dependency file which will be the same as the output path but with a '.d'
    /// extension.
    #[argh(switch)]
    pub depfile: bool,

    /// whether to use Fxfs instead of FVM to store the base system
    #[argh(switch)]
    pub use_fxfs: bool,

    /// path to Fxfs partition image (if --use_fxfs is set)
    #[argh(option)]
    pub fxfs: Option<Utf8PathBuf>,

    /// contents to put in a "qemu-commandline" partition
    #[argh(option)]
    pub qemu_commandline: Option<String>,
}
