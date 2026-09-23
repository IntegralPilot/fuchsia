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

fn generate_rgba_format_costs(
    modifier: fidl_fuchsia_images2::PixelFormatModifier,
    cost: f32,
) -> Vec<FormatCostEntry> {
    vec![PixelFormat::B8G8R8A8, PixelFormat::R8G8B8A8, PixelFormat::R8G8B8X8, PixelFormat::B8G8R8X8]
        .into_iter()
        .map(|pixel_format| FormatCostEntry {
            key: Some(FormatCostKey {
                pixel_format: Some(pixel_format),
                pixel_format_modifier: Some(modifier),
                buffer_usage_bits: Some(BufferUsage::default()),
                ..Default::default()
            }),
            cost: Some(cost),
            ..Default::default()
        })
        .collect()
}

fn generate_format_cost_entries() -> Vec<FormatCostEntry> {
    let mut result: Vec<FormatCostEntry> = vec![];

    // Split block is slightly worse than non-split-block for GPU<->GPU, but better for GPU->display.
    const SPLIT_COST: f32 = 10.0;
    const NON_YUV_COST: f32 = 100.0;
    // Tiled headers enable more optimizations and are more efficient, but alignment requirements make
    // them take up more RAM. They're still worthwhile for our usecases.
    const NON_TILED_HEADER_COST: f32 = 500.0;
    // Formats without sparse set are substantially worse for the GPU than sparse formats.
    const NON_SPARSE_COST: f32 = 1000.0;
    const NON_TE_COST: f32 = 2000.0;
    // Non-16X16 can have large advantages for the display, but it's much worse for the GPU.
    const NON_16X16_COST: f32 = 4000.0;

    let modifiers = vec![
        PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuvTeTiledHeader,
        PixelFormatModifier::ArmAfbc16X16Te,
        PixelFormatModifier::ArmAfbc32X8Te,
        PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuvTe,
        PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuvTiledHeader,
        PixelFormatModifier::ArmAfbc16X16SplitBlockSparseYuv,
        PixelFormatModifier::ArmAfbc16X16YuvTiledHeader,
        PixelFormatModifier::ArmAfbc16X16,
        PixelFormatModifier::ArmAfbc32X8,
    ];

    for modifier in modifiers {
        let mut cost: f32 = 0.0;
        let modifier_as_bits: u64 = modifier.into_primitive();

        if 0 == (modifier_as_bits & fidl_fuchsia_images2::FORMAT_MODIFIER_ARM_YUV_BIT) {
            cost += NON_YUV_COST;
        }
        if 0 == (modifier_as_bits & fidl_fuchsia_images2::FORMAT_MODIFIER_ARM_TILED_HEADER_BIT) {
            cost += NON_TILED_HEADER_COST;
        }
        if 0 != (modifier_as_bits & fidl_fuchsia_images2::FORMAT_MODIFIER_ARM_SPLIT_BLOCK_BIT) {
            cost += SPLIT_COST;
        }
        if 0 == (modifier_as_bits & fidl_fuchsia_images2::FORMAT_MODIFIER_ARM_SPARSE_BIT) {
            cost += NON_SPARSE_COST;
        }
        if 0 == (modifier_as_bits & fidl_fuchsia_images2::FORMAT_MODIFIER_ARM_TE_BIT) {
            cost += NON_TE_COST;
        }

        const AFBC_TYPE_MASK: u64 = 0xf;
        if (modifier_as_bits & AFBC_TYPE_MASK)
            != (fidl_fuchsia_images2::PixelFormatModifier::ArmAfbc16X16.into_primitive()
                & AFBC_TYPE_MASK)
        {
            cost += NON_16X16_COST;
        }

        result.append(&mut generate_rgba_format_costs(modifier, cost));
    }
    // Should be higher cost than all AFBC formats.
    result.append(&mut generate_rgba_format_costs(
        fidl_fuchsia_images2::PixelFormatModifier::ArmLinearTe,
        30000.0,
    ));

    result
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
