// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use fdf_component::{Driver, DriverContext, DriverError, Node, NodeBuilder, driver_register};
use fidl_fuchsia_hardware_i2c as i2c;
use log::info;

/// The implementation of our driver will live in this object, which implements [`Driver`].
#[allow(unused)]
struct ZirconChildDriver {
    /// The [`Node`] is our handle to the node we bound to. We need to keep this handle
    /// open to keep the node around.
    node: Node,
}

// This creates the exported driver registration structures that allow the driver host to
// find and run the start and stop methods on our `ZirconChildDriver`.
driver_register!(ZirconChildDriver);

impl Driver for ZirconChildDriver {
    const NAME: &str = "zircon_child_rust_driver";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!(
            "Binding node client. Every driver needs to do this for the driver to be considered loaded."
        );
        let node = context.take_node()?;

        let device = get_i2c_device(&context)?;
        let device_name = device.get_name().await?.map_err(zx::Status::err_from_raw)?;
        info!("i2c device name: {device_name}");

        info!("Adding child node with i2c device name as a property value");
        let child_node = NodeBuilder::new("transport-child")
            .add_property(bind_fuchsia_test::TEST_CHILD, device_name)
            .build();
        node.add_child(child_node).await?;

        Ok(Self { node })
    }

    async fn stop(&self) {
        info!("ZirconChildDriver::stop() was invoked. Use this function to do any cleanup needed.");
    }
}

fn get_i2c_device(context: &DriverContext) -> Result<i2c::DeviceProxy, Error> {
    let service_proxy = context.incoming.service_marker(i2c::ServiceMarker).connect()?;

    let proxy = service_proxy
        .connect_to_device()
        .context("Error connecting to i2c device proxy at driver startup")?;
    Ok(proxy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::ServiceOffer;
    use fdf_component::testing::harness::TestHarness;
    use fuchsia_async as fasync;
    use fuchsia_component::server::ServiceFs;
    use futures::TryStreamExt;

    const TEST_NAME: &str = "test_i2c";

    #[fuchsia::test]
    async fn verify_child_node() {
        let scope = fasync::Scope::new();
        let mut driver_incoming = ServiceFs::new();

        let scope_handle = scope.to_handle();
        let offer = ServiceOffer::new()
            .add_default_named(&mut driver_incoming, "default", move |i| {
                let i2c::ServiceRequest::Device(mut service) = i;
                scope_handle.spawn(async move {
                    while let Ok(Some(request)) = service.try_next().await {
                        match request {
                            i2c::DeviceRequest::Transfer { transactions: _, responder } => {
                                responder.send(Ok(&[vec![0xa_u8, 0xb_u8, 0xc_u8]])).unwrap();
                            }
                            i2c::DeviceRequest::GetName { responder } => {
                                responder.send(Ok(TEST_NAME)).unwrap();
                            }
                        }
                    }
                });
            })
            .build_zircon_offer();

        let mut harness = TestHarness::<ZirconChildDriver>::new()
            .add_offer(offer)
            .set_driver_incoming(driver_incoming);
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
            fidl_fuchsia_driver_framework::NodePropertyValue::StringValue(TEST_NAME.to_string())
        );

        started_driver.stop_driver().await;
    }
}
