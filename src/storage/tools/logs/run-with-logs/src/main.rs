// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, anyhow, bail};
use argh::FromArgs;
use block_client::{Cache, RemoteBlockClientSync};
use byteorder::{LittleEndian, WriteBytesExt};
use fidl::endpoints::DiscoverableProtocolMarker as _;
use fidl_fuchsia_storage_block::BlockMarker;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_channel_to_protocol_at_path;
use std::io::Write;
use std::process::{Command, Output};

/// Magic number we write to the disk before the log data. This allows the extractor to
/// differentiate between failures of this harness and a successful run where no output was
/// received, which would otherwise both be all zeros.
///
/// It says logs.
const MAGIC: u64 = 0x1092;

/// Run a binary, writing stdout and stderr to a block device so it can be extracted from the device.
///
/// Exactly one of `block_device_path` or `block_bus_path` must be provided.
#[derive(FromArgs)]
struct Args {
    /// block device service path to write the test output to, e.g.
    /// "/block/000/fuchsia.storage.block.Block".
    #[argh(option)]
    block_device_path: Option<String>,
    /// bus path to match against, e.g. "pci00:01.0".  Must be an exact match.
    #[argh(option)]
    block_bus_path: Option<String>,
    /// binary to run and capture the output of.
    #[argh(positional)]
    binary: String,
    /// any additional arguments for the binary.
    #[argh(positional, greedy)]
    args: Vec<String>,
}

async fn find_block_by_bus_path(
    bus_path: &str,
) -> Result<fidl::endpoints::ClientEnd<BlockMarker>, Error> {
    use fuchsia_fs::directory::{WatchEvent, Watcher};
    use futures::StreamExt;

    use std::time::Duration;

    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10);
    let retry_interval = Duration::from_millis(500);

    let (dir, mut watcher) = loop {
        if start.elapsed() >= timeout {
            bail!("Timed out waiting for /block to be ready");
        }
        let attempt: Result<_, Error> = async {
            let dir =
                fuchsia_fs::directory::open_in_namespace("/block", fidl_fuchsia_io::PERM_READABLE)?;
            let watcher = Watcher::new(&dir).await?;
            Ok((dir, watcher))
        }
        .await;

        match attempt {
            Ok(val) => break val,
            Err(e) => {
                eprintln!("Failed to open or watch /block: {:?}. Retrying...", e);
                fasync::Timer::new(retry_interval).await;
            }
        }
    };

    while let Some(message) = watcher.next().await {
        let message = message.context("watcher channel returned error")?;
        match message.event {
            WatchEvent::ADD_FILE | WatchEvent::EXISTING => {
                let block_subdir = message.filename.to_str().unwrap();
                if block_subdir == "." {
                    continue;
                }
                let bus_path_file_path = format!("{block_subdir}/bus_path");
                if let Ok(content) =
                    fuchsia_fs::directory::read_file_to_string(&dir, &bus_path_file_path).await
                {
                    if content.trim() == bus_path {
                        let block_path =
                            format!("/block/{block_subdir}/{}", BlockMarker::PROTOCOL_NAME);
                        let (client_end, server_end) =
                            fidl::endpoints::create_endpoints::<BlockMarker>();
                        fuchsia_component::client::connect_channel_to_protocol_at_path(
                            server_end.into_channel(),
                            &block_path,
                        )?;
                        return Ok(client_end);
                    }
                }
            }
            _ => (),
        }
    }
    Err(anyhow!("Watch stream unexpectedly ended"))
}

fn main() -> Result<(), Error> {
    let Args { block_device_path, block_bus_path, binary, args } = argh::from_env();
    if block_device_path.is_none() == block_bus_path.is_none() {
        bail!("Exactly one of --block-device-path or --block-bus-path must be provided.");
    }

    let block_client_end = if let Some(path) = block_device_path {
        let (client_end, server_end) = fidl::endpoints::create_endpoints::<BlockMarker>();
        connect_channel_to_protocol_at_path(server_end.into_channel(), &path)
            .expect("connecting to block device");
        client_end
    } else {
        let Some(bus_path) = block_bus_path else { unreachable!() };
        let mut executor = fasync::LocalExecutor::default();
        executor.run_singlethreaded(find_block_by_bus_path(&bus_path))?
    };
    let block_client =
        RemoteBlockClientSync::new(block_client_end).context("making remote block client")?;
    let mut cache = Cache::new(block_client).context("making block client cache")?;

    // Run the test process, extracting stdout and stderr. The `output` function reads everything
    // off of stdout and stderr before returning, so we don't need to worry about the process
    // exiting too fast. `output` will return Ok even if the exit status is non-zero, it only fails
    // if the command fails to execute in the first place, like if the binary doesn't exist. We
    // capture those errors and attempt to write them to stderr in place of the command output.
    let (stdout, stderr) = match Command::new(binary).args(&args).output() {
        Ok(Output { stdout, stderr, status: _ }) => (stdout, stderr),
        Err(e) => (Vec::new(), format!("command failed: {:?}", e).into_bytes()),
    };

    // Write the test process output to the block device in the expected format.
    // Format is <magic><length><data><length><data> where length is a u64, first for stdout then
    // stderr. The magic differentiates between an empty disk and one that has been written to.
    cache.write_u64::<LittleEndian>(MAGIC).context("writing magic")?;
    cache
        .write_u64::<LittleEndian>(stdout.len().try_into().context("usize to u64 conversion")?)
        .context("writing stdout length")?;
    cache.write_all(&stdout).context("writing stdout")?;
    cache
        .write_u64::<LittleEndian>(stderr.len().try_into().context("usize to u64 conversion")?)
        .context("writing stderr length")?;
    cache.write_all(&stderr).context("writing stderr")?;

    Ok(())
}
