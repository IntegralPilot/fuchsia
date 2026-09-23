// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_instance::ComponentInstanceForAnalyzer;
use crate::component_model::DynamicDictionaryConfig;
use ::routing::DictExt;
use ::routing::bedrock::aggregate_router::AggregateSource;
use ::routing::bedrock::program_output_dict;
use ::routing::bedrock::sandbox_construction::EventStreamSourceRouter;
use ::routing::bedrock::structured_dict::ComponentInput;
use ::routing::bedrock::with_policy_check::WithPolicyCheck;
use ::routing::component_instance::{
    WeakComponentInstanceInterface, WeakExtendedInstanceInterface,
};
use ::routing::error::{ComponentInstanceError, RouteRequestErrorInfo, RouteVerb, RoutingError};
use ::routing::policy::GlobalPolicyChecker;
use ::routing::rights::validate_rights;
use async_trait::async_trait;
use capability_source::{
    BuiltinSource, CapabilitySource, CapabilityToCapabilitySource, ComponentCapability,
    ComponentSource, FrameworkSource, InternalCapability, InternalEventStreamCapability,
    NamespaceSource,
};
use cm_config::RuntimeConfig;
use cm_rust::{
    CapabilityTypeName, ComponentDecl, ConfigSingleValue, ConfigValue, ConfigurationDecl,
    DeliveryType, DictionaryDecl, FidlIntoNative, ProtocolDecl,
};
use cm_types::{Name, Path, RelativePath};
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys;
use futures::stream::{FuturesUnordered, StreamExt};
use moniker::{ChildName, Moniker};
use router_error::RouterError;
use runtime_capabilities::{
    Capability, CapabilityBound, Connector, Data, Dictionary, DirConnector, Routable, Router,
    WeakInstanceToken,
};
use std::collections::HashMap;
use std::sync::Arc;

fn new_debug_only_specific_router<T>(source: CapabilitySource) -> Arc<Router<T>>
where
    T: CapabilityBound,
{
    Router::new(RootInputRouter { source, rights: None })
}

struct RootInputRouter {
    source: CapabilitySource,
    rights: Option<fio::Operations>,
}

