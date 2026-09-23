// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{Datelike, Local, Timelike};
use fdomain_fuchsia_io as fio;
use fdomain_fuchsia_ui_composition::{
    ScreenshotFormat, ScreenshotProxy, ScreenshotTakeFileRequest,
};
use ffx_target_screenshot_args::{Format, ScreenshotCommand};
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxContext, FfxMain, FfxTool};
use futures::stream::{FuturesOrdered, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter, Result as FmtResult};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use target_holders::moniker;

// Reads all of the contents of the given file from the current seek
// offset to end of file, returning the content. It errors if the seek pointer
// starts at an offset that results in reading less number of bytes read
// from the initial seek offset by the first request made by this function to EOF.
pub async fn read_data(file: &fio::FileProxy) -> Result<Vec<u8>> {
    // Number of concurrent read operations to maintain (aim for a 128kb
    // in-flight buffer, divided by the fuchsia.io chunk size). On a short range
    // network, 64kb should be more than sufficient, but on an LFN such as a
    // work-from-home scenario, having some more space further optimizes
    // performance.
    const CONCURRENCY: u64 = 131072 / fio::MAX_BUF;

    let mut out = Vec::new();

    let res = file
        .get_attributes(fio::NodeAttributesQuery::CONTENT_SIZE)
        .await
        .map_err(|e| anyhow!("Failed get_attributes wire call: {e}"))?;
    let (_mutable_attributes, immutable_attributes) =
        res.map_err(|e| anyhow!("Failed get_attributes of file: {e}"))?;
    let content_size = immutable_attributes
        .content_size
        .ok_or_else(|| anyhow!("Failed to get content size of file"))?;

    let mut queue = FuturesOrdered::new();

    for _ in 0..CONCURRENCY {
        queue.push_back(file.read(fio::MAX_BUF));
    }

    loop {
        if let Some(resp) = queue.next().await {
            let res = resp.context("read stream closed prematurely")?;
            let mut bytes: Vec<u8> = res
                .map_err(|status: i32| fho::Error::from(anyhow!("read error: status={status}")))?;

            if bytes.is_empty() {
                break;
            }
            out.append(&mut bytes);
        }

        while queue.len() < CONCURRENCY.try_into().unwrap() {
            queue.push_back(file.read(fio::MAX_BUF));
        }
    }

    if out.len() != usize::try_from(content_size).bug_context("failed to convert to usize")? {
        return Err(anyhow!(
            "Error: Expected {} bytes, but instead read {} bytes",
            content_size,
            out.len()
        )
        .into());
    }

    Ok(out)
}

#[derive(FfxTool)]
pub struct ScreenshotTool {
    #[command]
    cmd: ScreenshotCommand,
    #[with(moniker("/core/ui"))]
    screenshot_proxy: ScreenshotProxy,
}

fho::embedded_plugin!(ScreenshotTool);

