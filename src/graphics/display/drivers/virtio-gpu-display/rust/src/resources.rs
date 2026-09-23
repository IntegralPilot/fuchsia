// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{DriverContext, ServiceInstance};
use fidl_next::ClientEnd;
use log::error;

use fidl_next_fuchsia_hardware_pci as fidl_pci;
use fidl_next_fuchsia_sysmem2 as fidl_sysmem2;

/// Bundles all external resources used by the driver.
///
/// In production, the resources are obtained from the `DriverContext` provided
/// on driver startup. In testing, the struct is populated with test doubles.
pub struct PlatformResources {
    #[allow(unused)]
    pub pci_client: ClientEnd<fidl_pci::Device>,

    #[allow(unused)]
    pub sysmem_client: ClientEnd<fidl_sysmem2::Allocator>,
}

impl PlatformResources {
    /// Obtains all the resources used by the driver.
    pub fn new(context: &mut DriverContext) -> Result<Self, zx::Status> {
        let pci_service: ServiceInstance<fidl_pci::Service> =
            context.incoming.service().connect_next().map_err(|_| zx::Status::INTERNAL)?;

        let (pci_client, pci_server) = fidl_next::fuchsia::create_channel();
        pci_service.device(pci_server).map_err(|err| {
            error!("Failed to connect to PCI device: {err:?}");
            zx::Status::INTERNAL
        })?;

        let sysmem_client: ClientEnd<fidl_sysmem2::Allocator> =
            context.incoming.connect_protocol_next().map_err(|_| zx::Status::INTERNAL)?;

        Ok(PlatformResources { pci_client, sysmem_client })
    }
}
