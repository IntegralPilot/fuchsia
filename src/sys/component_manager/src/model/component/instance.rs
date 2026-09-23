// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framework::capabilities::RemoteRouter;
use crate::framework::{controller, get_framework_router};
use crate::model::actions::StopAction;
use crate::model::component::{
    Component, ComponentInstance, ExtendedInstance, IncarnationId, Package, StartReason,
    WeakComponentInstance, WeakExtendedInstance,
};
use crate::model::context::ModelContext;
use crate::model::escrow;
use crate::model::events::hook_observer::HookObserver;
use crate::model::events::names_from_filter;
use crate::model::events::use_router::EventStreamUseRouter;
use crate::model::namespace::create_namespace;
use crate::model::program::{Program, StopConclusion, StopDisposition};
use crate::model::routing::aggregate_router::AggregateRouter;
use crate::model::routing::{RoutedStorage, RoutingFailureErrorReporter};
use crate::model::start::Start;
use crate::model::storage::build_storage_admin_dictionary;
use crate::model::token::{InstanceToken, InstanceTokenState};
use crate::sandbox_util::RoutableExt;
use ::routing::bedrock::program_output_dict::{
    ProgramOutputGenerator, build_program_output_dictionary,
};
use ::routing::bedrock::request_metadata::event_stream_metadata;
use ::routing::bedrock::sandbox_construction::{
    ComponentSandbox, build_component_sandbox, extend_dict_with_offers,
};
use ::routing::bedrock::structured_dict::{ComponentInput, StructuredDictMap};
use ::routing::component_instance::{
    ComponentInstanceInterface, ResolvedInstanceInterface, ResolvedInstanceInterfaceExt,
    WeakComponentInstanceInterface,
};
use ::routing::error::{ComponentInstanceError, RoutingError};
use ::routing::error_logging_router::ErrorLoggingRouter;
use ::routing::resolving::{ComponentAddress, ComponentResolutionContext, ResolverError};
use ::routing::rights::validate_rights;
use ::routing::subdir::SubDir;
use ::routing::{DictExt, WeakInstanceTokenExt};
use async_trait::async_trait;
use async_utils::async_once::Once;
use capability_source::{
    CapabilitySource, ComponentCapability, ComponentSource, StorageBackingDirectorySource,
};
use clonable_error::ClonableError;
use cm_fidl_validator::error::{DeclType, Error as ValidatorError};
use cm_graph::DependencyNode;
use cm_rust::offer::{OfferDecl, OfferDeclCommon};
use cm_rust::{
    CapabilityDecl, CapabilityTypeName, ChildDecl, CollectionDecl, ComponentDecl, DeliveryType,
    FidlIntoNative, NativeIntoFidl, UseDecl, UseProtocolDecl,
};
use cm_types::{AllowedOffers, Name, Path, RelativePath};
use config_encoder::ConfigFields;
use derivative::Derivative;
use directed_graph::DirectedGraph;
use errors::{
    AddChildError, AddDynamicChildError, CapabilityProviderError, ComponentProviderError,
    CreateNamespaceError, DynamicCapabilityError, OpenError, OpenOutgoingDirError,
    ResolveActionError, StopError,
};
use fidl::endpoints::{DiscoverableProtocolMarker, ServerEnd, create_proxy};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_internal as finternal;
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use flyweights::FlyStr;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use hooks::{CapabilityReceiver, EventPayload, EventType};
use log::warn;
use moniker::{BorrowedChildName, ChildName, ExtendedMoniker, Moniker};
use router_error::{Explain, RouterError};
use runtime_capabilities::{
    Capability, CapabilityBound, Connector, Data, Dictionary, DirConnector, Routable, Router,
    WeakInstanceToken,
};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::{fmt, mem};
use vfs::ToObjectRequest;
use vfs::directory::entry::{DirectoryEntry, OpenRequest, SubNode};
use vfs::directory::immutable::simple as pfs;
use vfs::execution_scope::ExecutionScope;
use zx;

static DICTIONARY_ROUTER_PATH: LazyLock<Path> = LazyLock::new(|| {
    Path::new(format!("/svc/{}", fsandbox::DictionaryRouterMarker::PROTOCOL_NAME))
        .expect("Dictionary router path is statically known to be valid")
});

/// The mutable state of a component instance.
pub enum InstanceState {
    /// The instance has not been resolved yet. This is the initial state.
    Unresolved(UnresolvedInstanceState),
    /// The instance has been resolved.
    Resolved(Box<ResolvedInstanceState>),
    /// The instance has started running.
    Started(Box<ResolvedInstanceState>, StartedInstanceState),
    /// The instance has been shutdown, and may not run anymore.
    Shutdown(ShutdownInstanceState, Box<UnresolvedInstanceState>),
    /// The instance has been destroyed. It has no content and no further actions may be registered
    /// on it.
    Destroyed,
}

impl InstanceState {
    pub fn replace<F>(&mut self, f: F)
    where
        F: FnOnce(InstanceState) -> InstanceState,
    {
        // We place InstanceState::Destroyed into self temporarily, so that the function can take
        // ownership of the current InstanceState and move values out of it.
        *self = f(std::mem::replace(self, InstanceState::Destroyed));
    }

    /// Changes the state, checking invariants.
    /// The allowed transitions:
    /// • Unresolved <-> Resolved -> Destroyed
    /// • {Unresolved, Resolved} -> Destroyed
    pub fn set(&mut self, next: Self) {
        match (&self, &next) {
            (Self::Unresolved(_), Self::Unresolved(_))
            | (Self::Resolved(_), Self::Resolved(_))
            | (Self::Destroyed, Self::Destroyed)
            | (Self::Destroyed, Self::Unresolved(_))
            | (Self::Destroyed, Self::Resolved(_)) => {
                panic!("Invalid instance state transition from {:?} to {:?}", self, next);
            }
            _ => {
                *self = next;
            }
        }
    }

    pub fn is_shut_down(&self) -> bool {
        match &self {
            InstanceState::Shutdown(_, _) | InstanceState::Destroyed => true,
            _ => false,
        }
    }

    pub fn is_started(&self) -> bool {
        self.get_started_state().is_some()
    }

    pub fn get_resolved_state(&self) -> Option<&ResolvedInstanceState> {
        match &self {
            InstanceState::Resolved(state) | InstanceState::Started(state, _) => Some(state),
            _ => None,
        }
    }

    pub fn get_resolved_state_mut(&mut self) -> Option<&mut ResolvedInstanceState> {
        match self {
            InstanceState::Resolved(state) | InstanceState::Started(state, _) => Some(state),
            _ => None,
        }
    }

    pub fn get_started_state(&self) -> Option<&StartedInstanceState> {
        match &self {
            InstanceState::Started(_, state) => Some(state),
            _ => None,
        }
    }

    pub fn get_started_state_mut(&mut self) -> Option<&mut StartedInstanceState> {
        match self {
            InstanceState::Started(_, state) => Some(state),
            _ => None,
        }
    }

    /// Requests a token that represents this component instance, minting it if needed.
    ///
    /// If the component instance is destroyed or not discovered, returns `None`.
    pub fn instance_token(
        &mut self,
        moniker: &Moniker,
        context: &Arc<ModelContext>,
    ) -> Option<InstanceToken> {
        match self {
            InstanceState::Unresolved(unresolved_state) => {
                Some(unresolved_state.instance_token(moniker, context))
            }
            InstanceState::Shutdown(_, boxed_unresolved) => {
                Some((**boxed_unresolved).instance_token(moniker, context))
            }
            InstanceState::Resolved(resolved) | InstanceState::Started(resolved, _) => {
                Some(resolved.instance_token(moniker, context))
            }
            InstanceState::Destroyed => None,
        }
    }

    /// Scope server_end to `StartedInstanceState`. This ensures that the channel will be kept
    /// alive as long as the component is running. If the component is not started when this method
    /// is called, this operation is a no-op and the channel will be dropped.
    pub fn scope_server_end(&mut self, server_end: zx::Channel) {
        if let Some(started_state) = self.get_started_state_mut() {
            started_state.add_scoped_server_end(server_end);
        }
    }
}

impl fmt::Debug for InstanceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Unresolved(_) => "Unresolved",
            Self::Resolved(_) => "Resolved",
            Self::Started(_, _) => "Started",
            Self::Shutdown(_, _) => "Shutdown",
            Self::Destroyed => "Destroyed",
        };
        f.write_str(s)
    }
}

