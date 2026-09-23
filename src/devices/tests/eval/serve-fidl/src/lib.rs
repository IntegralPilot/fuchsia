// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, Node, driver_register};
use log::info;
use zx::Status;

struct DriverServeFidl {
    node: Node,
}

driver_register!(DriverServeFidl);

impl Driver for DriverServeFidl {
    const NAME: &str = "driver_serve_fidl";

    async fn start(mut context: DriverContext) -> Result<Self, Status> {
        info!("DriverServeFidl started");
        let node = context.take_node()?;
        Ok(Self { node })
    }

    async fn stop(&self) {
        info!("DriverServeFidl stopped");
    }
}
