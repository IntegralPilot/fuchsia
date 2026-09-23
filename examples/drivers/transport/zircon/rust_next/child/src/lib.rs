// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceInstance, driver_register,
};
use fidl_next_fuchsia_hardware_i2c as i2c;
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct ZirconTransportChild {
    /// The [`Node`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `ZirconTransportChild`.
driver_register!(ZirconTransportChild);

impl Driver for ZirconTransportChild {
    const NAME: &str = "zircon_child_rust_next_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        let device = get_i2c_device(&context)?.spawn();
        let device_name = device
            .get_name()
            .await?
            .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
            .name
            .to_string();
        info!("i2c device name: {device_name}");

        info!("Adding child node with i2c device name as a property value");
        let child_node = NodeBuilder::new("transport-child")
            .add_property(bind_fuchsia_test::TEST_CHILD, device_name)
            .build();
        node.add_child(child_node).await?;

        Ok(Self { node })
    }

    async fn stop(&self) {
        info!(
            "ZirconTransportChild::stop() was invoked. Use this function to do any cleanup needed."
        );
    }
}

fn get_i2c_device(context: &DriverContext) -> Result<fidl_next::ClientEnd<i2c::Device>, Error> {
    let service_proxy: ServiceInstance<i2c::Service> = context.incoming.service().connect_next()?;

    let (client_end, server_end) = fidl_next::fuchsia::create_channel();

    service_proxy
        .device(server_end)
        .context("Error connecting to i2c device proxy at driver startup")?;

    Ok(client_end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::ServiceOffer;
    use fdf_component::testing::harness::TestHarness;
    use fuchsia_async as fasync;
    use fuchsia_component::server::ServiceFs;

    const TEST_NAME: &str = "test_i2c";

    struct I2cServer {}

    impl i2c::DeviceServerHandler for I2cServer {
        async fn transfer(
            &mut self,
            _request: fidl_next::Request<i2c::device::Transfer>,
            responder: fidl_next::Responder<i2c::device::Transfer>,
        ) {
            responder.respond(vec![vec![0xa_u8, 0xb_u8, 0xc_u8]]).await.unwrap();
        }

        async fn get_name(&mut self, responder: fidl_next::Responder<i2c::device::GetName>) {
            responder.respond(TEST_NAME).await.unwrap();
        }
    }

    struct Service {
        scope: fasync::ScopeHandle,
    }

    impl i2c::ServiceHandler for Service {
        fn device(&self, server_end: fidl_next::ServerEnd<i2c::Device>) {
            server_end.spawn_on(I2cServer {}, &self.scope);
        }
    }

    #[fuchsia::test]
    async fn verify_child_node() {
        let scope = fasync::Scope::new();
        let mut driver_incoming = ServiceFs::new();

        let offer = ServiceOffer::<i2c::Service>::new_next()
            .add_default_named_next(
                &mut driver_incoming,
                "default",
                Service { scope: scope.to_handle() },
            )
            .build_zircon_offer_next();

        let mut harness = TestHarness::<ZirconTransportChild>::new()
            .add_offer(offer)
            .set_driver_incoming(driver_incoming);
        let started_driver = harness.start_driver().await.unwrap();

        // Access the driver's bound node and check that it's parenting one child node that has the
        // test property properly set to the test name.
        let children = started_driver.node().children();
        assert_eq!(children.len(), 1);
        assert!(children.contains_key("transport-child"));
        let child_node = children.get("transport-child").unwrap();
        let properties = child_node.properties();
        assert_eq!(properties.len(), 1);
        assert_eq!(properties[0].key, bind_fuchsia_test::TEST_CHILD);
        assert_eq!(
            properties[0].value,
            fidl_fuchsia_driver_framework::NodePropertyValue::StringValue(TEST_NAME.to_string())
        );

        started_driver.stop_driver().await;
    }
}
