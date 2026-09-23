// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use log::info;

pub struct HangOnStopDriver {
    _node: Node,
}

impl Driver for HangOnStopDriver {
    const NAME: &'static str = "hang_on_stop";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("hang-on-stop: Driver started.");
        let node = context.take_node()?;
        Ok(Self { _node: node })
    }

    async fn stop(&self) {
        info!("hang-on-stop: Hanging shutdown as requested...");
        futures::future::pending::<()>().await;
    }
}

driver_register!(HangOnStopDriver);
