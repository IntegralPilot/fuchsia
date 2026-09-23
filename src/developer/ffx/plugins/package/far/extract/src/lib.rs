// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use camino::Utf8PathBuf;
use ffx_package_far_extract_args::ExtractCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool, Result};
use fuchsia_archive as far;
use std::fs::{self, File};
use std::io::Write as _;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ExtractResult {
    pub extracted_files: Vec<String>,
}

#[derive(FfxTool)]
#[target(None)]
pub struct ExtractTool {
    #[command]
    pub cmd: ExtractCommand,
}

fho::embedded_plugin!(ExtractTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for ExtractTool {
    type Writer = VerifiedMachineWriter<ExtractResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        let extracted = self.cmd_extract()?;

        if writer.is_machine() {
            let extracted_files = extracted.into_iter().map(|p| p.into_string()).collect();
            writer.machine(&ExtractResult { extracted_files }).bug()?;
        } else if self.cmd.verbose {
            for path in extracted {
                writeln!(writer, "{}", path).bug()?;
            }
        }
        Ok(())
    }
}

impl ExtractTool {
    pub fn cmd_extract(&self) -> Result<Vec<Utf8PathBuf>> {
        let far_file = File::open(&self.cmd.far_file)
            .with_user_message(|| format!("failed to open file {}", self.cmd.far_file.display()))?;
        let mut reader = far::Utf8Reader::new(far_file).with_user_message(|| {
            format!("failed to parse FAR file {}", self.cmd.far_file.display())
        })?;

        // If no paths are given on the command line, extract everything.
        let paths = if self.cmd.paths.is_empty() {
            reader.list().map(|entry| Utf8PathBuf::from(entry.path())).collect()
        } else {
            self.cmd.paths.clone()
        };

        let mut extracted = Vec::new();
        for path in paths {
            // Note that this implicitly does some validation on `path`.
            //
            // E.g., it can't be:
            // * empty,
            // * start or end with "/",
            // * contain "." or ".." as a segment.
            let bytes = reader.read_file(path.as_str()).with_user_message(|| {
                format!("failed to read {path} from {}", self.cmd.far_file.display())
            })?;

            let out_path = self.cmd.output_dir.join(&path);
            let parent = out_path.parent().expect("`path` must be non-empty");
            fs::create_dir_all(parent)
                .with_user_message(|| format!("failed to create directory {parent}"))?;
            fs::write(&out_path, &bytes)
                .with_user_message(|| format!("failed to write file {out_path}"))?;

            extracted.push(out_path);
        }

        Ok(extracted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};
    use std::collections::BTreeMap;
    use std::io::Read;
    use tempfile::TempDir;

    fn create_test_far(file_name: &str, content: &[u8]) -> Vec<u8> {
        let mut path_content_map: BTreeMap<&str, (u64, Box<dyn Read>)> = BTreeMap::new();
        path_content_map.insert(file_name, (content.len().try_into().unwrap(), Box::new(content)));
        let mut far_contents = Vec::new();
        fuchsia_archive::write(&mut far_contents, path_content_map).unwrap();
        far_contents
    }

    #[test]
    fn test_extract() -> Result<()> {
        let tmp_dir = TempDir::new().unwrap();
        let far_contents = create_test_far("foo/bar", b"hello world");
        let far_path = tmp_dir.path().join("test.far");
        fs::write(&far_path, far_contents).unwrap();

        let output_dir = Utf8PathBuf::from_path_buf(tmp_dir.path().join("out")).unwrap();

        let cmd = ExtractCommand {
            far_file: far_path,
            output_dir: output_dir.clone(),
            paths: vec![],
            verbose: true,
        };

        let tool = ExtractTool { cmd };

        let extracted = tool.cmd_extract().unwrap();

        let expected_out_file = output_dir.join("foo/bar");
        assert!(expected_out_file.exists());
        let content = fs::read_to_string(expected_out_file).unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(extracted.len(), 1);
        assert!(extracted[0].as_str().contains("foo/bar"));

        Ok(())
    }

    #[test]
    fn test_extract_machine_format() -> Result<()> {
        let tmp_dir = TempDir::new().unwrap();
        let output_dir = tmp_dir.path().join("out");
        let extracted_path = output_dir.join("foo/bar");

        let buffers = TestBuffers::default();
        let mut writer = <ExtractTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let extracted_files = vec![extracted_path.to_string_lossy().to_string()];
        let result = ExtractResult { extracted_files };
        writer.machine(&result)?;

        let expected = format!("{}\n", serde_json::to_string(&result).unwrap());
        let stdout = buffers.into_stdout_str();
        assert_eq!(stdout, expected);

        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        <ExtractTool as FfxMain>::Writer::verify_schema(&value).unwrap();

        Ok(())
    }
}