pub struct ShutdownInstanceState {
    /// The children of this component, which is retained in case a destroy action is performed, as
    /// in that case the children will need to be destroyed as well.
    pub children: Box<CompactChildren>,

    /// Information about used storage capabilities the component had in its manifest. This is
    /// retained because the storage contents will be deleted if this component is destroyed.
    pub routed_storage: Vec<RoutedStorage>,
}

pub struct UnresolvedInstanceState {
    /// Caches an instance token.
    instance_token_state: InstanceTokenState,

    /// The dict containing all capabilities that the parent wished to provide to us.
    pub component_input: ComponentInput,
}

impl UnresolvedInstanceState {
    pub fn new(component_input: ComponentInput) -> Self {
        Self { instance_token_state: Default::default(), component_input }
    }

    fn instance_token(&mut self, moniker: &Moniker, context: &Arc<ModelContext>) -> InstanceToken {
        self.instance_token_state.set(moniker, context)
    }

    /// Returns relevant information and prepares to enter the resolved state.
    pub fn to_resolved(&mut self) -> (InstanceTokenState, ComponentInput) {
        (std::mem::take(&mut self.instance_token_state), self.component_input.clone())
    }

    /// Creates a new UnresolvedInstanceState by either cloning values from this struct or moving
    /// values from it (and replacing the values with their default values). This struct should be
    /// dropped after this function is called.
    pub fn take(&mut self) -> Self {
        Self {
            instance_token_state: std::mem::take(&mut self.instance_token_state),
            component_input: self.component_input.clone(),
        }
    }
}

// Thin wrapper for Children.
#[derive(Clone, Default)]
pub enum CompactChildren {
    #[default]
    None,
    Single(ChildName, Arc<ComponentInstance>),
    Multiple(HashMap<ChildName, Arc<ComponentInstance>>),
}

impl CompactChildren {
    pub fn insert(&mut self, name: ChildName, instance: Arc<ComponentInstance>) {
        match self {
            Self::None => *self = Self::Single(name, instance),
            Self::Single(old_name, old_instance) => {
                let mut map = HashMap::new();
                map.insert(old_name.clone(), old_instance.clone());
                map.insert(name, instance);
                *self = Self::Multiple(map);
            }
            Self::Multiple(map) => {
                map.insert(name, instance);
            }
        }
    }

    pub fn get(&self, m: &BorrowedChildName) -> Option<&Arc<ComponentInstance>> {
        match self {
            Self::None => None,
            Self::Single(k, v) => {
                if k == m {
                    Some(v)
                } else {
                    None
                }
            }
            Self::Multiple(map) => map.get(m),
        }
    }

    #[cfg(test)]
    pub fn get_by_name(&self, name: &str) -> Option<&Arc<ComponentInstance>> {
        match self {
            Self::None => None,
            Self::Single(k, v) => {
                if k.name() == &name {
                    Some(v)
                } else {
                    None
                }
            }
            Self::Multiple(map) => {
                map.values().find(|child| child.moniker.leaf().unwrap().name() == &name)
            }
        }
    }

    pub fn remove(&mut self, m: &BorrowedChildName) -> Option<Arc<ComponentInstance>> {
        match self {
            Self::None => None,
            Self::Single(k, _v) => {
                if k == m {
                    let old = std::mem::replace(self, Self::None);
                    match old {
                        Self::Single(_, v) => Some(v),
                        _ => unreachable!(),
                    }
                } else {
                    None
                }
            }
            Self::Multiple(map) => map.remove(m),
        }
    }

    pub fn iter(
        &self,
    ) -> Box<dyn Iterator<Item = (&ChildName, &Arc<ComponentInstance>)> + Send + '_> {
        match self {
            Self::None => Box::new(std::iter::empty()),
            Self::Single(k, v) => Box::new(std::iter::once((k, v))),
            Self::Multiple(map) => Box::new(map.iter()),
        }
    }
}

// impl<'a> IntoIterator for &'a CompactChildren {
//     type Item = (&'a ChildName, &'a Arc<ComponentInstance>);
//     type IntoIter = Box<dyn Iterator<Item = Self::Item> + Send + 'a>;

//     fn into_iter(self) -> Self::IntoIter {
//         self.iter()
//     }
// }

impl<'a> IntoIterator for &'a Box<CompactChildren> {
    type Item = (&'a ChildName, &'a Arc<ComponentInstance>);
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + Send + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        (**self).iter()
    }
}

/// The mutable state of a resolved component instance.
pub struct ResolvedInstanceState {
    /// Weak reference to the component that owns this state.
    weak_component: WeakComponentInstance,

    /// The component's execution scope, shared with [ComponentInstance::execution_scope].
    pub execution_scope: ExecutionScope,

    /// Caches an instance token.
    instance_token_state: InstanceTokenState,

    /// Result of resolving the component.
    pub resolved_component: Component,

    /// All child instances, indexed by child moniker.
    pub children: Box<CompactChildren>,

    /// The next unique identifier for a dynamic children created in this realm.
    /// (Static instances receive identifier 0.)
    next_dynamic_instance_id: IncarnationId,

    /// Directory that represents the program's namespace.
    ///
    /// This is only used for introspection, e.g. in RealmQuery. The program receives a
    /// namespace created in StartAction. The latter may have additional entries from
    /// [StartChildArgs].
    namespace_dir: Once<Arc<pfs::Simple>>,

    /// Holds a [Dictionary] mapping the component's exposed capabilities. Created on demand.
    exposed_dict: Once<Arc<Dictionary>>,

    /// Hosts a directory mapping the component's exposed capabilities, generated from `exposed_dict`.
    /// Created on demand.
    exposed_dir: Once<Arc<dyn DirectoryEntry>>,

    /// The as-resolved location of the component: either an absolute component
    /// URL, or (with a package context) a relative path URL.
    address: ComponentDomain,

    /// The sandbox holds all dictionaries involved in capability routing.
    pub sandbox: ComponentSandbox,

    /// State held by the framework on behalf of the component's program, including
    /// its outgoing directory server endpoint. Present if and only if the component
    /// has a program.
    program_escrow: Option<escrow::Actor>,

    /// When this component is resolved, we want to immediately register a hook to catch capability
    /// requested events. It's possible for capability requested events to trigger as soon as
    /// resolution is complete, and if we don't properly handle those then the handles will fall
    /// over to being delivered over the component's outgoing directory, which is incorrect if
    /// there's a capability requested event stream configured.
    ///
    /// This field holds the hook implementor that watches for those capability requested events,
    /// and the mpsc channel that buffers them. When the component starts and connects to its event
    /// stream, the mpsc receiver is taken and drained.
    ///
    /// The keys here are the capability names the hook observer is watching for, and are derived
    /// from the `filter` field in `UseEventStreamDecl`.
    pub capability_requested_receivers: HashMap<
        Vec<Name>,
        (Arc<HookObserver>, Arc<Mutex<mpsc::UnboundedReceiver<fcomponent::Event>>>),
    >,

    /// The namespace paths of storage capabilities routed to this component. We
    /// store them here as a memory optimization so that we can teardown storage
    /// component shutdown without needing to retain the complete component
    /// decl.
    pub storage_paths: Vec<Path>,

    /// The collection declarations of this component instance. We store them here as a memory
    /// optimization so that we do not need to retain the complete decl.
    pub collection_decls: Vec<CollectionDecl>,

    /// The declarations of storage and service capabilities used by this
    /// component. We store them here as a memory optimization so that we do not
    /// need to retain the complete decl.
    pub storage_service_use_decls: Vec<UseDecl>,

    /// The declarations of capabilities offered by this component instance.
    /// We store them here as a memory optimization so that we do not need to
    /// retain the complete decl.
    pub offer_decls: Vec<OfferDecl>,

    /// The declaration of the logger capability used by this component.
    pub logger_decl: Option<UseProtocolDecl>,
}

/// Abbreviated equivalent to [ComponentAddress] that omits the actual url string.
/// This is a memory optimization: we can reconstruct the full [ComponentAddress]
/// by combining this type with the string [`ComponentInstance::component_url`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentDomain {
    /// A fully-qualified component URL.
    Absolute,

    /// A relative Component URL, starting with the package path; for example a
    /// subpackage relative URL such as "needed_package#meta/dep_component.cm".
    RelativePath {
        /// An opaque value (from the perspective of component resolution)
        /// required by the resolver when resolving a relative package path.
        /// For a given child component, this property is populated from a
        /// parent component's `resolution_context`, as returned by the parent
        /// component's resolver.
        context: ComponentResolutionContext,

        scheme: FlyStr,
    },
}

