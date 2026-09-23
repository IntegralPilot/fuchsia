// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use dml_config as fbdc;
use fidl_fuchsia_driver_metadata as fdr;
use std::fs::File;
use std::io::Read;

fn main() -> Result<(), anyhow::Error> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        anyhow::bail!("Usage: decode_config <path_to_board_config.fidl>");
    }
    let path = &args[1];
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;

    let board_config: fbdc::BoardConfig =
        fidl::unpersist(&bytes).context("Failed to unpersist BoardConfig")?;

    println!("BoardConfig: {:#?}", board_config);

    for dev in board_config.devices.iter().flatten() {
        for meta in dev.metadata.iter().flatten() {
            if let (Some(data), Some(id)) = (&meta.data, &meta.id) {
                let dev_name = dev.name.as_deref().unwrap_or("<unknown>");
                match fidl::unpersist::<fdr::Dictionary>(data) {
                    Ok(dict) => {
                        println!("  Device '{dev_name}' Metadata '{id}' (Dictionary): {dict:#?}");
                    }
                    Err(e) => {
                        println!(
                            "  Device '{dev_name}' Metadata '{id}' failed to decode as Dictionary: {e}"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}
