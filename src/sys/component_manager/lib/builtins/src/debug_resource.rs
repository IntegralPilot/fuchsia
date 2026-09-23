// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.DebugResource protocol.
pub struct DebugResource {
    resource: Resource,
}

impl DebugResource {
    /// `resource` must be the Debug resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_DEBUG_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Debug resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::DebugResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::DebugResourceRequest::Get { responder }) = stream.try_next().await?
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

    async fn get_debug_resource() -> Result<Resource, Error> {
        let debug_resource_provider = connect_to_protocol::<fkernel::DebugResourceMarker>()?;
        let debug_resource_handle = debug_resource_provider.get().await?;
        Ok(Resource::from(debug_resource_handle))
    }

    async fn serve_debug_resource() -> Result<fkernel::DebugResourceProxy, Error> {
        let debug_resource = get_debug_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::DebugResourceMarker>();
        fasync::Task::local(
            DebugResource::new(debug_resource)
                .unwrap_or_else(|e| panic!("Error while creating debug resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving debug resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn base_type_is_debug() -> Result<(), Error> {
        let debug_resource_provider = serve_debug_resource().await?;
        let debug_resource: Resource = debug_resource_provider.get().await?;
        let resource_info = debug_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_DEBUG_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
