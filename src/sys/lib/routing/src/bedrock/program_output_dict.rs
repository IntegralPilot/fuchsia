// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bedrock::structured_dict::ComponentInput;
use crate::bedrock::with_policy_check::WithPolicyCheck;
use crate::component_instance::{
    ComponentInstanceInterface, ExtendedInstanceInterface, WeakComponentInstanceInterface,
    WeakExtendedInstanceInterface,
};
use crate::error::{RouteVerb, RoutingError};
use crate::intermediate_router::{IntermediateRouter, RouteRequest, WeakDictionaryOrRouter};
use crate::{DictExt, WeakInstanceTokenExt};
use async_trait::async_trait;
use capability_source::{CapabilitySource, ComponentCapability, ComponentSource};
use cm_rust::{CapabilityTypeName, NativeIntoFidl};
use cm_types::{Path, RelativePath};
use component_id_index::InstanceId;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_io as fio;
use moniker::{ChildName, ExtendedMoniker, Moniker};
use router_error::RouterError;
use runtime_capabilities::{
    Capability, Connector, Data, Dictionary, DirConnector, Routable, Router, WeakInstanceToken,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Arc;

pub trait ProgramOutputGenerator<C: ComponentInstanceInterface + 'static> {
    /// Get a router for [Dictionary] that forwards the request to a [Router] served at `path`
    /// in the program's outgoing directory.
    fn new_program_dictionary_router(
        &self,
        component: WeakComponentInstanceInterface<C>,
        path: Path,
        capability: ComponentCapability,
    ) -> Arc<Router<Dictionary>>;

    /// Get an outgoing directory router for `capability` that returns [Connector]. `capability`
    /// should be a type that maps to [Connector].
    fn new_outgoing_dir_connector_router(
        &self,
        component: &Arc<C>,
        decl: &cm_rust::ComponentDecl,
        capability: &cm_rust::CapabilityDecl,
    ) -> Arc<Router<Connector>>;

    /// Get an outgoing directory router for `capability` that returns [DirConnector]. `capability`
    /// should be a type that maps to [DirConnector].
    fn new_outgoing_dir_dir_connector_router(
        &self,
        component: &Arc<C>,
        decl: &cm_rust::ComponentDecl,
        capability: &cm_rust::CapabilityDecl,
    ) -> Arc<Router<DirConnector>>;
}

pub fn build_program_output_dictionary<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    decl: &cm_rust::ComponentDecl,
    component_input: &ComponentInput,
    child_outgoing_dictionary_routers: &HashMap<ChildName, Arc<Router<Dictionary>>>,
    router_gen: &impl ProgramOutputGenerator<C>,
) -> (Arc<Dictionary>, Arc<Dictionary>) {
    let program_output_dict = Dictionary::new();
    let declared_dictionaries = Dictionary::new();
    for capability in &decl.capabilities {
        extend_dict_with_capability(
            component,
            decl,
            capability,
            &program_output_dict,
            &declared_dictionaries,
            component_input,
            child_outgoing_dictionary_routers,
            router_gen,
        );
    }
    (program_output_dict, declared_dictionaries)
}

