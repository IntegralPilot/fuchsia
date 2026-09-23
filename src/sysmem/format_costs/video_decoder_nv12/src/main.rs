// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use argh::FromArgs;
use camino::Utf8PathBuf;
use fidl_fuchsia_images2::{PixelFormat, PixelFormatModifier};
use fidl_fuchsia_sysmem2::{BufferUsage, FormatCostEntry, FormatCostKey, FormatCosts};

#[derive(FromArgs)]
/// Produce the bootfs/packages allowlist for assembly-generated files.
struct Args {
    /// path to output file
    #[argh(option)]
    output: Utf8PathBuf,
}

fn generate_format_cost_entries() -> Vec<FormatCostEntry> {
    vec![FormatCostEntry {
        key: Some(FormatCostKey {
            pixel_format: Some(PixelFormat::Nv12),
            pixel_format_modifier: Some(PixelFormatModifier::Linear),
            buffer_usage_bits: Some(BufferUsage {
                video: Some(fidl_fuchsia_sysmem2::VIDEO_USAGE_HW_DECODER),
                ..Default::default()
            }),
            ..Default::default()
        }),
        cost: Some(100.0),
        ..Default::default()
    }]
}

fn generate_and_write_format_costs(output_filename: &Utf8PathBuf) -> Result<(), Error> {
    let format_costs =
        FormatCosts { format_costs: Some(generate_format_cost_entries()), ..Default::default() };
    let format_costs_persistent_fidl_vec = fidl::persist(&format_costs).context("fidl::Persist")?;
    std::fs::write(output_filename, &format_costs_persistent_fidl_vec).context("std::fs::write")?;
    Ok(())
}

fn main() {
    let args: Args = argh::from_env();
    generate_and_write_format_costs(&args.output).expect("generate_and_write_format_costs");
}
