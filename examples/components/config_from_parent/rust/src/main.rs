// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use example_config::Config;
use log::info;

#[fuchsia::main]
async fn main() {
    let config = Config::take_from_startup_handle();
    info!("Hello, {}! (from Rust)", config.greeting);

    let inspector = fuchsia_inspect::component::inspector();
    inspector.root().record_child("config", |config_node| config.record_inspect(config_node));

    if let Some(inspect_server) =
        inspect_runtime::publish(inspector, inspect_runtime::PublishOptions::default())
    {
        inspect_server.await
    }
}