/// Adds `capability` to the program output dict given the resolved `decl`. The program output dict
/// is a dict of routers, keyed by capability name.
fn extend_dict_with_capability<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    decl: &cm_rust::ComponentDecl,
    capability: &cm_rust::CapabilityDecl,
    program_output_dict: &Arc<Dictionary>,
    declared_dictionaries: &Arc<Dictionary>,
    component_input: &ComponentInput,
    child_outgoing_dictionary_routers: &HashMap<ChildName, Arc<Router<Dictionary>>>,
    router_gen: &impl ProgramOutputGenerator<C>,
) {
    match capability {
        cm_rust::CapabilityDecl::Service(_) => {
            let router =
                router_gen.new_outgoing_dir_dir_connector_router(component, decl, capability);
            let router = router.with_policy_check::<C>(
                CapabilitySource::Component(ComponentSource {
                    capability: ComponentCapability::from(capability.clone()),
                    moniker: component.moniker().clone(),
                }),
                component.policy_checker().clone(),
            );
            let prev = program_output_dict
                .insert_capability(capability.name(), Capability::DirConnectorRouter(router));
            assert!(prev.is_none(), "failed to insert {}: preexisting value", capability.name());
        }
        cm_rust::CapabilityDecl::Directory(_) => {
            let router =
                router_gen.new_outgoing_dir_dir_connector_router(component, decl, capability);
            let router = router.with_policy_check::<C>(
                CapabilitySource::Component(ComponentSource {
                    capability: ComponentCapability::from(capability.clone()),
                    moniker: component.moniker().clone(),
                }),
                component.policy_checker().clone(),
            );
            let prev = program_output_dict
                .insert_capability(capability.name(), Capability::DirConnectorRouter(router));
            assert!(prev.is_none(), "failed to insert {}: preexisting value", capability.name());
        }
        cm_rust::CapabilityDecl::Storage(cm_rust::StorageDecl {
            name,
            source,
            backing_dir,
            subdir,
            storage_id,
        }) => {
            let backing_source: WeakDictionaryOrRouter = match source {
                cm_rust::StorageDirectorySource::Parent => {
                    Arc::downgrade(&component_input.capabilities()).into()
                }
                cm_rust::StorageDirectorySource::Self_ => {
                    Arc::downgrade(program_output_dict).into()
                }
                cm_rust::StorageDirectorySource::Child(child_name) => {
                    let child_name = ChildName::parse(child_name).expect("invalid child name");
                    let Some(child_component_output) =
                        child_outgoing_dictionary_routers.get(&child_name)
                    else {
                        panic!(
                            "use declaration in manifest for component {} has a source of a nonexistent child {}, this should be prevented by manifest validation",
                            component.moniker(),
                            child_name
                        );
                    };
                    Arc::downgrade(child_component_output).into()
                }
            };
            let router: Arc<Router<DirConnector>> = IntermediateRouter::new(
                backing_source,
                vec![backing_dir.clone()].into(),
                RouteRequest {
                    build_type_name: CapabilityTypeName::Directory,
                    availability: Some(cm_rust::Availability::Required),
                    directory_rights: Some(fio::PERM_READABLE | fio::PERM_WRITABLE),
                    sub_directory_path: Some(RelativePath::dot()),
                    inherit_rights: Some(false),
                    ..Default::default()
                },
                component.as_weak().into(),
                component.moniker().clone(),
                RouteVerb::Declare,
                source.clone().native_into_fidl(),
            )
            .try_into()
            .expect("wrong type from intermediate router");

            #[derive(Debug)]
            struct StorageBackingDirRouter<C: ComponentInstanceInterface + 'static> {
                subdir: RelativePath,
                storage_id: fdecl::StorageId,
                backing_dir_router: Arc<Router<DirConnector>>,
                storage_source_moniker: Moniker,
                backing_dir_target: Arc<WeakInstanceToken>,
                _component_type: PhantomData<C>,
            }

            impl<C: ComponentInstanceInterface + 'static> StorageBackingDirRouter<C> {
                fn prepare_route(
                    &self,
                    mut request: fruntime::RouteRequest,
                    target: Arc<WeakInstanceToken>,
                ) -> Result<fruntime::RouteRequest, RouterError> {
                    fn generate_moniker_based_storage_path(
                        subdir: Option<String>,
                        moniker: &Moniker,
                        instance_id: Option<&InstanceId>,
                    ) -> PathBuf {
                        let mut dir_path = vec![];
                        if let Some(subdir) = subdir {
                            dir_path.push(subdir);
                        }

                        if let Some(id) = instance_id {
                            dir_path.push(id.to_string());
                            return dir_path.into_iter().collect();
                        }

                        let path = moniker.path();
                        let mut path = path.iter();
                        if let Some(p) = path.next() {
                            dir_path.push(format!("{p}:0"));
                        }
                        while let Some(p) = path.next() {
                            dir_path.push("children".to_string());
                            dir_path.push(format!("{p}:0"));
                        }

                        // Storage capabilities used to have a hardcoded set of types, which would be appended
                        // here. To maintain compatibility with the old paths (and thus not lose data when this was
                        // migrated) we append "data" here. This works because this is the only type of storage
                        // that was actually used in the wild.
                        //
                        // This is only temporary, until the storage instance id migration changes this layout.
                        dir_path.push("data".to_string());
                        dir_path.into_iter().collect()
                    }
                    let StorageBackingDirRouter {
                        subdir,
                        storage_id,
                        backing_dir_router: _,
                        storage_source_moniker,
                        backing_dir_target: _,
                        _component_type: _,
                    } = self;
                    let instance: ExtendedInstanceInterface<C> = target.upgrade().unwrap();
                    let instance = match instance {
                        ExtendedInstanceInterface::Component(c) => c,
                        ExtendedInstanceInterface::AboveRoot(_) => {
                            panic!("unexpected component manager instance")
                        }
                    };
                    let index = instance.component_id_index();
                    let instance_id = index.id_for_moniker(instance.moniker());
                    match storage_id {
                        fdecl::StorageId::StaticInstanceId if instance_id.is_none() => {
                            return Err(RouterError::from(RoutingError::ComponentNotInIdIndex {
                                source_moniker: storage_source_moniker.clone(),
                                target_name: instance.moniker().leaf().map(Into::into),
                            }));
                        }
                        _ => (),
                    }
                    let moniker = match WeakInstanceTokenExt::<C>::moniker(&target) {
                        ExtendedMoniker::ComponentInstance(m) => m,
                        ExtendedMoniker::ComponentManager => {
                            panic!("component manager is the target of a storage capability")
                        }
                    };
                    let moniker = match moniker.strip_prefix(&storage_source_moniker) {
                        Ok(v) => v,
                        Err(_) => moniker,
                    };
                    let subdir_opt = if subdir.is_dot() { None } else { Some(subdir.to_string()) };
                    let isolated_storage_path =
                        generate_moniker_based_storage_path(subdir_opt, &moniker, instance_id);
                    request.isolated_storage_path =
                        Some(format!("{}", isolated_storage_path.display()));
                    request.build_type_name = Some(CapabilityTypeName::Directory.to_string());
                    request.directory_rights = Some(fio::PERM_READABLE | fio::PERM_WRITABLE);
                    request.inherit_rights = Some(false);
                    request.storage_sub_directory_path = Some(subdir.to_string());
                    request.storage_source_moniker = Some(storage_source_moniker.to_string());
                    Ok(request)
                }
            }

            #[async_trait]
            impl<C: ComponentInstanceInterface + 'static> Routable<DirConnector>
                for StorageBackingDirRouter<C>
            {
                async fn route(
                    &self,
                    request: fruntime::RouteRequest,
                    target: Arc<WeakInstanceToken>,
                ) -> Result<Option<Arc<DirConnector>>, RouterError> {
                    let request = self.prepare_route(request, target)?;
                    self.backing_dir_router.route(request, self.backing_dir_target.clone()).await
                }

                async fn route_debug(
                    &self,
                    request: fruntime::RouteRequest,
                    target: Arc<WeakInstanceToken>,
                ) -> Result<CapabilitySource, RouterError> {
                    let request = self.prepare_route(request, target)?;
                    self.backing_dir_router
                        .route_debug(request, self.backing_dir_target.clone())
                        .await
                }
            }

            let router = router.with_policy_check::<C>(
                CapabilitySource::Component(ComponentSource {
                    capability: ComponentCapability::from(capability.clone()),
                    moniker: component.moniker().clone(),
                }),
                component.policy_checker().clone(),
            );
            let router = Router::new(StorageBackingDirRouter::<C> {
                subdir: subdir.clone(),
                storage_id: storage_id.clone(),
                backing_dir_router: router,
                storage_source_moniker: component.moniker().clone(),
                backing_dir_target: Arc::new(WeakInstanceToken {
                    inner: Box::new(WeakExtendedInstanceInterface::Component(component.as_weak())),
                }),
                _component_type: Default::default(),
            });
            let prev =
                program_output_dict.insert_capability(name, Capability::DirConnectorRouter(router));
            assert!(prev.is_none(), "failed to insert {}: preexisting value", capability.name());
        }
        cm_rust::CapabilityDecl::Protocol(_)
        | cm_rust::CapabilityDecl::Runner(_)
        | cm_rust::CapabilityDecl::Resolver(_) => {
            let router = router_gen.new_outgoing_dir_connector_router(component, decl, capability);
            let router = router.with_policy_check::<C>(
                CapabilitySource::Component(ComponentSource {
                    capability: ComponentCapability::from(capability.clone()),
                    moniker: component.moniker().clone(),
                }),
                component.policy_checker().clone(),
            );
            let prev = program_output_dict
                .insert_capability(capability.name(), Capability::ConnectorRouter(router));
            assert!(prev.is_none(), "failed to insert {}: preexisting value", capability.name());
        }
        cm_rust::CapabilityDecl::Dictionary(d) => {
            extend_dict_with_dictionary(
                component,
                d,
                program_output_dict,
                declared_dictionaries,
                router_gen,
            );
        }
        cm_rust::CapabilityDecl::Config(c) => {
            let data = Arc::new(Data::Bytes(
                fidl::persist(&c.value.clone().native_into_fidl()).unwrap().into(),
            ));
            struct ConfigRouter {
                data: Arc<Data>,
                source: CapabilitySource,
            }
            #[async_trait]
            impl Routable<Data> for ConfigRouter {
                async fn route(
                    &self,
                    _request: fruntime::RouteRequest,
                    _target: Arc<WeakInstanceToken>,
                ) -> Result<Option<Arc<Data>>, RouterError> {
                    Ok(Some(self.data.clone()))
                }
                async fn route_debug(
                    &self,
                    _request: fruntime::RouteRequest,
                    _target: Arc<WeakInstanceToken>,
                ) -> Result<CapabilitySource, RouterError> {
                    Ok(self.source.clone())
                }
            }
            let source = CapabilitySource::Component(ComponentSource {
                capability: ComponentCapability::from(capability.clone()),
                moniker: component.moniker().clone(),
            });
            let router = Router::new(ConfigRouter { data, source: source.clone() });
            let router = router.with_policy_check::<C>(source, component.policy_checker().clone());
            let prev = program_output_dict
                .insert_capability(capability.name(), Capability::DataRouter(router));
            assert!(prev.is_none(), "failed to insert {}: preexisting value", capability.name());
        }
        cm_rust::CapabilityDecl::EventStream(_) => {
            // Capabilities not supported in bedrock program output dict yet.
            return;
        }
    }
}

