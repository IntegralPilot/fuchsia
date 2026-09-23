// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fidl_fuchsia_kernel as fkernel;
use futures::prelude::*;
use std::sync::Arc;
use zx::Resource;

/// An implementation of fuchsia.kernel.ProfileResource protocol.
pub struct ProfileResource {
    resource: Resource,
}

impl ProfileResource {
    /// `resource` must be the Profile resource.
    pub fn new(resource: Resource) -> Result<Arc<Self>, Error> {
        let resource_info = resource.info()?;
        if resource_info.kind != zx::sys::ZX_RSRC_KIND_SYSTEM
            || resource_info.base != zx::sys::ZX_RSRC_SYSTEM_PROFILE_BASE
            || resource_info.size != 1
        {
            return Err(format_err!("Profile resource not available."));
        }
        Ok(Arc::new(Self { resource }))
    }

    pub async fn serve(
        self: Arc<Self>,
        mut stream: fkernel::ProfileResourceRequestStream,
    ) -> Result<(), Error> {
        while let Some(fkernel::ProfileResourceRequest::Get { responder }) =
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

    async fn get_profile_resource() -> Result<Resource, Error> {
        let profile_resource_provider = connect_to_protocol::<fkernel::ProfileResourceMarker>()?;
        let profile_resource_handle = profile_resource_provider.get().await?;
        Ok(Resource::from(profile_resource_handle))
    }

    async fn serve_profile_resource() -> Result<fkernel::ProfileResourceProxy, Error> {
        let profile_resource = get_profile_resource().await?;

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fkernel::ProfileResourceMarker>();
        fasync::Task::local(
            ProfileResource::new(profile_resource)
                .unwrap_or_else(|e| panic!("Error while creating profile resource service: {}", e))
                .serve(stream)
                .unwrap_or_else(|e| panic!("Error while serving PROFILE resource service: {}", e)),
        )
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    async fn kind_type_is_profile() -> Result<(), Error> {
        let profile_resource_provider = serve_profile_resource().await?;
        let profile_resource: Resource = profile_resource_provider.get().await?;
        let resource_info = profile_resource.info()?;
        assert_eq!(resource_info.kind, zx::sys::ZX_RSRC_KIND_SYSTEM);
        assert_eq!(resource_info.base, zx::sys::ZX_RSRC_SYSTEM_PROFILE_BASE);
        assert_eq!(resource_info.size, 1);
        Ok(())
    }
}
