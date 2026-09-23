// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utilities for preparing GPT full disk images for UEFI-based targets.

use camino::Utf8PathBuf;
use ffx_config::EnvironmentContext;
use ffx_config::environment::ExecutableKind;
use fho::{Result, return_bug, user_error};
use make_fuchsia_vol::args::{
    ABR_SIZE, Arch, EFI_SIZE, SYSTEM_PART_SIZE, TopLevel as MakeFuchsiaVolCmd, VBMETA_SIZE,
};
use sdk_metadata::CpuArchitecture;
use std::path::{Path, PathBuf};

pub const DEFAULT_ZEDBOOT_CMDLINE: &str = r#"bootloader.default=default
bootloader.timeout=5"#;

/// Default image size for emulator runs (20 GiB).
// By default, the image will be resized to 20G.
// TODO(https://fxbug.dev/380879811): Calculate this dynamically.
pub const DEFAULT_EMU_DISK_SIZE: u64 = 20 * 1024 * 1024 * 1024;

/// Holds the args needed to construct a full GPT disk image
#[derive(Debug, PartialEq)]
pub struct FuchsiaFullDiskImageBuilder {
    /// Architecture of the image to be generated
    arch: CpuArchitecture,
    /// Path to the zedboot command line file
    cmdline: Option<PathBuf>,
    /// Product bundle path. If provided, make_fuchsia_vol will look up the product name there.
    product_bundle: Option<PathBuf>,
    /// Path to mkfs-msdosfs. If provided, make_fuchsia_vol will use it instead of trying to
    /// derive it from the build output directory.
    mkfs_msdosfs_path: Option<PathBuf>,
    /// Output file of the disk image (full path)
    output_path: PathBuf,
    /// Create or resize the image to this size in bytes
    resize: Option<u64>,
    /// Use fxfs to store the base system
    use_fxfs: bool,
    /// Path to the ZBI. If not provided, make_fuchsia_vol will look it up from the product bundle.
    zbi: Option<PathBuf>,
    /// Path to the vbmeta, needs to be provided if the ZBI was modified from the one in the bundle.
    vbmeta: Option<PathBuf>,
}

impl Default for FuchsiaFullDiskImageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn convert_arch(arch: CpuArchitecture) -> Result<Arch> {
    match arch {
        CpuArchitecture::Arm64 => Ok(Arch::Arm64),
        CpuArchitecture::X64 => Ok(Arch::X64),
        a @ _ => return_bug!("arch {:?} is not supported yet for full disk GPT images", a),
    }
}

fn to_utf8_path_buf(path: PathBuf, label: &str) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(path)
        .map_err(|p| user_error!("error converting {label} path to UTF-8: {p:?}"))
}

fn to_opt_utf8_path_buf(path: Option<PathBuf>, label: &str) -> Result<Option<Utf8PathBuf>> {
    path.map(|p| to_utf8_path_buf(p, label)).transpose()
}

impl FuchsiaFullDiskImageBuilder {
    pub fn new() -> Self {
        Self {
            arch: CpuArchitecture::default(),
            cmdline: None,
            product_bundle: None,
            mkfs_msdosfs_path: None,
            output_path: PathBuf::default(),
            resize: None,
            use_fxfs: true,
            zbi: None,
            vbmeta: None,
        }
    }

    pub fn build(self, context: &EnvironmentContext) -> Result<()> {
        // TODO(https://fxbug.dev/380065101): When we have a fake make-fuchsia-vol, this should be
        // removed to increase test coverage.
        if context.exe_kind() == ExecutableKind::Test {
            log::debug!(
                "Building full GPT images as part of a test case is not supported yet, skipping."
            );
            return Ok(());
        }

        let mkfs_msdosfs = match self.mkfs_msdosfs_path {
            Some(path) => Some(to_utf8_path_buf(path, "mkfs-msdosfs")?),
            None => {
                let tool = ffx_config::get_host_tool(context, "mkfs-msdosfs")
                    .map_err(|e| user_error!("cannot locate mkfs-msdosfs tool: {e}"))?;
                Some(to_utf8_path_buf(tool, "mkfs-msdosfs")?)
            }
        };
        let vbmeta = to_opt_utf8_path_buf(self.vbmeta, "vbmeta")?;
        let cmd = MakeFuchsiaVolCmd {
            abr_size: ABR_SIZE,
            arch: convert_arch(self.arch)?,
            cmdline: to_opt_utf8_path_buf(self.cmdline, "cmdline")?,
            disk_path: to_utf8_path_buf(self.output_path, "image output")?,
            efi_size: EFI_SIZE,
            mkfs_msdosfs,
            product_bundle: to_opt_utf8_path_buf(self.product_bundle, "product bundle")?,
            resize: self.resize,
            use_fxfs: self.use_fxfs,
            vbmeta_a: vbmeta.clone(),
            vbmeta_b: vbmeta,
            vbmeta_size: VBMETA_SIZE,
            system_disk_size: Some(SYSTEM_PART_SIZE),
            // make_fuchsia_vol uses zbi for both A and B partitions unless specified otherwise:
            // //tools/make-fuchsia-vol/src/args.rs
            zbi: to_opt_utf8_path_buf(self.zbi, "zbi")?,
            ..Default::default()
        };
        Ok(make_fuchsia_vol::run(cmd)?)
    }

