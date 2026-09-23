// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::base_package::BasePackage;
use crate::{AssembledSystem, Image};

use anyhow::{Context, Result, anyhow};
use assembly_cli_args::AssemblyMode;
use assembly_constants::BootfsDestination;
use assembly_images_config::Zbi;
use assembly_package_list::{PackageList, WritablePackageList};
use assembly_tool::Tool;
use assembly_util::{InsertUniqueExt, MapEntry};
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_pkg::PackageManifest;
use image_assembly_config::ImageAssemblyConfig;
use std::collections::BTreeMap;
use std::fs::copy;
use utf8_path::path_relative_from_current_dir;
use zbi::ZbiBuilder;

#[allow(clippy::too_many_arguments)]
pub fn construct_zbi(
    zbi_tool: Box<dyn Tool>,
    assembled_system: &mut AssembledSystem,
    gendir: impl AsRef<Utf8Path>,
    product: &ImageAssemblyConfig,
    zbi_config: &Zbi,
    base_package: Option<&BasePackage>,
    ramdisk_image: Option<impl Into<Utf8PathBuf>>,
    assembly_mode: AssemblyMode,
) -> Result<Utf8PathBuf> {
    // If this is a ramdisk, don't touch it. Pass it on through.
    if matches!(assembly_mode, AssemblyMode::TestRamdisk) {
        let zbi_path_relative = path_relative_from_current_dir(&product.kernel.path)?;
        assembled_system.images.push(Image::TestRamdisk(zbi_path_relative));
        return Ok(product.kernel.path.clone());
    }

    let mut zbi_builder = ZbiBuilder::new(zbi_tool);

    // Add the kernel image.
    zbi_builder.set_kernel(&product.kernel.path);

    // Add the additional boot args.
    for boot_arg in &product.boot_args {
        zbi_builder.add_boot_arg(boot_arg);
    }

    // If a base merkle is supplied, then add the boot arguments for starting up pkgfs with the
    // merkle of the Base Package.
    if let Some(base_package) = &base_package {
        // Specify how to launch pkgfs: bin/pkgsvr <base-merkle>
        // This is still needed even though pkgfs has been removed because pkg-cache uses it to
        // obtain the base_package hash.
        zbi_builder
            .add_boot_arg(&format!("zircon.system.pkgfs.cmd=bin/pkgsvr+{}", base_package.merkle));
    }

    // Add the command line.
    for cmd in &product.kernel.args {
        zbi_builder.add_cmdline_arg(cmd);
    }

    if let Some(args) = &product.board_driver_arguments {
        zbi_builder.set_board_driver_arguments(args.clone());
    }

    // Below, we generate the package index used by early boot to resolve
    // components. The index relies of assembly_base_package utility as it heavily
    // overlaps with the work done to generate cache and static indices.

    // Mapping from human-readable package name to merkle root for that package's
    // meta.far.
    let mut bootfs_package_list = PackageList::default();

    // Add the packages to a map first, to ensure uniqueness, and ensure a deterministic order.
    let mut bootfs_packages: BTreeMap<String, PackageManifest> = BTreeMap::new();
    for manifest_path in &product.bootfs_packages {
        let manifest = PackageManifest::try_load_from(manifest_path)
            .with_context(|| format!("parsing {manifest_path} as a package manifest"))?;

        bootfs_packages
            .try_insert_unique(MapEntry(manifest.name().to_string(), manifest))
            .map_err(|e| anyhow!("Duplicate bootfs package found: {}", e.existing_entry.key()))?;
    }

    for (_, manifest) in bootfs_packages {
        let (meta_far, remainder) = manifest.clone().package_and_subpackage_blobs()?;
        for blob_info in remainder.into_values().chain(Some(meta_far)) {
            zbi_builder.add_bootfs_blob(&blob_info.source_path, blob_info.merkle);
        }
        bootfs_package_list.add_package(manifest)?;
    }

    // Write the bootfs package index to the gendir, unconditionally, to satisfy
    // ninja file-use.
    bootfs_package_list.write_index_file(
        gendir.as_ref(),
        "bootfs",
        BootfsDestination::BootfsPackageIndex.to_string(),
    )?;

    if !bootfs_package_list.is_empty() {
        let bootfs_package_index_source =
            gendir.as_ref().join(BootfsDestination::BootfsPackageIndex.to_string());

        // Write the bootfs package index from the gendir to the bootfs.
        zbi_builder.add_bootfs_file(
            &bootfs_package_index_source,
            BootfsDestination::BootfsPackageIndex.to_string(),
        );
    }

    // Add the BootFS files.
    for bootfs_entry in &product.bootfs_files {
        zbi_builder.add_bootfs_file(&bootfs_entry.source, &bootfs_entry.destination);
    }

    // Add the ramdisk image in the ZBI if necessary.
    if let Some(ramdisk) = ramdisk_image {
        zbi_builder.add_ramdisk(ramdisk);
    }

    // Add the extra items ZBI.
    if let Some(zbi_extra_items) = &product.zbi_extra_items {
        zbi_builder.add_zbi_extra_items(zbi_extra_items);
    }

    // Add the devicetree binary in the ZBI if necessary.
    if let Some(devicetree) = &product.devicetree {
        zbi_builder.add_devicetree(devicetree);
    }

    // Set the zbi compression to use.
    zbi_builder.set_compression(zbi_config.compression);

    // Create an output manifest that describes the contents of the built ZBI.
    zbi_builder.set_output_manifest(gendir.as_ref().join("zbi.json"));

    // Build and return the ZBI.
    let zbi_path = gendir.as_ref().join(format!("{}.zbi", zbi_config.name));
    zbi_builder.build(gendir, zbi_path.as_path())?;

    // Only add the unsigned ZBI to the images manifest if we will not be signing the ZBI.
    let zbi_path_relative = path_relative_from_current_dir(zbi_path)?;
    if zbi_config.postprocessing_script.is_none() {
        assembled_system.images.push(Image::ZBI { path: zbi_path_relative.clone(), signed: false });
    }

    Ok(zbi_path_relative)
}

