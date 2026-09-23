// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.IoportResource protocol.
pub struct IoportResource {
    resource: Resource,
}

impl IoportResource {
    /// `resource` must be the IOPORT resource.
    pub fn new(resource: Resource) -> Arc<Self> {
        Arc::new(Self { resource })
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::IoportResourceRequestStream,
    ) -> Result<(), Error> {
        if self.resource.info()?.kind != zx::sys::ZX_RSRC_KIND_IOPORT {
            return Err(format_err!("invalid handle kind, expected IOPORT"));
        }
        while let Some(fkernel::IoportResourceRequest::Get { responder }) =
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

    async fn get_ioport_resource() -> Result<Resource, Error> {
        let ioport_resource_provider = connect_to_protocol::<fkernel::IoportResourceMarker>()?;
        let ioport_resource_handle = ioport_resource_provider.get().await?;
        Ok(Resource::from(ioport_resource_handle))
    }

    async fn serve_ioport_resource() -> Result<fkernel::IoportResourceProxy, Error> {
        let ioport_resource = get_ioport_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::IoportResourceMarker>();
        fasync::Task::local(
            IoportResource::new(ioport_resource)
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving IOPORT resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn fail_with_no_ioport_resource() -> Result<(), Error> {
        let (_, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::IoportResourceMarker>();
        IoportResource::new(Resource::from(zx::NullableHandle::invalid()))
            .serve(stream)
            .await
            .expect_err("should fail to serve stream with an invalid resource");
        Ok(())
    }

    #[fuchsia::test]
    async fn kind_type_is_ioport() -> Result<(), Error> {
        let ioport_resource_provider = serve_ioport_resource().await?;
        let ioport_resource: Resource = ioport_resource_provider.get().await?;
        let resource_info = ioport_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_IOPORT);
        assert_eq!(resource_info.base, 0);
        assert_eq!(resource_info.size, 0);
        Ok(())
    }
}
