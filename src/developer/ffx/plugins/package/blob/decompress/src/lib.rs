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
pub struct DecompressTool {
    #[command]
    pub cmd: ffx_package_blob_decompress_args::DecompressCommand,
}

fho::embedded_plugin!(DecompressTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for DecompressTool {
    type Writer = VerifiedMachineWriter<CommandResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> Result<()> {
        match self.cmd_blob_decompress(&mut writer).await {
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

impl DecompressTool {
    pub async fn cmd_blob_decompress(
        self,
        writer: &mut <Self as fho::FfxMain>::Writer,
    ) -> Result<Vec<DecompressEntry>> {
        if self.cmd.paths.is_empty() {
            return_user_error!("Missing file path");
        }

        let output_is_file = !self.cmd.output.is_dir() && self.cmd.paths.len() == 1;

        let entries = self.cmd.paths.into_par_iter().map(|src| {
            DecompressEntry::new(src.clone(), &self.cmd.output, output_is_file)
                .with_context(|| format!("failed to decompress blob {src}"))
        });

        if writer.is_machine() {
            // Fails if any of the files fail
            entries.collect::<anyhow::Result<Vec<_>>>().map_err(|e| user_error!("{e:?}"))
        } else {
            // Only fails if writing to stdout/stderr fails.
            for entry in entries.collect::<Vec<_>>() {
                match entry {
                    Ok(DecompressEntry { src, dst }) => {
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
    Ok { data: Vec<DecompressEntry> },
    /// Unexpected error with string denoting error message.
    UnexpectedError(String),
    /// A known error that can be reported to the user.
    UserError(String),
}
#[derive(Serialize, JsonSchema)]
pub struct DecompressEntry {
    #[schemars(with = "String")]
    src: Utf8PathBuf,
    #[schemars(with = "String")]
    dst: Utf8PathBuf,
}

impl DecompressEntry {
    fn new(src: Utf8PathBuf, output: &Utf8Path, output_is_file: bool) -> anyhow::Result<Self> {
        let dst = if output_is_file {
            output.to_owned()
        } else {
            let blob_name = src.file_name().ok_or_else(|| anyhow::anyhow!("missing file name"))?;
            output.join(blob_name)
        };

        let bytes = std::fs::read(&src).context("read file")?;
        let out = File::create(&dst).with_context(|| format!("create file {dst}"))?;
        delivery_blob::decompress_to(&bytes, out)
            .with_context(|| format!("decompress to {dst}"))?;
        Ok(Self { src, dst })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use ffx_package_blob_decompress_args::DecompressCommand;
    use tempfile::TempDir;

    fn create_test_files(name_content_pairs: &[(&str, impl AsRef<[u8]>)]) -> TempDir {
        let dir = TempDir::new().unwrap();

        for (name, content) in name_content_pairs {
            let path = dir.path().join(name);
            let compressed =
                delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, content.as_ref());
            std::fs::write(path, compressed).unwrap();
        }

        dir
    }

    #[fuchsia::test]
    async fn decompress_single_blob() {
        let test_data = [("first_file", &"0123456789".repeat(1024))];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();

        let cmd = DecompressCommand { paths, output: dir_path.join("decompressed") };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <DecompressTool as FfxMain>::Writer::new_test(None, &buffers);
        DecompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!("{0}/first_file => {0}/decompressed\n", dir_path);

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        assert_eq!(
            std::fs::read(&dir_path.join("decompressed")).unwrap(),
            test_data[0].1.as_bytes()
        );
    }

    #[fuchsia::test]
    async fn decompress_multiple_blobs() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();
        let out_dir = dir_path.join("decompressed");
        std::fs::create_dir(&out_dir).unwrap();

        let cmd = DecompressCommand { paths, output: out_dir.clone() };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <DecompressTool as FfxMain>::Writer::new_test(None, &buffers);
        DecompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            "\
{0}/first_file => {0}/decompressed/first_file
{0}/second_file => {0}/decompressed/second_file
{0}/third_file => {0}/decompressed/third_file
",
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        for (name, content) in test_data {
            assert_eq!(std::fs::read(&out_dir.join(name)).unwrap(), content.as_bytes());
        }
    }

    #[fuchsia::test]
    async fn decompress_multiple_blobs_machine_mode() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();
        let out_dir = dir_path.join("decompressed");
        std::fs::create_dir(&out_dir).unwrap();

        let cmd = DecompressCommand { paths, output: out_dir.clone() };
        let buffers = ffx_writer::TestBuffers::default();
        let writer =
            <DecompressTool as FfxMain>::Writer::new_test(Some(ffx_writer::Format::Json), &buffers);
        DecompressTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            concat!(
                r#"{{"ok":{{"data":["#,
                r#"{{"src":"{0}/first_file","dst":"{0}/decompressed/first_file"}},"#,
                r#"{{"src":"{0}/second_file","dst":"{0}/decompressed/second_file"}},"#,
                r#"{{"src":"{0}/third_file","dst":"{0}/decompressed/third_file"}}"#,
                r#"]}}}}"#,
                "\n"
            ),
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");

        for (name, content) in test_data {
            assert_eq!(std::fs::read(&out_dir.join(name)).unwrap(), content.as_bytes());
        }
    }

    #[fuchsia::test]
    async fn file_not_found() {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = DecompressCommand { paths: vec![NAME.into()], output: ".".into() };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <DecompressTool as FfxMain>::Writer::new_test(None, &buffers);

        DecompressTool { cmd }.main(writer).await.expect("success");
        let (out, err) = buffers.into_strings();
        assert_eq!(
            err,
            format!(
                "failed to decompress blob {NAME}\n\nCaused by:\n    0: read file\n    1: No such file or directory (os error 2)\n"
            )
        );
        assert_eq!(out, "");
    }

    #[fuchsia::test]
    async fn file_not_found_machine_mode() {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = DecompressCommand { paths: vec![NAME.into()], output: ".".into() };
        let buffers = ffx_writer::TestBuffers::default();
        let writer =
            <DecompressTool as FfxMain>::Writer::new_test(Some(ffx_writer::Format::Json), &buffers);

        assert_matches!(DecompressTool { cmd }.main(writer).await, Err(fho::Error::User(_)));
        let (out, err) = buffers.into_strings();
        assert_eq!(
            out,
            format!(
                "{{\"user_error\":\"failed to decompress blob {NAME}\\n\\nCaused by:\\n    0: read file\\n    1: No such file or directory (os error 2)\"}}\n"
            )
        );
        assert_eq!(err, "");
    }
}
