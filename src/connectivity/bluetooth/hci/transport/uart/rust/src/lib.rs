// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod serial;

use serial::SerialConnection;

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fuchsia_async as fasync;
use log::info;

/// Bluetooth HCI UART transport driver in Rust.
pub struct BtTransportUart {
    _node: Node,
    _scope: fasync::Scope,
    serial: SerialConnection,
}

impl BtTransportUart {
    /// Returns the PID reported by the parent serial device.
    pub fn serial_pid(&self) -> u32 {
        self.serial.serial_pid()
    }
}

impl std::fmt::Debug for BtTransportUart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BtTransportUart").field("serial", &self.serial).finish_non_exhaustive()
    }
}

driver_register!(BtTransportUart);

impl Driver for BtTransportUart {
    const NAME: &str = "bt-transport-uart-rust";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("BtTransportUart (Rust)::start() invoked");
        let node = context.take_node()?;
        let scope = fasync::Scope::new();

        let serial = SerialConnection::connect_and_validate(&context).await?;

        Ok(Self { _node: node, _scope: scope, serial })
    }

    async fn stop(&self) {
        info!("BtTransportUart (Rust)::stop() invoked");
        self.serial.cancel_all().await;
    }
}

#[cfg(test)]
mod tests;
