// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use scrutiny_utils::blobfs_export::blobfs_export;
use serde_json::json;
use serde_json::value::Value;
use std::fs::{self};
use std::path::PathBuf;

pub struct BlobFsExtractController {}

impl BlobFsExtractController {
    pub fn extract(input: PathBuf, output: PathBuf) -> Result<Value> {
        fs::create_dir_all(&output)?;
        blobfs_export(
            input.to_str().expect("invalid input path"),
            output.to_str().expect("invalid output path"),
        )?;

        Ok(json!({"status": "ok"}))
    }
}
