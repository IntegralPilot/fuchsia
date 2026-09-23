// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde::{Deserialize, Serialize};
use std::io::BufReader;
use stressor_lib::aggressive::Stressor as Aggressive;
use stressor_lib::gentle::Stressor as Gentle;

#[derive(Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct AggressiveOptions {
    /// bytes to try to keep free in aggressive mode
    target_free_bytes: u64,
}

/// The mode in which to run the stressor.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Gentle,
    Aggressive(AggressiveOptions),
}

fn main() {
    diagnostics_log::initialize_sync(diagnostics_log::PublishOptions::default());

    // Attempt to load configuration from file.
    let mode = if let Ok(file) = std::fs::File::open("/data/config.json") {
        log::info!("Reading config from json file.");
        serde_json::from_reader(BufReader::new(file)).unwrap()
    } else {
        Mode::Gentle
    };

    log::info!("Config: {mode:?}");

    // Give the system some time to start.
    std::thread::sleep(std::time::Duration::from_secs(10));

    if let Mode::Aggressive(options) = mode {
        Aggressive::new("/data", options.target_free_bytes).run(8);
    } else {
        Gentle::new().run(8);
    }
}
