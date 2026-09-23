// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, Result, anyhow};
use assembly_constants::BootfsDestination;
use assembly_tool::Tool;
use camino::{Utf8Path, Utf8PathBuf};
use image_assembly_config::BoardDriverArguments;
use log::debug;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use zerocopy::IntoBytes;

use crate::zbi_items::new_platform_id;

/// Builder for the Zircon Boot Image (ZBI), which takes in a kernel, BootFS, boot args, and kernel
/// command line.
pub struct ZbiBuilder {
    /// The zbi host tool.
    tool: Box<dyn Tool>,

    kernel: Option<Utf8PathBuf>,
    // Map from file destination in the ZBI to the path of the source file on the host.
    bootfs_files: BTreeMap<String, Utf8PathBuf>,
    bootargs: Vec<String>,
    cmdline: Vec<String>,
    board_driver_arguments: Option<BoardDriverArguments>,

    // A ramdisk to add to the ZBI.
    ramdisk: Option<Utf8PathBuf>,

    // A non-bootable ZBI image containing extra items to be added.
    zbi_extra_items: Option<Utf8PathBuf>,

    // A devicetree binary to add to the ZBI
    devicetree: Option<Utf8PathBuf>,

    /// optional compression to use.
    compression: Option<String>,

    /// optional output manifest file
    output_manifest: Option<Utf8PathBuf>,
}

impl ZbiBuilder {
    /// Construct a new ZbiBuilder that uses the zbi |tool|.
    pub fn new(tool: Box<dyn Tool>) -> Self {
        Self {
            tool,
            kernel: None,
            bootfs_files: BTreeMap::default(),
            bootargs: Vec::default(),
            cmdline: Vec::default(),
            board_driver_arguments: None,
            ramdisk: None,
            zbi_extra_items: None,
            devicetree: None,
            compression: None,
            output_manifest: None,
        }
    }

    /// Set the kernel to be used.
    pub fn set_kernel(&mut self, kernel: impl Into<Utf8PathBuf>) {
        self.kernel = Some(kernel.into());
    }

    /// Add a file to the bootfs as a merkle root identified blob.
    pub fn add_bootfs_blob(
        &mut self,
        source: impl Into<Utf8PathBuf>,
        merkle_root: impl std::fmt::Display,
    ) {
        // Every file that is part of a package included in the bootfs image
        // will exist under a `blob` directory, and will be identified by
        // its merkle root.
        let bootfs_path = format!("blob/{}", merkle_root);
        self.bootfs_files.insert(bootfs_path, source.into());
    }

    /// Add a BootFS file to the ZBI.
    pub fn add_bootfs_file(
        &mut self,
        source: impl Into<Utf8PathBuf>,
        destination: impl AsRef<str>,
    ) {
        if self.bootfs_files.contains_key(destination.as_ref()) {
            println!("Found duplicate bootfs destination: {}", destination.as_ref());
            return;
        }
        self.bootfs_files.insert(destination.as_ref().to_string(), source.into());
    }

    /// Add a boot argument to the ZBI.
    pub fn add_boot_arg(&mut self, arg: &str) {
        self.bootargs.push(arg.to_string());
    }

    /// Add a kernel command line argument.
    pub fn add_cmdline_arg(&mut self, arg: &str) {
        self.cmdline.push(arg.to_string());
    }

    /// Add optional board driver arguments to be added as ZBI items.
    pub fn set_board_driver_arguments(&mut self, args: BoardDriverArguments) {
        self.board_driver_arguments = Some(args);
    }

    /// Add a ramdisk to the ZBI.
    pub fn add_ramdisk(&mut self, source: impl Into<Utf8PathBuf>) {
        self.ramdisk = Some(source.into());
    }

    /// Extend the ZBI with the items in the provided ZBI..
    pub fn add_zbi_extra_items(&mut self, source: impl Into<Utf8PathBuf>) {
        self.zbi_extra_items = Some(source.into());
    }

    /// Add a devicetree binary to the ZBI.
    pub fn add_devicetree(&mut self, source: impl Into<Utf8PathBuf>) {
        self.devicetree = Some(source.into());
    }

    /// Set the compression to use with the ZBI.
    pub fn set_compression(&mut self, compress: impl ToString) {
        self.compression = Some(compress.to_string());
    }

