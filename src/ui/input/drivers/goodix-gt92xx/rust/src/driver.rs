// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use log::info;

struct GoodixGt92xxDriver {
    _node: Node,
}

driver_register!(GoodixGt92xxDriver);

impl Driver for GoodixGt92xxDriver {
    const NAME: &str = "goodix-gt92xx";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("GoodixGt92xxDriver (Rust Skeleton) started!");
        let _node = context.take_node()?;
        Ok(Self { _node })
    }

    async fn stop(&self) {
        info!("GoodixGt92xxDriver (Rust Skeleton) stopped!");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_placeholder() {}
}
