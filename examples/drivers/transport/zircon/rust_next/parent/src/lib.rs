// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{
    Driver, DriverContext, DriverError, Node, NodeBuilder, ServiceOffer, driver_register,
};
use fidl_next::{Request, Responder, ServerEnd};
use fidl_next_fuchsia_hardware_i2c as i2c;
use fuchsia_async::{Scope, ScopeHandle};
use fuchsia_component::server::ServiceFs;
use futures::StreamExt as _;
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct ZirconTransportParent {
    /// The [`NodeProxy`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
    /// The scope for the driver.
    scope: Scope,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `ZirconTransportParent`.
driver_register!(ZirconTransportParent);

struct DeviceServer;

impl i2c::DeviceServerHandler for DeviceServer {
    async fn transfer(
        &mut self,
        _: Request<i2c::device::Transfer>,
        responder: Responder<i2c::device::Transfer>,
    ) {
        responder.respond([[0x1u8, 0x2, 0x3]]).await.unwrap();
    }

    async fn get_name(&mut self, responder: Responder<i2c::device::GetName>) {
        responder.respond("rust i2c server").await.unwrap();
    }
}

struct Service {
    scope: ScopeHandle,
}

impl i2c::ServiceHandler for Service {
    fn device(&self, server_end: ServerEnd<i2c::Device>) {
        server_end.spawn_on(DeviceServer, &self.scope);
    }
}

impl Driver for ZirconTransportParent {
    const NAME: &str = "zircon_parent_rust_next_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        let scope = Scope::new();

        info!("Offering an i2c service in the outgoing directory");
        let mut outgoing = ServiceFs::new();
        let offer = ServiceOffer::<i2c::Service>::new_next()
            .add_default_named_next(&mut outgoing, "default", Service { scope: scope.to_handle() })
            .build_zircon_offer_next();

        info!("Creating child node with a service offer");
        let child_node =
            NodeBuilder::new("zircon_transport_rust_next_child").add_offer(offer).build();
        node.add_child(child_node).await?;

        context.serve_outgoing(&mut outgoing)?;

        scope.spawn(outgoing.collect());

        Ok(Self { node, scope })
    }

    async fn stop(&self) {
        info!(
            "ZirconTransportParent::stop() was invoked. Use this function to do any cleanup needed."
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::ServiceInstance;
    use fdf_component::testing::harness::TestHarness;

    #[fuchsia::test]
    async fn connect_and_get_name() {
        let scope = fuchsia_async::Scope::new();
        let mut harness = TestHarness::<ZirconTransportParent>::new();
        let started_driver = harness.start_driver().await.unwrap();
        let children = started_driver.node().children();
        assert_eq!(children.len(), 1);
        assert!(children.contains_key("zircon_transport_rust_next_child"));

        let service_proxy: ServiceInstance<i2c::Service> =
            started_driver.driver_outgoing().service().connect_next().unwrap();
        let (client_end, server_end) = fidl_next::fuchsia::create_channel();
        service_proxy.device(server_end).unwrap();
        let client = client_end.spawn_on(&scope);
        let name = client.get_name().await.unwrap().unwrap();
        assert_eq!(name.name, "rust i2c server");
        client.close();
        started_driver.stop_driver().await;
    }
}
