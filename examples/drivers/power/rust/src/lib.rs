// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use fdf_power::{Suspendable, SuspendableDriver};
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct PowerRustDriver {
    /// The [`NodeProxy`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `PowerRustDriver` and registers for additional
// callbacks to invoke the suspend and resume methods during system suspension.
driver_register!(Suspendable<PowerRustDriver>);

impl Driver for PowerRustDriver {
    const NAME: &str = "example_power_rust_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(concat!(
            "PowerRustDriver::start() was invoked. Use this function to do basic initialization ",
            "like taking ownership over the node proxy, creating children, and connecting ",
            "to resources in the incoming namespace or serving resources to the ",
            "outgoing namespace."
        ));

        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        Ok(Self { node })
    }

    async fn stop(&self) {
        info!("PowerRustDriver::stop() was invoked. Use this function to do any cleanup needed.");
    }
}

impl SuspendableDriver for PowerRustDriver {
    async fn suspend(&self) {
        info!("PowerRustDriver::suspend() was invoked. Use this function to prepare for suspend.");
    }

    async fn resume(&self) {
        info!(concat!(
            "PowerRustDriver::resume() was invoked. Use this function to reinitialize state ",
            "reconfigured in suspend."
        ));
    }
    fn suspend_enabled(&self) -> bool {
        info!(concat!(
            "Use this function to enable or disable suspend functionality. This is often determined via ",
            "the fuchsia.power.SuspendEnabled config capabilitiy."
        ));
        true
    }
}
