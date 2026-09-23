// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_package_archive_cat_args::CatCommand;
use ffx_package_archive_utils::{
    FarArchiveReader, FarCatResult, FarListReader, read_file_entries, to_far_cat_result,
};
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool, Result, return_user_error};
use std::io::Write as _;

#[derive(FfxTool)]
#[target(None)]
pub struct ArchiveCatTool {
    #[command]
    pub cmd: CatCommand,
}

fho::embedded_plugin!(ArchiveCatTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ArchiveCatTool {
    type Writer = VerifiedMachineWriter<FarCatResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        let mut archive_reader = FarArchiveReader::new(&self.cmd.archive)?;

        let data = cat_implementation(self.cmd, &mut archive_reader)?;
        if writer.is_machine() {
            writer.machine(&to_far_cat_result(&data)).bug()?;
        } else {
            match writer.write_all(&data) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(e).bug(),
            }
        }
        Ok(())
    }
}

fn cat_implementation(cmd: CatCommand, reader: &mut dyn FarListReader) -> Result<Vec<u8>> {
    let file_name = cmd.far_path.to_string_lossy();

    let entries = read_file_entries(reader)?;
    if let Some(entry) = entries.iter().find(|x| x.name == file_name) {
        let data = reader.read_entry(entry)?;
        Ok(data)
    } else {
        return_user_error!("file {} not found in {}", file_name, cmd.archive.to_string_lossy());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_package_archive_utils::test_utils::{
        LIB_RUN_SO_BLOB, LIB_RUN_SO_PATH, create_mockreader, test_contents,
    };
    use ffx_writer::{Format, TestBuffers};
    use std::path::PathBuf;

    #[test]
    fn test_cat_filename() -> Result<()> {
        let cmd = CatCommand {
            archive: PathBuf::from("some.far"),
            far_path: PathBuf::from(LIB_RUN_SO_PATH),
        };

        let expected = test_contents(LIB_RUN_SO_BLOB);

        let output = cat_implementation(cmd, &mut create_mockreader())?;
        assert_eq!(expected, output);

        Ok(())
    }

    #[test]
    fn test_cat_filename_machine() -> Result<()> {
        let cmd = CatCommand {
            archive: PathBuf::from("some.far"),
            far_path: PathBuf::from(LIB_RUN_SO_PATH),
        };

        let buffers = TestBuffers::default();
        let mut writer =
            <ArchiveCatTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let data = cat_implementation(cmd, &mut create_mockreader())?;
        let result = to_far_cat_result(&data);
        writer.machine(&result)?;

        let expected = format!("{}\n", serde_json::to_string(&result).unwrap());
        let stdout = buffers.into_stdout_str();
        assert_eq!(stdout, expected);

        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        <ArchiveCatTool as FfxMain>::Writer::verify_schema(&value).unwrap();

        Ok(())
    }
}
