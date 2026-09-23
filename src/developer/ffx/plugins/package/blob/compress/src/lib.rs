// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use camino::{Utf8Path, Utf8PathBuf};
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{Error, FfxMain, FfxTool, Result, return_user_error, user_error};
use rayon::prelude::*;
use schemars::JsonSchema;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;

#[derive(FfxTool)]
#[target(None)]
pub struct CompressTool {
    #[command]
    pub cmd: ffx_package_blob_compress_args::CompressCommand,
}

fho::embedded_plugin!(CompressTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for CompressTool {
    type Writer = VerifiedMachineWriter<CommandResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> Result<()> {
        match self.cmd_blob_compress(&mut writer).await {
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

impl CompressTool {
    pub async fn cmd_blob_compress(
        self,
        writer: &mut <Self as fho::FfxMain>::Writer,
    ) -> Result<Vec<CompressEntry>> {
        if self.cmd.paths.is_empty() {
            return_user_error!("Missing file path");
        }

        let output_is_file = !self.cmd.output.is_dir() && self.cmd.paths.len() == 1;

        let entries = self.cmd.paths.into_par_iter().map(|src| {
            CompressEntry::new(
                src.clone(),
                &self.cmd.output,
                output_is_file,
                self.cmd.hash_as_name,
                self.cmd.delivery_type,
            )
            .with_context(|| {
                format!(
                    "failed to compress {src} to delivery blob type {}",
                    self.cmd.delivery_type as u32
                )
            })
        });

        if writer.is_machine() {
            // Fails if any of the files fail
            entries.collect::<anyhow::Result<Vec<_>>>().map_err(|e| user_error!("{e:?}"))
        } else {
            // Only fails if writing to stdout/stderr fails.
            for entry in entries.collect::<Vec<_>>() {
                match entry {
                    Ok(CompressEntry { src, dst }) => {
                        writeln!(writer, "{src} => {dst}").map_err(|e| user_error!(e))?;
                    }
                    Err(e) => {
                        // Display the error and continue.
                        writeln!(writer.stderr(), "{e:?}").map_err(|e| user_error!(e))?;
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
    Ok { data: Vec<CompressEntry> },
    /// Unexpected error with string denoting error message.
    UnexpectedError(String),
    /// A known error that can be reported to the user.
    UserError(String),
}
#[derive(Serialize, JsonSchema)]
pub struct CompressEntry {
    #[schemars(with = "String")]
    src: Utf8PathBuf,
    #[schemars(with = "String")]
    dst: Utf8PathBuf,
}

impl CompressEntry {
    fn new(
        src: Utf8PathBuf,
        output: &Utf8Path,
        output_is_file: bool,
        hash_as_name: bool,
        delivery_type: delivery_blob::DeliveryBlobType,
    ) -> anyhow::Result<Self> {
        let bytes = std::fs::read(&src).context("read file")?;

        let dst = if output_is_file {
            output.to_owned()
        } else {
            if hash_as_name {
                output.join(fuchsia_merkle::root_from_slice(&bytes).to_string())
            } else {
                let blob_name =
                    src.file_name().ok_or_else(|| anyhow::anyhow!("missing file name"))?;
                output.join(blob_name)
            }
        };

        let out = File::create(&dst).with_context(|| format!("create file {dst}"))?;
        delivery_blob::generate_to(delivery_type, &bytes, out)
            .with_context(|| format!("generate delivery blob to {dst}"))?;

        Ok(Self { src, dst })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use delivery_blob::DeliveryBlobType::Type1;
    use ffx_package_blob_compress_args::CompressCommand;
    use tempfile::TempDir;

    fn create_test_files(name_content_pairs: &[(&str, impl AsRef<[u8]>)]) -> TempDir {
        let dir = TempDir::new().unwrap();

        for (name, content) in name_content_pairs {
            let path = dir.path().join(name);
            std::fs::write(path, content).unwrap();
        }

        dir
    }

    #[fuchsia::test]
    async fn compress_single_blob() {
        let test_data = [("first_file", &"0123456789".repeat(1024))];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();

        let cmd = CompressCommand {
            paths,
            output: dir_path.join("compressed"),
            hash_as_name: false,
            delivery_type: Type1,
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <CompressTool as FfxMain>::Writer::new_test(None, &buffers);
        CompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!("{0}/first_file => {0}/compressed\n", dir_path);

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        let expected_bytes = delivery_blob::generate(Type1, test_data[0].1.as_ref());
        assert_eq!(std::fs::read(&dir_path.join("compressed")).unwrap(), expected_bytes);
    }

    #[fuchsia::test]
    async fn compress_multiple_blobs() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();
        let out_dir = dir_path.join("compressed");
        std::fs::create_dir(&out_dir).unwrap();

        let cmd = CompressCommand {
            paths,
            output: out_dir.clone(),
            hash_as_name: false,
            delivery_type: Type1,
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <CompressTool as FfxMain>::Writer::new_test(None, &buffers);
        CompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            "\
{0}/first_file => {0}/compressed/first_file
{0}/second_file => {0}/compressed/second_file
{0}/third_file => {0}/compressed/third_file
",
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        for (name, content) in test_data {
            let expected_bytes = delivery_blob::generate(Type1, content.as_bytes());
            assert_eq!(std::fs::read(&out_dir.join(name)).unwrap(), expected_bytes);
        }
    }

    #[fuchsia::test]
    async fn compress_multiple_blobs_hash_as_name() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();
        let out_dir = dir_path.join("compressed");
        std::fs::create_dir(&out_dir).unwrap();

        let cmd = CompressCommand {
            paths,
            output: out_dir.clone(),
            hash_as_name: true,
            delivery_type: Type1,
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <CompressTool as FfxMain>::Writer::new_test(None, &buffers);
        CompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            "\
{0}/first_file => {0}/compressed/15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b
{0}/second_file => {0}/compressed/e6a73dbd2d88e51ccdaa648cbf49b3939daac8a3e370169bc85e0324a41adbc2
{0}/third_file => {0}/compressed/f5a0dff4578d0150d3dace71b08733d5cd8cbe63a322633445c9ff0d9041b9c4
",
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        for (_name, content) in test_data {
            let expected_bytes = delivery_blob::generate(Type1, content.as_bytes());
            assert_eq!(
                std::fs::read(&out_dir.join(fuchsia_merkle::root_from_slice(content).to_string()))
                    .unwrap(),
                expected_bytes
            );
        }
    }

    #[fuchsia::test]
    async fn compress_multiple_blobs_machine_mode() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();
        let out_dir = dir_path.join("compressed");
        std::fs::create_dir(&out_dir).unwrap();

        let cmd = CompressCommand {
            paths,
            output: out_dir.clone(),
            hash_as_name: false,
            delivery_type: Type1,
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer =
            <CompressTool as FfxMain>::Writer::new_test(Some(ffx_writer::Format::Json), &buffers);
        CompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            concat!(
                r#"{{"ok":{{"data":["#,
                r#"{{"src":"{0}/first_file","dst":"{0}/compressed/first_file"}},"#,
                r#"{{"src":"{0}/second_file","dst":"{0}/compressed/second_file"}},"#,
                r#"{{"src":"{0}/third_file","dst":"{0}/compressed/third_file"}}"#,
                r#"]}}}}"#,
                "\n"
            ),
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        for (name, content) in test_data {
            let expected_bytes = delivery_blob::generate(Type1, content.as_bytes());
            assert_eq!(std::fs::read(&out_dir.join(name)).unwrap(), expected_bytes);
        }
    }

    #[fuchsia::test]
    async fn file_not_found() {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = CompressCommand {
            paths: vec![NAME.into()],
            output: ".".into(),
            hash_as_name: false,
            delivery_type: Type1,
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <CompressTool as FfxMain>::Writer::new_test(None, &buffers);

        CompressTool { cmd }.main(writer).await.expect("success");
        let (out, err) = buffers.into_strings();
        assert_eq!(
            err,
            format!(
                "failed to compress {NAME} to delivery blob type 1\n\nCaused by:\n    0: read file\n    1: No such file or directory (os error 2)\n"
            )
        );
        assert_eq!(out, "");
    }

    #[fuchsia::test]
    async fn file_not_found_machine_mode() {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = CompressCommand {
            paths: vec![NAME.into()],
            output: ".".into(),
            hash_as_name: false,
            delivery_type: Type1,
        };
        let buffers = ffx_writer::TestBuffers::default();
        let writer =
            <CompressTool as FfxMain>::Writer::new_test(Some(ffx_writer::Format::Json), &buffers);

        assert_matches!(CompressTool { cmd }.main(writer).await, Err(fho::Error::User(_)));
        let (out, err) = buffers.into_strings();
        assert_eq!(
            out,
            format!(
                "{{\"user_error\":\"failed to compress {NAME} to delivery blob type 1\\n\\nCaused by:\\n    0: read file\\n    1: No such file or directory (os error 2)\"}}\n"
            )
        );
        assert_eq!(err, "");
    }
}
