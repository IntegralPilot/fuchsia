// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fuchsia_archive::Utf8Reader as FarReader;
use serde_json::json;
use serde_json::value::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use std::io::Cursor;
use std::path::PathBuf;

pub struct FarMetaExtractController {}

impl FarMetaExtractController {
    pub fn extract(input: PathBuf) -> Result<Value> {
        let mut pkg_file = File::open(input)?;
        let mut pkg_buffer = Vec::new();
        pkg_file.read_to_end(&mut pkg_buffer)?;

        let mut cursor = Cursor::new(pkg_buffer);
        let mut far = FarReader::new(&mut cursor)?;

        let pkg_files: Vec<String> = far.list().map(|e| e.path().to_string()).collect();
        let mut meta_files = HashMap::new();
        // Extract all the far meta files.
        for file_name in pkg_files.iter() {
            let data = far.read_file(file_name)?;
            meta_files.insert(file_name, String::from(std::str::from_utf8(&data)?));
        }
        Ok(json!(meta_files))
    }
}