    /// Set the path to an optional JSON output manifest to produce.
    pub fn set_output_manifest(&mut self, manifest: impl Into<Utf8PathBuf>) {
        self.output_manifest = Some(manifest.into());
    }

    /// Build the ZBI.
    pub fn build(self, gendir: impl AsRef<Utf8Path>, output: impl AsRef<Utf8Path>) -> Result<()> {
        // Create the additional_boot_args.
        // TODO(https://fxbug.dev/42157396): Switch to the boot args file once we are no longer
        // comparing to the GN build.
        let additional_boot_args_path = gendir.as_ref().join("additional_boot_args.txt");
        let mut additional_boot_args = File::create(&additional_boot_args_path)
            .map_err(|e| Error::new(e).context("failed to create the additional boot args"))?;
        self.write_boot_args(&mut additional_boot_args)?;

        let (platform_id_path, board_info_path) =
            if let Some(BoardDriverArguments { vendor_id, product_id, revision, ref name }) =
                self.board_driver_arguments
            {
                let platform_id = new_platform_id(vendor_id, product_id, name)?;
                let board_info = zbi::BoardInfo { revision };

                let platform_id_path = gendir.as_ref().join("platform_id.bin");
                let board_info_path = gendir.as_ref().join("board_info.bin");

                std::fs::write(&platform_id_path, platform_id.as_bytes())
                    .with_context(|| format!("writing platform_id to: {}", platform_id_path))?;

                std::fs::write(&board_info_path, board_info.as_bytes())
                    .with_context(|| format!("writing board_info to: {}", platform_id_path))?;

                (Some(platform_id_path), Some(board_info_path))
            } else {
                (None, None)
            };

        // Create the BootFS manifest file that lists all the files to insert
        // into BootFS.
        let bootfs_manifest_path = gendir.as_ref().join("bootfs_files.list");
        let mut bootfs_manifest = File::create(&bootfs_manifest_path)
            .map_err(|e| Error::new(e).context("failed to create the bootfs manifest"))?;
        self.write_bootfs_manifest(additional_boot_args_path, &mut bootfs_manifest)?;

        // Run the zbi tool to construct the ZBI.
        let zbi_args = self.build_zbi_args(
            &bootfs_manifest_path,
            None::<Utf8PathBuf>,
            platform_id_path.clone(),
            board_info_path.clone(),
            output,
        )?;
        debug!("ZBI command args: {:?}", zbi_args);

        self.tool.run(&zbi_args)?;

        // Cleanup temporary files
        if let Some(platform_id_path) = platform_id_path {
            std::fs::remove_file(&platform_id_path)
                .with_context(|| format!("Removing temporary file: {platform_id_path}"))?;
        }
        if let Some(board_info_path) = board_info_path {
            std::fs::remove_file(&board_info_path)
                .with_context(|| format!("Removing temporary file: {board_info_path}"))?;
        }

        Ok(())
    }

    fn write_bootfs_manifest(
        &self,
        additional_boot_args_path: impl Into<Utf8PathBuf>,
        out: &mut impl Write,
    ) -> Result<()> {
        let mut bootfs_files = self.bootfs_files.clone();

        if !self.bootargs.is_empty() {
            bootfs_files.insert(
                BootfsDestination::AdditionalBootArgs.to_string(),
                additional_boot_args_path.into(),
            );
        }
        for (destination, source) in bootfs_files {
            write!(out, "{}", destination)?;
            write!(out, "=")?;
            // TODO(fxbug.dev76135): Use the zbi tool's set of valid inputs instead of constraining
            // to valid UTF-8.
            writeln!(out, "{}", source)?;
        }
        Ok(())
    }

    fn write_boot_args(&self, out: &mut impl Write) -> Result<()> {
        for arg in &self.bootargs {
            writeln!(out, "{}", arg)?;
        }
        Ok(())
    }

