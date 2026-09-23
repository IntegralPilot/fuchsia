// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START imports]
use example_config::Config;
// [END imports]
use log::info;

#[fuchsia::main]
async fn main() {
    // [START get_config]
    // Retrieve configuration
    let config = Config::take_from_startup_handle();
    // [END get_config]

    // Delay our print by the configured interval.
    std::thread::sleep(std::time::Duration::from_millis(config.delay_ms));

    // Print greeting to the log
    info!("Hello, {}! (from Rust)", config.greeting);

    // [START inspect]
    // Record configuration to inspect
    let inspector = fuchsia_inspect::component::inspector();
    inspector.root().record_child("config", |config_node| config.record_inspect(config_node));
    // [END inspect]

    if let Some(inspect_server) =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default())
    {
        inspect_server.await
    }
}
// [END code]
