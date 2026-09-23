// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::base_package::BasePackage;
use crate::blobfs::construct_blobfs;
use crate::{AssembledSystem, Image};

use anyhow::{Context, Result};
use assembly_fvm::{Filesystem, FilesystemAttributes, FvmBuilder, FvmType, NandFvmBuilder};
use assembly_images_config::{Fvm, FvmFilesystem, FvmOutput, SparseFvm};
use assembly_tool::ToolProvider;
use camino::{Utf8Path, Utf8PathBuf};
use image_assembly_config::ImageAssemblyConfig;
use log::info;
use std::collections::HashMap;
use utf8_path::path_relative_from_current_dir;

/// Constructs up-to four FVM files. Calling this function generates
/// a default FVM, a sparse FVM, a sparse blob-only FVM, and optionally a FVM
/// ready for fastboot flashing. This function returns the paths to each
/// generated FVM.
///
/// If the |fvm_config| includes information for an EMMC, then an EMMC-supported
/// sparse FVM will also be generated for fastboot flashing.
///
/// If the |fvm_config| includes information for a NAND, then an NAND-supported
/// sparse FVM will also be generated for fastboot flashing.
#[allow(clippy::too_many_arguments)]
// Update construct_fvm function to return an fvm image path to be added to the disk image.
pub fn construct_fvm(
    gendir: impl AsRef<Utf8Path>,
    tools: &impl ToolProvider,
    assembled_system: &mut AssembledSystem,
    assembly_config: &ImageAssemblyConfig,
    fvm_config: Fvm,
    compress_blobfs: bool,
    include_account: bool,
    base_package: &BasePackage,
) -> Result<Option<Utf8PathBuf>> {
    let mut builder = MultiFvmBuilder::new(
        gendir,
        assembly_config,
        assembled_system,
        compress_blobfs,
        fvm_config.slice_size,
        include_account,
        base_package,
    );
    for filesystem in fvm_config.filesystems {
        builder.filesystem(filesystem);
    }
    for output in fvm_config.outputs {
        builder.output(output);
    }
    builder.build(tools)
}

/// A builder that can produce multiple FVMs of various types in a single step. This is useful when
/// multiple fvms must be produced that share the same underlying filesystem, but we do not want
/// the cost of generating the filesystem multiple times.
pub struct MultiFvmBuilder<'a> {
    /// Map from the name of the filesystem to its entry.
    filesystems: HashMap<String, FilesystemEntry>,
    /// List of the FVMs to generate.
    outputs: Vec<FvmOutput>,
    /// The directory to write the intermediate outputs into.
    gendir: Utf8PathBuf,
    /// The image assembly config.
    assembly_config: &'a ImageAssemblyConfig,
    /// The manifest of images to add new FVMs to.
    assembled_system: &'a mut AssembledSystem,
    /// Whether blobfs should be compressed.
    compress_blobfs: bool,
    /// The size of a slice for the FVM.
    slice_size: u64,
    /// Whether to include an account partition in the FVMs.
    include_account: bool,
    /// The base package to add to blobfs.
    base_package: &'a BasePackage,
}

/// A single filesystem that can be added to the FVMs.
/// This is either the params to generate the filesystem, or the struct that contains how to use
/// the generated filesystem.
pub enum FilesystemEntry {
    Params(FvmFilesystem),
    Filesystem(Filesystem),
}

