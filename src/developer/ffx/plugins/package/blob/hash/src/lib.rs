// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use camino::Utf8PathBuf;
use ffx_writer::{ToolIO as _, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, return_user_error, user_error};
use rayon::prelude::*;
use schemars::JsonSchema;
use serde::Serialize;
use std::fs::File;
use std::io::prelude::*;

#[derive(FfxTool)]
#[target(None)]
pub struct BlobHashTool {
    #[command]
    pub cmd: ffx_package_blob_hash_args::HashCommand,
}

fho::embedded_plugin!(BlobHashTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for BlobHashTool {
    type Writer = VerifiedMachineWriter<CommandResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as fho::FfxMain>::Writer) -> fho::Result<()> {
        match self.cmd_blob_hash(&mut writer).await {
            Ok(data) => {
                writer.machine(&CommandResult::Ok { data })?;
                Ok(())
            }
            Err(e @ fho::Error::User(_)) => {
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

impl BlobHashTool {
    pub async fn cmd_blob_hash(
        self,
        writer: &mut <Self as fho::FfxMain>::Writer,
    ) -> fho::Result<Vec<BlobHashEntry>> {
        if self.cmd.paths.is_empty() {
            return_user_error!("Missing file path");
        }

        let entries = self.cmd.paths.into_par_iter().map(|path| {
            BlobHashEntry::new(path.clone(), self.cmd.uncompressed)
                .with_context(|| format!("failed to hash blob {path}"))
        });

        if writer.is_machine() {
            // Fails if any of the files don't exist.
            entries.collect::<anyhow::Result<Vec<_>>>().map_err(|e| user_error!("{e:?}"))
        } else {
            // Only fails if writing to stdout/stderr fails.
            for entry in entries.collect::<Vec<_>>() {
                match entry {
                    Ok(BlobHashEntry { path, hash }) => {
                        writeln!(writer, "{hash}  {path}").map_err(|e| user_error!(e))?;
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
    Ok { data: Vec<BlobHashEntry> },
    /// Unexpected error with string denoting error message.
    UnexpectedError(String),
    /// A known error that can be reported to the user.
    UserError(String),
}
#[derive(Serialize, JsonSchema)]
pub struct BlobHashEntry {
    #[schemars(with = "String")]
    path: Utf8PathBuf,
    hash: String,
}

impl BlobHashEntry {
    fn new(path: Utf8PathBuf, uncompressed: bool) -> anyhow::Result<Self> {
        let hash = if uncompressed {
            let file = File::open(&path).context("open file")?;

            fuchsia_merkle::root_from_reader(file).context("read file")?
        } else {
            let bytes = std::fs::read(&path).context("read file")?;
            delivery_blob::calculate_digest(&bytes).context("calculate digest")?
        };
        Ok(BlobHashEntry { path, hash: hash.to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use ffx_package_blob_hash_args::HashCommand;
    use tempfile::TempDir;

    fn create_test_files(name_content_pairs: &[(&str, impl AsRef<[u8]>)]) -> TempDir {
        let dir = TempDir::new().unwrap();

        for (name, content) in name_content_pairs {
            let path = dir.path().join(name);
            std::fs::write(path, content).unwrap();
        }

        dir
    }

    /// * Create a few temp files.
    /// * Pass those paths to the hash command.
    /// * Verify that the output is correct.
    #[fuchsia::test]
    async fn validate_output() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();

        let cmd = HashCommand { paths, uncompressed: true };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <BlobHashTool as FfxMain>::Writer::new_test(None, &buffers);
        BlobHashTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            "\
15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b  {0}/first_file
e6a73dbd2d88e51ccdaa648cbf49b3939daac8a3e370169bc85e0324a41adbc2  {0}/second_file
f5a0dff4578d0150d3dace71b08733d5cd8cbe63a322633445c9ff0d9041b9c4  {0}/third_file
",
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");
    }

    /// * Create a few temp delivery blobs.
    /// * Pass those paths to the hash command.
    /// * Verify that the output is correct.
    #[fuchsia::test]
    async fn validate_delivery_blob_output() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ]
        .map(|(name, content)| {
            (
                name,
                delivery_blob::generate(delivery_blob::DeliveryBlobType::Type1, content.as_bytes()),
            )
        });

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();

        let cmd = HashCommand { paths, uncompressed: false };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <BlobHashTool as FfxMain>::Writer::new_test(None, &buffers);
        BlobHashTool { cmd }.main(writer).await.expect("success");

        let (out, err) = &buffers.into_strings();

        let expected_output = format!(
            "\
15ec7bf0b50732b49f8228e07d24365338f9e3ab994b00af08e5a3bffe55fd8b  {0}/first_file
e6a73dbd2d88e51ccdaa648cbf49b3939daac8a3e370169bc85e0324a41adbc2  {0}/second_file
f5a0dff4578d0150d3dace71b08733d5cd8cbe63a322633445c9ff0d9041b9c4  {0}/third_file
",
            dir_path,
        );

        assert_eq!(out, &expected_output);
        assert_eq!(err, "");
    }

    /// * Create a few temp files.
    /// * Pass those paths to the hash command,
    ///   producing **machine-readable** output.
    /// * Verify that the output is correct.
    #[fuchsia::test]
    async fn validate_output_machine_mode() {
        let test_data = [
            ("first_file", ""),
            ("second_file", "Hello world!"),
            ("third_file", &"0123456789".repeat(1024)),
        ];

        let dir = create_test_files(&test_data);
        let dir_path = Utf8PathBuf::from_path_buf(dir.path().to_owned()).unwrap();
        let paths = test_data.iter().map(|(name, _)| dir_path.join(name)).collect();

        let cmd = HashCommand { paths, uncompressed: true };
        let buffers = ffx_writer::TestBuffers::default();
        let writer =
            <BlobHashTool as FfxMain>::Writer::new_test(Some(ffx_writer::Format::Json), &buffers);
        BlobHashTool { cmd }.main(writer).await.expect("success");

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
            dir_path,
        );

        assert_eq!(out, expected_output);
        assert_eq!(err, "");
    }

    /// * Run the command on a file that doesn't exist.
    /// * Check for a specific error message.
    #[fuchsia::test]
    async fn file_not_found() {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = HashCommand { paths: vec![NAME.into()], uncompressed: true };
        let buffers = ffx_writer::TestBuffers::default();
        let writer = <BlobHashTool as FfxMain>::Writer::new_test(None, &buffers);

        BlobHashTool { cmd }.main(writer).await.expect("success");
        let (out, err) = buffers.into_strings();
        assert_eq!(
            err,
            format!(
                "failed to hash blob {NAME}\n\nCaused by:\n    0: open file\n    1: No such file or directory (os error 2)\n"
            )
        );
        assert_eq!(out, "");
    }

    /// * Run the command **in machine mode** on a file that doesn't exist.
    /// * It should return an error, **instead of** printing to stderr.
    #[fuchsia::test]
    async fn file_not_found_machine_mode() {
        const NAME: &str = "filename_that_does_not_exist";

        let cmd = HashCommand { paths: vec![NAME.into()], uncompressed: true };
        let buffers = ffx_writer::TestBuffers::default();
        let writer =
            <BlobHashTool as FfxMain>::Writer::new_test(Some(ffx_writer::Format::Json), &buffers);

        assert_matches!(BlobHashTool { cmd }.main(writer).await, Err(fho::Error::User(_)));
        let (out, err) = buffers.into_strings();
        assert_eq!(
            out,
            format!(
                "{{\"user_error\":\"failed to hash blob {NAME}\\n\\nCaused by:\\n    0: open file\\n    1: No such file or directory (os error 2)\"}}\n"
            )
        );
        assert_eq!(err, "");
    }
}
