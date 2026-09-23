// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{ComponentInstance, WeakComponentInstance};
use crate::model::routing::service::{
    AnonymizedAggregateCapabilityProvider, AnonymizedAggregateServiceDir, AnonymizedServiceRoute,
};
use async_trait::async_trait;
use capability_source::{
    AggregateInstance, AnonymizedAggregateSource, CapabilitySource,
    FilteredAggregateProviderSource, ServiceInstance,
};
use cm_rust::CapabilityTypeName;
use cm_types::{Name, RelativePath};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_io as fio;
use fuchsia_fs::directory::readdir;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use futures::lock::Mutex as AsyncMutex;
use futures::stream::FuturesUnordered;
use router_error::RouterError;
use routing::bedrock::aggregate_router::AggregateSource;
use routing::component_instance::ComponentInstanceInterface;
use routing::error::{ComponentInstanceError, RouteVerb, RoutingError};
use runtime_capabilities::{
    Capability, CapabilityBound, Dictionary, DirConnector, Routable, Router, RouterErrorInfo,
    WeakInstanceToken,
};
use std::cmp::Ordering;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

#[derive(Debug, Clone)]
enum AnonymizedOrFiltered {
    AnonymizedAggregate(AnonymizedAggregateSource),
    FilteredAggregateProvider(FilteredAggregateProviderSource),
}

impl From<AnonymizedOrFiltered> for CapabilitySource {
    fn from(source: AnonymizedOrFiltered) -> CapabilitySource {
        match source {
            AnonymizedOrFiltered::AnonymizedAggregate(source) => {
                CapabilitySource::AnonymizedAggregate(source)
            }
            AnonymizedOrFiltered::FilteredAggregateProvider(source) => {
                CapabilitySource::FilteredAggregateProvider(source)
            }
        }
    }
}

impl AnonymizedOrFiltered {
    fn name(&self) -> Name {
        let source = CapabilitySource::from(self.clone());
        source
            .source_name()
            .expect("aggregate service sources will always have source names")
            .clone()
    }
}

/// A router which will return a directory that aggregates together entries from the `sources`.
pub struct AggregateRouter {
    component: WeakComponentInstance,
    capability_source: AnonymizedOrFiltered,
    sources: Vec<AggregateSource>,
    // This is set to `Some` when the aggregate router has been started.
    aggregate_directory: AsyncMutex<Option<Arc<DirConnector>>>,
    // This is set to `Some` when the aggregate router has been started, if this is an anonymizing
    // aggregate.
    anonymized_aggregate_service_dir: Mutex<Option<Arc<AnonymizedAggregateServiceDir>>>,
    scope: ExecutionScope,
}

#[async_trait]
impl Routable<DirConnector> for AggregateRouter {
    async fn route(
        &self,
        request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<DirConnector>>, RouterError> {
        let aggregate_dir = self.get_aggregate_dir(request).await?;
        Ok(Some(aggregate_dir))
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        let _aggregate_dir = self.get_aggregate_dir(request).await?;
        Ok(self.get_capability_source_with_instances())
    }

    fn error_info(&self) -> Option<RouterErrorInfo> {
        Some(RouterErrorInfo {
            capability_type: CapabilityTypeName::Service,
            name: self.capability_source.name(),
            availability: cm_rust::Availability::Required,
        })
    }
}

impl AggregateRouter {
    /// Creates a new aggregate router. It will either filter/rename entries or it will aggregate
    /// them together, depending on the value of `capability_source`.
    pub fn new(
        component: Arc<ComponentInstance>,
        sources: Vec<AggregateSource>,
        capability_source: CapabilitySource,
    ) -> Arc<Router<DirConnector>> {
        let capability_source = match capability_source {
            CapabilitySource::AnonymizedAggregate(source) => {
                AnonymizedOrFiltered::AnonymizedAggregate(source)
            }
            CapabilitySource::FilteredAggregateProvider(source) => {
                AnonymizedOrFiltered::FilteredAggregateProvider(source)
            }
            source => panic!("non-aggregate source passed to aggregate router: {:?}", source),
        };
        Router::new(Self {
            component: component.as_weak(),
            capability_source,
            sources,
            aggregate_directory: AsyncMutex::new(None),
            anonymized_aggregate_service_dir: Mutex::new(None),
            scope: component.execution_scope.clone(),
        })
    }

