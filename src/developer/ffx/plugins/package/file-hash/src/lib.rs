// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use ffx_package_file_hash_args::FileHashCommand;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{Error, FfxContext, FfxMain, FfxTool, Result, return_user_error, user_error};
use rayon::prelude::*;
use schemars::JsonSchema;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;

#[derive(FfxTool)]
#[target(None)]
pub struct FileHashTool {
    #[command]
    pub cmd: FileHashCommand,
}

fho::embedded_plugin!(FileHashTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for FileHashTool {
    type Writer = VerifiedMachineWriter<CommandResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> Result<()> {
        match self.cmd_file_hash(&mut writer).await {
            Ok(data) => {
                writer.machine(&CommandResult::Ok { data })?;
                Ok(())
            }
            Err(e @ Error::User(_)) => {
                writer.machine(&CommandResult::UserError(e.to_string()))?;
                Err(e)
            }
            Err(e) => {
                writer.machine(&CommandResult::UnexpectedError(e.to_string()))?;
                Err(e)
            }
        }
    }
}

impl FileHashTool {
    pub async fn cmd_file_hash(
        &self,
        writer: &mut <Self as fho::FfxMain>::Writer,
    ) -> Result<Vec<FileHashEntry>> {
        if self.cmd.paths.is_empty() {
            return_user_error!("Missing file path");
        }

        let entries = self.cmd.paths.clone().into_par_iter().map(FileHashEntry::new);

        if writer.is_machine() {
            // Fails if any of the files don't exist.
            let entries: Vec<_> = entries.collect::<Result<_>>()?;
            return Ok(entries);
        } else {
            // Only fails if writing to stdout/stderr fails.
            for entry in entries.collect::<Vec<_>>() {
                match entry {
                    Ok(FileHashEntry { path, hash }) => {
                        writeln!(writer, "{}  {}", hash, path.display()).bug()?;
                    }
                    Err(e) => {
                        // Display the error and continue.
                        writeln!(writer.stderr(), "{e}").bug()?;
                    }
                }
            }
            Ok(vec![])
        }
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CommandResult {
    /// Success.
    Ok { data: Vec<FileHashEntry> },
    /// Unexpected error with string denoting error message.
    UnexpectedError(String),
    /// A known error that can be reported to the user.
    UserError(String),
}
#[derive(Serialize, JsonSchema)]
pub struct FileHashEntry {
    path: PathBuf,
    hash: String,
}

impl FileHashEntry {
    /// Compute the merkle root hash of a file.
    fn new(path: PathBuf) -> Result<Self> {
        let file = File::open(&path)
            .map_err(|e| user_error!("failed to open file {}: {e}", path.display()))?;

        let root = fuchsia_merkle::root_from_reader(file)
            .with_context(|| format!("failed to read file: {}", path.display()))?;

        Ok(Self { path, hash: root.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_writer::{Format, TestBuffers};
    use tempfile::TempDir;

    fn create_test_files(name_content_pairs: &[(&str, &str)]) -> anyhow::Result<TempDir> {
        let dir = TempDir::new()?;

        for (name, content) in name_content_pairs {
            let path = dir.path().join(name);
            let mut file = File::create(path)?;
            write!(file, "{content}")?;
        }

        Ok(dir)
    }

    /// * Create a few temp files.
    /// * Pass those paths to the file-hash command.
    /// * Verify that the output is correct.
    #[fuchsia::test]
    async fn validate_output() -> Result<()> {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data)?;
        let paths = test_data.iter().map(|(name, _)| dir.path().join(name)).collect();

        let cmd = FileHashCommand { paths };
        let tool = FileHashTool { cmd };
        let buffers = TestBuffers::default();
        let writer = <FileHashTool as FfxMain>::Writer::new_test(None, &buffers);
        tool.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            "\
15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b  {0}/first_file
e6a73dbd2d88e51ccdaa648cbf49b3939daac8a3e370169bc85e0324a41adbc2  {0}/second_file
f5a0dff4578d0150d3dace71b08733d5cd8cbe63a322633445c9ff0d9041b9c4  {0}/third_file
",
            dir.path().display(),
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        // Clean up temp files.
        drop(dir);

        Ok(())
    }

    /// * Create a few temp files.
    /// * Pass those paths to the file-hash command,
    ///   producing **machine-readable** output.
    /// * Verify that the output is correct.
    #[fuchsia::test]
    async fn validate_output_machine_mode() -> Result<()> {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data)?;
        let paths = test_data.iter().map(|(name, _)| dir.path().join(name)).collect();

        let cmd = FileHashCommand { paths };
        let tool = FileHashTool { cmd };
        let buffers = TestBuffers::default();
        let writer = <FileHashTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);
        tool.main(writer).await.expect("success");

        let (out, err) = buffers.into_strings();
        let expected_output = format!(
            concat!(
                r#"{{"ok":{{"data":["#,
                r#"{{"path":"{0}/first_file","hash":"15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b"}},"#,
                r#"{{"path":"{0}/second_file","hash":"e6a73dbd2d88e51ccdaa648cbf49b3939daac8a3e370169bc85e0324a41adbc2"}},"#,
                r#"{{"path":"{0}/third_file","hash":"f5a0dff4578d0150d3dace71b08733d5cd8cbe63a322633445c9ff0d9041b9c4"}}"#,
                r#"]}}}}"#,
                "\n"
            ),
            dir.path().display(),
        );

        assert_eq!(out, expected_output);
        assert_eq!(err, "");

        // Clean up temp files.
        drop(dir);

        Ok(())
    }

    /// * Run the command on a file that doesn't exist.
    /// * Check for a specific error message.
    #[fuchsia::test]
    async fn file_not_found() -> Result<()> {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = FileHashCommand { paths: vec![NAME.into()] };
        let tool = FileHashTool { cmd };
        let buffers = TestBuffers::default();
        let writer = <FileHashTool as FfxMain>::Writer::new_test(None, &buffers);

        tool.main(writer).await.expect("success");
        let (out, err) = buffers.into_strings();
        assert_eq!(
            err,
            format!("failed to open file {NAME}: No such file or directory (os error 2)\n")
        );
        assert_eq!(out, "");

        Ok(())
    }

    /// * Run the command **in machine mode** on a file that doesn't exist.
    /// * It should return an error, **instead of** printing to stderr.
    #[fuchsia::test]
    async fn file_not_found_machine_mode() -> Result<()> {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = FileHashCommand { paths: vec![NAME.into()] };
        let tool = FileHashTool { cmd };
        let buffers = TestBuffers::default();
        let writer = <FileHashTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let res = tool.main(writer).await;
        let (out, err) = buffers.into_strings();
        match res {
            Ok(_) => panic!("Unexpected success"),
            Err(_) => assert_eq!(
                out,
                format!(
                    "{{\"user_error\":\"failed to open file {NAME}: No such file or directory (os error 2)\"}}\n"
                )
            ),
        };

        assert_eq!(err, "");

        Ok(())
    }
}
