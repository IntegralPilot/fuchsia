// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use argh::FromArgs;
use futures::executor::block_on;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use storage_device::Device;
use storage_device::file_backed_device::FileBackedDevice;
use super_parser::SuperParser;

#[derive(FromArgs, PartialEq, Debug)]
/// A tool to inspect and extract from super block images.
struct TopLevel {
    /// path to the image file
    #[argh(option, short = 'f')]
    file: PathBuf,

    #[argh(subcommand)]
    subcommand: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Info(InfoCommand),
    Extract(ExtractCommand),
}

#[derive(FromArgs, PartialEq, Debug)]
/// Display info about a super block image.
#[argh(subcommand, name = "info")]
struct InfoCommand {}

#[derive(FromArgs, PartialEq, Debug)]
/// Extract a partition from a super block image.
#[argh(subcommand, name = "extract")]
struct ExtractCommand {
    /// the slot to read from
    #[argh(positional)]
    slot: String,
    /// the partition to extract
    #[argh(positional)]
    partition: String,
    /// the file to write the partition to
    #[argh(positional)]
    out_file: PathBuf,
}

fn print_info(super_parser: &SuperParser) -> Result<(), Error> {
    let metadata = super_parser.super_metadata();
    let logical_block_size = metadata.geometry.logical_block_size;
    let metadata_max_size = metadata.geometry.metadata_max_size;
    let metadata_slot_count = metadata.geometry.metadata_slot_count;
    println!("Super Block Geometry:");
    println!("  Logical Block Size: {}", logical_block_size);
    println!("  Metadata Max Size: {}", metadata_max_size);
    println!("  Metadata Slot Count: {}", metadata_slot_count);
    println!();

    for (i, slot) in metadata.metadata_slots.iter().enumerate() {
        println!("# Slot {} Metadata", i);
        println!("Partitions:");
        for (name, partition) in slot.partitions() {
            let attributes = partition.attributes;
            let first_extent_index = partition.first_extent_index;
            let num_extents = partition.num_extents;
            println!("  - Name: {}", name);
            println!("    Attributes: {:?}", attributes);
            println!("    First Extent Index: {}", first_extent_index);
            println!("    Number of Extents: {}", num_extents);
            let extents = slot.extent_locations_for_partition(name)?;
            let extent_ranges: Vec<_> = extents.iter().map(|e| e.0.clone()).collect();
            println!("    Extents (bytes): {:?}", extent_ranges);
        }
        println!();
    }
    Ok(())
}

fn main() -> Result<(), Error> {
    let args: TopLevel = argh::from_env();
    let file = OpenOptions::new().read(true).open(args.file)?;
    let device = FileBackedDevice::new(file, 512);
    let super_parser = block_on(SuperParser::new(std::sync::Arc::new(device)))?;

    match args.subcommand {
        SubCommand::Info(_) => {
            print_info(&super_parser)?;
        }
        SubCommand::Extract(extract_args) => {
            let slot_index: usize = extract_args.slot.parse()?;
            let partition_device =
                super_parser.get_sub_partition(&extract_args.partition, slot_index)?;
            let mut out_file =
                OpenOptions::new().write(true).create_new(true).open(extract_args.out_file)?;
            let partition_size = partition_device.size();
            let mut offset = 0u64;
            const CHUNK_SIZE: u64 = 1024 * 1024; // 1MB
            let mut write_buf = vec![0u8; CHUNK_SIZE as usize];

            while offset < partition_size {
                let bytes_to_read = std::cmp::min(CHUNK_SIZE, partition_size - offset);
                let mut buffer = block_on(partition_device.allocate_buffer(bytes_to_read as usize));
                block_on(partition_device.read(offset, buffer.as_mut()))?;
                let write_slice = &mut write_buf[..bytes_to_read as usize];
                buffer.copy_to_slice(write_slice);
                out_file.write_all(write_slice)?;
                offset += bytes_to_read;
            }
        }
    }
    Ok(())
}