    pub fn arch(mut self, arch: CpuArchitecture) -> Self {
        self.arch = arch;
        self
    }

    pub fn cmdline(mut self, path: &Path) -> Self {
        self.cmdline = Some(path.to_path_buf());
        self
    }

    pub fn mkfs_msdosfs_path(mut self, path: &Path) -> Self {
        self.mkfs_msdosfs_path = Some(path.to_path_buf());
        self
    }

    pub fn output_path(mut self, path: &Path) -> Self {
        self.output_path = path.to_path_buf();
        self
    }

    pub fn product_bundle(mut self, path: &Path) -> Self {
        self.product_bundle = Some(path.to_path_buf());
        self
    }

    pub fn resize(mut self, size: u64) -> Self {
        self.resize = Some(size);
        self
    }

    pub fn use_fxfs(mut self, fxfs: bool) -> Self {
        self.use_fxfs = fxfs;
        self
    }

    pub fn vbmeta(mut self, path: Option<PathBuf>) -> Self {
        self.vbmeta = path;
        self
    }

    pub fn zbi(mut self, path: Option<PathBuf>) -> Self {
        self.zbi = path;
        self
    }
}

/// Writes the Zedboot bootloader command line to `path`, using `DEFAULT_ZEDBOOT_CMDLINE` if `cmd` is `None`.
pub fn write_zedboot_cmdline(path: &Path, cmd: Option<&str>) -> Result<()> {
    let cmdline = if let Some(c) = cmd { c } else { DEFAULT_ZEDBOOT_CMDLINE };

    std::fs::write(path, cmdline).map_err(|e| user_error!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile;

    #[fuchsia::test]
    fn test_create_default_cmdline_file_in_designated_location() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("cmdline");
        write_zedboot_cmdline(&f, None).unwrap();
        let cmdline = std::fs::read_to_string(f).unwrap();
        assert_eq!(cmdline, DEFAULT_ZEDBOOT_CMDLINE);
    }

    #[fuchsia::test]
    fn test_construct_default_make_fuchsia_vol_argument() {
        let builder = FuchsiaFullDiskImageBuilder::new();
        assert_eq!(builder, FuchsiaFullDiskImageBuilder::default());
        assert!(builder.cmdline.is_none());
        assert!(builder.product_bundle.is_none());
        assert!(builder.vbmeta.is_none());
        assert!(builder.zbi.is_none());
        assert!(builder.use_fxfs);
        assert_eq!(builder.arch, CpuArchitecture::X64);
        assert_eq!(builder.output_path, PathBuf::new());
        assert_eq!(builder.resize, None);
    }

    #[fuchsia::test]
    fn test_construct_custom_make_fuchsia_vol_argument() {
        const CMDLINE: &str = "/path/to/cmdline";
        const PRODUCT_BUNDLE_PATH: &str = "/path/to/product_bundle";
        const MKFS_MSDOSFS_PATH: &str = "/path/to/mkfs-msdosfs";
        const OUTPUT_PATH: &str = "/path/to/output";
        const VBMETA_PATH: &str = "/path/to/vbmeta";
        const ZBI_PATH: &str = "/path/to/zbi";
        let builder = FuchsiaFullDiskImageBuilder::new()
            .arch(CpuArchitecture::Arm64)
            .cmdline(&PathBuf::from(CMDLINE))
            .output_path(&PathBuf::from(OUTPUT_PATH))
            .mkfs_msdosfs_path(&PathBuf::from(MKFS_MSDOSFS_PATH))
            .product_bundle(&PathBuf::from(PRODUCT_BUNDLE_PATH))
            .vbmeta(Some(VBMETA_PATH.into()))
            .zbi(Some(ZBI_PATH.into()))
            .resize(1000)
            .use_fxfs(false);
        assert!(!builder.use_fxfs);
        assert_eq!(builder.arch, CpuArchitecture::Arm64);
        assert_eq!(builder.cmdline, Some(PathBuf::from(CMDLINE)));
        assert_eq!(builder.output_path, PathBuf::from(OUTPUT_PATH));
        assert_eq!(builder.mkfs_msdosfs_path, Some(PathBuf::from(MKFS_MSDOSFS_PATH)));
        assert_eq!(builder.product_bundle, Some(PRODUCT_BUNDLE_PATH.into()));
        assert_eq!(builder.vbmeta, Some(VBMETA_PATH.into()));
        assert_eq!(builder.zbi, Some(ZBI_PATH.into()));
        assert_eq!(builder.resize, Some(1000));
    }
}
