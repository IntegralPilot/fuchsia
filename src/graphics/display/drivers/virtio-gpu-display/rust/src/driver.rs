// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::resources::PlatformResources;
use crate::virtio::{VirtioFeatureBits, VirtioPciDevice, VirtioPciDeviceBuilder};

use fdf_component::{Driver, DriverContext, DriverError, Node, driver_register};
use log::info;
use std::sync::{Arc, Mutex};

/// Interfaces with the Fuchsia Driver Framework.
struct VirtioGpuDisplayDriver {
    /// The driver must maintain an open connection to the Node.
    #[expect(unused)]
    node: Node,

    #[expect(unused)]
    pci_device: Arc<Mutex<VirtioPciDevice>>,
}

driver_register!(VirtioGpuDisplayDriver);

impl Driver for VirtioGpuDisplayDriver {
    const NAME: &str = "virtio-gpu-display";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        info!("VirtioGpuDisplayDriver::start()");

        let platform_resources = PlatformResources::new(&mut context)?;

        let mut pci_device_builder =
            VirtioPciDeviceBuilder::new(platform_resources.pci_client).await?;
        // TODO(https://fxbug.dev/504722357): Add virtio-gpu feature negotiation.
        pci_device_builder.accept_features(VirtioFeatureBits::default()).await?;
        let pci_device = pci_device_builder.build()?;

        let node = context.take_node()?;

        Ok(Self { node, pci_device: Arc::new(Mutex::new(pci_device)) })
    }

    async fn stop(&self) {
        info!("VirtioGpuDisplayDriver::stop()");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::testing::harness::TestHarness;

    // TODO(https://fxbug.dev/504722357): Figure out driver-level testing once
    // the Rust port is complete.
    #[fuchsia::test]
    #[ignore]
    async fn test_driver_start() {
        let mut harness = TestHarness::<VirtioGpuDisplayDriver>::new();

        let started_driver = harness.start_driver().await.expect("Driver start failed");

        started_driver.stop_driver().await;
    }
}
