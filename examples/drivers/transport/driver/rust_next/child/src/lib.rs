// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceInstance, driver_register,
};
use fidl_next_fuchsia_hardware_i2cimpl as i2cimpl;
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct DriverTransportChild {
    /// The [`Node`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `DriverTransportChild`.
driver_register!(DriverTransportChild);

impl Driver for DriverTransportChild {
    const NAME: &str = "driver_child_rust_next_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        let device = get_i2cimpl_device(&context)?.spawn();
        let transfer_size = device
            .get_max_transfer_size()
            .await?
            .map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?
            .size;
        info!("i2cimpl max transfer size: {transfer_size}");

        info!("Adding child node with i2cimpl max transfer size as a property value");
        let child_node = NodeBuilder::new("transport-child")
            .add_property(bind_fuchsia_test::TEST_CHILD, u64::from(transfer_size) as u32)
            .build();
        node.add_child(child_node).await?;

        device.set_bitrate(0x5u32).await?.map_err(|s| s.err().unwrap_or(zx::Status::INTERNAL))?;

        Ok(Self { node })
    }

    async fn stop(&self) {
        info!(
            "DriverTransportChild::stop() was invoked. Use this function to do any cleanup needed."
        );
    }
}

fn get_i2cimpl_device(
    context: &DriverContext,
) -> Result<fidl_next::ClientEnd<i2cimpl::Device>, anyhow::Error> {
    let service_proxy: ServiceInstance<i2cimpl::Service> =
        context.incoming.service().connect_next()?;

    let (client_end, server_end) = fdf_fidl::create_channel();

    service_proxy
        .device(server_end)
        .context("Error connecting to i2cimpl device proxy at driver startup")?;

    Ok(client_end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf::AsyncDispatcher;
    use fdf_component::ServiceOffer;
    use fdf_component::testing::harness::TestHarness;
    use fuchsia_component::server::ServiceFs;
    use futures::lock::Mutex;
    use std::sync::Arc;

    const MAX_TRANSFER_SIZE: u32 = 0x1234567;
    const BITRATE: u32 = 0x5;

    struct I2cImplServer {
        bitrate: Arc<Mutex<u32>>,
    }

    impl i2cimpl::DeviceServerHandler<fdf_fidl::DriverChannel> for I2cImplServer {
        async fn get_max_transfer_size(
            &mut self,
            responder: fidl_next::Responder<
                i2cimpl::device::GetMaxTransferSize,
                fdf_fidl::DriverChannel,
            >,
        ) {
            responder.respond(MAX_TRANSFER_SIZE as u64).await.unwrap();
        }

        async fn set_bitrate(
            &mut self,
            request: fidl_next::Request<i2cimpl::device::SetBitrate, fdf_fidl::DriverChannel>,
            responder: fidl_next::Responder<i2cimpl::device::SetBitrate, fdf_fidl::DriverChannel>,
        ) {
            *self.bitrate.lock().await = request.payload().bitrate;
            responder.respond(()).await.unwrap();
        }

        async fn transact(
            &mut self,
            _request: fidl_next::Request<i2cimpl::device::Transact, fdf_fidl::DriverChannel>,
            responder: fidl_next::Responder<i2cimpl::device::Transact, fdf_fidl::DriverChannel>,
        ) {
            responder.respond(Vec::<i2cimpl::ReadData>::new()).await.unwrap();
        }
    }

    struct Service {
        dispatcher: AsyncDispatcher,
        bitrate: Arc<Mutex<u32>>,
    }

    impl i2cimpl::ServiceHandler for Service {
        fn device(
            &self,
            server_end: fidl_next::ServerEnd<i2cimpl::Device, fdf_fidl::DriverChannel>,
        ) {
            let bitrate = self.bitrate.clone();
            server_end.spawn_on(
                I2cImplServer { bitrate },
                &fdf_fidl::FidlExecutor::from(self.dispatcher.clone()),
            );
        }
    }

    #[fuchsia::test]
    async fn verify_query_values() {
        let mut driver_incoming = ServiceFs::new();
        let harness = TestHarness::<DriverTransportChild>::new();
        let bitrate = Arc::new(Mutex::new(0u32));
        let offer = ServiceOffer::<i2cimpl::Service>::new_next()
            .add_default_named_next(
                &mut driver_incoming,
                "default",
                Service { dispatcher: harness.dispatcher(), bitrate: bitrate.clone() },
            )
            .build_driver_offer();

        let mut harness = harness.add_offer(offer).set_driver_incoming(driver_incoming);
        let started_driver = harness.start_driver().await.unwrap();

        // Access the driver's bound node and check that it's parenting one child node that has the
        // test property properly set to the max transfer size we return.
        let children = started_driver.node().children();
        assert_eq!(children.len(), 1);
        assert!(children.contains_key("transport-child"));
        let child_node = children.get("transport-child").unwrap();
        let properties = child_node.properties();
        assert_eq!(properties.len(), 1);
        assert_eq!(properties[0].key, bind_fuchsia_test::TEST_CHILD);
        assert_eq!(
            properties[0].value,
            fidl_fuchsia_driver_framework::NodePropertyValue::IntValue(MAX_TRANSFER_SIZE)
        );

        // Check that the driver set the bitrate to the one we expect.
        assert_eq!(*bitrate.lock().await, BITRATE);

        started_driver.stop_driver().await;
    }
}