    /// Returns `self.capability_source` with the `instances` field filled in if this is an
    /// anonymizing aggregate.
    fn get_capability_source_with_instances(&self) -> CapabilitySource {
        let mut anonymized_aggregate_source = match &self.capability_source {
            AnonymizedOrFiltered::AnonymizedAggregate(source) => source.clone(),
            AnonymizedOrFiltered::FilteredAggregateProvider(source) => {
                return CapabilitySource::FilteredAggregateProvider(source.clone());
            }
        };

        let mut instances = vec![];
        let service_dir_guard = self.anonymized_aggregate_service_dir.lock();
        if let Some(anonymized_aggregate_service_dir) = &*service_dir_guard {
            let mut dir_entries = anonymized_aggregate_service_dir.entries();
            // Sort the entries (they can show up in any order)
            dir_entries.sort_by(|a, b| match a.source_id.cmp(&b.source_id) {
                Ordering::Equal => a.service_instance.cmp(&b.service_instance),
                o => o,
            });
            for service_entry in dir_entries {
                instances.push(ServiceInstance {
                    instance_name: service_entry.name.clone(),
                    child_name: service_entry.source_id.to_string(),
                    child_instance_name: Name::new(service_entry.service_instance.clone()).unwrap(),
                });
            }
        }
        anonymized_aggregate_source.instances = instances;
        CapabilitySource::AnonymizedAggregate(anonymized_aggregate_source)
    }

    /// Returns the directory containing aggregated entries, and initializes it if necessary.
    async fn get_aggregate_dir(
        &self,
        request: RouteRequest,
    ) -> Result<Arc<DirConnector>, RouterError> {
        let mut maybe_directory = self.aggregate_directory.lock().await;
        if let Some(aggregate_directory) = &*maybe_directory {
            return Ok(aggregate_directory.clone());
        }
        // We hold the `self.aggregate_directory` lock going forward because we want to ensure that
        // only one task is doing this work. Multiple tasks can't make this go any faster, and once
        // we're done the following tasks can immediately grab a clone of the directory we create.

        let aggregate_directory = match &self.capability_source {
            AnonymizedOrFiltered::AnonymizedAggregate(anonymized_source) => {
                self.create_anonymized_aggregate(anonymized_source).await?
            }
            AnonymizedOrFiltered::FilteredAggregateProvider(_) => {
                self.create_filtered_aggregate(request).await?
            }
        };

        *maybe_directory = Some(aggregate_directory.clone());
        Ok(aggregate_directory)
    }

    async fn create_anonymized_aggregate(
        &self,
        anonymized_source: &AnonymizedAggregateSource,
    ) -> Result<Arc<DirConnector>, RouterError> {
        let route = AnonymizedServiceRoute {
            source_moniker: self.component.moniker.clone(),
            members: anonymized_source.members.clone(),
            service_name: anonymized_source.capability.source_name().clone(),
        };
        let aggregate_capability_provider = AnonymizedAggregateServiceProvider {
            component: self.component.clone(),
            sources: self.sources.clone(),
            service_name: anonymized_source.capability.source_name().clone(),
        };
        let service_dir = Arc::new(AnonymizedAggregateServiceDir::new(
            self.component.clone(),
            route,
            Box::new(aggregate_capability_provider),
        ));
        // Errors returned by this function are already logged, so there's not much for us to do if
        // it returns an error. Some children may have been added successfully, so we might as well
        // continue.
        self.component.upgrade().map_err(RoutingError::from)?.hooks.install(service_dir.hooks());
        let _ = service_dir.add_entries_from_children().await;
        *self.anonymized_aggregate_service_dir.lock() = Some(service_dir.clone());
        Ok(DirConnector::from_directory_entry(service_dir.dir_entry(), fio::PERM_READABLE))
    }

