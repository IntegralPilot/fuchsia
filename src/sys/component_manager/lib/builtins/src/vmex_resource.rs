// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.VmexResource protocol.
pub struct VmexResource {
    resource: Resource,
}

impl VmexResource {
    /// `resource` must be the Vmex resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_VMEX_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Vmex resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::VmexResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::VmexResourceRequest::Get { responder }) = stream.try_next().await? {
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

    async fn get_vmex_resource() -> Result<Resource, Error> {
        let vmex_resource_provider = connect_to_protocol::<fkernel::VmexResourceMarker>()?;
        let vmex_resource_handle = vmex_resource_provider.get().await?;
        Ok(Resource::from(vmex_resource_handle))
    }

    async fn serve_vmex_resource() -> Result<fkernel::VmexResourceProxy, Error> {
        let vmex_resource = get_vmex_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::VmexResourceMarker>();
        fasync::Task::local(
            VmexResource::new(vmex_resource)
                .unwrap_or_else(|e| panic!("Error while creating vmex resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving VMEX resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn base_type_is_vmex() -> Result<(), Error> {
        let vmex_resource_provider = serve_vmex_resource().await?;
        let vmex_resource: Resource = vmex_resource_provider.get().await?;
        let resource_info = vmex_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_VMEX_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