    fn build_zbi_args(
        &self,
        bootfs_manifest_path: impl AsRef<Utf8Path>,
        boot_args_path: Option<impl AsRef<Utf8Path>>,
        platform_id_path: Option<impl AsRef<Utf8Path>>,
        board_info_path: Option<impl AsRef<Utf8Path>>,
        output_path: impl AsRef<Utf8Path>,
    ) -> Result<Vec<String>> {
        // Ensure a kernel is supplied.
        let kernel = &self.kernel.as_ref().ok_or_else(|| anyhow!("No kernel image supplied"))?;

        let mut args: Vec<String> = Vec::new();

        // Add the kernel itself, first, to make a bootable ZBI.
        args.push("--type=container".to_string());
        args.push(kernel.to_string());

        // Add the extra items next.
        if let Some(zbi_extra_items) = &self.zbi_extra_items {
            args.push("--type=container".to_string());
            args.push(zbi_extra_items.to_string())
        }

        // Then, add the kernel cmdline args.
        args.push("--type=cmdline".to_string());
        for cmd in &self.cmdline {
            args.push(format!("--entry={}", cmd));
        }

        if let Some(platform_id_path) = platform_id_path {
            args.push("--type=platform_id".to_string());
            args.push(platform_id_path.as_ref().to_string());
        }

        if let Some(board_info_path) = board_info_path {
            args.push("--type=drv_board_info".to_string());
            args.push(board_info_path.as_ref().to_string());
        }

        if let Some(devicetree_path) = &self.devicetree {
            args.push("--type=devicetree".to_string());
            args.push(devicetree_path.to_string());
        }

        // Then, add the bootfs files.
        args.push("--files".to_string());
        args.push(bootfs_manifest_path.as_ref().to_string());

        // Instead of supplying the additional_boot_args.txt file, we could use boot args. This is disabled
        // by default, in order to allow for binary diffing the ZBI to the in-tree built ZBI.
        if let Some(boot_args_path) = boot_args_path {
            let boot_args_path = boot_args_path.as_ref().to_string();
            args.push("--type=image_args".to_string());
            args.push(format!("--entry={}", boot_args_path));
        }

        // Add the ramdisk if needed.
        if let Some(ramdisk) = &self.ramdisk {
            args.push("--type=ramdisk".to_string());
            if let Some(compression) = &self.compression {
                args.push(format!("--compress={}", compression));
            }
            args.push(ramdisk.to_string());
        }

        // Set the compression level for bootfs files.
        if let Some(compression) = &self.compression {
            args.push(format!("--compressed={}", compression));
        }

        // Set the output file to write.
        args.push("--output".into());
        args.push(output_path.as_ref().to_string());

        // Create an output manifest that describes the contents of the built ZBI.
        if let Some(output_manifest) = &self.output_manifest {
            args.push("--json-output".into());
            args.push(output_manifest.to_string());
        }
        Ok(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assembly_tool::ToolProvider;
    use assembly_tool::testing::FakeToolProvider;

    #[test]
    fn bootfs_manifest_additional_boot_args_only() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        let mut output: Vec<u8> = Vec::new();

        builder.add_boot_arg("fakearg");
        builder.write_bootfs_manifest("additional_boot_args.txt", &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(
            output_str,
            "config/additional_boot_args=additional_boot_args.txt\n".to_string()
        );

        let mut output: Vec<u8> = Vec::new();
        builder.add_bootfs_file("path/to/file2", "bin/file2");
        builder.add_bootfs_file("path/to/file1", "lib/file1");
        builder.write_bootfs_manifest("additional_boot_args.txt", &mut output).unwrap();
        let output_str = String::from_utf8(output).unwrap();
        assert_eq!(
            output_str,
            "bin/file2=path/to/file2\nconfig/additional_boot_args=additional_boot_args.txt\nlib/file1=path/to/file1\n"
                .to_string()
        );
    }

    #[test]
    fn bootfs_manifest() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        let mut output: Vec<u8> = Vec::new();

        builder.add_bootfs_file("path/to/file2", "bin/file2");
        builder.add_bootfs_file("path/to/file1", "lib/file1");
        builder.add_bootfs_blob("path/to/file1", "my_merkle");
        builder.add_boot_arg("fakearg");
        builder.write_bootfs_manifest("additional_boot_args.txt", &mut output).unwrap();
        assert_eq!(
            output,
            b"bin/file2=path/to/file2\nblob/my_merkle=path/to/file1\nconfig/additional_boot_args=additional_boot_args.txt\nlib/file1=path/to/file1\n"
        );
    }

    #[test]
    fn boot_args() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        let mut output: Vec<u8> = Vec::new();

        builder.write_boot_args(&mut output).unwrap();
        assert_eq!(output, b"");

        output.clear();
        builder.add_boot_arg("boot-arg1");
        builder.add_boot_arg("boot-arg2");
        builder.write_boot_args(&mut output).unwrap();
        assert_eq!(output, b"boot-arg1\nboot-arg2\n");
    }

