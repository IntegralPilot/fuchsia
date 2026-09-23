// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use log::info;

struct MyDriverRust {
    _node: Node,
}

driver_register!(MyDriverRust);

impl Driver for MyDriverRust {
    const NAME: &str = "my_driver_rust";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("MyDriverRust::start()");

        let node = context.take_node()?;

        Ok(Self { _node: node })
    }

    async fn stop(&self) {
        info!("MyDriverRust::stop()");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::testing::harness::TestHarness;

    #[fuchsia::test]
    async fn test_driver_start() {
        let mut harness = TestHarness::<MyDriverRust>::new();
        let started_driver = harness.start_driver().await.unwrap();

        // Verify driver started successfully
        assert!(true);

        started_driver.stop_driver().await;
    }
}