    async fn create_filtered_aggregate(
        &self,
        request: RouteRequest,
    ) -> Result<Arc<DirConnector>, RouterError> {
        let source_dir_routers = self.sources.iter().filter_map(|source| match source {
            AggregateSource::DirectoryRouter { source_instance: _, router } => Some(router),
            AggregateSource::Collection { collection_name: _ } => None,
        });

        let mut routing_futures = FuturesUnordered::new();

        for router in source_dir_routers {
            let request = request.clone();
            let target: Arc<WeakInstanceToken> = self.component.clone().into();

            routing_futures.push(async move {
                let route_result = router.route(request, target).await;
                let source_dir = match route_result {
                    Ok(Some(dir_connector)) => dir_connector,
                    Ok(None) => return None,
                    Err(router_error) => {
                        log::warn!(
                            "failed to route service capability for aggregate: {:?}",
                            router_error
                        );
                        return None;
                    }
                };

                let (source_dir_proxy, server_end) = create_proxy::<fio::DirectoryMarker>();
                source_dir.send(server_end, RelativePath::dot(), None).inspect_err(|e|  {
                    log::warn!(
                        "failed to open service capability for aggregate on dir connector {source_dir:?}: {:?}",
                        e
                    );
                }).ok()?;

                let entries = readdir(&source_dir_proxy).await.unwrap_or_default();
                Some((source_dir, entries))
            });
        }

        let aggregate_dictionary = Dictionary::new();

        while let Some(result) = routing_futures.next().await {
            let Some((source_dir, entries)) = result else { continue };

            // Renames have already been applied by the `with_service_renames_and_filter` function
            // in `source_dir`, so we don't need to do any remappings here.
            for entry in entries {
                if &entry.name == "." || entry.kind != fuchsia_fs::directory::DirentKind::Directory
                {
                    continue;
                }

                let path = match RelativePath::new(&entry.name) {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!(
                            "Ignoring invalid relative path from aggregate VFS: {}, error: {:?}",
                            entry.name,
                            e
                        );
                        continue;
                    }
                };

                let name = match entry.name.parse::<Name>() {
                    Ok(n) => n,
                    Err(e) => {
                        log::warn!(
                            "Ignoring invalid component name from aggregate VFS: {}, error: {:?}",
                            entry.name,
                            e
                        );
                        continue;
                    }
                };

                let sub_dir = source_dir.clone().with_subdir(path);

                aggregate_dictionary.insert(name.clone(), sub_dir.into()).inspect(|v| {
                    log::warn!(
                        "Name collision detected in aggregate dictionary for name: {name}. \
                        An untrusted component may be serving duplicates.: {v:?}"
                    );
                });
            }
        }

        Ok(DirConnector::from_directory_entry(
            aggregate_dictionary
                .try_into_directory_entry(self.scope.clone(), self.component.clone().into())
                .unwrap(),
            fio::PERM_READABLE,
        )
        .into())
    }
}

struct AnonymizedAggregateServiceProvider {
    component: WeakComponentInstance,
    sources: Vec<AggregateSource>,
    service_name: Name,
}

#[async_trait]
impl AnonymizedAggregateCapabilityProvider for AnonymizedAggregateServiceProvider {
    async fn list_instances(&self) -> Result<Vec<AggregateInstance>, RoutingError> {
        let mut instances = vec![];
        for source in &self.sources {
            match source {
                AggregateSource::DirectoryRouter { source_instance, router: _ } => {
                    instances.push(source_instance.clone())
                }
                AggregateSource::Collection { collection_name } => {
                    let children_in_collection = {
                        let component = self.component.upgrade()?;
                        let resolved_state =
                            component.lock_resolved_state().await.map_err(|e| {
                                ComponentInstanceError::ResolveFailed {
                                    moniker: self.component.moniker.clone(),
                                    err: anyhow::format_err!("{:?}", e).into(),
                                }
                            })?;
                        resolved_state.children_in_collection(collection_name)
                    };
                    for (child_name, child_instance) in children_in_collection {
                        // If we can't resolve the child, then it's not contributing to this
                        // service.
                        if let Ok(child_resolved_state) = child_instance.lock_resolved_state().await
                        {
                            if child_resolved_state
                                .sandbox
                                .component_output
                                .capabilities()
                                .get(&self.service_name)
                                .is_some()
                            {
                                instances.push(AggregateInstance::Child(child_name));
                            }
                        }
                    }
                }
            }
        }
        Ok(instances)
    }

