// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.TracingResource protocol.
pub struct TracingResource {
    resource: Resource,
}

impl TracingResource {
    /// `resource` must be the tracing resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_TRACING_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Tracing resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::TracingResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::TracingResourceRequest::Get { responder }) =
            stream.try_next().await?
        {
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

    async fn get_tracing_resource() -> Result<Resource, Error> {
        let tracing_resource_provider = connect_to_protocol::<fkernel::TracingResourceMarker>()?;
        let tracing_resource_handle = tracing_resource_provider.get().await?;
        Ok(Resource::from(tracing_resource_handle))
    }

    async fn serve_tracing_resource() -> Result<fkernel::TracingResourceProxy, Error> {
        let tracing_resource = get_tracing_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::TracingResourceMarker>();
        fasync::Task::local(
            TracingResource::new(tracing_resource)
                .unwrap_or_else(|e| panic!("Error while creating tracing resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving tracing resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_tracing() -> Result<(), Error> {
        let tracing_resource_provider = serve_tracing_resource().await?;
        let tracing_resource: Resource = tracing_resource_provider.get().await?;
        let resource_info = tracing_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_TRACING_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
