// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fidl_fuchsia_hardware_i2c as i2c;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct ZirconParentDriver {
    /// The [`NodeProxy`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `ZirconParentDriver`.
driver_register!(ZirconParentDriver);

async fn i2c_server(mut service: i2c::DeviceRequestStream) {
    use i2c::DeviceRequest::*;
    while let Some(req) = service.try_next().await.unwrap() {
        match req {
            Transfer { responder, .. } => responder.send(Ok(&[vec![0x1u8, 0x2, 0x3]])),
            GetName { responder } => responder.send(Ok("rust i2c server")),
        }
        .unwrap();
    }
}

impl Driver for ZirconParentDriver {
    const NAME: &str = "zircon_parent_rust_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        info!("Offering an i2c service in the outgoing directory");
        let mut outgoing = ServiceFs::new();
        let offer = ServiceOffer::new()
            .add_default_named(&mut outgoing, "default", |i| {
                // Since we're only acting on one kind of service here, we just unwrap it and that's
                // the type of our handler. If you were handling more services, you would usually
                // wrap it in an enum containing one discriminant per protocol handled.
                let i2c::ServiceRequest::Device(service) = i;
                service
            })
            .build_zircon_offer();

        info!("Creating child node with a service offer");
        let child_node = NodeBuilder::new("zircon_transport_rust_child").add_offer(offer).build();
        node.add_child(child_node).await?;

        context.serve_outgoing(&mut outgoing)?;

        fuchsia_async::Task::spawn(async move {
            outgoing.for_each_concurrent(None, i2c_server).await;
        })
        .detach();

        Ok(Self { node })
    }

    async fn stop(&self) {
        info!(
            "ZirconParentDriver::stop() was invoked. Use this function to do any cleanup needed."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::testing::harness::TestHarness;

    #[fuchsia::test]
    async fn connect_and_get_name() {
        let mut harness = TestHarness::<ZirconParentDriver>::new();
        let started_driver = harness.start_driver().await.unwrap();
        let children = started_driver.node().children();
        assert_eq!(children.len(), 1);
        assert!(children.contains_key("zircon_transport_rust_child"));

        let service_proxy =
            started_driver.driver_outgoing().service_marker(i2c::ServiceMarker).connect().unwrap();

        let client = service_proxy.connect_to_device().unwrap();
        let name = client.get_name().await.unwrap().unwrap();
        assert_eq!(name, "rust i2c server");

        started_driver.stop_driver().await;
    }
}