#[async_trait(?Send)]
impl FfxMain for ScreenshotTool {
    type Writer = VerifiedMachineWriter<ScreenshotOutput>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        screenshot_impl(self.screenshot_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn screenshot_impl<W: ToolIO<OutputItem = ScreenshotOutput>>(
    screenshot_proxy: ScreenshotProxy,
    cmd: ScreenshotCommand,
    writer: &mut W,
) -> Result<()> {
    let mut screenshot_file_path = match cmd.output_directory {
        Some(file_dir) => {
            let dir = Path::new(&file_dir);
            if !dir.is_dir() {
                bail!("ERROR: Path provided is not a directory");
            }
            dir.to_path_buf().join("screenshot")
        }
        None => {
            let dir = default_output_dir();
            fs::create_dir_all(&dir)?;
            dir.join("screenshot")
        }
    };

    let format = match cmd.format {
        Format::PNG => {
            screenshot_file_path.set_extension("png");
            ScreenshotFormat::Png
        }
        Format::BGRA => {
            screenshot_file_path.set_extension("bgra");
            ScreenshotFormat::BgraRaw
        }
        Format::RGBA => {
            screenshot_file_path.set_extension("rgba");
            ScreenshotFormat::BgraRaw
        }
    };

    // TODO(https://fxbug.dev/42060028): Use rgba format when available.
    let screenshot_response = screenshot_proxy
        .take_file(ScreenshotTakeFileRequest { format: Some(format), ..Default::default() })
        .await
        .map_err(|e| {
            fho::Error::from(anyhow!("Error: Could not get the screenshot from the target: {e:?}"))
        })?;

    let img_size = screenshot_response.size.expect("no data size returned from screenshot");
    let client_end = screenshot_response.file.expect("no file returned from screenshot");

    let file_proxy = client_end.into_proxy();

    let mut img_data = read_data(&file_proxy).await?;
    // VMO in |file_proxy| may be padded for alignment.
    img_data.resize((img_size.width * img_size.height * 4).try_into().unwrap(), 0);

    if cmd.format == Format::RGBA {
        bgra_to_rgba(&mut img_data);
    }

    let mut screenshot_file = create_file(&mut screenshot_file_path);
    screenshot_file.write_all(&img_data[..]).expect("failed to write image data.");
    writer.item(&ScreenshotOutput { output_file: screenshot_file_path })?;

    Ok(())
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ScreenshotOutput {
    output_file: PathBuf,
}

impl Display for ScreenshotOutput {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "Exported {}", self.output_file.display())
    }
}

fn default_output_dir() -> PathBuf {
    let now = Local::now();

    Path::new("/tmp").join("screenshot").join(format!(
        "{}{:02}{:02}_{:02}{:02}{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    ))
}

fn create_file(screenshot_file_path: &mut PathBuf) -> fs::File {
    fs::File::create(screenshot_file_path.clone())
        .unwrap_or_else(|_| panic!("cannot create file {}", screenshot_file_path.to_string_lossy()))
}

/// Performs inplace BGRA -> RGBA.
fn bgra_to_rgba(img_data: &mut Vec<u8>) {
    let bytes_per_pixel = 4;
    let mut blue_pos = 0;
    let mut red_pos = 2;
    let img_data_size = img_data.len();

    while blue_pos < img_data_size && red_pos < img_data_size {
        let blue = img_data[blue_pos];
        img_data[blue_pos] = img_data[red_pos];
        img_data[red_pos] = blue;
        blue_pos = blue_pos + bytes_per_pixel;
        red_pos = red_pos + bytes_per_pixel;
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_math::SizeU;
    use fdomain_fuchsia_ui_composition::{ScreenshotRequest, ScreenshotTakeFileResponse};
    use ffx_writer::{Format as WriterFormat, TestBuffers};
    use futures::TryStreamExt;
    use std::os::unix::ffi::OsStrExt;
    use target_holders::fake_proxy;
    use tempfile::tempdir;

    fn serve_fake_file(server: fdomain_client::fidl::ServerEnd<fio::FileMarker>) {
        fuchsia_async::Task::local(async move {
            let data: [u8; 16] = [1, 2, 3, 4, 1, 2, 3, 4, 4, 3, 2, 1, 4, 3, 2, 1];
            let mut stream = server.into_stream();

            let mut cc: u32 = 0;
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    fio::FileRequest::Read { count: _, responder } => {
                        cc = cc + 1;
                        if cc == 1 {
                            responder.send(Ok(&data)).expect("writing file test response");
                        } else {
                            responder.send(Ok(&[])).expect("writing file test response");
                        }
                    }
                    fio::FileRequest::GetAttributes { query, responder } => {
                        let attrs = fio::NodeAttributes2 {
                            mutable_attributes: fio::MutableNodeAttributes {
                                creation_time: query
                                    .contains(fio::NodeAttributesQuery::CREATION_TIME)
                                    .then_some(0),
                                modification_time: query
                                    .contains(fio::NodeAttributesQuery::MODIFICATION_TIME)
                                    .then_some(0),
                                mode: query.contains(fio::NodeAttributesQuery::MODE).then_some(0),
                                ..Default::default()
                            },
                            immutable_attributes: fio::ImmutableNodeAttributes {
                                protocols: query
                                    .contains(fio::NodeAttributesQuery::PROTOCOLS)
                                    .then_some(fio::NodeProtocolKinds::FILE),
                                content_size: query
                                    .contains(fio::NodeAttributesQuery::CONTENT_SIZE)
                                    .then_some(data.len() as u64),
                                storage_size: query
                                    .contains(fio::NodeAttributesQuery::STORAGE_SIZE)
                                    .then_some(data.len() as u64),
                                link_count: query
                                    .contains(fio::NodeAttributesQuery::LINK_COUNT)
                                    .then_some(0),
                                id: query.contains(fio::NodeAttributesQuery::ID).then_some(0),
                                ..Default::default()
                            },
                        };
                        responder
                            .send(Ok((&attrs.mutable_attributes, &attrs.immutable_attributes)))
                            .expect("sending attributes");
                    }
                    e => panic!("not supported {:?}", e),
                }
            }
        })
        .detach();
    }

    fn setup_fake_screenshot_server() -> ScreenshotProxy {
        let client = fdomain_local::local_client_empty();
        fake_proxy(client.clone(), move |req| match req {
            ScreenshotRequest::TakeFile { payload: _, responder } => {
                let mut screenshot = ScreenshotTakeFileResponse::default();

                let (file_client_end, file_server_end) =
                    client.create_endpoints::<fio::FileMarker>();

                let _ = screenshot.file.insert(file_client_end);
                let _ = screenshot.size.insert(SizeU { width: 2, height: 2 });
                serve_fake_file(file_server_end);
                responder.send(screenshot).unwrap();
            }

            _ => assert!(false),
        })
    }

    async fn run_screenshot_test(cmd: ScreenshotCommand) -> ScreenshotOutput {
        let screenshot_proxy = setup_fake_screenshot_server();

        let test_buffers = TestBuffers::default();
        let mut writer = VerifiedMachineWriter::<ScreenshotOutput>::new_test(
            Some(WriterFormat::Json),
            &test_buffers,
        );
        let result = screenshot_impl(screenshot_proxy, cmd, &mut writer).await;
        assert!(result.is_ok());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(stderr, "", "no warnings or errors should be reported");
        serde_json::from_str(&stdout).unwrap()
    }

    #[fuchsia::test]
    async fn test_bgra() -> Result<()> {
        let ScreenshotOutput { output_file } =
            run_screenshot_test(ScreenshotCommand { output_directory: None, format: Format::BGRA })
                .await;
        assert_eq!(
            output_file.file_name().unwrap().as_bytes(),
            "screenshot.bgra".as_bytes(),
            "{output_file:?} must have filename==screenshot.bgra",
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn test_output_dir_bgra() -> Result<()> {
        // Create a test directory in TempFile::tempdir.
        let output_dir = PathBuf::from(tempdir().unwrap().path()).join("screenshot_test");
        fs::create_dir_all(&output_dir)?;
        let ScreenshotOutput { output_file } = run_screenshot_test(ScreenshotCommand {
            output_directory: Some(output_dir.to_string_lossy().to_string()),
            format: Format::BGRA,
        })
        .await;
        assert_eq!(output_file, output_dir.join("screenshot.bgra"));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_png() -> Result<()> {
        let ScreenshotOutput { output_file } =
            run_screenshot_test(ScreenshotCommand { output_directory: None, format: Format::PNG })
                .await;
        assert_eq!(
            output_file.file_name().unwrap().as_bytes(),
            "screenshot.png".as_bytes(),
            "{output_file:?} must have filename==screenshot.png",
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn test_output_dir_png() -> Result<()> {
        // Create a test directory in TempFile::tempdir.
        let output_dir = PathBuf::from(tempdir().unwrap().path()).join("screenshot_test");
        fs::create_dir_all(&output_dir)?;
        let ScreenshotOutput { output_file } = run_screenshot_test(ScreenshotCommand {
            output_directory: Some(output_dir.to_string_lossy().to_string()),
            format: Format::PNG,
        })
        .await;
        assert_eq!(output_file, output_dir.join("screenshot.png"));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_rgba() -> Result<()> {
        let ScreenshotOutput { output_file } =
            run_screenshot_test(ScreenshotCommand { output_directory: None, format: Format::RGBA })
                .await;
        assert_eq!(
            output_file.file_name().unwrap().as_bytes(),
            "screenshot.rgba".as_bytes(),
            "{output_file:?} must have filename==screenshot.rgba",
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn test_output_dir_rgba() -> Result<()> {
        // Create a test directory in TempFile::tempdir.
        let output_dir = PathBuf::from(tempdir().unwrap().path()).join("screenshot_test");
        fs::create_dir_all(&output_dir)?;
        let ScreenshotOutput { output_file } = run_screenshot_test(ScreenshotCommand {
            output_directory: Some(output_dir.to_string_lossy().to_string()),
            format: Format::RGBA,
        })
        .await;
        assert_eq!(output_file, output_dir.join("screenshot.rgba"));
        Ok(())
    }
}
