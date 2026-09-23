// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.StallResource protocol.
pub struct StallResource {
    resource: Resource,
}

impl StallResource {
    /// `resource` must be the stall resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_STALL_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Stall resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::StallResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::StallResourceRequest::Get { responder }) = stream.try_next().await?
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

    async fn get_stall_resource() -> Result<Resource, Error> {
        let stall_resource_provider = connect_to_protocol::<fkernel::StallResourceMarker>()?;
        let stall_resource_handle = stall_resource_provider.get().await?;
        Ok(Resource::from(stall_resource_handle))
    }

    async fn serve_stall_resource() -> Result<fkernel::StallResourceProxy, Error> {
        let stall_resource = get_stall_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::StallResourceMarker>();
        fasync::Task::local(
            StallResource::new(stall_resource)
                .unwrap_or_else(|e| panic!("Error while creating stall resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving stall resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_stall() -> Result<(), Error> {
        let stall_resource_provider = serve_stall_resource().await?;
        let stall_resource: Resource = stall_resource_provider.get().await?;
        let resource_info = stall_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_STALL_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
