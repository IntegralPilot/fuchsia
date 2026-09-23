// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_package_far_create_args::CreateCommand;
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool, Result};
use fuchsia_archive as far;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use walkdir::WalkDir;

#[derive(FfxTool)]
#[target(None)]
pub struct FarCreateTool {
    #[command]
    pub cmd: CreateCommand,
}

fho::embedded_plugin!(FarCreateTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for FarCreateTool {
    type Writer = VerifiedMachineWriter<()>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        let mut entries = BTreeMap::new();

        for file in WalkDir::new(&self.cmd.input_directory).follow_links(true) {
            let file = file.with_user_message(|| {
                format!("failed to read directory entry in {}", self.cmd.input_directory.display())
            })?;
            if file.file_type().is_dir() {
                continue;
            }
            if !file.file_type().is_file() {
                writeln!(
                    writer.stderr(),
                    "Not a regular file; ignoring: {}",
                    file.path().display()
                )
                .bug()?;
                continue;
            }

            let len = file
                .metadata()
                .with_user_message(|| {
                    format!("failed to read file metadata for {}", file.path().display())
                })?
                .len();
            let reader = File::open(file.path())
                .with_user_message(|| format!("failed to open file {}", file.path().display()))?;
            let reader: Box<dyn Read> = Box::new(reader);

            // Omit the base directory (which is common to all paths).
            let path = file.path().strip_prefix(&self.cmd.input_directory).unwrap();
            let path = path
                .to_str()
                .with_user_message(|| format!("non-unicode file path: {}", path.display()))?;

            entries.insert(String::from(path), (len, reader));
        }

        let output_file = File::create(&self.cmd.output_file).with_user_message(|| {
            format!("failed to create file: {}", self.cmd.output_file.display())
        })?;
        far::write(output_file, entries).with_user_message(|| {
            format!("failed to write FAR file: {}", self.cmd.output_file.display())
        })?;

        writer.machine(&()).bug()?;
        Ok(())
    }
}
