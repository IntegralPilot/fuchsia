// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use log::info;
use scrutiny_utils::blobfs_export::blobfs_export;
use scrutiny_utils::bootfs::*;
use scrutiny_utils::fvm::*;
use scrutiny_utils::zbi::*;
use serde_json::json;
use serde_json::value::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::prelude::*;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;

pub struct ZbiExtractController {}

impl ZbiExtractController {
    pub fn extract(input: PathBuf, output: PathBuf) -> Result<Value> {
        let mut zbi_file = File::open(input)?;
        let mut zbi_buffer = Vec::new();
        zbi_file.read_to_end(&mut zbi_buffer)?;
        let mut reader = ZbiReader::new(zbi_buffer);
        let zbi_sections = reader.parse()?;

        fs::create_dir_all(&output)?;
        let sections_dir = output.join("sections");
        fs::create_dir_all(&sections_dir)?;
        let mut section_count = HashMap::new();
        let mut extracted_bootfs = false;
        for section in zbi_sections.iter() {
            let section_str = format!("{:?}", section.section_type).to_lowercase();
            let section_name = if let Some(count) = section_count.get_mut(&section.section_type) {
                *count += 1;
                format!("{}.{}.blk", section_str, count)
            } else {
                section_count.insert(section.section_type, 0);
                format!("{}.blk", section_str)
            };
            let path = sections_dir.join(section_name);
            let mut file = File::create(path)?;
            file.write_all(&section.buffer)?;

            // Expand bootfs into its own folder as well.
            if section.section_type == zbi::Type::StorageBootfs {
                if !extracted_bootfs {
                    extracted_bootfs = true;
                    let bootfs_dir = output.join("bootfs");
                    fs::create_dir_all(&bootfs_dir)?;
                    let mut bootfs_reader = BootfsReader::new(section.buffer.clone());
                    let bootfs_files = bootfs_reader.parse()?;
                    for (file_name, data) in bootfs_files.iter() {
                        let bootfs_file_path = bootfs_dir.join(file_name);
                        if let Some(parent_dir) = bootfs_file_path.as_path().parent() {
                            fs::create_dir_all(parent_dir)?;
                        }
                        let mut bootfs_file = File::create(bootfs_file_path)?;
                        bootfs_file.write_all(&data)?;
                    }
                }
            } else if section.section_type == zbi::Type::StorageRamdisk {
                info!("Attempting to load FvmPartitions");
                let mut fvm_reader = FvmReader::new(section.buffer.clone());
                if let Ok(fvm_partitions) = fvm_reader.parse() {
                    info!(total = fvm_partitions.len(); "Extracting Partitions in StorageRamdisk");
                    let fvm_dir = output.join("fvm");
                    fs::create_dir_all(&fvm_dir)?;

                    let mut partition_count = HashMap::<FvmPartitionType, u64>::new();
                    for partition in fvm_partitions.iter() {
                        let file_name = if let Some(count) =
                            partition_count.get_mut(&partition.partition_type)
                        {
                            *count += 1;
                            format!("{}.{}.blk", partition.partition_type, count)
                        } else {
                            section_count.insert(section.section_type, 0);
                            format!("{}.blk", partition.partition_type)
                        };
                        let fvm_partition_path = fvm_dir.join(&file_name);
                        let mut fvm_file = File::create(&fvm_partition_path)?;
                        for slice_data in &partition.buffer {
                            fvm_file.seek(SeekFrom::Start(slice_data.offset()))?;
                            fvm_file.write_all(slice_data.data())?;
                        }

                        // Write out the blobfs data.
                        if partition.partition_type == FvmPartitionType::BlobFs {
                            info!("Extracting BlobFs FVM partition");

                            let blobfs_dir = fvm_dir.join("blobfs");
                            fs::create_dir_all(&blobfs_dir)?;

                            blobfs_export(
                                &fvm_partition_path.to_str().expect("invalid input path"),
                                blobfs_dir.to_str().expect("invalid output path"),
                            )?;
                        }
                    }
                } else {
                    info!("No FvmPartitions found in StorageRamdisk");
                }
            }
        }

        Ok(json!({"status": "ok"}))
    }
}