fn extend_dict_with_dictionary<C: ComponentInstanceInterface + 'static>(
    component: &Arc<C>,
    decl: &cm_rust::DictionaryDecl,
    program_output_dict: &Arc<Dictionary>,
    declared_dictionaries: &Arc<Dictionary>,
    router_gen: &impl ProgramOutputGenerator<C>,
) {
    let router;
    let declared_dict;
    if let Some(source_path) = decl.source_path.as_ref() {
        // Dictionary backed by program's outgoing directory.
        router = router_gen.new_program_dictionary_router(
            component.as_weak(),
            source_path.clone(),
            ComponentCapability::Dictionary(decl.clone()),
        );
        declared_dict = None;
    } else {
        let dict = Dictionary::new();
        router = make_simple_dict_router(dict.clone(), component, decl);
        declared_dict = Some(dict);
    }
    if let Some(dict) = declared_dict {
        let prev = declared_dictionaries.insert_capability(&decl.name, dict.into());
        assert!(prev.is_none(), "failed to insert {}: preexisting value", decl.name);
    }
    let prev = program_output_dict.insert_capability(&decl.name, router.into());
    assert!(prev.is_none(), "failed to insert {}: preexisting value", decl.name);
}

/// Makes a router that always returns the given dictionary.
fn make_simple_dict_router<C: ComponentInstanceInterface + 'static>(
    dict: Arc<Dictionary>,
    component: &Arc<C>,
    decl: &cm_rust::DictionaryDecl,
) -> Arc<Router<Dictionary>> {
    struct DictRouter {
        dict: Arc<Dictionary>,
        source: CapabilitySource,
    }
    #[async_trait]
    impl Routable<Dictionary> for DictRouter {
        async fn route(
            &self,
            _request: fruntime::RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<Option<Arc<Dictionary>>, RouterError> {
            Ok(Some(self.dict.clone()))
        }

        async fn route_debug(
            &self,
            _request: fruntime::RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<CapabilitySource, RouterError> {
            Ok(self.source.clone())
        }
    }
    let source = CapabilitySource::Component(ComponentSource {
        capability: ComponentCapability::Dictionary(decl.clone()),
        moniker: component.moniker().clone(),
    });
    Router::<Dictionary>::new(DictRouter { dict, source })
}
