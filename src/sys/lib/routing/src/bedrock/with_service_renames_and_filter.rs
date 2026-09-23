// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use capability_source::{AggregateCapability, CapabilitySource, FilteredProviderSource};
use cm_rust::NameMapping;
use cm_rust::offer::{OfferDecl, OfferServiceDecl};
use fidl_fuchsia_component_runtime::RouteRequest;
use router_error::RouterError;
#[cfg(target_os = "fuchsia")]
use runtime_capabilities::CapabilityBound;
use runtime_capabilities::{
    Capability, Connector, Data, Dictionary, DirConnector, Routable, Router, WeakInstanceToken,
};
use std::sync::Arc;

/// A router that will apply renames/filtering on any dictionaries routed through it.
struct ServiceRenameRouter {
    router: Arc<Router<DirConnector>>,
    // This field is not read on host
    #[allow(dead_code)]
    renames: Box<[NameMapping]>,
    offer_service_decl: OfferServiceDecl,
}

#[async_trait]
impl Routable<DirConnector> for ServiceRenameRouter {
    async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<DirConnector>>, RouterError> {
        let result = self.router.route(request, target.clone()).await;
        match result {
            #[cfg(target_os = "fuchsia")]
            Ok(Some(source_services_directory)) => {
                let target_services_dict = Dictionary::new();
                for rename in &self.renames {
                    let path = cm_types::RelativePath::new(&rename.source_name).unwrap();
                    let dir_connector = source_services_directory.clone().with_subdir(path);
                    let prev = target_services_dict.insert(
                        rename.target_name.clone(),
                        Capability::DirConnector(dir_connector),
                    );
                    assert!(prev.is_none(), "failed to insert into target services dict");
                }
                let dir_entry = target_services_dict
                    .try_into_directory_entry(
                        vfs::execution_scope::ExecutionScope::new(),
                        target.clone(),
                    )
                    .unwrap();
                let dir_connector =
                    DirConnector::from_directory_entry(dir_entry, fidl_fuchsia_io::PERM_READABLE);
                return Ok(Some(dir_connector));
            }
            #[cfg(not(target_os = "fuchsia"))]
            Ok(Some(_)) => {
                let (_receiver, dir_connector) = DirConnector::new();
                return Ok(Some(dir_connector));
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        let source = self.router.route_debug(request, target).await?;
        Ok(self.wrap_debug_response(source))
    }
}

impl ServiceRenameRouter {
    /// Converts a capability source into `CapabilitySource::FilteredProvider`
    fn wrap_debug_response(&self, capability_source: CapabilitySource) -> CapabilitySource {
        match capability_source {
            CapabilitySource::Component(source) => {
                CapabilitySource::FilteredProvider(FilteredProviderSource {
                    capability: AggregateCapability::Service(
                        self.offer_service_decl.source_name.clone(),
                    ),
                    moniker: source.moniker.clone(),
                    service_capability: source.capability,
                    offer_service_decl: self.offer_service_decl.clone(),
                })
            }
            CapabilitySource::FilteredProvider(earlier_filtered_provider) => {
                CapabilitySource::FilteredProvider(FilteredProviderSource {
                    capability: AggregateCapability::Service(
                        self.offer_service_decl.source_name.clone(),
                    ),
                    moniker: earlier_filtered_provider.moniker.clone(),
                    service_capability: earlier_filtered_provider.service_capability,
                    offer_service_decl: self.offer_service_decl.clone(),
                })
            }
            other_source => panic!("unexpected source? {:?}", other_source),
        }
    }
}

pub trait WithServiceRenamesAndFilter {
    /// When a dictionary is returned through this router a new dictionary will instead be returned
    /// with the entries from the first dictionary, but renamed and filtered as described in
    /// `offer_service_decl`.
    ///
    /// This will be a no-op if `offer` is not `OfferDecl::Service`.
    fn with_service_renames_and_filter(self, offer: OfferDecl) -> Capability;
}

impl WithServiceRenamesAndFilter for Arc<Router<Dictionary>> {
    fn with_service_renames_and_filter(self, _offer: OfferDecl) -> Capability {
        self.into()
    }
}

impl WithServiceRenamesAndFilter for Arc<Router<Data>> {
    fn with_service_renames_and_filter(self, _offer: OfferDecl) -> Capability {
        self.into()
    }
}

impl WithServiceRenamesAndFilter for Arc<Router<Connector>> {
    fn with_service_renames_and_filter(self, _offer: OfferDecl) -> Capability {
        self.into()
    }
}

impl WithServiceRenamesAndFilter for Arc<Router<DirConnector>> {
    fn with_service_renames_and_filter(self, offer: OfferDecl) -> Capability {
        let offer_service_decl = match offer {
            OfferDecl::Service(decl) => decl,
            _ => {
                return self.into();
            }
        };
        let renames = process_offer_renames(&offer_service_decl);
        if renames.is_empty() {
            // There are no renames or filters set, rendering as a no-op. Return the underlying
            // router, because we won't do anything.
            return self.into();
        }
        Capability::DirConnectorRouter(Router::new(ServiceRenameRouter {
            router: self,
            renames,
            offer_service_decl: *offer_service_decl,
        }))
    }
}

pub fn process_offer_renames(service_offer_decl: &OfferServiceDecl) -> Box<[NameMapping]> {
    match (
        service_offer_decl.renamed_instances.as_ref(),
        service_offer_decl.source_instance_filter.as_ref(),
    ) {
        (Some(renames), Some(filter)) if !renames.is_empty() && !filter.is_empty() => {
            // If rename mappings and a filter are set, we ignore mappings that aren't included
            // in the filter.
            IntoIterator::into_iter(renames)
                .filter(|mapping| filter.contains(&mapping.target_name))
                .cloned()
                .collect()
        }
        (Some(renames), _) if !renames.is_empty() => renames.clone(),
        (_, Some(filter)) if !filter.is_empty() => {
            // If a filter is set and no renames, we can implement the filter as a set of
            // renames. This helps reduce code duplication.
            IntoIterator::into_iter(filter)
                .map(|name| NameMapping { source_name: name.clone(), target_name: name.clone() })
                .collect()
        }
        _ => Box::from([]),
    }
}
