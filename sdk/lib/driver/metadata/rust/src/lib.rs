// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod serde;

use fdf_component::ServiceOffer;
use fidl_fuchsia_driver_framework as fdf;
use fidl_next::{Responder, ServerEnd};
use fidl_next_fuchsia_driver_metadata as fmetadata;
use fidl_next_fuchsia_hardware_platform_device as fdevice_next;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use std::sync::Arc;

pub use serde::*;

pub struct MetadataServer {
    data: Option<Arc<Vec<u8>>>,
    name: String,
}

impl MetadataServer {
    pub fn new(name: impl Into<String>) -> Self {
        Self { data: None, name: name.into() }
    }

    pub fn with_metadata<T: fidl::Persistable>(self, metadata: &T) -> Result<Self, zx::Status> {
        let data = fidl::persist(metadata).map_err(|_| zx::Status::INTERNAL)?;
        Ok(Self { data: Some(Arc::new(data)), name: self.name })
    }

    pub async fn forward_from_pdev(
        self,
        pdev: &fidl_fuchsia_hardware_platform_device::DeviceProxy,
    ) -> Result<Self, zx::Status> {
        let data = pdev
            .get_metadata(&self.name)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(Self { data: Some(Arc::new(data)), name: self.name })
    }

    pub async fn forward_from_pdev_next(
        self,
        pdev: &fidl_next::Client<fdevice_next::Device>,
    ) -> Result<Self, zx::Status> {
        let data = pdev
            .get_metadata(&self.name)
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(|e| e.err().unwrap_or(zx::Status::INTERNAL))?
            .metadata;
        Ok(Self { data: Some(Arc::new(data)), name: self.name })
    }

    pub fn create_handler(&self, scope: fasync::ScopeHandle) -> Option<MetadataHandler> {
        let data = self.data.clone()?;
        Some(MetadataHandler { data, scope })
    }

    pub fn create_service_offer(&self) -> Option<ServiceOffer<fmetadata::Service>> {
        Some(ServiceOffer::<fmetadata::Service>::new_next())
    }

    pub fn serve<O>(
        &self,
        fs: &mut ServiceFs<O>,
        scope: fasync::ScopeHandle,
        instance_name: impl Into<String>,
    ) -> Option<fdf::Offer>
    where
        O: ServiceObjTrait,
    {
        let data = self.data.clone()?;
        let handler = MetadataHandler { data, scope };

        let offer = ServiceOffer::<fmetadata::Service>::new_with_name(self.name.clone())
            .add_default_named_next(fs, instance_name, handler)
            .build_zircon_offer_next();

        Some(offer)
    }
}

pub struct MetadataHandler {
    data: Arc<Vec<u8>>,
    scope: fasync::ScopeHandle,
}

impl fmetadata::ServiceHandler for MetadataHandler {
    fn metadata(&self, server_end: ServerEnd<fmetadata::Metadata>) {
        let data = self.data.clone();
        self.scope.spawn_local(async move {
            let dispatcher = fidl_next::ServerDispatcher::new(server_end);
            let _ = dispatcher.run_local(MetadataServerImpl { data }).await;
        });
    }
}

struct MetadataServerImpl {
    data: Arc<Vec<u8>>,
}

impl fmetadata::MetadataLocalServerHandler for MetadataServerImpl {
    async fn get_persisted_metadata(
        &mut self,
        responder: Responder<fmetadata::metadata::GetPersistedMetadata>,
    ) {
        let _ = responder.respond(self.data.as_slice()).await;
    }
}
