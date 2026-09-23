// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use scrutiny_utils::fvm::*;
use serde_json::json;
use serde_json::value::Value;
use std::fs::{self, File};
use std::io::prelude::*;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;

pub struct FvmExtractController {}

impl FvmExtractController {
    pub fn extract(input: PathBuf, output: PathBuf) -> Result<Value> {
        let mut fvm_file = File::open(input)?;
        let mut fvm_buffer = Vec::new();
        fvm_file.read_to_end(&mut fvm_buffer)?;
        let mut reader = FvmReader::new(fvm_buffer);
        let fvm_partitions = reader.parse()?;

        fs::create_dir_all(&output)?;
        let mut minfs_count = 0;
        let mut blobfs_count = 0;
        for partition in fvm_partitions {
            let partition_name = match partition.partition_type {
                FvmPartitionType::MinFs => {
                    if minfs_count == 0 {
                        format!("{}.blk", partition.partition_type)
                    } else {
                        format!("{}.{}.blk", partition.partition_type, minfs_count)
                    }
                }
                FvmPartitionType::BlobFs => {
                    if blobfs_count == 0 {
                        format!("{}.blk", partition.partition_type)
                    } else {
                        format!("{}.{}.blk", partition.partition_type, blobfs_count)
                    }
                }
            };
            let mut path = output.clone();
            path.push(partition_name);
            let mut file = File::create(path)?;
            for slice_data in partition.buffer {
                file.seek(SeekFrom::Start(slice_data.offset()))?;
                file.write_all(&slice_data.data())?;
            }

            match partition.partition_type {
                FvmPartitionType::MinFs => minfs_count += 1,
                FvmPartitionType::BlobFs => blobfs_count += 1,
            };
        }

        Ok(json!({"status": "ok"}))
    }
}