impl<'a> MultiFvmBuilder<'a> {
    /// Construct a new MultiFvmBuilder.
    /// These parameters are constant across all generated FVMs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gendir: impl AsRef<Utf8Path>,
        assembly_config: &'a ImageAssemblyConfig,
        assembled_system: &'a mut AssembledSystem,
        compress_blobfs: bool,
        slice_size: u64,
        include_account: bool,
        base_package: &'a BasePackage,
    ) -> Self {
        Self {
            filesystems: HashMap::new(),
            outputs: Vec::new(),
            gendir: gendir.as_ref().to_path_buf(),
            assembly_config,
            assembled_system,
            compress_blobfs,
            slice_size,
            include_account,
            base_package,
        }
    }

    /// Add a `filesystem` to the FVM.
    pub fn filesystem(&mut self, filesystem: FvmFilesystem) {
        let name = match &filesystem {
            FvmFilesystem::BlobFS(fs) => &fs.name,
            FvmFilesystem::EmptyData(fs) => &fs.name,
            FvmFilesystem::Reserved(fs) => &fs.name,
        };
        self.filesystems.insert(name.clone(), FilesystemEntry::Params(filesystem));
    }

    /// Add an `output` to be generated.
    pub fn output(&mut self, output: FvmOutput) {
        self.outputs.push(output);
    }

    /// Build all the FVM outputs.
    pub fn build(&mut self, tools: &impl ToolProvider) -> Result<Option<Utf8PathBuf>> {
        let outputs = self.outputs.clone();
        let mut fvm_disk_image: Option<Utf8PathBuf> = None;
        for output in outputs {
            let image_path = self
                .build_output_and_add_to_manifest(tools, &output)
                .with_context(|| format!("Building {output}"))?;

            if fvm_disk_image.is_none() {
                fvm_disk_image = image_path;
            }
        }
        Ok(fvm_disk_image)
    }

    /// Build a single FVM output, and always add the result to the |assembled_system|.
    fn build_output_and_add_to_manifest(
        &mut self,
        tools: &impl ToolProvider,
        output: &FvmOutput,
    ) -> Result<Option<Utf8PathBuf>> {
        let add_to_manifest = true;
        self.build_output(tools, output, add_to_manifest)
    }

    /// Build a single FVM output, and let the caller choose whether to add it to the
    /// |assembled_system|.
    fn build_output(
        &mut self,
        tools: &impl ToolProvider,
        output: &FvmOutput,
        add_to_manifest: bool,
    ) -> Result<Option<Utf8PathBuf>> {
        match &output {
            FvmOutput::Standard(config) => {
                let fvm_tool = tools.get_tool("fvm")?;
                let path = self.gendir.join(format!("{}.blk", config.name));
                let fvm_type = FvmType::Standard {
                    resize_image_file_to_fit: config.resize_image_file_to_fit,
                    truncate_to_length: config.truncate_to_length,
                };
                let mut builder = FvmBuilder::new(
                    fvm_tool,
                    &path,
                    self.slice_size,
                    self.include_account,
                    config.compress,
                    fvm_type,
                );
                for filesystem_name in &config.filesystems {
                    let fs = self
                        .get_filesystem(tools, filesystem_name)
                        .with_context(|| format!("Including filesystem: {}", filesystem_name))?;
                    builder.filesystem(fs);
                }
                builder.build()?;
                let mut image_path = None;
                if add_to_manifest {
                    let path_relative = path_relative_from_current_dir(path)?;
                    let image = match config.name.as_str() {
                        // Even though this is a standard FVM, people expect it to find it using
                        // the fvm.fastboot key in the AssembledSystem.
                        "fvm.fastboot" => Image::FVMFastboot(path_relative),
                        _ => {
                            image_path = Some(path_relative.clone());
                            Image::FVM(path_relative)
                        }
                    };
                    self.assembled_system.images.push(image);
                }
                Ok(image_path)
            }
            FvmOutput::Sparse(config) => {
                let fvm_tool = tools.get_tool("fvm")?;
                let path = self.gendir.join(format!("{}.blk", config.name));
                let fvm_type = FvmType::Sparse { max_disk_size: config.max_disk_size };
                let compress = true;
                let mut builder = FvmBuilder::new(
                    fvm_tool,
                    &path,
                    self.slice_size,
                    self.include_account,
                    compress,
                    fvm_type,
                );

                for filesystem_name in &config.filesystems {
                    let fs = self.get_filesystem(tools, filesystem_name)?;
                    builder.filesystem(fs);
                }

                builder.build()?;
                if add_to_manifest {
                    let path_relative = path_relative_from_current_dir(path)?;
                    self.assembled_system.images.push(Image::FVMSparse(path_relative));
                }
                Ok(None)
            }
            FvmOutput::Nand(config) => {
                // First, build the sparse FVM.
                let sparse_tmp_name = format!("{}.tmp", config.name);
                let sparse_output = FvmOutput::Sparse(SparseFvm {
                    name: sparse_tmp_name.clone(),
                    filesystems: config.filesystems.clone(),
                    max_disk_size: config.max_disk_size,
                });
                let do_not_add_to_manifest = false;
                self.build_output(tools, &sparse_output, do_not_add_to_manifest)?;

                // Second, prepare it for NAND.
                let tool = tools.get_tool("fvm")?;
                let sparse_output = self.gendir.join(format!("{}.blk", sparse_tmp_name));
                let output = self.gendir.join(format!("{}.blk", config.name));
                let compression = if config.compress { Some("lz4".to_string()) } else { None };
                let builder = NandFvmBuilder {
                    tool,
                    output: output.clone(),
                    sparse_fvm: sparse_output,
                    max_disk_size: config.max_disk_size,
                    compression,
                    page_size: config.page_size,
                    oob_size: config.oob_size,
                    pages_per_block: config.pages_per_block,
                    block_count: config.block_count,
                };
                builder.build()?;

                if add_to_manifest {
                    let path_relative = path_relative_from_current_dir(output)?;
                    self.assembled_system.images.push(Image::FVMFastboot(path_relative));
                }
                Ok(None)
            }
        }
    }

    /// Return the info for the filesystem identified by the |name|.
    /// Reuses prebuilt info if possible.
    /// Builds the filesystem if necessary.
    fn get_filesystem(&mut self, tools: &impl ToolProvider, name: &String) -> Result<Filesystem> {
        let entry = match self.filesystems.get(name) {
            Some(e) => e,
            _ => anyhow::bail!("Filesystem is not specified: {}", name),
        };

        match entry {
            // Return the already assembled info.
            FilesystemEntry::Filesystem(filesystem) => Ok(filesystem.clone()),
            // Build the filesystem and assemble the info.
            FilesystemEntry::Params(params) => {
                info!("Creating FVM filesystem: {}", name);
                let (image, filesystem) = self
                    .build_filesystem(tools, params)
                    .with_context(|| format!("Building filesystem: {name}"))?;
                if let Some(image) = image {
                    self.assembled_system.images.push(image);
                }
                self.filesystems
                    .insert(name.clone(), FilesystemEntry::Filesystem(filesystem.clone()));
                Ok(filesystem)
            }
        }
    }

    /// Build a filesystem and return the info to use it, and optionally the image metadata to
    /// insert into the image manifest.
    fn build_filesystem(
        &self,
        tools: &impl ToolProvider,
        params: &FvmFilesystem,
    ) -> Result<(Option<Image>, Filesystem)> {
        let (image, filesystem) = match &params {
            FvmFilesystem::BlobFS(config) => {
                let (path, contents) = construct_blobfs(
                    tools.get_tool("blobfs")?,
                    &self.gendir,
                    self.assembly_config,
                    config,
                    self.compress_blobfs,
                    self.base_package,
                )
                .context("Constructing blobfs")?;
                (
                    Some(Image::BlobFS {
                        path: path_relative_from_current_dir(path.clone())?,
                        contents,
                    }),
                    Filesystem::BlobFS {
                        path,
                        attributes: FilesystemAttributes {
                            name: config.name.clone(),
                            minimum_inodes: config.minimum_inodes,
                            minimum_data_bytes: config.minimum_data_bytes,
                            maximum_bytes: config.maximum_bytes,
                        },
                    },
                )
            }
            FvmFilesystem::EmptyData(_config) => (None, Filesystem::EmptyData {}),
            FvmFilesystem::Reserved(config) => {
                (None, Filesystem::Reserved { slices: config.slices })
            }
        };
        Ok((image, filesystem))
    }
}

