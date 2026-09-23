// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START include]
use example_config_driver_config::Config;
// [END include]
use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct ConfigRustDriver {
    /// The [`NodeProxy`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `ConfigRustDriver` and registers for additional
// callbacks to invoke the suspend and resume methods during system suspension.
driver_register!(ConfigRustDriver);

impl Driver for ConfigRustDriver {
    const NAME: &str = "example_config_rust_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        // [START use]
        let config = context.take_config::<Config>()?;
        info!("My config value is: {}", config.suspend_enabled);
        // [END use]

        let node = context.take_node()?;
        Ok(Self { node })
    }

    async fn stop(&self) {}
}
