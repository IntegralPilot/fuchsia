// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.MsiResource protocol.
pub struct MsiResource {
    resource: Resource,
}

impl MsiResource {
    /// `resource` must be the msi resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_MSI_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Msi resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::MsiResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::MsiResourceRequest::Get { responder }) = stream.try_next().await? {
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

    async fn get_msi_resource() -> Result<Resource, Error> {
        let msi_resource_provider = connect_to_protocol::<fkernel::MsiResourceMarker>()?;
        let msi_resource_handle = msi_resource_provider.get().await?;
        Ok(Resource::from(msi_resource_handle))
    }

    async fn serve_msi_resource() -> Result<fkernel::MsiResourceProxy, Error> {
        let msi_resource = get_msi_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::MsiResourceMarker>();
        fasync::Task::local(
            MsiResource::new(msi_resource)
                .unwrap_or_else(|e| panic!("Error while creating msi resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving MSI resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn base_type_is_msi() -> Result<(), Error> {
        let msi_resource_provider = serve_msi_resource().await?;
        let msi_resource: Resource = msi_resource_provider.get().await?;
        let resource_info = msi_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_MSI_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