#[cfg(test)]
mod tests {
    use super::MultiFvmBuilder;

    use crate::AssembledSystem;
    use crate::base_package::BasePackage;

    use assembly_images_config::{
        BlobFS, BlobfsLayout, EmptyData, FvmFilesystem, FvmOutput, NandFvm, Reserved, SparseFvm,
        StandardFvm,
    };
    use assembly_release_info::SystemReleaseInfo;
    use assembly_tool::testing::FakeToolProvider;
    use assembly_tool::{ToolCommandLog, ToolProvider};
    use camino::{Utf8Path, Utf8PathBuf};
    use image_assembly_config::ImageAssemblyConfig;
    use serde_json::json;
    use std::fs::File;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn construct_no_outputs() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let assembly_config = ImageAssemblyConfig::new_for_testing("path/to/kernel");

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let base_package =
            BasePackage { merkle: [0u8; 32].into(), manifest_path: Utf8PathBuf::default() };
        let include_account = false;
        let compress_blobfs = true;
        let slice_size = 0;
        let mut builder = MultiFvmBuilder::new(
            dir,
            &assembly_config,
            &mut assembled_system,
            compress_blobfs,
            slice_size,
            include_account,
            &base_package,
        );
        let tools = FakeToolProvider::default();
        builder.build(&tools).unwrap();