/// If the board requires the zbi to be post-processed to make it bootable by
/// the bootloaders, then perform that task here.
pub fn vendor_sign_zbi(
    signing_tool: Box<dyn Tool>,
    assembled_system: &mut AssembledSystem,
    gendir: impl AsRef<Utf8Path>,
    zbi_config: &Zbi,
    zbi: impl AsRef<Utf8Path>,
) -> Result<Utf8PathBuf> {
    let script = match &zbi_config.postprocessing_script {
        Some(script) => script,
        _ => return Err(anyhow!("Missing postprocessing_script")),
    };

    // The resultant file path
    let signed_path = gendir.as_ref().join(format!("{}.zbi.signed", zbi_config.name));

    // If the script config defines extra arguments, add them:
    let mut args = Vec::new();
    args.extend_from_slice(&script.args[..]);

    // Add the parameters of the script that are required:
    args.push("-z".to_string());
    args.push(zbi.as_ref().to_string());
    args.push("-o".to_string());
    args.push(signed_path.to_string());

    // Run the tool.
    signing_tool.run(&args)?;

    // It is intended to copy the signed zbi to foo.zbi and move the unsigned
    // to a new location (foo.zbi.unsigned).
    let output_zbi_path = gendir.as_ref().join(format!("{}.zbi", zbi_config.name));
    if signed_path.is_file() {
        let unsigned_zbi_path = gendir.as_ref().join(format!("{}.zbi.unsigned", zbi_config.name));
        copy(&output_zbi_path, &unsigned_zbi_path).context("Copy unsigned zbi")?;
        copy(&signed_path, &output_zbi_path).context("Copy signed zbi")?;
    }

    assembled_system.images.push(Image::ZBI { path: output_zbi_path.clone(), signed: true });
    Ok(output_zbi_path)
}

