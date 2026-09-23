// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.SamplingResource protocol.
pub struct SamplingResource {
    resource: Resource,
}

impl SamplingResource {
    /// `resource` must be the Sampling resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_SAMPLING_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Sampling resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::SamplingResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::SamplingResourceRequest::Get { responder }) =
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

    async fn get_sampling_resource() -> Result<Resource, Error> {
        let sampling_resource_provider = connect_to_protocol::<fkernel::SamplingResourceMarker>()?;
        let sampling_resource_handle = sampling_resource_provider.get().await?;
        Ok(Resource::from(sampling_resource_handle))
    }

    async fn serve_sampling_resource() -> Result<fkernel::SamplingResourceProxy, Error> {
        let sampling_resource = get_sampling_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::SamplingResourceMarker>();
        fasync::Task::local(
            SamplingResource::new(sampling_resource)
                .unwrap_or_else(|e| panic!("Error while creating sampling resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving sampling resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn base_type_is_sampling() -> Result<(), Error> {
        let sampling_resource_provider = serve_sampling_resource().await?;
        let sampling_resource: Resource = sampling_resource_provider.get().await?;
        let resource_info = sampling_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_SAMPLING_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