        let expected_log: ToolCommandLog = serde_json::from_value(json!({
            "commands": []
        }))
        .unwrap();
        assert_eq!(&expected_log, tools.log());
    }

    #[test]
    fn construct_standard_no_fs() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let assembly_config = ImageAssemblyConfig::new_for_testing("path/to/kernel");

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let base_package =
            BasePackage { merkle: [0u8; 32].into(), manifest_path: Utf8PathBuf::default() };
        let include_account = false;
        let compress_blobfs = true;
        let slice_size = 0;
        let mut builder = MultiFvmBuilder::new(
            dir,
            &assembly_config,
            &mut assembled_system,
            compress_blobfs,
            slice_size,
            include_account,
            &base_package,
        );
        builder.output(FvmOutput::Standard(StandardFvm {
            name: "fvm".into(),
            filesystems: Vec::new(),
            compress: false,
            resize_image_file_to_fit: false,
            truncate_to_length: None,
        }));
        let tools = FakeToolProvider::default();
        builder.build(&tools).unwrap();

        let output_path = dir.join("fvm.blk");
        let expected_log: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
            {
                "tool": "./host_x64/fvm",
                "args": [
                    output_path,
                    "create",
                    "--slice",
                    "0",
                ]
            }
            ]
        }))
        .unwrap();
        assert_eq!(&expected_log, tools.log());
    }

    #[test]
    fn construct_multiple_no_fs() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let assembly_config = ImageAssemblyConfig::new_for_testing("path/to/kernel");

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let base_package =
            BasePackage { merkle: [0u8; 32].into(), manifest_path: Utf8PathBuf::default() };
        let include_account = false;
        let compress_blobfs = true;
        let slice_size = 0;
        let mut builder = MultiFvmBuilder::new(
            dir,
            &assembly_config,
            &mut assembled_system,
            compress_blobfs,
            slice_size,
            include_account,
            &base_package,
        );
        builder.output(FvmOutput::Standard(StandardFvm {
            name: "fvm".into(),
            filesystems: Vec::new(),
            compress: false,
            resize_image_file_to_fit: false,
            truncate_to_length: None,
        }));
        builder.output(FvmOutput::Sparse(SparseFvm {
            name: "fvm.sparse".into(),
            filesystems: Vec::new(),
            max_disk_size: None,
        }));
        builder.output(FvmOutput::Nand(NandFvm {
            name: "fvm.nand".into(),
            filesystems: Vec::new(),
            max_disk_size: None,
            compress: false,
            block_count: 1,
            oob_size: 2,
            page_size: 3,
            pages_per_block: 4,
        }));
        let tools = FakeToolProvider::default();
        builder.build(&tools).unwrap();

        let standard_path = dir.join("fvm.blk");
        let sparse_path = dir.join("fvm.sparse.blk");
        let nand_tmp_path = dir.join("fvm.nand.tmp.blk");
        let nand_path = dir.join("fvm.nand.blk");
        let expected_log: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
            {
                "tool": "./host_x64/fvm",
                "args": [
                    standard_path,
                    "create",
                    "--slice",
                    "0",
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    sparse_path,
                    "sparse",
                    "--slice",
                    "0",
                    "--compress",
                    "lz4",
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    nand_tmp_path,
                    "sparse",
                    "--slice",
                    "0",
                    "--compress",
                    "lz4",
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    nand_path,
                    "ftl-raw-nand",
                    "--nand-page-size",
                    "3",
                    "--nand-oob-size",
                    "2",
                    "--nand-pages-per-block",
                    "4",
                    "--nand-block-count",
                    "1",
                    "--sparse",
                    nand_tmp_path,
                ]
            },
            ]
        }))
        .unwrap();
        assert_eq!(&expected_log, tools.log());
    }

    #[test]
    fn construct_standard_with_fs() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let assembly_config = ImageAssemblyConfig::new_for_testing("path/to/kernel");

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };

        let base_package_path = dir.join("base.far");
        let mut base_package_file = File::create(&base_package_path).unwrap();
        write!(base_package_file, "base package").unwrap();
        let base_package_manifest_path = dir.join("package_manifest.json");
        let mut base_package_manifest_file = File::create(&base_package_manifest_path).unwrap();
        let contents = r#"{
            "version": "1",
            "package": {
                "name": "system_image",
                "version": "0"
            },
            "blobs": []
        }
        "#;
        write!(base_package_manifest_file, "{contents}").unwrap();
        let base_package =
            BasePackage { merkle: [0u8; 32].into(), manifest_path: base_package_manifest_path };

        let include_account = true;
        let compress_blobfs = false;
        let slice_size = 0;
        let mut builder = MultiFvmBuilder::new(
            dir,
            &assembly_config,
            &mut assembled_system,
            compress_blobfs,
            slice_size,
            include_account,
            &base_package,
        );
        builder.filesystem(FvmFilesystem::BlobFS(BlobFS {
            name: "blob".into(),
            layout: BlobfsLayout::Compact,
            maximum_bytes: None,
            minimum_data_bytes: None,
            minimum_inodes: None,
            maximum_contents_size: None,
        }));
        builder.filesystem(FvmFilesystem::EmptyData(EmptyData { name: "empty-data".into() }));
        builder
            .filesystem(FvmFilesystem::Reserved(Reserved { name: "reserved".into(), slices: 10 }));
        builder.output(FvmOutput::Standard(StandardFvm {
            name: "fvm".into(),
            filesystems: vec!["blob".into(), "empty-data".into(), "reserved".into()],
            compress: false,
            resize_image_file_to_fit: false,
            truncate_to_length: None,
        }));
        let tools = FakeToolProvider::default();
        builder.build(&tools).unwrap();

        let blobfs_path = dir.join("blob.blk");
        let blobs_json_path = dir.join("blobs.json");
        let blob_manifest_path = dir.join("blob.manifest");
        let standard_path = dir.join("fvm.blk");
        let expected_log: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
            {
                "tool": "./host_x64/blobfs",
                "args": [
                    "--json-output",
                    blobs_json_path,
                    blobfs_path,
                    "create",
                    "--manifest",
                    blob_manifest_path,
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    standard_path,
                    "create",
                    "--slice",
                    "0",
                    "--blob",
                    blobfs_path,
                    "--with-empty-data",
                    "--reserve-slices",
                    "10",
                    "--with-empty-account-partition",
                ]
            }
            ]
        }))
        .unwrap();
        assert_eq!(&expected_log, tools.log());
    }

    #[test]
    fn construct_multiple_with_fs() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        let assembly_config = ImageAssemblyConfig::new_for_testing("path/to/kernel");

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };

        let base_package_path = dir.join("base.far");
        let mut base_package_file = File::create(&base_package_path).unwrap();
        write!(base_package_file, "base package").unwrap();
        let base_package_manifest_path = dir.join("package_manifest.json");
        let mut base_package_manifest_file = File::create(&base_package_manifest_path).unwrap();
        let contents = r#"{
            "version": "1",
            "package": {
                "name": "system_image",
                "version": "0"
            },
            "blobs": []
        }
        "#;
        write!(base_package_manifest_file, "{contents}").unwrap();
        let base_package =
            BasePackage { merkle: [0u8; 32].into(), manifest_path: base_package_manifest_path };

        let include_account = false;
        let compress_blobfs = false;
        let slice_size = 0;
        let mut builder = MultiFvmBuilder::new(
            dir,
            &assembly_config,
            &mut assembled_system,
            compress_blobfs,
            slice_size,
            include_account,
            &base_package,
        );
        builder.filesystem(FvmFilesystem::BlobFS(BlobFS {
            name: "blob".into(),
            layout: BlobfsLayout::Compact,
            maximum_bytes: None,
            minimum_data_bytes: None,
            minimum_inodes: None,
            maximum_contents_size: None,
        }));
        builder.filesystem(FvmFilesystem::EmptyData(EmptyData { name: "empty-data".into() }));
        builder.output(FvmOutput::Standard(StandardFvm {
            name: "fvm".into(),
            filesystems: vec!["blob".into(), "empty-data".into()],
            compress: false,
            resize_image_file_to_fit: false,
            truncate_to_length: None,
        }));
        builder.output(FvmOutput::Sparse(SparseFvm {
            name: "fvm.sparse".into(),
            filesystems: vec!["blob".into(), "empty-data".into()],
            max_disk_size: None,
        }));
        builder.output(FvmOutput::Nand(NandFvm {
            name: "fvm.nand".into(),
            filesystems: vec!["blob".into(), "empty-data".into()],
            max_disk_size: None,
            compress: false,
            block_count: 1,
            oob_size: 2,
            page_size: 3,
            pages_per_block: 4,
        }));
        let tools = FakeToolProvider::default();
        builder.build(&tools).unwrap();

        let blobfs_path = dir.join("blob.blk");
        let blobs_json_path = dir.join("blobs.json");
        let blob_manifest_path = dir.join("blob.manifest");
        let standard_path = dir.join("fvm.blk");
        let sparse_path = dir.join("fvm.sparse.blk");
        let nand_tmp_path = dir.join("fvm.nand.tmp.blk");
        let nand_path = dir.join("fvm.nand.blk");
        let expected_log: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
            {
                "tool": "./host_x64/blobfs",
                "args": [
                    "--json-output",
                    blobs_json_path,
                    blobfs_path,
                    "create",
                    "--manifest",
                    blob_manifest_path,
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    standard_path,
                    "create",
                    "--slice",
                    "0",
                    "--blob",
                    blobfs_path,
                    "--with-empty-data",
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    sparse_path,
                    "sparse",
                    "--slice",
                    "0",
                    "--compress",
                    "lz4",
                    "--blob",
                    blobfs_path,
                    "--with-empty-data",
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    nand_tmp_path,
                    "sparse",
                    "--slice",
                    "0",
                    "--compress",
                    "lz4",
                    "--blob",
                    blobfs_path,
                    "--with-empty-data",
                ]
            },
            {
                "tool": "./host_x64/fvm",
                "args": [
                    nand_path,
                    "ftl-raw-nand",
                    "--nand-page-size",
                    "3",
                    "--nand-oob-size",
                    "2",
                    "--nand-pages-per-block",
                    "4",
                    "--nand-block-count",
                    "1",
                    "--sparse",
                    nand_tmp_path,
                ]
            },
            ]
        }))
        .unwrap();
        assert_eq!(&expected_log, tools.log());
    }
}
