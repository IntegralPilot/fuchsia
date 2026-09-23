// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use block_client::{BlockClient as _, BufferSlice, MutableBufferSlice, RemoteBlockClient};
use clap::{Parser, Subcommand};
use fidl_fuchsia_storage_block::BlockMarker;
use fuchsia_component::client;
use fuchsia_fs::{PERM_READABLE, directory};

const BLOCK_CLASS_PATH: &str = "/dev/class/block/";

#[derive(Parser, Debug)]
struct Config {
    block_size: u32,
    pci_bus: u8,
    pci_device: u8,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    #[command(name = "check")]
    Check { block_count: u64 },
    #[command(name = "read")]
    Read { offset: u64, expected: u8 },
    #[command(name = "write")]
    Write { offset: u64, value: u8 },
}

#[fuchsia::main]
async fn main() -> Result<(), anyhow::Error> {
    let Config { block_size, pci_bus, pci_device, cmd } = Config::parse();

    // The filename is will contain the BDF in the form <bus>:<device>.<function>. The function is
    // always zero for virtio block devices.
    let bdf = format!("{:02}:{:02}.0", pci_bus, pci_device);

    // This tool has access to block nodes in the /dev/class/block path, but we need to match
    // against composite PCI devices which sit in the /dev/ root we cannot get blanket access to. To
    // achieve this, we walk all the block devices and look up their corresponding topological path
    // to see if it corresponds to the device we're looking for.
    //
    // Scan current files in the directory and watch for new ones in case we don't find the block
    // device we're looking for. There's a slim possibility a partition may not yet be available
    // when this tool is executed in a test.
    let block_dir = directory::open_in_namespace(BLOCK_CLASS_PATH, PERM_READABLE)?;
    let block_proxy = device_watcher::wait_for_device_with(
        &block_dir,
        |device_watcher::DeviceInfo { filename, topological_path }| {
            topological_path.contains(&bdf).then(|| {
                client::connect_to_named_protocol_at_dir_root::<BlockMarker>(&block_dir, filename)
            })
        },
    )
    .await??;
    let block_client = RemoteBlockClient::new(block_proxy).await?;

    let result = match cmd {
        Command::Check { block_count } => {
            let actual_block_size = block_client.block_size();
            let actual_block_count = block_client.block_count();
            if actual_block_size != block_size || actual_block_count != block_count {
                Err(anyhow::anyhow!(
                    "actual_block_size={} != block_size={} || actual_block_count={} != block_count={}",
                    actual_block_size,
                    block_size,
                    actual_block_count,
                    block_count,
                ))
            } else {
                Ok(())
            }
        }
        Command::Read { offset, expected } => {
            let device_offset = offset.checked_mul(block_size.into()).ok_or_else(|| {
                anyhow::anyhow!("offset={} * block_size={} overflows", offset, block_size)
            })?;
            let block_size = block_size.try_into()?;
            let mut data = {
                let mut data = Vec::new();
                let () = data.resize(block_size, !expected);
                data.into_boxed_slice()
            };
            let () = block_client
                .read_at(MutableBufferSlice::Memory(&mut (*data)[..]), device_offset)
                .await?;
            // TODO(https://github.com/rust-lang/rust/issues/59878): Box<[T]> is not IntoIter.
            let mismatches =
                data.iter().copied().enumerate().try_fold(String::new(), |mut acc, (i, b)| {
                    use std::fmt::Write as _;

                    if b != expected {
                        let () = write!(&mut acc, "\n{}:{:b}", i, b)?;
                    }
                    Ok::<_, anyhow::Error>(acc)
                })?;
            if !mismatches.is_empty() {
                Err(anyhow::anyhow!(
                    "offset={} expected={:b} mismatches={}",
                    offset,
                    expected,
                    mismatches
                ))
            } else {
                Ok(())
            }
        }
        Command::Write { offset, value } => {
            let device_offset = offset.checked_mul(block_size.into()).ok_or_else(|| {
                anyhow::anyhow!("offset={} * block_size={} overflows", offset, block_size)
            })?;
            let block_size = block_size.try_into()?;
            let data = {
                let mut data = Vec::new();
                let () = data.resize(block_size, value);
                data.into_boxed_slice()
            };
            let () =
                block_client.write_at(BufferSlice::Memory(&(*data)[..]), device_offset).await?;
            let () = block_client.flush().await?;
            Ok(())
        }
    };
    match result.as_ref() {
        Ok(()) => {
            println!("PASS")
        }
        Err(err) => {
            println!("FAIL: {:#?}", err)
        }
    }
    result
}