#[cfg(test)]
mod tests {
    use super::{construct_zbi, vendor_sign_zbi};

    use crate::AssembledSystem;
    use crate::base_package::BasePackage;

    use assembly_constants::BootfsDestination;
    use assembly_images_config::{PostProcessingScript, Zbi, ZbiCompression};
    use assembly_release_info::SystemReleaseInfo;
    use assembly_tool::testing::FakeToolProvider;
    use assembly_tool::{ToolCommandLog, ToolProvider};
    use camino::{Utf8Path, Utf8PathBuf};
    use fuchsia_hash::Hash;
    use image_assembly_config::ImageAssemblyConfig;
    use regex::Regex;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::Write;
    use std::str::FromStr;
    use tempfile::tempdir;

    // These tests must be run serially, because otherwise they will affect each
    // other through process spawning. If a test spawns a process while the
    // other test has an open file, then the spawned process will get a copy of
    // the open file descriptor, preventing the other test from executing it.
    #[test]
    fn construct() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();

        // Create fake product/board definitions.
        let kernel_path = dir.join("kernel");
        let mut product_config = ImageAssemblyConfig::new_for_testing(&kernel_path);

        let zbi_config = Zbi {
            name: "fuchsia".into(),
            compression: ZbiCompression::ZStd,
            postprocessing_script: None,
        };

        // Create a kernel which is equivalent to: zbi --ouput <zbi-name>
        let kernel_bytes = vec![
            0x42, 0x4f, 0x4f, 0x54, 0x00, 0x00, 0x00, 0x00, 0xe6, 0xf7, 0x8c, 0x86, 0x00, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x29, 0x17, 0x78, 0xb5,
            0xd6, 0xe8, 0x87, 0x4a,
        ];
        std::fs::write(&kernel_path, kernel_bytes).unwrap();

        // Create a fake pkgfs.
        let pkgfs_manifest_path = generate_test_manifest_file(dir, "pkgfs");
        product_config.base.push(pkgfs_manifest_path);

        // Create a fake base package.
        let base_path = dir.join("base.far");
        std::fs::write(&base_path, "fake base").unwrap();
        let base = BasePackage {
            merkle: Hash::from_str(
                "0000000000000000000000000000000000000000000000000000000000000000",
            )
            .unwrap(),
            manifest_path: Utf8PathBuf::default(),
        };

        // Create a fake zbi tool.
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        construct_zbi(
            zbi_tool,
            &mut assembled_system,
            dir,
            &product_config,
            &zbi_config,
            Some(&base),
            None::<Utf8PathBuf>,
            Default::default(),
        )
        .unwrap();

        let bootfs_index_string =
            std::fs::read_to_string(dir.join(BootfsDestination::BootfsPackageIndex.to_string()))
                .unwrap();
        assert_eq!("", bootfs_index_string);

        // Create a fake archivist.
        let archivist_manifest_path = generate_test_manifest_file_with_subpackage(dir, "archivist");
        product_config.bootfs_packages.push(archivist_manifest_path);

        // Create a new fake zbi tool for isolated logs.
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();

        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        construct_zbi(
            zbi_tool,
            &mut assembled_system,
            dir,
            &product_config,
            &zbi_config,
            Some(&base),
            None::<Utf8PathBuf>,
            Default::default(),
        )
        .unwrap();

        let bootfs_index_string =
            std::fs::read_to_string(dir.join(BootfsDestination::BootfsPackageIndex.to_string()))
                .unwrap();

        assert_eq!(
            "archivist/1=0000000000000000000000000000000000000000000000000000000000000000\n",
            bootfs_index_string
        );

        let zbi_args: Vec<String> = tools
            .log()
            .commands
            .borrow()
            .iter()
            .find(|command| command.tool == "./host_x64/zbi")
            .unwrap()
            .args
            .clone();
        #[allow(clippy::cmp_owned)]
        let bootfs_file_index =
            zbi_args.iter().enumerate().find(|(_, arg)| **arg == "--files".to_string()).unwrap().0
                + 1;