    async fn route_instance(
        &self,
        instance: &AggregateInstance,
    ) -> Result<(Arc<Router<DirConnector>>, CapabilitySource), RoutingError> {
        let maybe_router = self
            .sources
            .iter()
            .filter_map(|source| match source {
                AggregateSource::DirectoryRouter { source_instance, router }
                    if instance == source_instance =>
                {
                    Some(router.clone())
                }
                _ => None,
            })
            .next();
        let router = match maybe_router {
            Some(router) => router,
            None => {
                // Both of the panics below should never be reached. For self, parent, and
                // statically declared children `build_component_sandbox` will create a source of
                // `AggregateSource::DirectoryRouter`. If we are asked to route an instance for any
                // of these and there is not a `DirectoryRouter`, then there's a bug in the sandbox
                // construction code.
                let child_name = match instance {
                    AggregateInstance::Child(name) => name,
                    AggregateInstance::Parent | AggregateInstance::Self_ => {
                        panic!(
                            "found a parent or self in a collection? this is impossible, and surely a bug"
                        );
                    }
                };
                if child_name.collection().is_none() {
                    panic!(
                        "found a static child in a collection? this is impossible, and surely a bug"
                    );
                }
                let child_instance = {
                    let component = self.component.upgrade()?;
                    let resolved_state = component.lock_resolved_state().await.map_err(|err| {
                        RoutingError::from(ComponentInstanceError::ResolveFailed {
                            moniker: self.component.moniker.clone(),
                            err: anyhow::format_err!("{:?}", err).into(),
                        })
                    })?;
                    resolved_state
                        .get_child(child_name)
                        .ok_or_else(|| {
                            RoutingError::from(ComponentInstanceError::InstanceNotFound {
                                moniker: self.component.moniker.child(child_name.clone()),
                            })
                        })?
                        .clone()
                };
                let child_resolved_state =
                    child_instance.lock_resolved_state().await.map_err(|err| {
                        RoutingError::from(ComponentInstanceError::ResolveFailed {
                            moniker: self.component.moniker.child(child_name.clone()),
                            err: anyhow::format_err!("{:?}", err).into(),
                        })
                    })?;
                let capability = child_resolved_state
                    .sandbox
                    .component_output
                    .capabilities()
                    .get(&self.service_name)
                    .ok_or_else(|| RoutingError::RouteSourceNotFound {
                        moniker: self.component.moniker.clone(),
                        verb: RouteVerb::IncludeInAggregate,
                        counter_verb: RouteVerb::Expose,
                        source: child_name.clone().into(),
                        capability_type: CapabilityTypeName::Service,
                        capability_name: vec![self.service_name.clone()].into(),
                    })?;
                match capability {
                    Capability::DirConnectorRouter(r) => r,
                    other_type => {
                        return Err(RoutingError::BedrockWrongCapabilityType {
                            actual: format!("{:?}", other_type),
                            expected: "Router<DirConnector>".to_string(),
                            moniker: self.component.moniker.clone().into(),
                        });
                    }
                }
            }
        };
        let source = router
            .route_debug(RouteRequest::default(), self.component.clone().into())
            .await
            .map_err(|e| {
                RoutingError::try_from(e).expect("failed to convert RouterError to RoutingError")
            })?;
        Ok((router, source))
    }
}
