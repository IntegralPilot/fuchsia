// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.MmioResource protocol.
pub struct MmioResource {
    resource: Resource,
}

impl MmioResource {
    /// `resource` must be the MMIO resource.
    pub fn new(resource: Resource) -> Arc<Self> {
        Arc::new(Self { resource })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::MmioResourceRequestStream,
    ) -> Result<(), Error> {
        if self.resource.info()?.kind != zx::sys::ZX_RSRC_KIND_MMIO {
            return Err(format_err!("invalid handle kind, expected MMIO"));
        }
        while let Some(fkernel::MmioResourceRequest::Get { responder }) = stream.try_next().await? {
            responder.send(self.resource.duplicate_handle(zx::Rights::SAME_RIGHTS)?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_kernel as fkernel;
    use fuchsia_async as fasync;
    use fuchsia_component::client::connect_to_protocol;

    async fn get_mmio_resource() -> Result<Resource, Error> {
        let mmio_resource_provider = connect_to_protocol::<fkernel::MmioResourceMarker>()?;
        let mmio_resource_handle = mmio_resource_provider.get().await?;
        Ok(Resource::from(mmio_resource_handle))
    }

    async fn serve_mmio_resource() -> Result<fkernel::MmioResourceProxy, Error> {
        let mmio_resource = get_mmio_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::MmioResourceMarker>();
        fasync::Task::local(
            MmioResource::new(mmio_resource)
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving MMIO resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_mmio() -> Result<(), Error> {
        let mmio_resource_provider = serve_mmio_resource().await?;
        let mmio_resource: Resource = mmio_resource_provider.get().await?;
        let resource_info = mmio_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_MMIO);
        assert_eq!(resource_info.base, 0);
        assert_eq!(resource_info.size, 0);
        Ok(())
    }
}