        let bootfs_files = std::fs::read_to_string(&zbi_args[bootfs_file_index]).unwrap();

        let expected_bootfs_files_regex = Regex::new(concat!(
            r"blob/0000000000000000000000000000000000000000000000000000000000000000=path/to/archivist/meta\.far\n",
            r"blob/1111111111111111111111111111111111111111111111111111111111111111=/.*/archivist_data\.txt\n",
            r"blob/2222222222222222222222222222222222222222222222222222222222222222=path/to/archivist_subpackage/meta.far\n",
            r"blob/3333333333333333333333333333333333333333333333333333333333333333=/.*/archivist_data_subpackage.txt\n",
            r"config/additional_boot_args=/.*/additional_boot_args\.txt\n",
            r"data/bootfs_packages=/.*/data/bootfs_packages.*"
        )).unwrap();

        assert!(
            expected_bootfs_files_regex.is_match(&bootfs_files),
            "Failed to regex match: {bootfs_files:?}"
        );
    }

    #[test]
    fn vendor_sign() {
        let tmp = tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap();
        let expected_output = dir.join("fuchsia.zbi");

        // Create a fake zbi.
        let zbi_path = dir.join("fuchsia.zbi");
        std::fs::write(&zbi_path, "fake zbi").unwrap();

        // Create fake zbi config.
        let zbi = Zbi {
            name: "fuchsia".into(),
            compression: ZbiCompression::ZStd,
            postprocessing_script: Some(PostProcessingScript {
                path: Some(Utf8PathBuf::from("fake")),
                board_script_path: None,
                args: vec!["arg1".into(), "arg2".into()],
                inputs: BTreeMap::default(),
            }),
        };

        // Sign the zbi.
        let tools = FakeToolProvider::default();
        let signing_tool = tools.get_tool("fake").unwrap();
        let mut assembled_system = AssembledSystem {
            images: Default::default(),
            board_name: "my_board".into(),
            partitions_config: None,
            system_release_info: SystemReleaseInfo::new_for_testing(),
            platform_tools: vec![],
        };
        let signed_zbi_path =
            vendor_sign_zbi(signing_tool, &mut assembled_system, dir, &zbi, &zbi_path).unwrap();
        assert_eq!(signed_zbi_path, expected_output);

        let expected_commands: ToolCommandLog = serde_json::from_value(json!({
            "commands": [
                {
                    "tool": "./host_x64/fake",
                    "args": [
                        "arg1",
                        "arg2",
                        "-z",
                        zbi_path.as_str(),
                        "-o",
                        expected_output.as_str().to_owned() + ".signed",
                    ]
                }
            ]
        }))
        .unwrap();
        assert_eq!(&expected_commands, tools.log());
    }

    // Generates a package manifest to be used for testing. The file is written
    // into `dir`, and the location is returned. The `name` is used in the blob
    // file names to make each manifest somewhat unique.
    // TODO(https://fxbug.dev/42156958): See if we can share this with BasePackage.
    pub fn generate_test_manifest_file(
        dir: impl AsRef<Utf8Path>,
        name: impl AsRef<str>,
    ) -> Utf8PathBuf {
        // Create a data file for the package.
        let data_file_name = format!("{}_data.txt", name.as_ref());
        let data_path = dir.as_ref().join(&data_file_name);
        let data_file = File::create(&data_path).unwrap();
        write!(&data_file, "bleh").unwrap();

        // Create the manifest.
        let manifest_path = dir.as_ref().join(format!("{}.json", name.as_ref()));
        let manifest_file = File::create(&manifest_path).unwrap();
        serde_json::to_writer(
            &manifest_file,
            &json!({
                    "version": "1",
                    "repository": "testrepository.com",
                    "package": {
                        "name": name.as_ref(),
                        "version": "1",
                    },
                    "blobs": [
                        {
                            "source_path": format!("path/to/{}/meta.far", name.as_ref()),
                            "path": "meta/",
                            "merkle":
                                "0000000000000000000000000000000000000000000000000000000000000000",
                            "size": 1
                        },
                        {
                            "source_path": &data_path,
                            "path": &data_file_name,
                            "merkle":
                                "1111111111111111111111111111111111111111111111111111111111111111",
                            "size": 1
                        },
                    ]
                }
            ),
        )
        .unwrap();
        manifest_path
    }

    // Generates a package manifest to be used for testing. The package has a content blob and a
    // subpackage that itself has a content blob. The manifest, the subpackage's manifest, and both
    // content blobs are written into `dir`, and the manifest's path is returned. `name` is used in
    // the file names to make each manifest somewhat unique.
    // TODO(https://fxbug.dev/42156958): See if we can share this with BasePackage.
    fn generate_test_manifest_file_with_subpackage(
        dir: impl AsRef<Utf8Path>,
        name: impl AsRef<str>,
    ) -> Utf8PathBuf {
        // Create a data file for the subpackage.
        let subpackage_data_file_name = format!("{}_data_subpackage.txt", name.as_ref());
        let subpackage_data_path = dir.as_ref().join(&subpackage_data_file_name);
        let subpackage_data_file = File::create(&subpackage_data_path).unwrap();
        write!(&subpackage_data_file, "subpackage-bleh").unwrap();

        // Create the subpackage manifest.
        let subpackage_manifest_path =
            dir.as_ref().join(format!("{}_subpackage.json", name.as_ref()));
        let subpackage_manifest_file = File::create(&subpackage_manifest_path).unwrap();
        serde_json::to_writer(
            &subpackage_manifest_file,
            &json!({
                    "version": "1",
                    "repository": "testrepository.com",
                    "package": {
                        "name": &format!("{}_subpackage", name.as_ref()),
                        "version": "1",
                    },
                    "blobs": [
                        {
                            "source_path": format!("path/to/{}_subpackage/meta.far", name.as_ref()),
                            "path": "meta/",
                            "merkle":
                                "2222222222222222222222222222222222222222222222222222222222222222",
                            "size": 1
                        },
                        {
                            "source_path": &subpackage_data_path,
                            "path": &subpackage_data_file_name,
                            "merkle":
                                "3333333333333333333333333333333333333333333333333333333333333333",
                            "size": 1
                        },
                    ]
                }
            ),
        )
        .unwrap();

        // Create a data file for the package.
        let data_file_name = format!("{}_data.txt", name.as_ref());
        let data_path = dir.as_ref().join(&data_file_name);
        let data_file = File::create(&data_path).unwrap();
        write!(&data_file, "bleh").unwrap();

        // Create the manifest.
        let manifest_path = dir.as_ref().join(format!("{}.json", name.as_ref()));
        let manifest_file = File::create(&manifest_path).unwrap();
        serde_json::to_writer(
            &manifest_file,
            &json!({
                    "version": "1",
                    "repository": "testrepository.com",
                    "package": {
                        "name": name.as_ref(),
                        "version": "1",
                    },
                    "blobs": [
                        {
                            "source_path": format!("path/to/{}/meta.far", name.as_ref()),
                            "path": "meta/",
                            "merkle":
                                "0000000000000000000000000000000000000000000000000000000000000000",
                            "size": 1
                        },
                        {
                            "source_path": &data_path,
                            "path": &data_file_name,
                            "merkle":
                                "1111111111111111111111111111111111111111111111111111111111111111",
                            "size": 1
                        },
                    ],
                    "subpackages": [{
                        "name": "my-subpackage",
                        "merkle":
                            "2222222222222222222222222222222222222222222222222222222222222222",
                        "manifest_path": &subpackage_manifest_path,
                    }]
                }
            ),
        )
        .unwrap();
        manifest_path
    }
}
