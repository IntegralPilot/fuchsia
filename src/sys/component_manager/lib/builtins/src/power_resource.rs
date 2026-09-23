// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.PowerResource protocol.
pub struct PowerResource {
    resource: Resource,
}

impl PowerResource {
    /// `resource` must be the Power resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_POWER_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Power resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::PowerResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::PowerResourceRequest::Get { responder }) = stream.try_next().await?
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

    async fn get_power_resource() -> Result<Resource, Error> {
        let power_resource_provider = connect_to_protocol::<fkernel::PowerResourceMarker>()?;
        let power_resource_handle = power_resource_provider.get().await?;
        Ok(Resource::from(power_resource_handle))
    }

    async fn serve_power_resource() -> Result<fkernel::PowerResourceProxy, Error> {
        let power_resource = get_power_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::PowerResourceMarker>();
        fasync::Task::local(
            PowerResource::new(power_resource)
                .unwrap_or_else(|e| panic!("Error while creating power resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving POWER resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_power() -> Result<(), Error> {
        let power_resource_provider = serve_power_resource().await?;
        let power_resource: Resource = power_resource_provider.get().await?;
        let resource_info = power_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_POWER_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
