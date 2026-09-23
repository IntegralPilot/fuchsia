// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.MexecResource protocol.
pub struct MexecResource {
    resource: Resource,
}

impl MexecResource {
    /// `resource` must be the MEXEC resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_MEXEC_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("MEXEC resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::MexecResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::MexecResourceRequest::Get { responder }) = stream.try_next().await?
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

    async fn get_mexec_resource() -> Result<Resource, Error> {
        let mexec_resource_provider = connect_to_protocol::<fkernel::MexecResourceMarker>()?;
        let mexec_resource_handle = mexec_resource_provider.get().await?;
        Ok(Resource::from(mexec_resource_handle))
    }

    async fn serve_mexec_resource() -> Result<fkernel::MexecResourceProxy, Error> {
        let mexec_resource = get_mexec_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::MexecResourceMarker>();
        fasync::Task::local(
            MexecResource::new(mexec_resource)
                .unwrap_or_else(|e| panic!("Error while creating MEXEC resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving MEXEC resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_mexec() -> Result<(), Error> {
        let mexec_resource_provider = serve_mexec_resource().await?;
        let mexec_resource: Resource = mexec_resource_provider.get().await?;
        let resource_info = mexec_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_MEXEC_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
