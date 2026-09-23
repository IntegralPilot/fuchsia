// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.CpuResource protocol.
pub struct CpuResource {
    resource: Resource,
}

impl CpuResource {
    /// `resource` must be the Cpu resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_CPU_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("CPU resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::CpuResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::CpuResourceRequest::Get { responder }) = stream.try_next().await? {
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

    async fn get_cpu_resource() -> Result<Resource, Error> {
        let cpu_resource_provider = connect_to_protocol::<fkernel::CpuResourceMarker>()?;
        let cpu_resource_handle = cpu_resource_provider.get().await?;
        Ok(Resource::from(cpu_resource_handle))
    }

    async fn serve_cpu_resource() -> Result<fkernel::CpuResourceProxy, Error> {
        let cpu_resource = get_cpu_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::CpuResourceMarker>();
        fasync::Task::local(
            CpuResource::new(cpu_resource)
                .unwrap_or_else(|e| panic!("Error while creating CPU resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving CPU resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn base_type_is_cpu() -> Result<(), Error> {
        let cpu_resource_provider = serve_cpu_resource().await?;
        let cpu_resource: Resource = cpu_resource_provider.get().await?;
        let resource_info = cpu_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_CPU_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
