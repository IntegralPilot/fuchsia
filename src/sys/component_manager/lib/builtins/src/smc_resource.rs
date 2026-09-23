// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.SmcResource protocol.
pub struct SmcResource {
    resource: Resource,
}

impl SmcResource {
    /// `resource` must be the SMC resource.
    pub fn new(resource: Resource) -> Arc<Self> {
        Arc::new(Self { resource })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::SmcResourceRequestStream,
    ) -> Result<(), Error> {
        if self.resource.info()?.kind != zx::sys::ZX_RSRC_KIND_SMC {
            return Err(format_err!("invalid handle kind, expected SMC"));
        }
        while let Some(fkernel::SmcResourceRequest::Get { responder }) = stream.try_next().await? {
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

    async fn get_smc_resource() -> Result<Resource, Error> {
        let smc_resource_provider = connect_to_protocol::<fkernel::SmcResourceMarker>()?;
        let smc_resource_handle = smc_resource_provider.get().await?;
        Ok(Resource::from(smc_resource_handle))
    }

    async fn serve_smc_resource() -> Result<fkernel::SmcResourceProxy, Error> {
        let smc_resource = get_smc_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::SmcResourceMarker>();
        fasync::Task::local(
            SmcResource::new(smc_resource)
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving SMC resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_smc() -> Result<(), Error> {
        let smc_resource_provider = serve_smc_resource().await?;
        let smc_resource: Resource = smc_resource_provider.get().await?;
        let resource_info = smc_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SMC);
        assert_eq!(resource_info.base, 0);
        assert_eq!(resource_info.size, 0);
        Ok(())
    }
}
