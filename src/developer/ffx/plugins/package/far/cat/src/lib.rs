// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_package_archive_utils::{FarCatResult, to_far_cat_result};
use ffx_package_far_cat_args::CatCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool, Result};
use fuchsia_archive as far;
use std::fs::File;
use std::io::Write as _;

#[derive(FfxTool)]
#[target(None)]
pub struct FarCatTool {
    #[command]
    pub cmd: CatCommand,
}

fho::embedded_plugin!(FarCatTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for FarCatTool {
    type Writer = VerifiedMachineWriter<FarCatResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        let far_file = File::open(&self.cmd.far_file).with_user_message(|| {
            format!("failed to open file: {}", self.cmd.far_file.display())
        })?;
        let mut reader = far::Reader::new(far_file).with_user_message(|| {
            format!("failed to parse FAR file: {}", self.cmd.far_file.display())
        })?;

        let bytes = far_cat_impl(&mut reader, self.cmd.path.as_str(), &self.cmd.far_file)?;

        if writer.is_machine() {
            writer.machine(&to_far_cat_result(&bytes)).bug()?;
        } else {
            match writer.write_all(&bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                Err(e) => return Err(e).bug(),
            }
        }

        Ok(())
    }
}

fn far_cat_impl<R: std::io::Read + std::io::Seek>(
    reader: &mut far::Reader<R>,
    path: &str,
    far_file_path: &std::path::Path,
) -> Result<Vec<u8>> {
    let bytes = reader.read_file(path.as_bytes()).with_user_message(|| {
        format!("failed to read path {path} from FAR file {}", far_file_path.display())
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};
    use std::collections::BTreeMap;
    use std::io::Read;

    fn create_test_far(file_name: &str, content: &[u8]) -> Vec<u8> {
        let mut path_content_map: BTreeMap<&str, (u64, Box<dyn Read>)> = BTreeMap::new();
        path_content_map.insert(file_name, (content.len().try_into().unwrap(), Box::new(content)));
        let mut far_contents = Vec::new();
        fuchsia_archive::write(&mut far_contents, path_content_map).unwrap();
        far_contents
    }

    #[test]
    fn test_far_cat_impl() -> Result<()> {
        let file_data = b"hello world";
        let far_contents = create_test_far("foo/bar", file_data);

        let mut cursor = std::io::Cursor::new(far_contents);
        let mut reader = far::Reader::new(&mut cursor).unwrap();

        let bytes = far_cat_impl(&mut reader, "foo/bar", std::path::Path::new("fake.far"))?;
        assert_eq!(bytes, file_data);
        Ok(())
    }

    #[test]
    fn test_far_cat_machine_format() -> Result<()> {
        let file_data = b"hello world";
        let buffers = TestBuffers::default();
        let mut writer = <FarCatTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = to_far_cat_result(file_data);
        writer.machine(&result)?;

        let expected = format!("{}\n", serde_json::to_string(&result).unwrap());
        let stdout = buffers.into_stdout_str();
        assert_eq!(stdout, expected);

        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        <FarCatTool as FfxMain>::Writer::verify_schema(&value).unwrap();

        Ok(())
    }
}