    #[test]
    fn zbi_args_missing_kernel() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let builder = ZbiBuilder::new(zbi_tool);
        assert!(
            builder
                .build_zbi_args(
                    "bootfs",
                    Some("bootargs"),
                    None::<String>,
                    None::<String>,
                    "output"
                )
                .is_err()
        );
    }

    #[test]
    fn zbi_args_with_kernel() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        let args = builder
            .build_zbi_args("bootfs", Some("bootargs"), None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--files",
                "bootfs",
                "--type=image_args",
                "--entry=bootargs",
                "--output",
                "output",
            ]
        );
    }

    #[test]
    fn zbi_args_with_kernel_with_zbi_extra_items() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.add_zbi_extra_items("path/to/zbi_with_extra_items");
        let args = builder
            .build_zbi_args("bootfs", Some("bootargs"), None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=container",
                "path/to/zbi_with_extra_items",
                "--type=cmdline",
                "--files",
                "bootfs",
                "--type=image_args",
                "--entry=bootargs",
                "--output",
                "output",
            ]
        );
    }

    #[test]
    fn zbi_args_with_kernel_with_devicetree() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.add_devicetree("path/to/devicetree");
        let args = builder
            .build_zbi_args("bootfs", Some("bootargs"), None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--type=devicetree",
                "path/to/devicetree",
                "--files",
                "bootfs",
                "--type=image_args",
                "--entry=bootargs",
                "--output",
                "output",
            ]
        );
    }

    #[test]
    fn zbi_args_with_cmdline() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.add_cmdline_arg("cmd-arg1");
        builder.add_cmdline_arg("cmd-arg2");
        let args = builder
            .build_zbi_args("bootfs", Some("bootargs"), None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--entry=cmd-arg1",
                "--entry=cmd-arg2",
                "--files",
                "bootfs",
                "--type=image_args",
                "--entry=bootargs",
                "--output",
                "output",
            ]
        );
    }

    #[test]
    fn zbi_args_without_boot_args() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.add_cmdline_arg("cmd-arg1");
        builder.add_cmdline_arg("cmd-arg2");
        let args = builder
            .build_zbi_args("bootfs", None::<String>, None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--entry=cmd-arg1",
                "--entry=cmd-arg2",
                "--files",
                "bootfs",
                "--output",
                "output",
            ]
        );
    }

    #[test]
    fn zbi_args_with_compression() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.add_cmdline_arg("cmd-arg1");
        builder.add_cmdline_arg("cmd-arg2");
        builder.set_compression("zstd.max");
        let args = builder
            .build_zbi_args("bootfs", None::<String>, None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--entry=cmd-arg1",
                "--entry=cmd-arg2",
                "--files",
                "bootfs",
                "--compressed=zstd.max",
                "--output",
                "output",
            ]
        );
    }

    #[test]
    fn zbi_args_with_manifest() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.add_cmdline_arg("cmd-arg1");
        builder.add_cmdline_arg("cmd-arg2");
        builder.set_output_manifest("path/to/manifest");
        let args = builder
            .build_zbi_args("bootfs", None::<String>, None::<String>, None::<String>, "output")
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--entry=cmd-arg1",
                "--entry=cmd-arg2",
                "--files",
                "bootfs",
                "--output",
                "output",
                "--json-output",
                "path/to/manifest",
            ]
        );
    }

    #[test]
    fn zbi_args_with_board_driver_args() {
        let tools = FakeToolProvider::default();
        let zbi_tool = tools.get_tool("zbi").unwrap();
        let mut builder = ZbiBuilder::new(zbi_tool);
        builder.set_kernel("path/to/kernel");
        builder.set_output_manifest("path/to/manifest");
        let args = builder
            .build_zbi_args(
                "bootfs",
                None::<String>,
                Some("gen/platform_id_path"),
                Some("gen/board_info_path"),
                "output",
            )
            .unwrap();
        assert_eq!(
            args,
            [
                "--type=container",
                "path/to/kernel",
                "--type=cmdline",
                "--type=platform_id",
                "gen/platform_id_path",
                "--type=drv_board_info",
                "gen/board_info_path",
                "--files",
                "bootfs",
                "--output",
                "output",
                "--json-output",
                "path/to/manifest",
            ]
        );
    }
}