impl From<ComponentAddress> for ComponentDomain {
    fn from(a: ComponentAddress) -> Self {
        match a {
            ComponentAddress::Absolute { .. } => ComponentDomain::Absolute,
            ComponentAddress::RelativePath { context, scheme, .. } => {
                ComponentDomain::RelativePath { context, scheme: scheme.into() }
            }
        }
    }
}

/// Extracts a mutable reference to the `target` field of an `OfferDecl`, or
/// `None` if the offer type is unknown.
fn offer_target_mut(offer: &mut fdecl::Offer) -> Option<&mut Option<fdecl::Ref>> {
    match offer {
        fdecl::Offer::Service(fdecl::OfferService { target, .. })
        | fdecl::Offer::Protocol(fdecl::OfferProtocol { target, .. })
        | fdecl::Offer::Directory(fdecl::OfferDirectory { target, .. })
        | fdecl::Offer::Storage(fdecl::OfferStorage { target, .. })
        | fdecl::Offer::Runner(fdecl::OfferRunner { target, .. })
        | fdecl::Offer::Config(fdecl::OfferConfiguration { target, .. })
        | fdecl::Offer::Resolver(fdecl::OfferResolver { target, .. }) => Some(target),
        fdecl::OfferUnknown!() => None,
    }
}

impl ResolvedInstanceState {
    pub async fn new(
        component: &Arc<ComponentInstance>,
        resolved_component: Component,
        address: ComponentDomain,
        instance_token_state: InstanceTokenState,
        component_input: ComponentInput,
    ) -> Result<Self, ResolveActionError> {
        let weak_component = WeakComponentInstance::new(component);

        let decl = &resolved_component
            .decl
            .as_ref()
            .expect("component decl dropped before component instantiated");
        // Perform the policy check for debug capabilities now, instead of during routing. All the info we
        // need to perform this check is already available to us. This way, we don't have to propagate this
        // info to sandbox capabilities just so they can enforce the policy.
        for env in &decl.environments {
            for cm_rust::DebugRegistration::Protocol(registration) in &env.debug_capabilities {
                component.policy_checker().can_register_debug_capability(
                    CapabilityTypeName::Protocol,
                    &registration.source_name,
                    &component.moniker,
                    &env.name,
                )?;
            }
        }

        let program_escrow = if decl.get_runner().is_some() {
            let escrow = escrow::Actor::new(
                component.moniker.clone(),
                &component.execution_scope,
                component.as_weak(),
            );
            Some(escrow)
        } else {
            None
        };

        let capability_requested_receivers =
            Self::initialize_capability_requested_hooks(&component, decl, &component_input).await;

        let storage_paths = decl
            .uses
            .iter()
            .filter_map(|use_| match use_ {
                UseDecl::Storage(storage_use) => Some(storage_use.target_path.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        let collection_decls =
            decl.collections.iter().map(|collection| collection.clone()).collect::<Vec<_>>();

        let storage_service_use_decls = decl
            .uses
            .iter()
            .filter_map(|use_| match use_ {
                UseDecl::Storage(_) | UseDecl::Service(_) => Some(use_.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();

        let offer_decls =
            match decl.collections.iter().find_map(|collection| match collection.allowed_offers {
                AllowedOffers::StaticAndDynamic => Some(()),
                AllowedOffers::StaticOnly => None,
            }) {
                Some(()) => decl.offers.iter().cloned().collect::<Vec<_>>(),
                None => vec![],
            };
        let logger_decl = decl.uses.iter().find_map(|use_| match use_ {
            cm_rust::UseDecl::Protocol(decl) => (decl.source_name
                == fidl_fuchsia_logger::LogSinkMarker::PROTOCOL_NAME)
                .then_some(decl.clone()),
            _ => None,
        });

        let mut state = Self {
            weak_component,
            execution_scope: component.execution_scope.clone(),
            instance_token_state,
            resolved_component,
            children: Box::new(CompactChildren::None),
            next_dynamic_instance_id: 1,
            namespace_dir: Once::default(),
            exposed_dict: Once::default(),
            exposed_dir: Once::default(),
            address,
            sandbox: Default::default(),
            program_escrow,
            capability_requested_receivers,
            storage_paths,
            collection_decls,
            storage_service_use_decls,
            offer_decls,
            logger_decl,
        };
        state.add_static_children(component).await?;

        struct MyGenerator {}
        impl ProgramOutputGenerator<ComponentInstance> for MyGenerator {
            fn new_program_dictionary_router(
                &self,
                component: WeakComponentInstanceInterface<ComponentInstance>,
                source_path: Path,
                capability: ComponentCapability,
            ) -> Arc<Router<Dictionary>> {
                Router::<Dictionary>::new(ProgramDictionaryRouter {
                    component,
                    source_path,
                    capability,
                })
            }

            fn new_outgoing_dir_connector_router(
                &self,
                component: &Arc<ComponentInstance>,
                decl: &cm_rust::ComponentDecl,
                capability: &cm_rust::CapabilityDecl,
            ) -> Arc<Router<Connector>> {
                ResolvedInstanceState::make_program_outgoing_connector_router(
                    component, decl, capability,
                )
            }

            fn new_outgoing_dir_dir_connector_router(
                &self,
                component: &Arc<ComponentInstance>,
                decl: &cm_rust::ComponentDecl,
                capability: &cm_rust::CapabilityDecl,
            ) -> Arc<Router<DirConnector>> {
                ResolvedInstanceState::make_program_outgoing_dir_connector_router(
                    component, decl, capability,
                )
            }
        }
        let child_outgoing_dictionary_routers =
            state.get_child_component_output_dictionary_routers();
        let decl = &state
            .resolved_component
            .decl
            .as_ref()
            .expect("component decl dropped before component instantiated");
        let (program_output_dict, declared_dictionaries) = build_program_output_dictionary(
            component,
            decl,
            &component_input,
            &child_outgoing_dictionary_routers,
            &MyGenerator {},
        );

        let component_sandbox = build_component_sandbox(
            &component,
            child_outgoing_dictionary_routers,
            decl,
            component_input,
            program_output_dict,
            get_framework_router(&component),
            build_storage_admin_dictionary(
                component,
                &state
                    .resolved_component
                    .decl
                    .as_ref()
                    .expect("component decl dropped before component instantiated"),
            ),
            declared_dictionaries,
            RoutingFailureErrorReporter::new(),
            &AggregateRouter::new,
            &EventStreamUseRouter::new,
        );
        Self::extend_program_input_namespace_with_injected_capabilities(
            &component,
            &component_sandbox.program_input.namespace(),
        )
        .await;

        state.sandbox = component_sandbox;
        state.populate_child_inputs(&state.sandbox.child_inputs).await;
        Ok(state)
    }

    async fn initialize_capability_requested_hooks(
        component: &Arc<ComponentInstance>,
        component_decl: &ComponentDecl,
        component_input: &ComponentInput,
    ) -> HashMap<
        Vec<Name>,
        (Arc<HookObserver>, Arc<Mutex<mpsc::UnboundedReceiver<fcomponent::Event>>>),
    > {
        let use_event_stream_decls = component_decl.uses.iter().filter_map(|use_| match use_ {
            cm_rust::UseDecl::EventStream(use_event_stream_decl) => Some(use_event_stream_decl),
            _ => None,
        });
        let mut capability_requested_receivers = HashMap::new();
        for use_event_stream_decl in use_event_stream_decls {
            let Some(Capability::DictionaryRouter(offered_router)) =
                component_input.capabilities().get(&use_event_stream_decl.source_name)
            else {
                continue;
            };
            let route_metadata = match &use_event_stream_decl.scope {
                Some(scope) => Some((component.moniker().clone(), scope.clone())),
                None => None,
            };
            let request = event_stream_metadata(use_event_stream_decl.availability, route_metadata);
            let Ok(Some(dictionary)) =
                offered_router.route(request, component.as_weak().into()).await
            else {
                continue;
            };
            let capability_name = match dictionary.get("event_stream_name") {
                Some(Capability::Data(data)) => match &data {
                    Data::String(name) => name.clone(),
                    other_value => {
                        panic!(
                            "missing or unexpected value for event_stream_name: {:?}",
                            other_value
                        )
                    }
                },
                other_value => {
                    panic!("missing or unexpected value for event_stream_name: {:?}", other_value)
                }
            };
            let event_type =
                EventType::try_from(capability_name.to_string()).expect("invalid event type");
            if event_type != EventType::CapabilityRequested {
                continue;
            }
            let cap = dictionary.get("event_stream_route_metadata").expect("missing metadata");
            let bytes = match cap {
                Capability::Data(data) => match &data {
                    Data::Bytes(bytes) => bytes.clone(),
                    _ => panic!("invalid event route metadata"),
                },
                _ => panic!("invalid event route metadata"),
            };
            let route_metadata: finternal::EventStreamRouteMetadata =
                fidl::unpersist(&bytes).expect("invalid event stream route metadata");
            let Some(names) = names_from_filter(&use_event_stream_decl.filter) else {
                continue;
            };
            let (sender, receiver) = futures::channel::mpsc::unbounded();
            let receiver = Arc::new(futures::lock::Mutex::new(receiver));

            let native_scope = route_metadata.scope.as_ref().map(|s| {
                let native_box: Box<[cm_rust::EventScope]> = s.clone().fidl_into_native();
                native_box.into_vec()
            });

            let parsed_scope_moniker = route_metadata
                .scope_moniker
                .as_ref()
                .map(|s| s.parse().expect("valid moniker"))
                .unwrap_or(ExtendedMoniker::ComponentManager);

            let capability_requested_hook = Arc::new(HookObserver {
                event_type: EventType::CapabilityRequested,
                subscriber: component.moniker.clone(),
                sender,
                weak_scope: component.execution_scope.as_weak(),
                filter: use_event_stream_decl.filter.clone(),
                native_scope,
                parsed_scope_moniker,
            });
            component.hooks.install(capability_requested_hook.hooks());
            capability_requested_receivers.insert(names, (capability_requested_hook, receiver));
        }
        capability_requested_receivers
    }

    /// Creates a `ConnectorRouter` that requests the specified capability from the
    /// program's outgoing directory.
    pub fn make_program_outgoing_connector_router(
        component: &Arc<ComponentInstance>,
        component_decl: &ComponentDecl,
        capability_decl: &cm_rust::CapabilityDecl,
    ) -> Arc<Router<Connector>> {
        if component_decl.get_runner().is_none() {
            return Router::<Connector>::new_error(OpenOutgoingDirError::InstanceNonExecutable);
        }
        let name = capability_decl.name();
        let path = capability_decl.path().expect("must have path").to_string();
        let path = fuchsia_fs::canonicalize_path(&path);
        let entry_type = ComponentCapability::from(capability_decl.clone()).type_name().into();
        let relative_path = vfs::path::Path::validate_and_split(path).unwrap();
        let outgoing_dir_entry = component.get_outgoing();
        #[derive(Derivative)]
        #[derivative(Debug)]
        struct OutgoingConnector {
            #[derivative(Debug = "ignore")]
            node: Arc<dyn DirectoryEntry>,
        }
        impl runtime_capabilities::Connectable for OutgoingConnector {
            fn send(&self, channel: zx::Channel) -> Result<(), ()> {
                let scope = ExecutionScope::new();
                const FLAGS: fio::Flags = fio::Flags::PROTOCOL_SERVICE;
                FLAGS.to_object_request(channel).handle(|object_request| {
                    let path = vfs::path::Path::dot();
                    self.node.clone().open_entry(OpenRequest::new(
                        scope,
                        FLAGS,
                        path,
                        object_request,
                    ))
                });
                Ok(())
            }
        }
        let node = Arc::new(SubNode::new(outgoing_dir_entry, relative_path, entry_type));
        let connector = runtime_capabilities::Connector::new_sendable(OutgoingConnector { node });
        let router = Router::new(CapabilityRequestedHook {
            source: component.as_weak(),
            name: name.clone(),
            connector,
            capability_decl: capability_decl.clone(),
        });
        match capability_decl {
            CapabilityDecl::Protocol(p) => match p.delivery {
                DeliveryType::Immediate => router,
                DeliveryType::OnReadable => router.on_readable(component.execution_scope.clone()),
            },
            _ => router,
        }
    }

    /// Creates a `Router<DirConnector>` that requests the specified capability from the
    /// program's outgoing directory.
    pub fn make_program_outgoing_dir_connector_router(
        component: &Arc<ComponentInstance>,
        component_decl: &ComponentDecl,
        capability_decl: &cm_rust::CapabilityDecl,
    ) -> Arc<Router<DirConnector>> {
        if component_decl.get_runner().is_none() {
            return Router::<DirConnector>::new_error(OpenOutgoingDirError::InstanceNonExecutable);
        }
        let rights = match capability_decl {
            cm_rust::CapabilityDecl::Directory(decl) => decl.rights,
            cm_rust::CapabilityDecl::Storage(_) => fio::RW_STAR_DIR,
            cm_rust::CapabilityDecl::Service(_) => fio::R_STAR_DIR,
            _ => panic!("unsupported capability type for DirConnector"),
        };
        let path = capability_decl.path().expect(
            "unable to construct dir connector router for capability type that doesn't \
                    have a source path",
        );
        let path = vfs::path::Path::validate_and_split(path.to_string()).unwrap();
        let capability_source = match capability_decl {
            cm_rust::CapabilityDecl::Directory(_) | cm_rust::CapabilityDecl::Storage(_) => {
                CapabilitySource::StorageBackingDirectory(StorageBackingDirectorySource {
                    capability: capability_decl.clone().into(),
                    moniker: component.moniker.clone(),
                    backing_dir_subdir: RelativePath::dot(),
                    storage_subdir: RelativePath::dot(),
                    storage_source_moniker: Moniker::root(),
                })
            }
            cm_rust::CapabilityDecl::Service(_) => CapabilitySource::Component(ComponentSource {
                capability: capability_decl.clone().into(),
                moniker: component.moniker.clone(),
            }),
            _ => panic!("unsupported capability type for DirConnector"),
        };
        Router::new(DirConnectorOutgoingRouter {
            source_component: component.as_weak(),
            capability_source,
            path,
            rights,
        })
    }

    /// Returns a reference to the component's validated declaration.
    pub fn decl(&self) -> Option<&ComponentDecl> {
        self.resolved_component.decl.as_ref().map(|d| d.as_ref())
    }

    /// Returns relevant information and prepares to enter the unresolved state.
    pub fn to_unresolved(&mut self) -> UnresolvedInstanceState {
        UnresolvedInstanceState {
            instance_token_state: std::mem::replace(
                &mut self.instance_token_state,
                Default::default(),
            ),
            component_input: self.sandbox.component_input.clone(),
        }
    }

    fn instance_token(&mut self, moniker: &Moniker, context: &Arc<ModelContext>) -> InstanceToken {
        self.instance_token_state.set(moniker, context)
    }

    pub fn program_escrow(&self) -> Option<&escrow::Actor> {
        self.program_escrow.as_ref()
    }

    pub async fn address_for_relative_url(
        &self,
        fragment: &str,
    ) -> Result<ComponentAddress, ::routing::resolving::ResolverError> {
        self.address().await?.consume_with_new_resource(fragment.strip_prefix("#"))
    }

    /// Returns an iterator over all children.
    pub fn children(&self) -> Box<dyn Iterator<Item = (&ChildName, &Arc<ComponentInstance>)> + '_> {
        match &*self.children {
            CompactChildren::None => Box::new(std::iter::empty()),
            CompactChildren::Single(k, v) => Box::new(std::iter::once((k, v))),
            CompactChildren::Multiple(map) => Box::new(map.iter()),
        }
    }

    /// Returns a reference to a child.
    pub fn get_child(&self, m: &BorrowedChildName) -> Option<&Arc<ComponentInstance>> {
        match &*self.children {
            CompactChildren::None => None,
            CompactChildren::Single(k, v) => {
                if k == m {
                    Some(v)
                } else {
                    None
                }
            }
            CompactChildren::Multiple(map) => map.get(m),
        }
    }

    /// Returns a vector of the children in `collection`.
    pub fn children_in_collection(
        &self,
        collection: &Name,
    ) -> Vec<(ChildName, Arc<ComponentInstance>)> {
        self.children()
            .filter(move |(m, _)| match m.collection() {
                Some(name) if name == collection => true,
                _ => false,
            })
            .map(|(m, c)| (m.clone(), Arc::clone(c)))
            .collect()
    }

    /// Returns a directory that represents the program's namespace at resolution time.
    ///
    /// This may not exactly match the namespace when the component is started since StartAction
    /// may add additional entries.
    pub async fn namespace_dir(&self) -> Result<Arc<pfs::Simple>, CreateNamespaceError> {
        let create_namespace_dir = async || {
            let component = self
                .weak_component
                .upgrade()
                .map_err(CreateNamespaceError::ComponentInstanceError)?;
            // Build a namespace and convert it to a directory.
            let namespace_builder = create_namespace(
                self.resolved_component.package.as_ref(),
                &component,
                &self.storage_service_use_decls,
                &self.sandbox.program_input.namespace(),
                component.execution_scope.clone(),
            )
            .await?;
            let namespace = namespace_builder.serve().map_err(|e| {
                CreateNamespaceError::BuildNamespaceError {
                    moniker: component.moniker.clone(),
                    err: e,
                }
            })?;
            let namespace_dir: Arc<pfs::Simple> =
                namespace.try_into().map_err(|err| CreateNamespaceError::ConvertToDirectory {
                    moniker: component.moniker.clone(),
                    err: ClonableError::from(anyhow::Error::from(err)),
                })?;
            Ok(namespace_dir)
        };

        Ok(self
            .namespace_dir
            .get_or_try_init::<_, CreateNamespaceError>(create_namespace_dir)
            .await?
            .clone())
    }

    async fn extend_program_input_namespace_with_injected_capabilities(
        component: &Arc<ComponentInstance>,
        out_dict: &Arc<Dictionary>,
    ) {
        let top_instance = component.top_instance().await;

        for entry in &component.context.runtime_config().inject_capabilities {
            // Skip this entry if it doesn't match the moniker of the component being created.
            if !entry.components.iter().any(|filter| filter.matches(&component.moniker)) {
                continue;
            }

            for use_ in &entry.use_ {
                let (capability, path) = match use_ {
                    cm_config::InjectedUse::Protocol(use_protocol) => {
                        let routable = top_instance
                            .as_ref()
                            .expect("Failed to get the top instance")
                            .get_root_exposed_capability_router(use_protocol.source_name.clone());
                        let router = Router::new(routable);
                        let capability = Capability::ConnectorRouter(router);
                        (capability, &use_protocol.target_path)
                    }
                };

                // Our injected capability takes precedence over the regular one, if present.
                if let Some(_) = out_dict.remove_capability(path) {
                    warn!("injected capability will shadow the one at {path}");
                }
                out_dict.insert_capability(path, capability);
            }
        }
    }

    /// Returns a [`Dictionary`] with contents similar to `component_output_dict`, but adds
    /// capabilities backed by legacy routing. This [`Dictionary`] is used to generate the
    /// `exposed_dir`.
    pub async fn get_exposed_dict(&self, self_target: Arc<WeakInstanceToken>) -> &Arc<Dictionary> {
        let create_exposed_dict = async || {
            let dict = Dictionary::new();
            for (key, value) in self.sandbox.component_output.capabilities().enumerate() {
                let error_info = value.error_info().expect("missing error info");
                let error_logging_router = ErrorLoggingRouter::new(
                    value,
                    error_info,
                    RoutingFailureErrorReporter::new(),
                    self_target.clone(),
                );
                let _ = dict.insert(key, error_logging_router);
            }
            dict
        };
        self.exposed_dict.get_or_init(create_exposed_dict).await
    }

    pub async fn get_exposed_dir(
        &self,
        self_target: Arc<WeakInstanceToken>,
    ) -> Arc<dyn DirectoryEntry> {
        let create_exposed_dir = async || {
            let exposed_dict = self.get_exposed_dict(self_target.clone()).await.clone();
            exposed_dict
                .try_into_directory_entry(self.execution_scope.clone(), self_target)
                .expect("converting exposed dict to open should always succeed")
        };
        self.exposed_dir.get_or_init(create_exposed_dir).await.clone()
    }

    /// Returns the resolved structured configuration of this instance, if any.
    pub fn config(&self) -> Option<&Arc<ConfigFields>> {
        self.resolved_component.config.as_ref()
    }

    /// Returns information about the package of the instance, if any.
    pub fn package(&self) -> Option<&Package> {
        self.resolved_component.package.as_ref()
    }

    /// Removes a child.
    pub fn remove_child(&mut self, moniker: &BorrowedChildName) {
        if self.children.remove(moniker).is_none() {
            return;
        }

        #[must_use]
        fn matches(o: &DependencyNode, moniker: &BorrowedChildName) -> bool {
            match o {
                DependencyNode::Child(name, coll) => {
                    name == moniker.name().as_ref()
                        && coll.as_ref().map(|s| s as &str)
                            == moniker.collection().map(|s| s.as_ref())
                }
                DependencyNode::Collection(name) => match moniker.collection() {
                    Some(n) => name == n.as_ref(),
                    None => false,
                },
                DependencyNode::Capability(_)
                | DependencyNode::Self_
                | DependencyNode::Environment(_) => false,
            }
        }

        // Delete any dynamic offers whose source or target matches the component we're deleting.
        Arc::make_mut(&mut self.resolved_component.dependencies)
            .retain(|a, b| !matches(a, moniker) && !matches(b, moniker));
        self.sandbox.child_outputs.lock().remove(moniker);
    }

    /// Adds a new child component instance.
    pub async fn add_child(
        &mut self,
        component: &Arc<ComponentInstance>,
        child: &ChildDecl,
        collection: Option<&CollectionDecl>,
        dynamic_offers: Option<Vec<fdecl::Offer>>,
        controller: Option<ServerEnd<fcomponent::ControllerMarker>>,
        input: ComponentInput,
    ) -> Result<Arc<ComponentInstance>, AddDynamicChildError> {
        let child =
            self.add_child_internal(component, child, collection, dynamic_offers, input).await?;

        if let Some(controller) = controller {
            let stream = controller.into_stream();
            child
                .execution_scope
                .spawn(controller::run_controller(WeakComponentInstance::new(&child), stream));
        }
        Ok(child)
    }

    pub fn insert_child(&mut self, name: ChildName, instance: Arc<ComponentInstance>) {
        let current_children = std::mem::replace(&mut *self.children, CompactChildren::None);

        let new_children = match current_children {
            CompactChildren::None => CompactChildren::Single(name, instance),
            CompactChildren::Single(old_name, old_instance) => {
                let mut map = HashMap::new();
                map.insert(old_name, old_instance);
                map.insert(name, instance);
                CompactChildren::Multiple(map)
            }
            CompactChildren::Multiple(mut map) => {
                map.insert(name, instance);
                CompactChildren::Multiple(map)
            }
        };

        *self.children = new_children;
    }

    async fn add_child_internal(
        &mut self,
        component: &Arc<ComponentInstance>,
        child: &ChildDecl,
        collection: Option<&CollectionDecl>,
        dynamic_offers: Option<Vec<fdecl::Offer>>,
        child_input: ComponentInput,
    ) -> Result<Arc<ComponentInstance>, AddChildError> {
        assert!(
            (dynamic_offers.is_none()) || collection.is_some(),
            "setting numbered handles or dynamic offers for static children",
        );
        let dynamic_offers =
            self.validate_and_convert_dynamic_component(dynamic_offers, child, collection)?;

        let child_name =
            ChildName::try_new(child.name.as_str(), collection.map(|c| c.name.as_str()))?;

        if !dynamic_offers.is_empty() {
            extend_dict_with_offers(
                &component,
                &self.sandbox,
                &self.offer_decls,
                &dynamic_offers,
                &child_input,
                &AggregateRouter::new,
            );
        }

        if self.get_child(&child_name).is_some() {
            return Err(AddChildError::InstanceAlreadyExists {
                moniker: component.moniker().clone(),
                child: child_name,
            });
        }
        let incarnation_id = match collection {
            Some(_) => {
                let id = self.next_dynamic_instance_id;
                self.next_dynamic_instance_id += 1;
                id
            }
            None => 0,
        };
        let child = ComponentInstance::new(
            child_input,
            component.moniker.child(child_name.clone()),
            incarnation_id,
            child.url.clone(),
            child.startup,
            child.on_terminate.unwrap_or(fdecl::OnTerminate::None),
            child.config_overrides.clone(),
            component.context.clone(),
            WeakExtendedInstance::Component(WeakComponentInstance::from(component)),
            component.hooks.clone(),
            component.persistent_storage_for_child(collection),
        )
        .await;
        self.insert_child(child_name.clone(), child.clone());
        self.sandbox.child_outputs.lock().insert(child_name, child.component_output());

        Arc::make_mut(&mut self.resolved_component.dependencies).extend(
            dynamic_offers.into_iter().map(NativeIntoFidl::native_into_fidl).filter_map(|o| {
                let (a, b) = cm_graph::get_dependency_from_offer(&o);
                let a = a?;
                let b = b?;
                Some((a, b))
            }),
        );
        Ok(child)
    }

    fn add_target_dynamic_offers(
        mut dynamic_offers: Vec<fdecl::Offer>,
        child: &ChildDecl,
        collection: &CollectionDecl,
    ) -> Result<Vec<fdecl::Offer>, DynamicCapabilityError> {
        for offer in &mut dynamic_offers {
            match offer {
                // TODO(https://fxbug.dev/436869061): We can support this once storage uses bedrock
                // routing
                fdecl::Offer::Storage(_) => {
                    return Err(DynamicCapabilityError::UnsupportedType { typename: "storage" });
                }
                // TODO(https://fxbug.dev/398830871): We can support this once event streams use
                // bedrock routing
                fdecl::Offer::EventStream(_) => {
                    return Err(DynamicCapabilityError::UnsupportedType {
                        typename: "event_stream",
                    });
                }
                fdecl::Offer::Service(fdecl::OfferService { target, .. })
                | fdecl::Offer::Protocol(fdecl::OfferProtocol { target, .. })
                | fdecl::Offer::Directory(fdecl::OfferDirectory { target, .. })
                | fdecl::Offer::Runner(fdecl::OfferRunner { target, .. })
                | fdecl::Offer::Resolver(fdecl::OfferResolver { target, .. })
                | fdecl::Offer::Config(fdecl::OfferConfiguration { target, .. })
                | fdecl::Offer::Dictionary(fdecl::OfferDictionary { target, .. }) => {
                    if target.is_some() {
                        return Err(DynamicCapabilityError::Invalid {
                            err: Box::new(cm_fidl_validator::error::ErrorList {
                                errs: vec![cm_fidl_validator::error::Error::extraneous_field(
                                    DeclType::Offer,
                                    "target",
                                )],
                            }),
                        });
                    }
                }
                fdecl::OfferUnknown!() => {
                    return Err(DynamicCapabilityError::UnknownOfferType);
                }
            }
            *offer_target_mut(offer).expect("validation should have found unknown enum type") =
                Some(fdecl::Ref::Child(fdecl::ChildRef {
                    name: child.name.clone().into(),
                    collection: Some(collection.name.clone().into()),
                }));
        }
        Ok(dynamic_offers)
    }

    fn validate_dynamic_component(
        &self,
        dependencies: &mut DirectedGraph<DependencyNode>,
        all_dynamic_children: Vec<(&str, &str)>,
        new_dynamic_offers: Vec<fdecl::Offer>,
    ) -> Result<(), AddChildError> {
        // Validate!
        cm_fidl_validator::validate_dynamic_offers(
            all_dynamic_children,
            dependencies,
            &new_dynamic_offers,
            &self.resolved_component.decl.as_ref().unwrap().as_ref().clone().native_into_fidl(),
        )
        .map_err(|err| {
            if err.errs.iter().all(|e| matches!(e, ValidatorError::DependencyCycle(_))) {
                DynamicCapabilityError::Cycle { err: Box::new(err) }
            } else {
                DynamicCapabilityError::Invalid { err: Box::new(err) }
            }
        })?;

        // Manifest validation is not informed of the contents of collections, and is thus unable
        // to confirm the source exists if it's in a collection. Let's check that here.
        let dynamic_offers: Vec<OfferDecl> =
            new_dynamic_offers.into_iter().map(FidlIntoNative::fidl_into_native).collect();
        for offer in &dynamic_offers {
            if !self
                .try_offer_source_exists(offer.source())
                .expect("component decl already dropped on instantiation code path")
            {
                return Err(DynamicCapabilityError::SourceNotFound { offer: offer.clone() }.into());
            }
        }
        Ok(())
    }

    pub fn validate_and_convert_dynamic_component(
        &mut self,
        dynamic_offers: Option<Vec<fdecl::Offer>>,
        child: &ChildDecl,
        collection: Option<&CollectionDecl>,
    ) -> Result<Vec<OfferDecl>, AddChildError> {
        if collection.is_none() {
            return Ok(vec![]);
        }

        let collection = collection.unwrap();
        let dynamic_offers =
            Self::add_target_dynamic_offers(dynamic_offers.unwrap_or_default(), child, collection)?;
        // TODO: This shifting around of the DirectedGraph is here to work around a lifetime
        // error, but it's slightly awkward. See if there's a better way -- can we decouple
        // `validate_dynamic_component` from having to depend on `self`?
        let mut dependencies =
            mem::replace(&mut self.resolved_component.dependencies, Arc::new(DirectedGraph::new()));

        let mut all_dynamic_children: Vec<_> = self
            .children()
            .filter_map(|(n, _)| {
                if let Some(collection) = n.collection() {
                    Some((n.name().as_str(), collection.as_str()))
                } else {
                    None
                }
            })
            .collect();
        all_dynamic_children.push((child.name.as_str(), collection.name.as_str()));
        self.validate_dynamic_component(
            Arc::make_mut(&mut dependencies),
            all_dynamic_children,
            dynamic_offers.clone(),
        )?;

        _ = mem::replace(&mut self.resolved_component.dependencies, dependencies);
        Ok(dynamic_offers.into_iter().map(|o| o.fidl_into_native()).collect())
    }

    async fn add_static_children(
        &mut self,
        component: &Arc<ComponentInstance>,
    ) -> Result<(), ResolveActionError> {
        // We can't hold an immutable reference to `self` while passing a mutable reference later
        // on. To get around this, clone the children.
        let children = self
            .resolved_component
            .decl
            .as_ref()
            .expect("component decl dropped before component instantiated")
            .children
            .clone();
        for child in &children {
            // `child_input` will be populated later, after the component's sandbox is
            // constructed.
            self.add_child_internal(component, child, None, None, ComponentInput::default())
                .await
                .map_err(|err| ResolveActionError::AddStaticChildError {
                    child_name: child.name.to_string(),
                    err,
                })?;
        }
        Ok(())
    }

    async fn populate_child_inputs(&self, child_inputs: &StructuredDictMap<ComponentInput>) {
        for (child_name, child_instance) in &self.children {
            if let Some(_) = child_name.collection() {
                continue;
            }
            let child_name =
                Name::new(child_name.name().as_str()).expect("child is static so name is not long");
            let child_input = child_inputs.get(&child_name).expect("missing child dict");
            let mut state = child_instance.lock_state().await;
            let InstanceState::Unresolved(state) = &mut *state else {
                unreachable!("still building sandbox, the child can't be resolved yet");
            };
            let _ = std::mem::replace(&mut state.component_input, child_input);
        }
    }

    fn get_child_component_output_dictionary_routers(
        &self,
    ) -> HashMap<ChildName, Arc<Router<Dictionary>>> {
        self.children.iter().map(|(name, child)| (name.clone(), child.component_output())).collect()
    }
}

#[async_trait]
impl ResolvedInstanceInterface for ResolvedInstanceState {
    type Component = ComponentInstance;

    fn try_uses(&self) -> Option<Box<[UseDecl]>> {
        self.resolved_component.decl.as_ref().map(|d| d.uses.clone())
    }

    fn try_exposes(&self) -> Option<Box<[cm_rust::ExposeDecl]>> {
        self.resolved_component.decl.as_ref().map(|d| d.exposes.clone())
    }

    fn try_offers(&self) -> Option<Box<[OfferDecl]>> {
        self.resolved_component.decl.as_ref().map(|d| d.offers.clone())
    }

    fn try_capabilities(&self) -> Option<Box<[cm_rust::CapabilityDecl]>> {
        self.resolved_component.decl.as_ref().map(|d| d.capabilities.clone())
    }

    fn try_collections(&self) -> Option<Box<[cm_rust::CollectionDecl]>> {
        self.resolved_component.decl.as_ref().map(|d| d.collections.clone())
    }

    fn get_child(&self, moniker: &BorrowedChildName) -> Option<Arc<ComponentInstance>> {
        ResolvedInstanceState::get_child(self, moniker).map(Arc::clone)
    }

    fn children_in_collection(
        &self,
        collection: &Name,
    ) -> Vec<(ChildName, Arc<ComponentInstance>)> {
        ResolvedInstanceState::children_in_collection(self, collection)
    }

    async fn address(&self) -> Result<ComponentAddress, ResolverError> {
        let component = self.weak_component.upgrade()?;
        match &self.address {
            ComponentDomain::Absolute => {
                ComponentAddress::from_url(&component.component_url, &component).await
            }
            ComponentDomain::RelativePath { context, scheme: _ } => {
                ComponentAddress::from_url_and_context(
                    &component.component_url,
                    context.clone(),
                    &component,
                )
                .await
            }
        }
    }

    fn context_to_resolve_children(&self) -> Option<ComponentResolutionContext> {
        self.resolved_component.context_to_resolve_children.clone()
    }
}

/// The execution state for a program instance that is running.
struct ProgramRuntime {
    /// Used to interact with the Runner to influence the program's execution.
    program: Program,

    /// Listens for the controller channel to close in the background. This task is cancelled when
    /// the [`ProgramRuntime`] is dropped.
    exit_listener: fasync::Task<()>,
}

impl ProgramRuntime {
    pub fn new(program: Program, component: WeakComponentInstance) -> Self {
        let terminated_fut = program.on_terminate();
        let exit_listener = fasync::Task::spawn(async move {
            terminated_fut.await;
            if let Ok(component) = component.upgrade() {
                let stop_nf = component.actions().register_no_wait(StopAction::new(false)).await;
                component.execution_scope.spawn(async move {
                    let _ = stop_nf.await.map_err(
                        |err| warn!(err:%; "Watching for program termination: Stop failed"),
                    );
                });
            }
        });
        Self { program, exit_listener }
    }

    pub async fn stop<'a, 'b>(
        self,
        stop_timer: BoxFuture<'a, ()>,
        kill_timer: BoxFuture<'b, ()>,
    ) -> Result<StopConclusion, StopError> {
        // Drop the program and join on the exit listener. Dropping the program
        // should cause the exit listener to stop waiting for the channel epitaph and
        // exit.
        //
        // Note: this is more reliable than just cancelling `exit_listener` because
        // even after cancellation future may still run for a short period of time
        // before getting dropped. If that happens there is a chance of scheduling a
        // duplicate Stop action.
        let res = self.program.stop_or_kill_with_timeout(stop_timer, kill_timer).await;
        self.exit_listener.await;
        res
    }
}

/// The execution state for a component instance that has started running.
///
/// If the component instance has a program, it may also have a [`ProgramRuntime`].
pub struct StartedInstanceState {
    /// If set, that means this component is associated with a running program.
    program: Option<ProgramRuntime>,

    /// Approximates when the component was started in nanoseconds since boot.
    pub timestamp: zx::BootInstant,

    /// Approximates when the component was started in monotonic time. This time doesn't measure
    /// the time since boot and won't include time the system spent suspended.
    pub timestamp_monotonic: zx::MonotonicInstant,

    /// Describes why the component instance was started
    pub start_reason: StartReason,

    /// Channels scoped to lifetime of this component's execution context. This
    /// should only be used for the server_end of the `fuchsia.component.Binder`
    /// connection.
    binder_server_ends: Vec<zx::Channel>,

    /// This stores the hook for notifying an ExecutionController about stop events for this
    /// component.
    execution_controller_task: Option<controller::ExecutionControllerTask>,
}

impl StartedInstanceState {
    /// Creates the state corresponding to a started component.
    ///
    /// If `program` is present, also creates a background task waiting for the program to
    /// terminate. When that happens, uses the [`WeakComponentInstance`] to stop the component.
    pub fn new(
        program: Option<Program>,
        component: WeakComponentInstance,
        start_reason: StartReason,
        execution_controller_task: Option<controller::ExecutionControllerTask>,
    ) -> Self {
        let timestamp = zx::BootInstant::get();
        let timestamp_monotonic = zx::MonotonicInstant::get();
        StartedInstanceState {
            program: program.map(|p| ProgramRuntime::new(p, component)),
            timestamp,
            timestamp_monotonic,
            binder_server_ends: vec![],
            start_reason,
            execution_controller_task,
        }
    }

    /// If this component has a program, obtain a capability representing its runtime directory.
    pub fn runtime_dir(&self) -> Option<&fio::DirectoryProxy> {
        self.program.as_ref().map(|program_runtime| program_runtime.program.runtime())
    }

    /// Stops the component. If the component has a program, the timer defines how long
    /// the runner is given to stop the program gracefully before we request the controller
    /// to terminate the program.
    pub async fn stop<'a, 'b>(
        mut self,
        stop_timer: BoxFuture<'a, ()>,
        kill_timer: BoxFuture<'b, ()>,
    ) -> Result<StopConclusion, StopError> {
        let program = self.program.take();
        // If the component has a program, also stop the program.
        let ret = if let Some(program) = program {
            program.stop(stop_timer, kill_timer).await
        } else {
            Ok(StopConclusion { disposition: StopDisposition::NoController, escrow_request: None })
        }?;
        if let Some(execution_controller_task) = self.execution_controller_task.as_mut() {
            execution_controller_task.set_stop_payload(ret.disposition.stop_info());
        }
        Ok(ret)
    }

    /// Add a channel scoped to the lifetime of this object.
    pub fn add_scoped_server_end(&mut self, server_end: zx::Channel) {
        self.binder_server_ends.push(server_end);
    }

    /// Gets a [`Koid`] that will uniquely identify the program.
    #[cfg(test)]
    pub fn program_koid(&self) -> Option<zx::Koid> {
        self.program.as_ref().map(|program_runtime| program_runtime.program.koid())
    }
}

/// This delegates to the event system if the capability request is
/// intercepted by some hook, and delegates to the current capability otherwise.
#[derive(Debug)]
struct CapabilityRequestedHook {
    source: WeakComponentInstance,
    name: Name,
    connector: Arc<Connector>,
    capability_decl: cm_rust::CapabilityDecl,
}

#[async_trait]

impl Routable<Connector> for CapabilityRequestedHook {
    async fn route(
        &self,
        _request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Connector>>, RouterError> {
        fn cm_unexpected() -> RouterError {
            RoutingError::from(ComponentInstanceError::ComponentManagerInstanceUnexpected {}).into()
        }

        let weak_extended_component: WeakExtendedInstance =
            target.clone().try_into().expect("invalid token");
        let ExtendedMoniker::ComponentInstance(target_moniker) =
            weak_extended_component.extended_moniker()
        else {
            return Err(cm_unexpected());
        };
        self.source
            .ensure_started(&StartReason::AccessCapability {
                target: target_moniker,
                name: self.name.clone(),
            })
            .await
            .map_err(|e| {
                RoutingError::from(ComponentInstanceError::StartFailed {
                    moniker: Moniker::root(),
                    err_msg: format!("{}", e),
                    err_as_zx: e.as_zx_status(),
                })
            })?;
        let source = self.source.upgrade().map_err(RoutingError::from)?;
        let ExtendedInstance::Component(target) =
            target.clone().upgrade().map_err(RoutingError::from)?
        else {
            return Err(cm_unexpected());
        };
        let (receiver, sender) = CapabilityReceiver::new();
        let event = target.new_event(EventPayload::CapabilityRequested {
            source_moniker: source.moniker.clone(),
            name: self.name.to_string(),
            receiver: receiver.clone(),
        });
        source.hooks.dispatch(&event).await;
        let resp = if receiver.is_taken() { sender } else { self.connector.clone() };
        Ok(Some(resp))
    }

    async fn route_debug(
        &self,
        _request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        Ok(CapabilitySource::Component(ComponentSource {
            capability: self.capability_decl.clone().into(),
            moniker: self.source.moniker.clone(),
        }))
    }
}

#[derive(Debug)]
struct DirConnectorOutgoingRouter {
    source_component: WeakComponentInstance,
    capability_source: CapabilitySource,
    path: vfs::path::Path,
    rights: fio::Operations,
}

#[async_trait]
impl Routable<DirConnector> for DirConnectorOutgoingRouter {
    async fn route(
        &self,
        mut request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<DirConnector>>, RouterError> {
        validate_rights(self.source_component.moniker.clone().into(), self.rights, &mut request)
            .map_err(|e| {
                log::warn!("validate_rights failed on request {request:?}");
                e
            })?;

        let subdir = request
            .sub_directory_path
            .map(|s| RelativePath::new(s).expect("invalid path"))
            .unwrap_or_else(|| RelativePath::dot());
        let subdir = vfs::path::Path::validate_and_split(format!("{}", subdir)).map_err(|e| {
            RoutingError::RouteRequestFailedToParseField {
                moniker: self.source_component.moniker.clone().into(),
                field: "sub_directory_path".to_string(),
                parse_error: format!("{e:?}"),
            }
        })?;
        let path = subdir.with_prefix(&self.path);
        let flags =
            request.directory_rights.ok_or_else(|| RoutingError::RouteRequestMissingField {
                moniker: self.source_component.moniker.clone().into(),
                missing_field: "directory_rights".to_string(),
            })?;
        let source_component = self.source_component.upgrade().map_err(RoutingError::from)?;
        let path = if let Some(isolated_storage_path) = request.isolated_storage_path {
            let isolated_storage_path =
                vfs::path::Path::validate_and_split(isolated_storage_path).unwrap();
            source_component.ensure_started(&StartReason::StorageAdmin).await.map_err(|err| {
                RoutingError::from(ComponentInstanceError::StartFailed {
                    moniker: source_component.moniker.clone(),
                    err_msg: format!("{err}"),
                    err_as_zx: err.as_zx_status(),
                })
            })?;
            let (proxy, server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            let flags = fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY;
            let mut obj_request = vfs::object_request::ObjectRequest::new(
                flags,
                &fio::Options::default(),
                server.into(),
            );
            let open_request = vfs::directory::entry::OpenRequest::new(
                source_component.execution_scope.clone(),
                flags,
                path.clone(),
                &mut obj_request,
            );
            source_component.get_outgoing().clone().open_entry(open_request).unwrap();
            let _ = fuchsia_fs::directory::create_directory_recursive(
                &proxy,
                isolated_storage_path.as_str(),
                flags,
            )
            .await
            .map_err(|err| {
                RoutingError::from(ComponentInstanceError::FailedToCreateStorage {
                    moniker: source_component.moniker.clone(),
                    err_msg: format!("{err}"),
                })
            })?;
            isolated_storage_path.with_prefix(&path)
        } else {
            path
        };

        struct OutgoingDirConnector {
            outgoing_dir: Arc<dyn DirectoryEntry>,
            scope: ExecutionScope,
            moniker: Moniker,
            path: vfs::path::Path,
            flags: fio::Flags,
        }
        impl fmt::Debug for OutgoingDirConnector {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&format!(
                    "OutgoingDirConnector {{ moniker: {}, path: {}, flags: {:?} }}",
                    self.moniker,
                    self.path.as_ref(),
                    self.flags
                ))
            }
        }
        impl runtime_capabilities::DirConnectable for OutgoingDirConnector {
            fn maximum_flags(&self) -> fio::Flags {
                self.flags
            }

            fn send(
                &self,
                dir: ServerEnd<fio::DirectoryMarker>,
                subdir: RelativePath,
                flags: Option<fio::Flags>,
            ) -> Result<(), ()> {
                let flags = flags.unwrap_or(self.flags | fio::Flags::PROTOCOL_DIRECTORY);
                let subdir = vfs::path::Path::validate_and_split(subdir).unwrap();
                let path = subdir.with_prefix(&self.path);
                let mut obj_request = vfs::object_request::ObjectRequest::new(
                    flags,
                    &fio::Options::default(),
                    dir.into(),
                );
                let open_request = vfs::directory::entry::OpenRequest::new(
                    self.scope.clone(),
                    flags,
                    path,
                    &mut obj_request,
                );
                // Nothing we can do about an error here. If the source component closed their
                // outgoing directory unexpectedly, then this handle is getting dropped.
                let _ = self.outgoing_dir.clone().open_entry(open_request);
                Ok(())
            }
        }
        let dir_connector =
            runtime_capabilities::DirConnector::new_sendable(OutgoingDirConnector {
                outgoing_dir: source_component.get_outgoing(),
                scope: source_component.execution_scope.clone(),
                moniker: source_component.moniker.clone(),
                path,
                flags,
            });
        Ok(Some(dir_connector))
    }

    async fn route_debug(
        &self,
        request: RouteRequest,
        _target: Arc<WeakInstanceToken>,
    ) -> Result<CapabilitySource, RouterError> {
        let subdir = request
            .sub_directory_path
            .map(|s| SubDir::new(s).expect("invalid subdir"))
            .unwrap_or_else(|| SubDir::dot());
        let subdir_relative = subdir.as_ref().clone();
        let storage_subdir = request
            .storage_sub_directory_path
            .map(|s| RelativePath::new(s).expect("invalid subdir"))
            .unwrap_or_else(|| RelativePath::dot());
        let storage_source_moniker = request
            .storage_source_moniker
            .map(|s| Moniker::parse_str(&s).expect("invalid moniker"))
            .unwrap_or_else(|| Moniker::root());

        let capability_source =
            if let CapabilitySource::StorageBackingDirectory(StorageBackingDirectorySource {
                capability,
                moniker,
                backing_dir_subdir: _,
                storage_subdir: _,
                storage_source_moniker: _,
            }) = &self.capability_source
            {
                CapabilitySource::StorageBackingDirectory(StorageBackingDirectorySource {
                    capability: capability.clone(),
                    moniker: moniker.clone(),
                    backing_dir_subdir: subdir_relative,
                    storage_subdir,
                    storage_source_moniker,
                })
            } else {
                self.capability_source.clone()
            };
        Ok(capability_source)
    }
}

struct ProgramDictionaryRouter {
    component: WeakComponentInstance,
    source_path: Path,
    capability: ComponentCapability,
}

#[async_trait]
impl Routable<Dictionary> for ProgramDictionaryRouter {
    async fn route(
        &self,
        request: RouteRequest,
        target: Arc<WeakInstanceToken>,
    ) -> Result<Option<Arc<Dictionary>>, RouterError> {
        fn open_error(e: OpenOutgoingDirError) -> OpenError {
            CapabilityProviderError::from(ComponentProviderError::from(e)).into()
        }

        let component = self.component.upgrade().map_err(|_| {
            RoutingError::from(ComponentInstanceError::instance_not_found(
                self.component.moniker.clone(),
            ))
        })?;
        let dir_entry = component.get_outgoing();
        const FLAGS: fio::Flags = fio::Flags::PROTOCOL_SERVICE;
        let path = vfs::Path::validate_and_split(self.source_path.to_string())
            .expect("path must be valid");
        if self.source_path == *DICTIONARY_ROUTER_PATH {
            let (inner_router, server_end) = create_proxy::<fsandbox::DictionaryRouterMarker>();
            FLAGS.to_object_request(server_end.into_channel()).handle(|request| {
                dir_entry.open_entry(OpenRequest::new(ExecutionScope::new(), FLAGS, path, request))
            });

            let (token_event_pair, server) = zx::EventPair::create();
            target.clone().register(token_event_pair.koid().unwrap(), server);
            let request = fsandbox::RouteRequest {
                requesting: Some(fsandbox::InstanceToken { token: token_event_pair }),
                ..Default::default()
            };

            let resp = inner_router
                .route(request)
                .await
                .map_err(|e| open_error(OpenOutgoingDirError::Fidl(e)))?
                .map_err(RouterError::from)?;
            match resp {
                fsandbox::DictionaryRouterRouteResponse::Dictionary(d) => {
                    let dictionary = Dictionary::try_from_fsandbox(d)
                        .map_err(|e| RouterError::NotFound(Arc::new(e)))?;
                    return Ok(Some(dictionary));
                }
                fsandbox::DictionaryRouterRouteResponse::Unavailable(_) => return Ok(None),
            }
        }

        let (inner_router, server_end) = create_proxy::<fruntime::DictionaryRouterMarker>();
        FLAGS.to_object_request(server_end.into_channel()).handle(|request| {
            dir_entry.open_entry(OpenRequest::new(ExecutionScope::new(), FLAGS, path, request))
        });

        let router = RemoteRouter::new(
            inner_router,
            component.context.remote_capabilities().clone(),
            component.moniker.clone(),
        );
        router.route(request, target).await
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