#[async_trait]
impl<C: CapabilityBound> Routable<C> for RootInputRouter {
    async fn route(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<C>>, RouterError> {
        panic!("unexpected non-debug route");
    }

    async fn route_debug(
        &self,
        mut request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        if let Some(rights) = self.rights {
            validate_rights(self.source.source_moniker(), rights, &mut request)?;
        }
        Ok(self.source.clone())
    }
}

pub fn build_root_component_input(
    runtime_config: &Arc<RuntimeConfig>,
    policy: &GlobalPolicyChecker,
) -> ComponentInput {
    let root_component_input = ComponentInput::default();
    let names_and_capability_sources = runtime_config
        .namespace_capabilities
        .iter()
        .filter_map(|capability_decl| match capability_decl {
            cm_rust::CapabilityDecl::Protocol(_)
            | cm_rust::CapabilityDecl::Directory(_)
            | cm_rust::CapabilityDecl::Runner(_) => Some((
                capability_decl.name().clone(),
                CapabilitySource::Namespace(NamespaceSource {
                    capability: capability_decl.clone().into(),
                }),
                CapabilityTypeName::from(capability_decl),
                RouteRequestErrorInfo::from(capability_decl),
            )),
            _ => None,
        })
        .chain(runtime_config.builtin_capabilities.iter().filter_map(|capability_decl| {
            match capability_decl {
                cm_rust::CapabilityDecl::Protocol(_)
                | cm_rust::CapabilityDecl::Directory(_)
                | cm_rust::CapabilityDecl::Resolver(_)
                | cm_rust::CapabilityDecl::Runner(_) => Some((
                    capability_decl.name().clone(),
                    CapabilitySource::Builtin(BuiltinSource {
                        capability: capability_decl.clone().into(),
                    }),
                    CapabilityTypeName::from(capability_decl),
                    RouteRequestErrorInfo::from(capability_decl),
                )),
                _ => None,
            }
        }));
    for (name, capability_source, capability_type, _route_request_info) in
        names_and_capability_sources
    {
        let router_capability: Capability = match capability_type {
            CapabilityTypeName::Protocol
            | CapabilityTypeName::Runner
            | CapabilityTypeName::Resolver => {
                Router::<Connector>::new_debug(capability_source.clone())
                    .with_policy_check::<ComponentInstanceForAnalyzer>(
                        capability_source,
                        policy.clone(),
                    )
                    .into()
            }
            CapabilityTypeName::Directory => {
                let rights = match &capability_source {
                    CapabilitySource::Namespace(namespace_src) => match &namespace_src.capability {
                        ComponentCapability::Directory(decl) => decl.rights,
                        _ => panic!("unsupported component capability type"),
                    },
                    _ => panic!("unsupported capability source type"),
                };
                let router = Router::<DirConnector>::new(RootInputRouter {
                    source: capability_source.clone(),
                    rights: Some(rights),
                })
                .with_policy_check::<ComponentInstanceForAnalyzer>(
                    capability_source,
                    policy.clone(),
                );
                router.into()
            }
            _ => unreachable!("other types were filtered out above"),
        };
        root_component_input.capabilities().insert_capability(&name, router_capability.clone());
        if capability_type == CapabilityTypeName::Runner {
            root_component_input
                .environment()
                .runners()
                .insert_capability(&name, router_capability);
        } else if capability_type == CapabilityTypeName::Resolver {
            root_component_input
                .environment()
                .resolvers()
                .insert_capability(&name, router_capability);
        }
    }
    let event_stream_decls =
        runtime_config.builtin_capabilities.iter().filter_map(|capability_decl| {
            match capability_decl {
                cm_rust::CapabilityDecl::EventStream(es) => Some(es),
                _ => None,
            }
        });
    for event_stream_decl in event_stream_decls {
        let event_stream_name = event_stream_decl.name.clone();
        struct EventStreamRouter {
            event_stream_name: Name,
        }
        #[async_trait]
        impl Routable<Dictionary> for EventStreamRouter {
            async fn route(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Dictionary>>, RouterError> {
                panic!("non-debug routing not supported");
            }

            async fn route_debug(
                &self,
                request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                Ok(CapabilitySource::Builtin(BuiltinSource {
                    capability: InternalCapability::EventStream(InternalEventStreamCapability {
                        name: self.event_stream_name.clone(),
                        scope_moniker: request.event_stream_scope_moniker.clone(),
                        scope: request.event_stream_scope.map(FidlIntoNative::fidl_into_native),
                    }),
                }))
            }
        }
        let porcelain_router = Router::new(EventStreamRouter { event_stream_name });
        root_component_input.capabilities().insert_capability(
            &event_stream_decl.name,
            Capability::DictionaryRouter(porcelain_router),
        );
    }
    root_component_input
}

pub(crate) fn build_framework_router(
    scope: &Arc<ComponentInstanceForAnalyzer>,
) -> Arc<Router<Dictionary>> {
    Router::new(FrameworkRouter { scope: scope.moniker().clone() })
}

struct FrameworkRouter {
    scope: Moniker,
}

#[async_trait]
impl Routable<Dictionary> for FrameworkRouter {
    async fn route(
        &self,
        _request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Dictionary>>, RouterError> {
        let target = target
            .inner
            .as_any()
            .downcast_ref::<WeakExtendedInstanceInterface<ComponentInstanceForAnalyzer>>()
            .ok_or(RouterError::Unknown)?;
        let component = match target {
            WeakExtendedInstanceInterface::<ComponentInstanceForAnalyzer>::Component(c) => c,
            WeakExtendedInstanceInterface::<ComponentInstanceForAnalyzer>::AboveRoot(_) => {
                panic!("component manager doesn't have a framework dictionary");
            }
        };
        let component = component.upgrade().map_err(RoutingError::from)?;
        if *component.moniker() != self.scope {
            panic!("framework dictionary should not be routed");
        }

        let framework_dict = Dictionary::new();
        for protocol_name in &[
            fcomponent::BinderMarker::PROTOCOL_NAME,
            fsandbox::CapabilityStoreMarker::PROTOCOL_NAME,
            fcomponent::IntrospectorMarker::PROTOCOL_NAME,
            fcomponent::NamespaceMarker::PROTOCOL_NAME,
            fcomponent::RealmMarker::PROTOCOL_NAME,
            fruntime::CapabilitiesMarker::PROTOCOL_NAME,
            fsys::ConfigOverrideMarker::PROTOCOL_NAME,
            fsys::LifecycleControllerMarker::PROTOCOL_NAME,
            fsys::RealmQueryMarker::PROTOCOL_NAME,
            fsys::RouteValidatorMarker::PROTOCOL_NAME,
            "fuchsia.sys2.RealmExplorer",
        ] {
            let name = cm_types::Name::new(*protocol_name).unwrap();
            let router = new_debug_only_specific_router::<Connector>(CapabilitySource::Framework(
                FrameworkSource {
                    capability: InternalCapability::Protocol(name.clone()),
                    moniker: component.moniker().clone(),
                },
            ));
            framework_dict.insert_capability(&name, Capability::ConnectorRouter(router));
        }
        let pkg_name = cm_types::Name::new("pkg").unwrap();
        framework_dict.insert_capability(
            &pkg_name,
            Capability::DirConnectorRouter(new_debug_only_specific_router::<DirConnector>(
                CapabilitySource::Framework(FrameworkSource {
                    capability: InternalCapability::Directory(pkg_name.clone()),
                    moniker: component.moniker().clone(),
                }),
            )),
        );
        Ok(Some(framework_dict))
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        panic!("should never be debug routed");
    }
}

pub fn build_capability_sourced_capabilities_dictionary(
    component: &Arc<ComponentInstanceForAnalyzer>,
    decl: &cm_rust::ComponentDecl,
) -> Arc<Dictionary> {
    let output = Dictionary::new();
    for capability in &decl.capabilities {
        if let cm_rust::CapabilityDecl::Storage(storage_decl) = capability {
            let router = new_debug_only_specific_router::<Connector>(CapabilitySource::Capability(
                CapabilityToCapabilitySource {
                    source_capability: ComponentCapability::Storage(storage_decl.clone()),
                    moniker: component.moniker().clone(),
                },
            ));
            output.insert_capability(&storage_decl.name, Capability::ConnectorRouter(router));
        }
    }
    output
}

pub struct ProgramOutputGenerator {
    pub dynamic_dictionaries: Arc<DynamicDictionaryConfig>,
    pub executable: bool,
}

struct ProgramDictionaryRouter {
    dynamic_dictionaries: Arc<DynamicDictionaryConfig>,
    component: WeakComponentInstanceInterface<ComponentInstanceForAnalyzer>,
    capability: ComponentCapability,
}

#[async_trait]
impl Routable<Dictionary> for ProgramDictionaryRouter {
    async fn route(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Dictionary>>, RouterError> {
        let ComponentCapability::Dictionary(DictionaryDecl { name: requested_name, .. }) =
            &self.capability
        else {
            return Err(RouterError::NotFound(Arc::new(
                RoutingError::BedrockWrongCapabilityType {
                    actual: self.capability.type_name().to_string(),
                    expected: CapabilityTypeName::Dictionary.to_string(),
                    moniker: self.component.moniker.clone().into(),
                },
            )));
        };
        let Some(configs) = self.dynamic_dictionaries.get(&self.component.moniker) else {
            return Err(RouterError::NotFound(Arc::new(
                RoutingError::DynamicDictionariesNotAllowed {
                    moniker: self.component.moniker.clone().into(),
                },
            )));
        };
        let Some((_, capabilities)) = configs.iter().find(|(name, _)| *name == requested_name)
        else {
            return Err(RouterError::NotFound(Arc::new(
                RoutingError::DynamicDictionariesNotAllowed {
                    moniker: self.component.moniker.clone().into(),
                },
            )));
        };
        let dict = Dictionary::new();
        for (capability_type, capability_name) in capabilities {
            match capability_type {
                CapabilityTypeName::Protocol => {
                    let router = new_debug_only_specific_router::<Connector>(
                        CapabilitySource::Component(ComponentSource {
                            capability: ComponentCapability::from(ProtocolDecl {
                                name: capability_name.clone(),
                                source_path: None,
                                delivery: DeliveryType::Immediate,
                            }),
                            moniker: self.component.moniker.clone(),
                        }),
                    );
                    dict.insert_capability(&capability_name, Capability::ConnectorRouter(router));
                }
                CapabilityTypeName::Config => {
                    let router = new_debug_only_specific_router::<Data>(
                        CapabilitySource::Component(ComponentSource {
                            capability: ComponentCapability::from(ConfigurationDecl {
                                name: capability_name.clone(),
                                value: ConfigValue::Single(ConfigSingleValue::Bool(true)),
                            }),
                            moniker: self.component.moniker.clone(),
                        }),
                    );
                    dict.insert_capability(&capability_name, Capability::DataRouter(router));
                }
                _ => unreachable!(
                    "Only protocol and config capabilities are supported through scrutinity in dynamic dicts at the moment"
                ),
            }
        }
        Ok(Some(dict))
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        Ok(CapabilitySource::Component(ComponentSource {
            capability: self.capability.clone(),
            moniker: self.component.moniker.clone(),
        }))
    }
}

impl program_output_dict::ProgramOutputGenerator<ComponentInstanceForAnalyzer>
    for ProgramOutputGenerator
{
    fn new_program_dictionary_router(
        &self,
        component: WeakComponentInstanceInterface<ComponentInstanceForAnalyzer>,
        _relative_path: Path,
        capability: ComponentCapability,
    ) -> Arc<Router<Dictionary>> {
        if !self.executable {
            return Router::<Dictionary>::new_error(RoutingError::from(
                ComponentInstanceError::InstanceNotExecutable { moniker: component.moniker },
            ));
        }
        Router::new(ProgramDictionaryRouter {
            dynamic_dictionaries: self.dynamic_dictionaries.clone(),
            component,
            capability,
        })
    }

    fn new_outgoing_dir_connector_router(
        &self,
        component: &Arc<ComponentInstanceForAnalyzer>,
        _decl: &cm_rust::ComponentDecl,
        capability: &cm_rust::CapabilityDecl,
    ) -> Arc<Router<Connector>> {
        if !self.executable {
            return Router::<Connector>::new_error(RoutingError::from(
                ComponentInstanceError::InstanceNotExecutable {
                    moniker: component.moniker().clone(),
                },
            ));
        }
        new_debug_only_specific_router::<Connector>(CapabilitySource::Component(ComponentSource {
            capability: ComponentCapability::from(capability.clone()),
            moniker: component.moniker().clone(),
        }))
    }

    fn new_outgoing_dir_dir_connector_router(
        &self,
        component: &Arc<ComponentInstanceForAnalyzer>,
        _decl: &cm_rust::ComponentDecl,
        capability: &cm_rust::CapabilityDecl,
    ) -> Arc<Router<DirConnector>> {
        if !self.executable {
            return Router::<DirConnector>::new_error(RoutingError::from(
                ComponentInstanceError::InstanceNotExecutable {
                    moniker: component.moniker().clone(),
                },
            ));
        }
        let rights = match capability {
            cm_rust::CapabilityDecl::Directory(dir_decl) => dir_decl.rights,
            cm_rust::CapabilityDecl::Storage(_) => fio::RW_STAR_DIR,
            cm_rust::CapabilityDecl::Service(_) => fio::R_STAR_DIR,
            _ => panic!("incompatible porcelain type using DirConnector"),
        };

        Router::<DirConnector>::new(RootInputRouter {
            source: CapabilitySource::Component(ComponentSource {
                capability: ComponentCapability::from(capability.clone()),
                moniker: component.moniker().clone(),
            }),
            rights: Some(rights),
        })
    }
}

pub(crate) fn static_children_component_output_dictionary_routers(
    component: &Arc<ComponentInstanceForAnalyzer>,
    decl: &ComponentDecl,
) -> HashMap<ChildName, Arc<Router<Dictionary>>> {
    struct ChildrenComponentOutputRouters {
        weak_component: WeakComponentInstanceInterface<ComponentInstanceForAnalyzer>,
        child_name: ChildName,
    }
    #[async_trait]
    impl Routable<Dictionary> for ChildrenComponentOutputRouters {
        async fn route(
            &self,
            _request: RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<Option<Arc<Dictionary>>, RouterError> {
            let component =
                self.weak_component.upgrade().expect("part of component tree was dropped");
            let child = component.children.read().get(&self.child_name).cloned().ok_or(
                RoutingError::RouteSourceNotFound {
                    moniker: self.weak_component.moniker.clone(),
                    verb: RouteVerb::Offer,
                    counter_verb: RouteVerb::Expose,
                    source: self.child_name.clone().into(),
                    capability_type: CapabilityTypeName::Dictionary,
                    capability_name: RelativePath::new("component_output_dictionary").unwrap(),
                },
            )?;
            let component_output_dict = child.sandbox.lock().component_output.capabilities();
            Ok(Some(component_output_dict))
        }

        async fn route_debug(
            &self,
            _request: RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<CapabilitySource, RouterError> {
            panic!("this should never be debug routed");
        }
    }

    let weak_component = WeakComponentInstanceInterface::new(component);
    let mut output = HashMap::new();
    for child_decl in decl.children.iter() {
        let child_name = ChildName::new(child_decl.name.clone(), None);
        output.insert(
            child_name.clone(),
            Router::<Dictionary>::new(ChildrenComponentOutputRouters {
                weak_component: weak_component.clone(),
                child_name,
            }),
        );
    }
    output
}

pub fn new_aggregate_router(
    _: Arc<ComponentInstanceForAnalyzer>,
    _: Vec<AggregateSource>,
    capability_source: CapabilitySource,
) -> Arc<Router<DirConnector>> {
    new_debug_only_specific_router(capability_source)
}

pub fn new_event_stream_multiplexing_router(
    _: &Arc<ComponentInstanceForAnalyzer>,
    sources: Vec<EventStreamSourceRouter>,
) -> Arc<Router<Connector>> {
    struct EventStreamMultiplexingRouter {
        sources: Vec<EventStreamSourceRouter>,
    }
    #[async_trait]
    impl Routable<Connector> for EventStreamMultiplexingRouter {
        async fn route(
            &self,
            _request: RouteRequest,
            _target: Arc<WeakInstanceToken>,
        ) -> Result<Option<Arc<Connector>>, RouterError> {
            panic!("non-debug routing is unsupported");
        }

        async fn route_debug(
            &self,
            _request: RouteRequest,
            target: Arc<WeakInstanceToken>,
        ) -> Result<CapabilitySource, RouterError> {
            let mut routing_tasks = FuturesUnordered::new();
            for EventStreamSourceRouter { router, .. } in self.sources.iter() {
                routing_tasks.push(router.route_debug(RouteRequest::default(), target.clone()));
            }
            let mut any_result = None;
            while let Some(result) = routing_tasks.next().await {
                match result {
                    Ok(result) => any_result = Some(result),
                    Err(e) => return Err(e),
                }
            }
            Ok(any_result.expect("no result produced, is sources empty?"))
        }
    }
    Router::new(EventStreamMultiplexingRouter { sources })
}
