// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.InfoResource protocol.
pub struct InfoResource {
    resource: Resource,
}

impl InfoResource {
    /// `resource` must be the Info resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_INFO_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Info resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::InfoResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::InfoResourceRequest::Get { responder }) = stream.try_next().await? {
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

    async fn get_info_resource() -> Result<Resource, Error> {
        let info_resource_provider = connect_to_protocol::<fkernel::InfoResourceMarker>()?;
        let info_resource_handle = info_resource_provider.get().await?;
        Ok(Resource::from(info_resource_handle))
    }

    async fn serve_info_resource() -> Result<fkernel::InfoResourceProxy, Error> {
        let info_resource = get_info_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::InfoResourceMarker>();
        fasync::Task::local(
            InfoResource::new(info_resource)
                .unwrap_or_else(|e| panic!("Error while creating info resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving INFO resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn base_type_is_info() -> Result<(), Error> {
        let info_resource_provider = serve_info_resource().await?;
        let info_resource: Resource = info_resource_provider.get().await?;
        let resource_info = info_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_INFO_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
