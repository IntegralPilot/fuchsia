// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use scrutiny_utils::zbi::*;
use serde_json::json;
use serde_json::value::Value;
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;

pub struct ZbiExtractCmdlineController {}

impl ZbiExtractCmdlineController {
    pub fn extract(input: PathBuf) -> Result<Value> {
        let mut zbi_file = File::open(input)?;
        let mut zbi_buffer = Vec::new();
        zbi_file.read_to_end(&mut zbi_buffer)?;
        let mut reader = ZbiReader::new(zbi_buffer);
        let zbi_sections = reader.parse()?;

        for section in zbi_sections.iter() {
            if section.section_type == zbi::Type::Cmdline {
                // The cmdline.blk contains a trailing 0.
                let mut cmdline_buffer = section.buffer.clone();
                cmdline_buffer.truncate(cmdline_buffer.len() - 1);
                return Ok(json!(
                    std::str::from_utf8(&cmdline_buffer)
                        .context("Failed to convert kernel arguments to utf-8")?
                ));
            }
        }
        Err(anyhow!("Failed to find a cmdline section in the provided ZBI"))
    }
}
