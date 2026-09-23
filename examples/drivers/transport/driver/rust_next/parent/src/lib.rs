// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fidl_next::{Request, Responder, ServerEnd};
use fidl_next_fuchsia_hardware_i2cimpl as i2cimpl;
use fidl_next_fuchsia_hardware_i2cimpl::device::{GetMaxTransferSize, SetBitrate, Transact};
use fidl_next_fuchsia_hardware_i2cimpl::generic::ReadData;
use fuchsia_async::Scope;
use fuchsia_component::server::ServiceFs;
use futures::StreamExt as _;
use log::{info, warn};
use zx::Status;

/// The implementation of our driver will live in this object, which implements [`Driver`].
struct DriverTransportParent {
    /// The [`NodeProxy`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    #[expect(unused)]
    node: Node,
    /// The scope for the driver.
    #[expect(unused)]
    scope: Scope,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `DriverTransportParent`.
driver_register!(DriverTransportParent);

struct DeviceServer;

impl i2cimpl::DeviceServerHandler for DeviceServer {
    async fn get_max_transfer_size(&mut self, responder: Responder<GetMaxTransferSize>) {
        responder
            .respond(0x1234ABCDu64)
            .await
            .unwrap_or_else(|err| warn!("Failed to send get_max_transfer_size response: {err:?}"));
    }

    async fn set_bitrate(
        &mut self,
        request: Request<SetBitrate>,
        responder: Responder<SetBitrate>,
    ) {
        if request.payload().bitrate == 5 {
            responder
                .respond(())
                .await
                .unwrap_or_else(|err| warn!("Failed to send set_bitrate response: {err:?}"));
        } else {
            responder
                .respond_err(Status::INVALID_ARGS)
                .await
                .unwrap_or_else(|err| warn!("Failed to send set_bitrate response: {err:?}"));
        }
    }

    async fn transact(&mut self, _request: Request<Transact>, responder: Responder<Transact>) {
        responder
            .respond([ReadData { data: [0, 1, 2] }])
            .await
            .unwrap_or_else(|err| warn!("Failed to send transact response: {err:?}"));
    }
}

struct Service;

impl i2cimpl::ServiceHandler for Service {
    fn device(&self, server_end: ServerEnd<i2cimpl::Device>) {
        server_end.spawn(DeviceServer);
    }
}

impl Driver for DriverTransportParent {
    const NAME: &str = "driver_parent_rust_next_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        let scope = Scope::new();

        info!("Offering an i2c service in the outgoing directory");
        let mut outgoing = ServiceFs::new();
        let offer = ServiceOffer::<i2cimpl::Service>::new_next()
            .add_default_named_next(&mut outgoing, "default", Service)
            .build_driver_offer();

        info!("Creating child node with a service offer");
        let child_node =
            NodeBuilder::new("driver_transport_rust_next_child").add_offer(offer).build();
        node.add_child(child_node).await?;

        context.serve_outgoing(&mut outgoing)?;

        scope.spawn(outgoing.collect());

        Ok(Self { node, scope })
    }

    async fn stop(&self) {
        info!(
            "DriverTransportParent::stop() was invoked. Use this function to do any cleanup needed."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf::OnDispatcher;
    use fdf_component::ServiceInstance;
    use fdf_component::testing::harness::TestHarness;
    use fidl_next::ClientDispatcher;
    use std::sync::mpsc;

    const BITRATE: u32 = 0x5;

    // TODO(https://fxbug.dev/470088116): re-enable after fixing flake.
    #[ignore]
    #[fuchsia::test]
    async fn test_client() {
        let mut harness = TestHarness::<DriverTransportParent>::new();
        let started_driver = harness.start_driver().await.unwrap();
        let service_proxy: ServiceInstance<i2cimpl::Service> =
            started_driver.driver_outgoing().service().connect_next().unwrap();
        let (client_end, server_end) = fdf_fidl::create_channel();
        service_proxy.device(server_end).unwrap();
        let (sender, receiver) = mpsc::channel();
        started_driver.harness().dispatcher().spawn(async move {
            let dispatcher = ClientDispatcher::new(client_end);
            let client = dispatcher.client();
            sender.send(client).unwrap();
            dispatcher.run_client().await.unwrap();
        });

        let client = receiver.recv().unwrap();

        // Retrieve and verify the max transfer size.
        assert_eq!(
            0x1234ABCD,
            u64::from(client.get_max_transfer_size().await.unwrap().unwrap().size)
        );

        // Set the bitrate to a value that should succeed.
        client.set_bitrate(BITRATE).await.unwrap().unwrap();

        // Set the bitrate to a value that should not succeed.
        let _ = client.set_bitrate(BITRATE + 1).await.unwrap().unwrap_err();

        // Send a Transact() request and verify the read data.
        let result = client.transact(Vec::<i2cimpl::I2cImplOp>::new()).await.unwrap();
        let read = result.unwrap();
        assert_eq!(read.read.len(), 1);
        assert_eq!(read.read[0].data.len(), 3);
        assert_eq!(read.read[0].data[0], 0);
        assert_eq!(read.read[0].data[1], 1);
        assert_eq!(read.read[0].data[2], 2);
        client.close();

        started_driver.stop_driver().await;
    }
}
