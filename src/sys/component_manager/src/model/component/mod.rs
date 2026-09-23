// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod instance;
pub mod manager;

use crate::framework::capabilities::RemotedRuntimeCapabilities;
use crate::framework::controller;
use crate::model::actions::{
    ActionsManager, DestroyAction, ResolveAction, ShutdownAction, ShutdownType, StartAction,
    UnresolveAction, start,
};
use crate::model::context::ModelContext;
use crate::model::logger::LoggerCache;
use crate::model::program::{StopConclusion, StopDisposition};
use crate::model::routing;
use crate::model::start::Start;
use ::namespace::Entry as NamespaceEntry;
use ::routing::bedrock::request_metadata::resolver_metadata;
use ::routing::bedrock::sandbox_construction::ComponentSandbox;
use ::routing::bedrock::structured_dict::ComponentInput;
use ::routing::component_instance::{
    ComponentInstanceInterface, ExtendedInstanceInterface, ResolvedInstanceInterface,
    WeakComponentInstanceInterface, WeakExtendedInstanceInterface,
};
use ::routing::error::{ComponentInstanceError, RoutingError};
use ::routing::policy::GlobalPolicyChecker;
use ::routing::resolving::{
    ComponentAddress, ComponentResolutionContext, ResolvedComponent, ResolvedPackage, ResolverError,
};
use anyhow::format_err;
use async_trait::async_trait;
use capability_source::CapabilitySource;
use clonable_error::ClonableError;
use cm_graph::DependencyNode;
use cm_rust::{CapabilityTypeName, ChildDecl, CollectionDecl, ComponentDecl, NativeIntoFidl};
use cm_types::{Name, Url};
use component_id_index::InstanceId;
use config_encoder::ConfigFields;
use directed_graph::DirectedGraph;
use errors::{
    ActionError, AddDynamicChildError, DestroyActionError, ModelError, OpenExposedDirError,
    OpenOutgoingDirError, ResolveActionError, StartActionError, StopActionError,
    StructuredConfigError,
};
use fidl::endpoints::{Proxy, create_proxy};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_resolution as fresolution;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_process as fprocess;
use fuchsia_async as fasync;
use futures::future::{BoxFuture, join_all};
use futures::lock::{MappedMutexGuard, Mutex, MutexGuard};
use hooks::{Event, EventPayload, Hooks};
use instance::{
    CompactChildren, InstanceState, ResolvedInstanceState, ShutdownInstanceState,
    UnresolvedInstanceState,
};
use log::{debug, error, warn};
use manager::ComponentManagerInstance;
use moniker::{BorrowedChildName, ChildName, Moniker};
use router_error::{Explain, RouterError};
use runner::component::StopInfo;
use runtime_capabilities::{
    Capability, Connector, Data, Dictionary, DirConnector, Routable, Router, WeakInstanceToken,
};
use std::clone::Clone;
use std::collections::HashSet;
use std::fmt;
use std::ops::DerefMut;
use std::sync::{Arc, Weak};
use std::time::Duration;
use version_history::AbiRevision;
use vfs::directory::entry::{
    DirectoryEntry, DirectoryEntryAsync, EntryInfo, GetEntryInfo, OpenRequest,
};
use vfs::execution_scope::ExecutionScope;

pub type WeakComponentInstance = WeakComponentInstanceInterface<ComponentInstance>;
pub type ExtendedInstance = ExtendedInstanceInterface<ComponentInstance>;
pub type WeakExtendedInstance = WeakExtendedInstanceInterface<ComponentInstance>;

pub type IncarnationId = u64;

/// Describes the reason a component instance is being requested to start.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum StartReason {
    /// Indicates that the target is starting the component because it wishes to access
    /// the capability at path.
    AccessCapability { target: Moniker, name: Name },
    /// Indicates that the component is starting because of a request to its outgoing
    /// directory.
    OutgoingDirectory,
    /// Indicates that the component is starting because it is in a single-run collection.
    SingleRun,
    /// Indicates that the component was explicitly started for debugging purposes.
    Debug,
    /// Indicates that the component was marked as eagerly starting by the parent.
    // TODO(https://fxbug.dev/42127825): Include the parent StartReason.
    // parent: ExtendedMoniker,
    // parent_start_reason: Option<Arc<StartReason>>
    Eager,
    /// Indicates that this component is starting because it is the root component.
    Root,
    /// Storage administration is occurring on this component.
    StorageAdmin,
    /// Indicates that this component is starting because the client of a
    /// `fuchsia.component.Controller` connection has called `Start()`
    Controller,
}

impl fmt::Display for StartReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                StartReason::AccessCapability { target, name } => {
                    format!("'{}' requested capability '{}'", target, name)
                }
                StartReason::OutgoingDirectory => {
                    "Instance started due to a request to its outgoing directory".to_string()
                }
                StartReason::SingleRun => "Instance is in a single_run collection".to_string(),
                StartReason::Debug => "Instance was started from debugging workflow".to_string(),
                StartReason::Eager => "Instance is an eager child".to_string(),
                StartReason::Root => "Instance is the root".to_string(),
                StartReason::StorageAdmin => "Storage administration on instance".to_string(),
                StartReason::Controller =>
                    "Instructed to start with the fuchsia.component.Controller protocol".to_string(),
            }
        )
    }
}

/// Component information returned by the resolver.
#[derive(Clone, Debug)]
pub struct Component {
    /// The context to be used to resolve a component from a path
    /// relative to this component (for example, a component in a subpackage).
    /// If `None`, the resolver cannot resolve relative path component URLs.
    pub context_to_resolve_children: Option<ComponentResolutionContext>,
    /// The declaration of the resolved manifest.
    pub decl: Option<Arc<ComponentDecl>>,
    /// The package info, if the component came from a package.
    pub package: Option<Package>,
    /// The component's validated configuration. If None, no configuration was provided.
    pub config: Option<Arc<ConfigFields>>,
    /// The component's target ABI revision, if available.
    pub abi_revision: Option<AbiRevision>,
    /// Dependency graph for the component. This is not part of the fidl type, but is derived
    /// from `decl`. As part of a component's resolved state, this should be kept up to date to
    /// reflect any dynamic offers.
    pub dependencies: Arc<DirectedGraph<DependencyNode>>,
}

impl Component {
    pub fn resolve_with_config(
        ResolvedComponent {
            context_to_resolve_children,
            // Component just resolved so this is fine
            decl,
            package,
            config_values,
            abi_revision,
            dependencies,
        }: ResolvedComponent,
        config_parent_overrides: Option<&[cm_rust::ConfigOverride]>,
    ) -> Result<Self, ResolveActionError> {
        let config = if let Some(config_decl) = decl.config.as_ref() {
            match config_decl.value_source {
                // If the config is provided via routing then `config_values` will be empty.
                cm_rust::ConfigValueSource::Capabilities(_) => None,
                // If the config is provided in our package then the resolver should give use the values.
                cm_rust::ConfigValueSource::PackagePath(_) => {
                    let values = config_values.ok_or(StructuredConfigError::ConfigValuesMissing)?;
                    let config =
                        ConfigFields::resolve(config_decl, values, config_parent_overrides)
                            .map_err(StructuredConfigError::ConfigResolutionFailed)?;
                    Some(Arc::new(config))
                }
            }
        } else {
            None
        };

        let package = package.map(|p| p.try_into()).transpose()?;
        Ok(Self {
            context_to_resolve_children,
            decl: Some(Arc::new(decl)),
            package,
            config,
            abi_revision,
            dependencies: Arc::new(dependencies),
        })
    }
}

impl From<&Component> for fresolution::Component {
    fn from(component: &Component) -> fresolution::Component {
        let bytes_to_fmem_data = |bytes: &[u8]| {
            let vmo = zx::Vmo::create(bytes.len() as u64).unwrap();
            vmo.write(&bytes, 0).unwrap();
            fmem::Data::Buffer(fmem::Buffer { vmo, size: bytes.len() as u64 })
        };
        let decl = component.decl.as_ref().map(|decl| {
            let decl = decl.as_ref().clone().native_into_fidl();
            bytes_to_fmem_data(&fidl::persist(&decl).expect(
                "we should always be able to persist a manifest that we got by unpersisting it",
            ))
        });
        let package = component.package.as_ref().map(|package| fresolution::Package {
            directory: fuchsia_fs::directory::clone(&package.package_dir)
                .ok()
                .and_then(|proxy| proxy.into_client_end().ok()),
            ..Default::default()
        });
        fresolution::Component {
            url: None,
            decl,
            package,
            config_values: component
                .config
                .as_ref()
                .map(|config| bytes_to_fmem_data(&(**config).clone().encode_as_fidl_struct())),
            resolution_context: component.context_to_resolve_children.as_ref().map(Into::into),
            abi_revision: component.abi_revision.as_ref().map(|abi_revision| abi_revision.as_u64()),
            ..Default::default()
        }
    }
}

/// Package information possibly returned by the resolver.
#[derive(Clone, Debug)]
pub struct Package {
    /// The package that this resolved component belongs to
    pub package_dir: fio::DirectoryProxy,
}

impl TryFrom<ResolvedPackage> for Package {
    type Error = ResolveActionError;

    fn try_from(package: ResolvedPackage) -> Result<Self, Self::Error> {
        Ok(Self { package_dir: package.directory.into_proxy() })
    }
}

/// The default amount of time that a component runner is given to stop a component. If exceeded, a
/// kill command will be issued. 5 seconds was chosen as a sensible default, but can be overridden
/// by a component's parent.
pub const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The amount of time that a component runner is given to kill a component.
pub const DEFAULT_KILL_TIMEOUT: Duration = Duration::from_secs(1);

/// Capabilities that a component receives dynamically.
pub struct IncomingCapabilities {
    pub numbered_handles: Vec<fprocess::HandleInfo>,
    pub additional_namespace_entries: Vec<NamespaceEntry>,
    pub dict: Option<Arc<Dictionary>>,
}

impl Default for IncomingCapabilities {
    fn default() -> Self {
        Self { numbered_handles: Vec::new(), additional_namespace_entries: Vec::new(), dict: None }
    }
}

impl IncomingCapabilities {
    pub fn from_start_child_args(
        remote_capabilities: &RemotedRuntimeCapabilities,
        mut args: fcomponent::StartChildArgs,
    ) -> Result<Self, fcomponent::Error> {
        let numbered_handles = args.numbered_handles.take().unwrap_or_default();

        let namespace: namespace::Namespace = args
            .namespace_entries
            .take()
            .unwrap_or_default()
            .try_into()
            .map_err(|_| fcomponent::Error::InvalidArguments)?;

        let dict_1 = if let Some(dict_ref) = args.dictionary {
            let fidl_capability = fsandbox::Capability::Dictionary(dict_ref);
            let cap = Capability::try_from(fidl_capability)
                .map_err(|_| fcomponent::Error::InvalidArguments)?;
            let Capability::Dictionary(dict) = cap else {
                return Err(fcomponent::Error::InvalidArguments);
            };
            Some(dict)
        } else {
            None
        };

        let dict_2 = if let Some(dictionary_handle) = args.additional_inputs {
            Some(
                remote_capabilities
                    .get::<Dictionary>(dictionary_handle)
                    .map_err(|_| fcomponent::Error::InvalidArguments)?,
            )
        } else {
            None
        };

        if dict_1.is_some() && dict_2.is_some() {
            return Err(fcomponent::Error::InvalidArguments);
        }

        Ok(Self {
            numbered_handles,
            additional_namespace_entries: namespace.into(),
            dict: dict_1.xor(dict_2),
        })
    }
}

/// Models a component instance, possibly with links to children.
pub struct ComponentInstance {
    /// The component's URL.
    pub component_url: Url,
    /// The mode of startup (lazy or eager).
    pub startup: fdecl::StartupMode,
    /// The policy to apply if the component terminates.
    pub on_terminate: fdecl::OnTerminate,
    /// The parent instance. Either a component instance or component manager's instance.
    pub parent: WeakExtendedInstance,
    /// The moniker of this instance.
    pub moniker: Moniker,
    /// The hooks scoped to this instance.
    pub hooks: Arc<Hooks>,
    /// Whether to persist isolated storage data of this component instance after it has been
    /// destroyed.
    pub persistent_storage: bool,

    /// The component's incarnation id. This is used to distinguish different instances of a
    /// dynamic component that share the same moniker.
    incarnation_id: IncarnationId,

    /// Configuration overrides provided by the parent component.
    config_parent_overrides: Option<Box<[cm_rust::ConfigOverride]>>,

    /// The context shared across the model.
    pub context: Arc<ModelContext>,

    // These locks must be taken in the order declared if held simultaneously.
    /// The component's mutable state.
    state: Mutex<InstanceState>,
    /// Actions on the instance that must eventually be completed.
    actions: ActionsManager,
    /// The ExecutionScope for this component. Pseudo directories should be hosted with this scope
    /// to tie their lifetime to that of the component. Tasks can block component destruction by
    /// using `active_guard()`.
    pub execution_scope: ExecutionScope,
}

impl Drop for ComponentInstance {
    fn drop(&mut self) {
        LoggerCache::purge(self);
    }
}

impl ComponentInstance {
    /// Instantiates a new root component instance.
    pub async fn new_root(
        input: ComponentInput,
        context: Arc<ModelContext>,
        component_manager_instance: Weak<ComponentManagerInstance>,
        component_url: Url,
    ) -> Arc<Self> {
        Self::new(
            input,
            Moniker::root(),
            0,
            component_url,
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            context,
            WeakExtendedInstance::AboveRoot(component_manager_instance),
            Arc::new(Hooks::new()),
            false,
        )
        .await
    }

    /// Instantiates a new component instance with the given contents.
    // TODO(https://fxbug.dev/42077692) convert this to a builder API
    pub async fn new(
        input: ComponentInput,
        moniker: Moniker,
        incarnation_id: IncarnationId,
        component_url: Url,
        startup: fdecl::StartupMode,
        on_terminate: fdecl::OnTerminate,
        config_parent_overrides: Option<Box<[cm_rust::ConfigOverride]>>,
        context: Arc<ModelContext>,
        parent: WeakExtendedInstance,
        hooks: Arc<Hooks>,
        persistent_storage: bool,
    ) -> Arc<Self> {
        let execution_scope = (context.scope_factory)();
        let self_ = Arc::new(Self {
            moniker,
            incarnation_id,
            component_url,
            startup,
            on_terminate,
            config_parent_overrides,
            context,
            parent,
            state: Mutex::new(InstanceState::Unresolved(UnresolvedInstanceState::new(input))),
            actions: ActionsManager::new(),
            hooks,
            persistent_storage,
            execution_scope,
        });
        self_.actions().set_component_reference(WeakComponentInstance::new(&self_)).await;
        self_
    }

    /// Locks and returns the instance's mutable state.
    // TODO(b/309656051): Remove this method from ComponentInstance's public API
    pub async fn lock_state(&self) -> MutexGuard<'_, InstanceState> {
        self.state.lock().await
    }

    /// Locks and returns the instance's action set.
    pub fn actions(&self) -> &ActionsManager {
        &self.actions
    }

    /// Returns true if the component is started, i.e. when it has a runtime.
    pub async fn is_started(&self) -> bool {
        self.lock_state().await.is_started()
    }

    /// Locks and returns a lazily resolved and populated `ResolvedInstanceState`. Does not
    /// register a `Resolve` action unless the resolved state is not already populated, so this
    /// function can be called re-entrantly from a Resolved hook. Returns an `InstanceNotFound`
    /// error if the instance is destroyed.
    // TODO(b/309656051): Remove this method from ComponentInstance's public API
    pub async fn lock_resolved_state<'a>(
        self: &'a Arc<Self>,
    ) -> Result<MappedMutexGuard<'a, InstanceState, ResolvedInstanceState>, ActionError> {
        loop {
            // Resolve is idempotent; it'll exit early if the component is already resolved. To
            // ensure that the component is resolved, we call it here without bothering to check
            // the component's current state. Once it returns successfully we know that the
            // component is resolved, even if the action did no actual work.
            self.resolve().await?;

            /// Returns Ok(Some(_)) when the component is in a resolved state, Ok(None) when the
            /// component is in a state from which it can be resolved, and Err(_) when the
            /// component is in a state from which it cannot be resolved.
            async fn get_mapped_mutex_or_error<'a>(
                self_: &'a Arc<ComponentInstance>,
            ) -> Result<
                Option<MappedMutexGuard<'a, InstanceState, ResolvedInstanceState>>,
                ActionError,
            > {
                let state = self_.state.lock().await;
                if state.get_resolved_state().is_some() {
                    return Ok(Some(MutexGuard::map(state, |s| {
                        s.get_resolved_state_mut().expect("not resolved")
                    })));
                }
                if let InstanceState::Destroyed = *state {
                    return Err(ResolveActionError::InstanceDestroyed {
                        moniker: self_.moniker.clone(),
                    }
                    .into());
                }
                if state.is_shut_down() {
                    return Err(ResolveActionError::InstanceShutDown {
                        moniker: self_.moniker.clone(),
                    }
                    .into());
                }
                Ok(None)
            }
            if let Some(mapped_guard) = get_mapped_mutex_or_error(&self).await? {
                return Ok(mapped_guard);
            }
            // If we've reached here, then the component must have been unresolved in-between our
            // calls to resolve and get_mapped_mutex_or_error. Our mission here remains to resolve
            // the component if necessary and then return the resolved state, so let's loop and try
            // to resolve it again.
        }
    }

    /// Resolves the component declaration, populating `ResolvedInstanceState` as necessary. A
    /// `Resolved` event is dispatched if the instance was not previously resolved or an error
    /// occurs.
    pub async fn resolve(self: &Arc<Self>) -> Result<(), ActionError> {
        ActionsManager::register(self.clone(), ResolveAction::new()).await
    }

    /// Unresolves the component using an UnresolveAction. The component will be shut down, then
    /// reset to the Discovered state without being destroyed. An Unresolved event is dispatched on
    /// success or error.
    pub async fn unresolve(self: &Arc<Self>) -> Result<(), ActionError> {
        ActionsManager::register(self.clone(), UnresolveAction::new()).await
    }

    /// Adds the dynamic child defined by `child_decl` to the given `collection_name`.
    pub async fn add_dynamic_child(
        self: &Arc<Self>,
        collection_name: String,
        child_decl: &ChildDecl,
        child_args: fcomponent::CreateChildArgs,
    ) -> Result<(), AddDynamicChildError> {
        let mut state = self.lock_resolved_state().await?;
        let collection_decl = state
            .collection_decls
            .iter()
            .find(|decl| decl.name == collection_name)
            .cloned()
            .ok_or_else(|| AddDynamicChildError::CollectionNotFound {
                name: collection_name.clone(),
            })?;
        let is_single_run_collection = collection_decl.durability == fdecl::Durability::SingleRun;
        // Start the child if it's created in a `SingleRun` collection or it's eager.
        let maybe_start_reason = if is_single_run_collection {
            Some(StartReason::SingleRun)
        } else if child_decl.startup == fdecl::StartupMode::Eager {
            Some(StartReason::Eager)
        } else {
            None
        };

        // Specifying numbered handles is only allowed if the component is started in
        // a single-run collection.
        let numbered_handles = child_args.numbered_handles.unwrap_or_default();
        if !is_single_run_collection && !numbered_handles.is_empty() {
            return Err(AddDynamicChildError::NumberedHandleNotInSingleRunCollection);
        }

        if !collection_decl.allow_long_names && child_decl.name.len() > cm_types::MAX_NAME_LENGTH {
            return Err(AddDynamicChildError::NameTooLong { max_len: cm_types::MAX_NAME_LENGTH });
        }

        let dynamic_offers = child_args.dynamic_offers.unwrap_or_else(Vec::new);
        if dynamic_offers.len() > 0
            && collection_decl.allowed_offers != cm_types::AllowedOffers::StaticAndDynamic
        {
            return Err(AddDynamicChildError::DynamicOffersNotAllowed { collection_name });
        }

        let child_input = state
            .sandbox
            .collection_inputs
            .get(&Name::new(&collection_name).unwrap())
            .expect("dict missing for declared collection")
            .shallow_copy();

        if child_args.dictionary.is_some() && child_args.additional_inputs.is_some() {
            return Err(AddDynamicChildError::DictionaryAndAdditionalInputsSet);
        }

        // Merge `ChildArgs.dictionary` entries into the child sandbox.
        if let Some(dictionary_ref) = child_args.dictionary {
            let fidl_capability = fsandbox::Capability::Dictionary(dictionary_ref);
            let any: Capability =
                fidl_capability.try_into().map_err(|_| AddDynamicChildError::InvalidDictionary)?;
            let dict = match any {
                Capability::Dictionary(d) => d,
                _ => return Err(AddDynamicChildError::InvalidDictionary),
            };
            let child_dict_entries = child_input.capabilities();
            for (key, value) in dict.drain() {
                let router: Capability = match value {
                    Capability::Connector(s) => Router::<Connector>::new_ok(s).into(),
                    Capability::DirConnector(s) => Router::<DirConnector>::new_ok(s).into(),
                    Capability::Dictionary(s) => Router::<Dictionary>::new_ok(s).into(),
                    Capability::Data(d) => Router::<Data>::new_ok(d).into(),
                    c => c,
                };

                if let Some(_) = child_dict_entries.insert(key.clone(), router) {
                    return Err(AddDynamicChildError::StaticRouteConflict { capability_name: key });
                }
            }
        }

        if let Some(dictionary_handle) = child_args.additional_inputs {
            let dictionary: Arc<Dictionary> = self
                .context
                .remote_capabilities()
                .get(dictionary_handle)
                .map_err(|_| AddDynamicChildError::InvalidDictionary)?;
            let child_dict_entries = child_input.capabilities();
            for (key, value) in dictionary.drain() {
                let router: Capability = match value {
                    Capability::Connector(s) => Router::<Connector>::new_ok(s).into(),
                    Capability::DirConnector(s) => Router::<DirConnector>::new_ok(s).into(),
                    Capability::Dictionary(s) => Router::<Dictionary>::new_ok(s).into(),
                    Capability::Data(d) => Router::<Data>::new_ok(d).into(),
                    c => c,
                };

                if let Some(_) = child_dict_entries.insert(key.clone(), router) {
                    return Err(AddDynamicChildError::StaticRouteConflict { capability_name: key });
                }
            }
        }

        let child = state
            .add_child(
                self,
                child_decl,
                Some(&collection_decl),
                Some(dynamic_offers),
                child_args.controller,
                child_input,
            )
            .await?;

        // Release the component state now that the component has been created and discovered.
        drop(state);

        if let Some(start_reason) = maybe_start_reason {
            child
                .start(
                    &start_reason,
                    None,
                    IncomingCapabilities {
                        numbered_handles,
                        additional_namespace_entries: vec![],
                        dict: None,
                    },
                )
                .await
                .map_err(|err| {
                    debug!(err:%, moniker:% = child.moniker; "failed to start component instance");
                    AddDynamicChildError::ActionError { err }
                })?;
        }

        Ok(())
    }

    /// Removes the dynamic child, returning a future that will execute the
    /// destroy action.
    pub async fn remove_dynamic_child(
        self: &Arc<Self>,
        child_moniker: &BorrowedChildName,
    ) -> Result<(), ActionError> {
        let incarnation = {
            let state = self.lock_state().await;
            let resolved_state = state
                .get_resolved_state()
                .ok_or(DestroyActionError::InstanceNotResolved { moniker: self.moniker.clone() })?;
            if let Some(c) = resolved_state.get_child(child_moniker) {
                c.incarnation_id()
            } else {
                let moniker = self.moniker.child(child_moniker.into());
                return Err(DestroyActionError::InstanceNotFound { moniker }.into());
            }
        };
        self.destroy_child(child_moniker.into(), incarnation).await
    }

    /// Stops this component.
    #[cfg(test)]
    pub async fn stop(self: &Arc<Self>) -> Result<(), ActionError> {
        ActionsManager::register(self.clone(), crate::model::actions::StopAction::new(false)).await
    }

    /// Shuts down this component. This means the component and its subrealm are stopped and never
    /// allowed to restart again.
    pub async fn shutdown(
        self: &Arc<Self>,
        shutdown_type: ShutdownType,
    ) -> Result<(), ActionError> {
        ActionsManager::register(self.clone(), ShutdownAction::new(shutdown_type)).await
    }

    /// Performs the stop protocol for this component instance. `shut_down` determines whether the
    /// instance is to be put into `InstanceState::Resolved` or `InstanceState::Shutdown`.
    ///
    /// Clients should not call this function directly, except for `StopAction` and
    /// `ShutdownAction`.
    ///
    /// TODO(https://fxbug.dev/42067346): Limit the clients that call this directly.
    ///
    /// REQUIRES: All dependents have already been stopped.
    pub async fn stop_instance_internal(
        self: &Arc<Self>,
        shut_down: bool,
    ) -> Result<(), StopActionError> {
        // If the component is started, we first move it back to the resolved state. We will move
        // it to the shutdown state after the stopping is complete.
        let mut started = None;
        let mut stop_timeout = None;
        self.lock_state().await.replace(|instance_state| match instance_state {
            InstanceState::Started(resolved_state, started_state) => {
                started = Some(started_state);
                stop_timeout = resolved_state.sandbox.component_input.environment().stop_timeout();
                InstanceState::Resolved(resolved_state)
            }
            other_state => other_state,
        });
        let stop_timeout =
            stop_timeout.map(|n| Duration::from_millis(n as u64)).unwrap_or(DEFAULT_STOP_TIMEOUT);

        let stop_result = {
            if let Some(started) = started {
                let started_timestamp = started.timestamp;
                let started_timestamp_monotonic = started.timestamp_monotonic;
                let stop_timer = Box::pin(async move {
                    let timer = fasync::Timer::new(fasync::MonotonicInstant::after(
                        zx::MonotonicDuration::from(stop_timeout),
                    ));
                    timer.await;
                });
                let kill_timer = Box::pin(async move {
                    let timer = fasync::Timer::new(fasync::MonotonicInstant::after(
                        zx::MonotonicDuration::from(DEFAULT_KILL_TIMEOUT),
                    ));
                    timer.await;
                });
                let ret = started
                    .stop(stop_timer, kill_timer)
                    .await
                    .map_err(StopActionError::ProgramStopError)?;
                if matches!(ret.disposition, StopDisposition::KilledAfterTimeout)
                    || matches!(ret.disposition, StopDisposition::Killed(_))
                {
                    // This log message is load-bearing server-side as it's used to identify driver
                    // hangs during shutdown.
                    // Please notify //src/developer/forensics/OWNERS upon changing.
                    warn!(
                        "component {} did not stop in {:?}. Killed it.",
                        self.moniker, stop_timeout,
                    );
                }
                let cleanly_stopped = matches!(
                    ret.disposition,
                    StopDisposition::Stopped(StopInfo{ termination_status, exit_code })
                        if termination_status.is_ok() && exit_code.unwrap_or(0) == 0
                );
                if !shut_down && !cleanly_stopped && self.on_terminate == fdecl::OnTerminate::Reboot
                {
                    // This log message is load-bearing server-side as it's used to extract the
                    // critical component responsible for the reboot.
                    // Please notify //src/developer/forensics/OWNERS upon changing.
                    warn!(
                        "Component with on_terminate=REBOOT terminated uncleanly: {} {:?}. \
                            Rebooting the system",
                        self.moniker, ret.disposition
                    );
                    let top_instance = self
                        .top_instance()
                        .await
                        .map_err(|_| StopActionError::GetTopInstanceFailed)?;
                    top_instance.trigger_reboot();
                }
                Some((ret, started_timestamp, started_timestamp_monotonic))
            } else {
                None
            }
        };

        // When the component is stopped, any child instances in collections must be destroyed.
        self.destroy_dynamic_children()
            .await
            .map_err(|err| StopActionError::DestroyDynamicChildrenFailed { err: Box::new(err) })?;

        if let Some((
            StopConclusion { disposition, escrow_request },
            _start_time,
            start_time_monotonic,
        )) = stop_result
        {
            let requested_escrow = escrow_request.is_some();

            // Store any escrowed state.
            {
                let mut state = self.lock_state().await;
                if let InstanceState::Resolved(resolved_state) = &mut *state {
                    if let Some(program_escrow) = resolved_state.program_escrow() {
                        program_escrow.did_stop(escrow_request);
                    }
                };
            }

            let stop_time = zx::BootInstant::get();
            let stop_time_monotonic = zx::MonotonicInstant::get();
            let event = self.new_event(EventPayload::Stopped {
                status: disposition.stop_info().termination_status,
                exit_code: disposition.stop_info().exit_code,
                stop_time,
                stop_time_monotonic,
                execution_duration: stop_time_monotonic - start_time_monotonic,
                requested_escrow,
            });
            self.hooks.dispatch(&event).await;
        }

        if shut_down {
            self.move_state_to_shutdown().await?;
        }

        if let ExtendedInstance::Component(parent) =
            self.try_get_parent().map_err(|_| StopActionError::GetParentFailed)?
        {
            parent
                .destroy_child_if_single_run(
                    self.child_moniker().expect("child is root instance?").into(),
                    self.incarnation_id(),
                )
                .await;
        }
        Ok(())
    }

    /// Moves the state of `self` to `InstanceState::Shutdown`, or panics. If the component was in
    /// the `Started` state, the `StartedInstanceState` is returned.
    async fn move_state_to_shutdown(self: &Arc<Self>) -> Result<(), StopActionError> {
        loop {
            // If the component is in a resolved state, then we have to route its storage
            // capabilities. We shouldn't do this while holding the state lock, so let's do this in
            // advance before grabbing the state lock below.
            let storage_paths = {
                let state = self.lock_state().await;
                match &*state {
                    InstanceState::Resolved(resolved_state) => resolved_state.storage_paths.clone(),
                    _ => Vec::new(),
                }
            };

            let mut routed_storage = Vec::with_capacity(storage_paths.len());
            for storage_path in &storage_paths {
                if let Ok(info) = routing::route_storage(storage_path, &self).await {
                    routed_storage.push(info);
                }
            }

            // Now that any necessary routing operations are out of the way, grab the state lock
            // and let's calculate our new state.
            let mut state = self.lock_state().await;
            let new_state = match state.deref_mut() {
                InstanceState::Unresolved(unresolved_state) => Some(InstanceState::Shutdown(
                    ShutdownInstanceState {
                        children: Box::new(CompactChildren::default()),
                        routed_storage: vec![],
                    },
                    Box::new(unresolved_state.take()),
                )),
                InstanceState::Resolved(resolved_state) => {
                    let children = resolved_state.children.clone();
                    // don't necessarily need the complete decl, just need some to test equality
                    let current_storage_iter = resolved_state.storage_paths.iter();

                    if current_storage_iter.ne(storage_paths.iter()) {
                        continue;
                    }

                    Some(InstanceState::Shutdown(
                        ShutdownInstanceState { children, routed_storage },
                        Box::new(resolved_state.to_unresolved()),
                    ))
                }
                InstanceState::Started(_, _) => {
                    error!(
                        "component {} was started while it was stopping or shutting down, this should be impossible",
                        self.moniker
                    );
                    return Err(StopActionError::ComponentStartedDuringShutdown);
                }
                InstanceState::Shutdown(_, _) | InstanceState::Destroyed => None,
            };
            if let Some(new_state) = new_state {
                state.set(new_state);
            }
            return Ok(());
        }
    }

    async fn destroy_child_if_single_run(
        self: &Arc<Self>,
        child_moniker: ChildName,
        incarnation: IncarnationId,
    ) {
        let single_run_colls = {
            let state = self.lock_state().await;
            if state.get_resolved_state().is_none() {
                // Component instance was not resolved, so no dynamic children.
                return;
            }
            let resolved_state = state.get_resolved_state().unwrap();
            resolved_state
                .collection_decls
                .iter()
                .filter_map(|c| match c.durability {
                    fdecl::Durability::SingleRun => Some(c.name.clone()),
                    fdecl::Durability::Transient => None,
                })
                .collect::<HashSet<_>>()
        };
        if let Some(coll) = child_moniker.collection() {
            if single_run_colls.contains(coll) {
                let self_clone = self.clone();
                fasync::Task::spawn(async move {
                    if let Err(error) =
                        self_clone.destroy_child(child_moniker.clone(), incarnation).await
                    {
                        let moniker = self_clone.moniker.child(child_moniker.clone());
                        warn!(
                            moniker:%,
                            error:%;
                            "single-run component could not be destroyed",
                        );
                    }
                })
                .detach();
            }
        }
    }

    /// Destroys this component instance.
    /// REQUIRES: All children have already been destroyed.
    pub async fn destroy_instance(self: &Arc<Self>) -> Result<(), DestroyActionError> {
        if self.persistent_storage {
            return Ok(());
        }
        // Clean up isolated storage.
        let routed_storage = {
            let mut state = self.lock_state().await;
            match *state {
                InstanceState::Shutdown(ref mut s, _) => std::mem::take(&mut s.routed_storage),
                _ => panic!(
                    "cannot destroy component instance {} because it is not shutdown, it is in state {:?}",
                    self.moniker, *state
                ),
            }
        };
        for storage in routed_storage {
            match routing::delete_storage(storage).await {
                Ok(()) => (),
                Err(error) => {
                    // We received an error we weren't expecting, but we still want to destroy
                    // this instance. It's bad to leave storage state undeleted, but it would
                    // be worse to not continue with destroying this instance. Log the error,
                    // and proceed.
                    warn!(
                        component:% = self.moniker, error:%;
                        "failed to delete storage during instance destruction, proceeding with destruction anyway",
                    );
                }
            }
        }
        Ok(())
    }

    /// Registers actions to destroy all dynamic children of collections belonging to this instance.
    async fn destroy_dynamic_children(self: &Arc<Self>) -> Result<(), ActionError> {
        let dynamic_children: Vec<_> = {
            match *self.lock_state().await {
                InstanceState::Resolved(ref state) | InstanceState::Started(ref state, _) => state
                    .children()
                    .filter(|(k, _)| k.collection().is_some())
                    .map(|(k, c)| (k.clone(), c.incarnation_id()))
                    .collect(),
                InstanceState::Shutdown(ref state, _) => state
                    .children
                    .iter()
                    .filter(|(k, _)| k.collection().is_some())
                    .map(|(k, c)| (k.clone(), c.incarnation_id()))
                    .collect(),
                _ => {
                    // Component instance was not resolved, so no dynamic children.
                    return Ok(());
                }
            }
        };

        let futures =
            join_all(dynamic_children.into_iter().map(|(m, id)| self.destroy_child(m.into(), id)))
                .await;

        #[allow(clippy::manual_try_fold, reason = "mass allow for https://fxbug.dev/381896734")]
        futures.into_iter().fold(Ok(()), |acc, r| acc.and(r))
    }

    pub async fn destroy_child(
        self: &Arc<Self>,
        moniker: ChildName,
        incarnation: IncarnationId,
    ) -> Result<(), ActionError> {
        // The child may not exist or may already be deleted by a previous DeleteChild action.
        let child = {
            let state = self.lock_state().await;
            match *state {
                InstanceState::Resolved(ref s) | InstanceState::Started(ref s, _) => {
                    let child = s.get_child(&moniker);
                    child
                }
                InstanceState::Shutdown(ref state, _) => {
                    state.children.get(&moniker).map(|r| r.clone())
                }
                InstanceState::Destroyed => None,
                InstanceState::Unresolved(_) => {
                    panic!("DestroyChild: target is not resolved");
                }
            }
        };

        let Some(child) = child else { return Ok(()) };

        if child.incarnation_id() != incarnation {
            // The instance of the child we pulled from our live children does not match the
            // instance of the child we were asked to delete. This is possible if destroy_child
            // was called twice for the same child, and after the first call a child with the
            // same name was recreated.
            //
            // If there's already a live child with a different instance than what we were
            // asked to destroy, then surely the instance we wanted to destroy is long gone,
            // and we can safely return without doing any work.
            return Ok(());
        }

        // Wait for the child component to be destroyed
        ActionsManager::register(child.clone(), DestroyAction::new()).await
    }

    /// Opens an object referenced by `path` from the outgoing directory of the component.  The
    /// component must have a program, or this method will fail.  Starts the component if necessary.
    ///
    /// TODO(https://fxbug.dev/332329856): If the component is to be started as a result of the open
    /// call, and the starting failed, that error is not returned here. If you would like to observe
    /// start errors, call `ensure_started` before this function.
    pub async fn open_outgoing(
        &self,
        open_request: OpenRequest<'_>,
    ) -> Result<(), OpenOutgoingDirError> {
        let escrow = {
            let state = self.lock_state().await;
            match &*state {
                InstanceState::Resolved(resolved) | InstanceState::Started(resolved, _) => resolved
                    .program_escrow()
                    .cloned()
                    .ok_or(OpenOutgoingDirError::InstanceNonExecutable)?,
                _ => return Err(OpenOutgoingDirError::InstanceNotResolved),
            }
        };

        escrow.open_outgoing(open_request).await?;
        Ok(())
    }

    /// Returns a `DirectoryEntry` representation of the outgoing directory of the component. It
    /// performs the same checks as `open_outgoing`, but errors are surfaced at the server
    /// endpoint.
    pub fn get_outgoing(self: &Arc<Self>) -> Arc<dyn DirectoryEntry> {
        struct GetOutgoing {
            component: WeakComponentInstance,
        }

        impl DirectoryEntry for GetOutgoing {
            fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
                let component = self.component.upgrade().map_err(|e| e.as_zx_status())?;
                request.spawn(component);
                Ok(())
            }
        }

        impl GetEntryInfo for GetOutgoing {
            fn entry_info(&self) -> EntryInfo {
                EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
            }
        }

        Arc::new(GetOutgoing { component: WeakComponentInstance::from(self) })
    }

    /// Obtains the component output dict.
    pub async fn get_component_output_dict(
        self: &Arc<Self>,
    ) -> Result<Arc<Dictionary>, RouterError> {
        Ok(self
            .lock_resolved_state()
            .await
            .map_err(|e| {
                RoutingError::from(ComponentInstanceError::ResolveFailed {
                    moniker: self.moniker.clone(),
                    err: ClonableError::from(format_err!("{:?}", e)),
                })
            })?
            .sandbox
            .component_output
            .capabilities())
    }

    /// Returns a router that delegates to the component output dict.
    pub(super) fn component_output(self: &Arc<Self>) -> Arc<Router<Dictionary>> {
        #[derive(Debug)]
        struct ComponentOutput {
            component: WeakComponentInstance,
        }

        #[async_trait]
        impl Routable<Dictionary> for ComponentOutput {
            async fn route(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Dictionary>>, RouterError> {
                let component = self.component.upgrade().map_err(RoutingError::from)?;
                Ok(Some(component.get_component_output_dict().await?))
            }
            async fn route_debug(
                &self,
                _request: RouteRequest,
                _target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                panic!("ComponentOutput router does not support debug routes");
            }
        }

        Router::<Dictionary>::new(ComponentOutput { component: self.as_weak() })
    }

    /// Opens this instance's exposed directory if it has been resolved.
    pub async fn open_exposed(
        self: &Arc<Self>,
        open_request: OpenRequest<'_>,
    ) -> Result<(), OpenExposedDirError> {
        let state = self.lock_state().await;
        match &*state {
            InstanceState::Unresolved(_) | InstanceState::Shutdown(_, _) => {
                Err(OpenExposedDirError::InstanceNotResolved)
            }
            InstanceState::Resolved(resolved_instance_state)
            | InstanceState::Started(resolved_instance_state, _) => {
                resolved_instance_state
                    .get_exposed_dir(self.as_weak().into())
                    .await
                    .open_entry(open_request)?;
                Ok(())
            }
            InstanceState::Destroyed => Err(OpenExposedDirError::InstanceDestroyed),
        }
    }

    /// Binds to the component instance in this instance, starting it if it's not already running.
    pub async fn start(
        self: &Arc<Self>,
        reason: &StartReason,
        execution_controller_task: Option<controller::ExecutionControllerTask>,
        incoming: IncomingCapabilities,
    ) -> Result<(), ActionError> {
        // Skip starting a component instance that was already started. It's important to bail out
        // here so we don't waste time starting eager children more than once.
        {
            let state = self.lock_state().await;
            if let Some(res) = start::should_return_early(&state, &self.moniker) {
                return res.map_err(Into::into);
            }
        }
        ActionsManager::register(
            self.clone(),
            StartAction::new(reason.clone(), execution_controller_task, incoming),
        )
        .await?;

        let eager_children: Vec<_> = {
            let state = self.lock_state().await;
            match *state {
                InstanceState::Resolved(ref s) | InstanceState::Started(ref s, _) => s
                    .children()
                    .filter_map(|(_, r)| match r.startup {
                        fdecl::StartupMode::Eager => Some(r.clone()),
                        fdecl::StartupMode::Lazy => None,
                    })
                    .collect(),
                InstanceState::Shutdown(_, _) => {
                    return Err(StartActionError::InstanceShutDown {
                        moniker: self.moniker.clone(),
                    }
                    .into());
                }
                InstanceState::Destroyed => {
                    return Err(StartActionError::InstanceDestroyed {
                        moniker: self.moniker.clone(),
                    }
                    .into());
                }
                InstanceState::Unresolved(_) => {
                    panic!("start: not resolved")
                }
            }
        };
        Self::start_eager_children_recursive(self, eager_children).await;
        Ok(())
    }

    /// Starts a list of instances, and any eager children they may return.
    // This function recursively calls `start`, so it returns a BoxFuture,
    fn start_eager_children_recursive<'a>(
        self: &Arc<Self>,
        instances_to_bind: Vec<Arc<ComponentInstance>>,
    ) -> BoxFuture<'a, ()> {
        let this = self.clone();
        let f = async move {
            join_all(instances_to_bind.iter().map(|component| {
                let this = this.clone();
                async move {
                    match component.ensure_started(&StartReason::Eager).await {
                        Ok(()) => {}
                        Err(err) => {
                            this.log(
                                log::Level::Warn,
                                format!("eager child of {} failed to start", this.moniker),
                                &[
                                    &("moniker", format!("{}", component.moniker).as_str()),
                                    &("err", format!("{err}").as_str()),
                                ],
                            )
                            .await;
                        }
                    }
                }
            }))
            .await;
        };
        Box::pin(f)
    }

    pub fn incarnation_id(&self) -> IncarnationId {
        self.incarnation_id
    }

    pub fn instance_id(&self) -> Option<&InstanceId> {
        self.context.component_id_index().id_for_moniker(&self.moniker)
    }

    pub async fn log(
        self: &Arc<Self>,
        level: log::Level,
        message: impl fmt::Display,
        key_values: &[&(dyn log::kv::Source + Send + Sync)],
    ) {
        let state = self.lock_state().await;
        let mut builder = log::Record::builder();
        builder.level(level);
        for key_value in key_values {
            builder.key_values(key_value);
        }

        if let Some(resolved_instance_state) = state.get_resolved_state() {
            LoggerCache::log(
                &self.moniker,
                resolved_instance_state,
                &builder.args(format_args!("{}", message)).build(),
                self.as_weak().into(),
            );
        } else {
            log::logger().log(&builder.args(format_args!("{}", message)).build());
        }
    }

    /// Scoped this server_end to the component instance's Runtime. For the duration
    /// of the component's lifetime, when it's running, this channel will be
    /// kept alive.
    pub async fn scope_to_runtime(self: &Arc<Self>, server_end: zx::Channel) {
        let mut state = self.lock_state().await;
        state.scope_server_end(server_end);
    }

    /// Returns the top instance (component manager's instance) by traversing parent links.
    async fn top_instance(self: &Arc<Self>) -> Result<Arc<ComponentManagerInstance>, ModelError> {
        let mut current = self.clone();
        loop {
            match current.try_get_parent()? {
                ExtendedInstance::Component(parent) => {
                    current = parent;
                }
                ExtendedInstance::AboveRoot(parent) => {
                    return Ok(parent);
                }
            }
        }
    }

    /// Returns the effective persistent storage setting for a child.
    /// If the CollectionDecl exists and the `persistent_storage` field is set, return the setting.
    /// Otherwise, if the CollectionDecl or its `persistent_storage` field is not set, return
    /// `self.persistent_storage` as a default value for the child to inherit.
    pub fn persistent_storage_for_child(&self, collection: Option<&CollectionDecl>) -> bool {
        let default_persistent_storage = self.persistent_storage;
        if let Some(collection) = collection {
            collection.persistent_storage.unwrap_or(default_persistent_storage)
        } else {
            default_persistent_storage
        }
    }

    /// Looks up a component by moniker.
    ///
    /// The component instance in the component will be resolved if that has not already happened.
    #[cfg(test)]
    pub async fn find_and_maybe_resolve(
        self: &Arc<Self>,
        look_up_moniker: &Moniker,
    ) -> Result<Arc<ComponentInstance>, ModelError> {
        let path = look_up_moniker.path();
        let mut name_iterator = path.iter();
        let mut component = self.clone();
        loop {
            // Check the resolved state directly instead of calling `lock_resolve_state`
            // because that function will wait on any in-progress resolve actions, and we may
            // have been called from a resolve action.
            if component.lock_state().await.get_resolved_state().is_none() {
                component.resolve().await?;
            }
            let Some(child_name) = name_iterator.next() else {
                return Ok(component);
            };
            let state = component.lock_state().await;
            let Some(resolved_state) = state.get_resolved_state() else {
                // The component must have unresolved while waiting on the resolve action.
                continue;
            };
            if let Some(child) = resolved_state.get_child(child_name) {
                let child = child.clone();
                drop(state);
                component = child;
            } else {
                return Err(ModelError::instance_not_found(look_up_moniker.clone()));
            }
        }
    }

    /// Finds a component matching the moniker, if such a component exists.
    /// This function has no side-effects.
    pub async fn find(
        self: &Arc<Self>,
        look_up_moniker: &Moniker,
    ) -> Option<Arc<ComponentInstance>> {
        let mut cur = self.clone();
        for moniker in look_up_moniker.path() {
            let next = cur
                .lock_state()
                .await
                .get_resolved_state()
                .and_then(|r| r.get_child(moniker))
                .cloned()?;
            cur = next
        }
        Some(cur)
    }

    /// Finds a resolved component matching the moniker, if such a component exists.
    /// This function has no side-effects.
    #[cfg(all(test, not(feature = "src_model_tests")))]
    pub async fn find_resolved(
        self: &Arc<Self>,
        find_moniker: &Moniker,
    ) -> Option<Arc<ComponentInstance>> {
        let mut cur = self.clone();
        for moniker in find_moniker.path() {
            let next = cur
                .lock_state()
                .await
                .get_resolved_state()
                .and_then(|r| r.get_child(moniker))
                .cloned()?;
            cur = next
        }
        // Found the moniker, the last child in the chain of resolved parents. Is it resolved?
        if cur.lock_state().await.get_resolved_state().is_some() { Some(cur.clone()) } else { None }
    }

    /// Starts the component instance in the given component if it's not already running.
    /// Returns the component that was bound to.
    #[cfg(test)]
    pub async fn start_instance<'a>(
        self: &Arc<Self>,
        moniker: &'a Moniker,
        reason: &StartReason,
    ) -> Result<Arc<ComponentInstance>, ModelError> {
        let component = self.find_and_maybe_resolve(moniker).await?;
        component.start(reason, None, IncomingCapabilities::default()).await?;
        Ok(component)
    }

    pub fn new_event(&self, payload: EventPayload) -> Event {
        self.new_event_with_timestamp(payload, zx::BootInstant::get())
    }

    pub fn new_event_with_timestamp(
        &self,
        payload: EventPayload,
        timestamp: zx::BootInstant,
    ) -> Event {
        Event {
            target_moniker: self.moniker.clone().into(),
            component_url: self.component_url.clone(),
            timestamp,
            payload,
        }
    }

    /// Routes this component's resolver capability from `component_input`, establishes a
    /// connection to the capability provider, asks it to resolve `component_address`, and returns
    /// the results.
    ///
    /// If `component_input` is `None` then the component input dictionary from this component will
    /// be used.
    pub async fn perform_resolve(
        self: &Arc<Self>,
        component_input: Option<ComponentInput>,
        component_address: &ComponentAddress,
    ) -> Result<ResolvedComponent, ResolverError> {
        let component_input = match component_input {
            Some(input) => input,
            None => match &*self.lock_state().await {
                InstanceState::Unresolved(state) => state.component_input.clone(),
                InstanceState::Resolved(state) | InstanceState::Started(state, _) => {
                    state.sandbox.component_input.clone()
                }
                InstanceState::Shutdown(_, _) | InstanceState::Destroyed => {
                    return Err(ResolverError::routing_error(RoutingError::RouteSourceShutdown {
                        moniker: self.moniker.clone(),
                    }));
                }
            },
        };

        if let Some(resource) = component_address.resource() {
            if resource.ends_with(".cml") {
                warn!(
                    "Component resource ends with .cml instead of .cm, which was \
probably not intended: {}",
                    component_address.url()
                );
            }
        }

        let resolvers_dict = component_input.environment().resolvers();
        let resolver_capability_res =
            resolvers_dict.get(&Name::new(component_address.scheme()).unwrap());
        let resolver_router = match resolver_capability_res {
            Some(Capability::ConnectorRouter(resolver_router)) => resolver_router,
            _ => {
                return Err(ResolverError::SchemeNotRegistered);
            }
        };
        let resp = resolver_router
            .route(
                resolver_metadata(cm_rust::Availability::Required),
                self.clone().as_weak().into(),
            )
            .await
            .map_err(|err| ResolverError::routing_error(err))?;
        let resolver_proxy = match resp {
            // Built-in resolver are hosted by a LaunchTaskOnReceive, which returns a Connector
            // capability for new routes.
            Some(resolver_connector) => {
                let (proxy, server_end) = create_proxy::<fresolution::ResolverMarker>();
                resolver_connector.send(server_end.into_channel()).map_err(|_| {
                    ResolverError::routing_error(RoutingError::BedrockFailedToSend {
                        moniker: self.moniker.clone().into(),
                        capability_id: component_address.scheme().to_string(),
                    })
                })?;
                proxy
            }
            None => {
                return Err(ResolverError::routing_error(
                    RoutingError::RouteUnexpectedUnavailable {
                        type_name: CapabilityTypeName::Resolver,
                        moniker: self.moniker.clone().into(),
                    },
                ));
            }
        };
        let (component_url, some_context) = component_address.to_url_and_context();
        let component = if component_address.is_relative_path() {
            let context = some_context.ok_or_else(|| {
                error!(url:% = component_url; "calling resolve_with_context() with absolute");
                ResolverError::RelativeUrlMissingContext(component_url.to_string())
            })?;
            resolver_proxy
                .resolve_with_context(component_url, &context.into())
                .await
                .map_err(ResolverError::fidl_error)??
        } else {
            resolver_proxy.resolve(component_url).await.map_err(ResolverError::fidl_error)??
        };
        component.try_into()
    }
}

impl DirectoryEntry for ComponentInstance {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.spawn(self);
        Ok(())
    }
}

impl GetEntryInfo for ComponentInstance {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl DirectoryEntryAsync for ComponentInstance {
    async fn open_entry_async(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        self.open_outgoing(request).await.map_err(|e| e.as_zx_status())
    }
}

#[async_trait]
impl ComponentInstanceInterface for ComponentInstance {
    type TopInstance = ComponentManagerInstance;

    fn moniker(&self) -> &Moniker {
        &self.moniker
    }

    fn url(&self) -> &Url {
        &self.component_url
    }

    fn policy_checker(&self) -> &GlobalPolicyChecker {
        &self.context.policy()
    }

    fn component_id_index(&self) -> &component_id_index::Index {
        self.context.component_id_index()
    }

    fn config_parent_overrides(&self) -> Option<&[cm_rust::ConfigOverride]> {
        self.config_parent_overrides.as_ref().map(|c| &**c)
    }

    fn try_get_parent(&self) -> Result<ExtendedInstance, ComponentInstanceError> {
        self.parent.upgrade()
    }

    async fn lock_resolved_state<'a>(
        self: &'a Arc<Self>,
    ) -> Result<Box<dyn ResolvedInstanceInterface<Component = Self> + 'a>, ComponentInstanceError>
    {
        Ok(Box::new(ComponentInstance::lock_resolved_state(self).await.map_err(|err| {
            let err: anyhow::Error = err.into();
            ComponentInstanceError::ResolveFailed { moniker: self.moniker.clone(), err: err.into() }
        })?))
    }

    async fn component_sandbox(
        self: &Arc<Self>,
    ) -> Result<ComponentSandbox, ComponentInstanceError> {
        ComponentInstance::lock_resolved_state(self)
            .await
            .map(|state| state.sandbox.clone())
            .map_err(|err| {
                let err: anyhow::Error = err.into();
                ComponentInstanceError::ResolveFailed {
                    moniker: self.moniker.clone(),
                    err: err.into(),
                }
            })
    }
}

impl std::fmt::Debug for ComponentInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentInstance")
            .field("component_url", &self.component_url)
            .field("startup", &self.startup)
            .field("moniker", &self.moniker)
            .finish()
    }
}

#[cfg(all(test, feature = "src_model_tests"))]
pub mod testing {
    use fidl_fuchsia_component as fcomponent;
    use hooks::EventType;

    pub async fn wait_until_event_get_timestamp(
        event_stream: &fcomponent::EventStreamProxy,
        event_type: EventType,
    ) -> zx::BootInstant {
        let mut events = event_stream.get_next().await.unwrap();
        let next_event = events.remove(0);
        if !events.is_empty() {
            // We'll need to discard events if we receive too many, but test logic might depend on
            // those events. To make this issue happen as early as possible, panic here.
            panic!("got too many events at once")
        }
        assert_eq!(
            next_event.header.as_ref().unwrap().event_type,
            Some(event_type.into()),
            "wrong event type observed!"
        );
        next_event.header.as_ref().unwrap().timestamp.unwrap()
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use super::*;
    use crate::model::actions::StopAction;
    use crate::model::actions::test_utils::is_discovered;
    use crate::model::testing::mocks::ControllerActionResponse;
    use crate::model::testing::out_dir::OutDir;
    use crate::model::testing::routing_test_helpers::{RoutingTest, RoutingTestBuilder};
    use crate::model::testing::test_helpers::{
        ActionsTest, ComponentInfo, assert_event_type_and_moniker, component_decl_with_test_runner,
        get_n_events,
    };
    use ::routing::bedrock::structured_dict::ComponentInput;
    use assert_matches::assert_matches;
    use cm_rust::offer::{OfferDecl, OfferProtocolDecl, OfferSource, OfferTarget};
    use cm_rust::{Availability, ChildRef, DependencyType, ExposeSource};
    use cm_rust_testing::*;
    use errors::{AddChildError, DynamicCapabilityError};
    use fasync::TestExecutor;
    use fidl::endpoints::DiscoverableProtocolMarker;
    use fidl_fuchsia_logger as flogger;
    use fuchsia_async as fasync;
    use futures::channel::mpsc;
    use futures::{FutureExt, StreamExt};
    use hooks::EventType;
    use instance::UnresolvedInstanceState;
    use routing_test_helpers::component_id_index::make_index_file;
    use std::panic;
    use std::task::Poll;
    use vfs::ToObjectRequest;
    use vfs::path::Path as VfsPath;
    use vfs::service::host;
    use zx::AsHandleRef;

    const FLAGS: fio::Flags = fio::PERM_READABLE;

    #[fuchsia::test]
    async fn started_event_timestamp_matches_component() {
        let test =
            RoutingTest::new("root", vec![("root", ComponentDeclBuilder::new().build())]).await;

        let event_stream = test
            .new_event_stream(vec![
                EventType::Resolved,
                EventType::Started,
                EventType::DebugStarted,
            ])
            .await;

        let root = test.model.root().clone();
        let (f, bind_handle) = async move {
            root.start_instance(&Moniker::root(), &StartReason::Root).await.expect("failed to bind")
        }
        .remote_handle();
        fasync::Task::spawn(f).detach();
        let events = get_n_events(&event_stream, 3).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Resolved, ".");
        assert_event_type_and_moniker(&events[1], fcomponent::EventType::Started, ".");
        assert_event_type_and_moniker(&events[2], fcomponent::EventType::DebugStarted, ".");
        let get_timestamp = |e: &fcomponent::Event| e.header.as_ref().unwrap().timestamp.unwrap();
        let resolved_timestamp = get_timestamp(&events[0]);
        let started_timestamp = get_timestamp(&events[1]);
        let debug_started_timestamp = get_timestamp(&events[2]);

        assert!(resolved_timestamp < started_timestamp);
        assert!(started_timestamp == debug_started_timestamp);

        let component = bind_handle.await;
        let component_timestamp =
            component.lock_state().await.get_started_state().unwrap().timestamp;
        assert_eq!(component_timestamp, started_timestamp);
    }

    #[fuchsia::test]
    /// Validate that if the ComponentController channel is closed that the
    /// the component is stopped.
    async fn test_early_component_exit() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("a").eager().build())
                    .build(),
            ),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager().build())
                    .build(),
            ),
            ("b", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;

        let stop_event_stream = test.new_event_stream(vec![EventType::Stopped]).await;

        let a_moniker: Moniker = ["a"].try_into().unwrap();
        let b_moniker: Moniker = ["a", "b"].try_into().unwrap();

        let component_b = test.look_up(b_moniker.clone()).await;

        // Start the root so it and its eager children start.
        let _root = test
            .model
            .root()
            .start_instance(&Moniker::root(), &StartReason::Root)
            .await
            .expect("failed to start root");
        test.runner.wait_for_urls(&["test:///root", "test:///a", "test:///b"]).await;

        // Check that the eager 'b' has started.
        assert!(component_b.is_started().await);

        let b_info = ComponentInfo::new(component_b.clone()).await;
        b_info.check_not_shut_down(&test.runner).await;

        // Tell the runner to close the controller channel
        test.runner.abort_controller(&b_info.channel_id);

        // Verify that we get a stop event as a result of the controller
        // channel close being observed.
        let stop_event = stop_event_stream.get_next().await.unwrap();
        assert_eq!(stop_event[0].header.as_ref().unwrap().moniker, Some(b_moniker.to_string()));

        // Verify that a parent of the exited component can still be stopped
        // properly.
        ActionsManager::register(
            test.look_up(a_moniker.clone()).await,
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("Couldn't trigger shutdown");
        // Check that we get a stop even which corresponds to the parent.
        let stop_event_2 = stop_event_stream.get_next().await.unwrap();
        assert_eq!(stop_event_2[0].header.as_ref().unwrap().moniker, Some(a_moniker.to_string()));
    }

    #[fuchsia::test]
    async fn unresolve_test() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager().build())
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager().build())
                    .child(ChildBuilder::new().name("d").eager().build())
                    .build(),
            ),
            ("c", component_decl_with_test_runner()),
            ("d", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;

        // Resolve each component.
        test.look_up(Moniker::root()).await;
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let _component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let _component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let _component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

        // Just unresolve component a and children
        assert_matches!(component_a.unresolve().await, Ok(()));
        // component a is now resolved
        assert!(is_discovered(&component_a).await);
        // Component a no longer has children, due to not being resolved
        assert_matches!(component_a.find(&["b"].try_into().unwrap()).await, None);
        assert_matches!(component_a.find(&["b", "c"].try_into().unwrap()).await, None);
        assert_matches!(component_a.find(&["b", "d"].try_into().unwrap()).await, None);

        // Unresolve again, which is ok because UnresolveAction is idempotent.
        assert_matches!(component_a.unresolve().await, Ok(()));
        assert!(is_discovered(&component_a).await);
    }

    #[fuchsia::test]
    async fn realm_instance_id() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("a").eager().build())
                    .build(),
            ),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager().build())
                    .build(),
            ),
            ("b", component_decl_with_test_runner()),
        ];

        let instance_id = InstanceId::new_random(&mut rand::rng());
        let index = {
            let mut index = component_id_index::Index::default();
            index
                .insert(component_id_index::IndexEntry {
                    moniker: Moniker::root(),
                    instance_id: instance_id.clone(),
                    ignore_duplicate_id: false,
                })
                .unwrap();
            index
        };
        let component_id_index_path = make_index_file(index).unwrap();
        let test = RoutingTestBuilder::new("root", components)
            .set_component_id_index_path(
                component_id_index_path.path().to_owned().try_into().unwrap(),
            )
            .build()
            .await;

        let root = test.model.root();
        let root_realm = root.start_instance(&Moniker::root(), &StartReason::Root).await.unwrap();
        assert_eq!(instance_id, *root_realm.instance_id().unwrap());

        let a_realm = root
            .start_instance(&Moniker::try_from(["a"]).unwrap(), &StartReason::Root)
            .await
            .unwrap();
        assert_eq!(None, a_realm.instance_id());
    }

    #[fuchsia::test]
    async fn shutdown_component_interface_no_dynamic() {
        let example_offer = OfferBuilder::directory()
            .name("foo")
            .source_static_child("a")
            .target_static_child("b")
            .build();
        let example_capability = CapabilityBuilder::protocol().name("bar").build();
        let example_expose =
            ExposeBuilder::protocol().name("bar").source(ExposeSource::Self_).build();
        let example_use = UseBuilder::protocol().name("baz").build();
        let env_a = EnvironmentBuilder::new().name("env_a").build();
        let env_b = EnvironmentBuilder::new().name("env_b").build();

        let root_decl = ComponentDeclBuilder::new()
            .environment(env_a.clone())
            .environment(env_b.clone())
            .child(ChildBuilder::new().name("a").environment("env_a").build())
            .child(ChildBuilder::new().name("b").environment("env_b").build())
            .child_default("c")
            .collection_default("coll")
            .offer(example_offer.clone())
            .expose(example_expose.clone())
            .capability(example_capability.clone())
            .use_(example_use.clone())
            .build();
        let components = vec![
            ("root", root_decl.clone()),
            ("a", component_decl_with_test_runner()),
            ("b", component_decl_with_test_runner()),
            ("c", component_decl_with_test_runner()),
        ];

        let test = RoutingTestBuilder::new("root", components).build().await;

        let root_component =
            test.model.root().start_instance(&Moniker::root(), &StartReason::Root).await.unwrap();

        let root_resolved = root_component.lock_resolved_state().await.expect("resolve failed");

        assert_eq!(&[example_capability], &*root_resolved.try_capabilities().unwrap());
        assert_eq!(&[example_use], &*root_resolved.try_uses().unwrap());
        assert_eq!(&[example_offer], &*root_resolved.try_offers().unwrap());
        assert_eq!(&[example_expose], &*root_resolved.try_exposes().unwrap());
        assert_eq!(&[root_decl.collections[0].clone()], &*root_resolved.try_collections().unwrap());
        assert_eq!(
            &[env_a, env_b],
            &*root_resolved.resolved_component.decl.as_ref().unwrap().environments
        );

        let mut children = root_resolved
            .children()
            .map(|(moniker, _instance)| moniker.clone())
            .collect::<Vec<ChildName>>();
        children.sort();
        assert_eq!(
            &[
                ChildName::parse("a").unwrap(),
                ChildName::parse("b").unwrap(),
                ChildName::parse("c").unwrap(),
            ],
            &*children
        );
    }

    #[fuchsia::test]
    async fn shutdown_component_interface_dynamic_children_and_offers() {
        let example_offer = OfferBuilder::directory()
            .name("foo")
            .source_static_child("a")
            .target_static_child("b")
            .build();

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .environment(EnvironmentBuilder::new().name("env_a"))
                    .environment(EnvironmentBuilder::new().name("env_b"))
                    .child(ChildBuilder::new().name("a").environment("env_a").build())
                    .child_default("b")
                    .collection(
                        CollectionBuilder::new()
                            .name("coll_1")
                            .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                    )
                    .collection(
                        CollectionBuilder::new()
                            .name("coll_2")
                            .environment("env_b")
                            .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                    )
                    .offer(example_offer.clone())
                    .build(),
            ),
            ("a", component_decl_with_test_runner()),
            ("b", component_decl_with_test_runner()),
        ];

        let test = ActionsTest::new("root", components, Some(Moniker::root())).await;

        test.create_dynamic_child("coll_1", "a").await;
        test.create_dynamic_child_with_args(
            "coll_1",
            "b",
            fcomponent::CreateChildArgs {
                dynamic_offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "a".into(),
                        collection: Some("coll_1".parse().unwrap()),
                    })),
                    source_name: Some("dyn_offer_source_name".to_string()),
                    target_name: Some("dyn_offer_target_name".to_string()),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    ..Default::default()
                })]),
                ..Default::default()
            },
        )
        .await
        .expect("failed to create child");
        test.create_dynamic_child("coll_2", "a").await;

        OfferBuilder::service()
            .name("n0")
            .source(OfferSource::Child(ChildRef {
                name: "a".parse().unwrap(),
                collection: Some("coll_1".parse().unwrap()),
            }))
            .target(OfferTarget::Child(ChildRef {
                name: "b".parse().unwrap(),
                collection: Some("coll_1".parse().unwrap()),
            }))
            .availability(Availability::Transitional)
            .build();

        let root_component = test.look_up(Moniker::root()).await;

        {
            let root_resolved = root_component.lock_resolved_state().await.expect("resolving");

            let mut children = root_resolved
                .children()
                .map(|(moniker, _instance)| moniker.clone())
                .collect::<Vec<ChildName>>();
            children.sort();
            pretty_assertions::assert_eq!(
                &[
                    ChildName::parse("a").unwrap(),
                    ChildName::parse("b").unwrap(),
                    ChildName::parse("coll_1:a").unwrap(),
                    ChildName::parse("coll_1:b").unwrap(),
                    ChildName::parse("coll_2:a").unwrap(),
                ],
                &*children
            );

            let expected_deps = Arc::new(DirectedGraph::from([
                (
                    DependencyNode::Environment("env_a".into()),
                    DependencyNode::Child("a".into(), None),
                ),
                (
                    DependencyNode::Environment("env_b".into()),
                    DependencyNode::Collection("coll_2".into()),
                ),
                (
                    DependencyNode::Environment("env_b".into()),
                    DependencyNode::Child("a".into(), Some("coll_2".into())),
                ),
                (DependencyNode::Child("a".into(), None), DependencyNode::Child("b".into(), None)),
                (
                    DependencyNode::Child("a".into(), Some("coll_1".into())),
                    DependencyNode::Child("b".into(), Some("coll_1".into())),
                ),
            ]));

            pretty_assertions::assert_eq!(
                &[example_offer.clone()],
                &*root_resolved.try_offers().unwrap()
            );
            pretty_assertions::assert_eq!(
                root_resolved.resolved_component.dependencies,
                expected_deps
            );
        }

        // Destroy `coll_1:b`. It should not be listed. The dynamic offer should be deleted.
        root_component
            .destroy_child("coll_1:b".try_into().unwrap(), 2)
            .await
            .expect("destroy failed");

        {
            let root_resolved = root_component.lock_resolved_state().await.expect("resolving");

            let mut children = root_resolved
                .children()
                .map(|(moniker, _instance)| moniker.clone())
                .collect::<Vec<ChildName>>();
            children.sort();
            pretty_assertions::assert_eq!(
                &[
                    ChildName::parse("a").unwrap(),
                    ChildName::parse("b").unwrap(),
                    ChildName::parse("coll_1:a").unwrap(),
                    ChildName::parse("coll_2:a").unwrap(),
                ],
                &*children
            );

            let expected_deps = Arc::new(DirectedGraph::from([
                (
                    DependencyNode::Environment("env_a".into()),
                    DependencyNode::Child("a".into(), None),
                ),
                (
                    DependencyNode::Environment("env_b".into()),
                    DependencyNode::Collection("coll_2".into()),
                ),
                (
                    DependencyNode::Environment("env_b".into()),
                    DependencyNode::Child("a".into(), Some("coll_2".into())),
                ),
                (DependencyNode::Child("a".into(), None), DependencyNode::Child("b".into(), None)),
            ]));

            pretty_assertions::assert_eq!(
                &[example_offer.clone()],
                &*root_resolved.try_offers().unwrap()
            );
            pretty_assertions::assert_eq!(
                root_resolved.resolved_component.dependencies,
                expected_deps,
            );
        }

        // Recreate `coll_1:b`, this time with a dynamic offer from `a` in the other
        // collection. Both versions should be listed.
        test.create_dynamic_child_with_args(
            "coll_1",
            "b",
            fcomponent::CreateChildArgs {
                dynamic_offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                    source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "a".parse().unwrap(),
                        collection: Some("coll_2".parse().unwrap()),
                    })),
                    source_name: Some("dyn_offer2_source_name".to_string()),
                    target_name: Some("dyn_offer2_target_name".to_string()),
                    dependency_type: Some(fdecl::DependencyType::Strong),
                    ..Default::default()
                })]),
                ..Default::default()
            },
        )
        .await
        .expect("failed to create child");

        {
            let root_resolved = root_component.lock_resolved_state().await.expect("resolving");

            let mut children = root_resolved
                .children()
                .map(|(moniker, _instance)| moniker.clone())
                .collect::<Vec<ChildName>>();
            children.sort();
            pretty_assertions::assert_eq!(
                &[
                    ChildName::parse("a").unwrap(),
                    ChildName::parse("b").unwrap(),
                    ChildName::parse("coll_1:a").unwrap(),
                    ChildName::parse("coll_1:b").unwrap(),
                    ChildName::parse("coll_2:a").unwrap(),
                ],
                &*children
            );

            let expected_deps = Arc::new(DirectedGraph::from([
                (
                    DependencyNode::Environment("env_a".into()),
                    DependencyNode::Child("a".into(), None),
                ),
                (
                    DependencyNode::Environment("env_b".into()),
                    DependencyNode::Child("a".into(), Some("coll_2".into())),
                ),
                (
                    DependencyNode::Environment("env_b".into()),
                    DependencyNode::Collection("coll_2".into()),
                ),
                (DependencyNode::Child("a".into(), None), DependencyNode::Child("b".into(), None)),
                (
                    DependencyNode::Child("a".into(), Some("coll_2".into())),
                    DependencyNode::Child("b".into(), Some("coll_1".into())),
                ),
            ]));

            pretty_assertions::assert_eq!(
                &[example_offer.clone()],
                &*root_resolved.try_offers().unwrap()
            );
            pretty_assertions::assert_eq!(
                root_resolved.resolved_component.dependencies,
                expected_deps,
            );
        }
    }

    #[fuchsia::test]
    async fn creating_dynamic_child_with_offer_cycle_fails() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child_default("static_child")
                    .collection(
                        CollectionBuilder::new()
                            .name("coll")
                            .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                    )
                    .offer(
                        OfferBuilder::service()
                            .name("foo")
                            .source(OfferSource::Collection("coll".parse().unwrap()))
                            .target_static_child("static_child"),
                    )
                    .build(),
            ),
            ("static_child", component_decl_with_test_runner()),
        ];

        let test = ActionsTest::new("root", components, Some(Moniker::root())).await;

        let res = test
            .create_dynamic_child_with_args(
                "coll",
                "dynamic_child",
                fcomponent::CreateChildArgs {
                    dynamic_offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "static_child".into(),
                            collection: None,
                        })),
                        source_name: Some("bar".to_string()),
                        target_name: Some("bar".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    })]),
                    ..Default::default()
                },
            )
            .await;
        assert_matches!(res, Err(fcomponent::Error::DependencyCycle));
    }

    #[fuchsia::test]
    async fn creating_cycle_between_collections_fails() {
        let components = vec![(
            "root",
            ComponentDeclBuilder::new()
                .collection(
                    CollectionBuilder::new()
                        .name("coll1")
                        .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                )
                .collection(
                    CollectionBuilder::new()
                        .name("coll2")
                        .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                )
                .offer(
                    OfferBuilder::service()
                        .name("foo")
                        .source(OfferSource::Collection("coll1".parse().unwrap()))
                        .target(OfferTarget::Collection("coll2".parse().unwrap())),
                )
                .build(),
        )];

        let test = ActionsTest::new("root", components, Some(Moniker::root())).await;
        test.create_dynamic_child("coll2", "dynamic_src").await;
        let cycle_res = test
            .create_dynamic_child_with_args(
                "coll1",
                "dynamic_sink",
                fcomponent::CreateChildArgs {
                    dynamic_offers: Some(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "dynamic_src".into(),
                            collection: Some("coll2".parse().unwrap()),
                        })),
                        source_name: Some("bar".to_string()),
                        target_name: Some("bar".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        ..Default::default()
                    })]),
                    ..Default::default()
                },
            )
            .await;
        assert_matches!(cycle_res, Err(fcomponent::Error::DependencyCycle));
    }

    #[fuchsia::test]
    async fn creating_dynamic_child_with_offer_from_undefined_on_self_fails() {
        let components = vec![(
            "root",
            ComponentDeclBuilder::new()
                .collection(
                    CollectionBuilder::new()
                        .name("coll")
                        .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                )
                .build(),
        )];

        let test = ActionsTest::new("root", components, Some(Moniker::root())).await;

        let res = test
            .create_dynamic_child_with_args(
                "coll",
                "dynamic_child",
                fcomponent::CreateChildArgs {
                    dynamic_offers: Some(vec![fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        source_name: Some("foo".to_string()),
                        target_name: Some("foo".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    })]),
                    ..Default::default()
                },
            )
            .await;
        assert_matches!(res, Err(fcomponent::Error::InvalidArguments));
    }

    #[fuchsia::test]
    async fn creating_dynamic_child_with_offer_target_set_fails() {
        let components = vec![(
            "root",
            ComponentDeclBuilder::new()
                .collection(
                    CollectionBuilder::new()
                        .name("coll")
                        .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                )
                .build(),
        )];

        let test = ActionsTest::new("root", components, Some(Moniker::root())).await;

        let res = test
            .create_dynamic_child_with_args(
                "coll",
                "dynamic_child",
                fcomponent::CreateChildArgs {
                    dynamic_offers: Some(vec![fdecl::Offer::Directory(fdecl::OfferDirectory {
                        source: Some(fdecl::Ref::Self_(fdecl::SelfRef {})),
                        source_name: Some("foo".to_string()),
                        target_name: Some("foo".to_string()),
                        target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "dynamic_child".into(),
                            collection: Some("coll".parse().unwrap()),
                        })),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Required),
                        ..Default::default()
                    })]),
                    ..Default::default()
                },
            )
            .await;
        assert_matches!(res, Err(fcomponent::Error::InvalidArguments));
    }

    async fn new_component() -> Arc<ComponentInstance> {
        ComponentInstance::new(
            ComponentInput::default(),
            Moniker::root(),
            0,
            "fuchsia-pkg://fuchsia.com/foo#at_root.cm".parse().unwrap(),
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstanceInterface::AboveRoot(Weak::new()),
            Arc::new(Hooks::new()),
            false,
        )
        .await
    }

    async fn new_resolved() -> InstanceState {
        let comp = new_component().await;
        let resolved_component = Component {
            context_to_resolve_children: None,
            decl: Some(Arc::new(ComponentDeclBuilder::new().build())),
            package: None,
            config: None,
            abi_revision: None,
            dependencies: Arc::new(DirectedGraph::new()),
        };
        let ris = ResolvedInstanceState::new(
            &comp,
            resolved_component,
            ComponentAddress::from_url(&comp.component_url, &comp).await.unwrap().into(),
            Default::default(),
            Default::default(),
        )
        .await
        .unwrap();
        InstanceState::Resolved(Box::new(ris))
    }

    async fn new_unresolved() -> InstanceState {
        InstanceState::Unresolved(UnresolvedInstanceState::new(ComponentInput::default()))
    }

    #[fuchsia::test]
    async fn instance_state_transitions_test() {
        // Unresolved --> Destroyed.
        let mut is = new_unresolved().await;
        is.set(InstanceState::Destroyed);
        assert_matches!(is, InstanceState::Destroyed);

        // Unresolved --> Resolved.
        let mut is = new_unresolved().await;
        is.set(new_resolved().await);
        assert_matches!(is, InstanceState::Resolved(_));

        // Discovered --> Destroyed.
        let mut is = new_unresolved().await;
        is.set(InstanceState::Destroyed);
        assert_matches!(is, InstanceState::Destroyed);

        // Resolved --> Unresolved.
        let mut is = new_resolved().await;
        is.set(new_unresolved().await);
        assert_matches!(is, InstanceState::Unresolved(_));

        // Resolved --> Destroyed.
        let mut is = new_resolved().await;
        is.set(InstanceState::Destroyed);
        assert_matches!(is, InstanceState::Destroyed);
    }

    // Macro to make the panicking tests more readable.
    macro_rules! panic_test {
        (   [$(
                $test_name:ident( // Test case name.
                    $($args:expr),+$(,)? // Arguments for test case.
                )
            ),+$(,)?]
        ) => {
            $(paste::paste!{
                #[allow(non_snake_case)]
                #[fuchsia::test(allow_stalls = false)]
                #[should_panic]
                async fn [< confirm_invalid_transition___ $test_name>]() {
                    confirm_invalid_transition($($args,)+).await;
                }
            })+
        }
    }

    async fn confirm_invalid_transition(cur: InstanceState, next: InstanceState) {
        let mut is = cur;
        is.set(next);
    }

    // Use the panic_test! macro to enumerate the invalid InstanceState transitions that are invalid
    // and should panic. As a result of the macro, the test names will be generated like
    // `confirm_invalid_transition___p2r`.
    panic_test!([
        // Destroyed !-> {Destroyed, Resolved, Unresolved}
        p2p(InstanceState::Destroyed, InstanceState::Destroyed),
        p2r(InstanceState::Destroyed, new_resolved().await),
        p2d(InstanceState::Destroyed, new_unresolved().await),
        // Resolved !-> Resolved
        r2r(new_resolved().await, new_resolved().await),
        // Discovered !-> Unresolved
        d2d(new_unresolved().await, new_unresolved().await),
    ]);

    #[fuchsia::test]
    async fn validate_and_convert_dynamic_offers() {
        let components = vec![(
            "root",
            ComponentDeclBuilder::new()
                .collection(
                    CollectionBuilder::new()
                        .name("col")
                        .allowed_offers(cm_types::AllowedOffers::StaticAndDynamic),
                )
                .build(),
        )];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();

        let _root = root
            .start_instance(&Moniker::root(), &StartReason::Root)
            .await
            .expect("failed to start root");
        test.runner.wait_for_urls(&["test:///root"]).await;

        let collection_decl = root
            .lock_resolved_state()
            .await
            .expect("failed to get resolved state")
            .collection_decls
            .iter()
            .find(|c| c.name.as_str() == "col")
            .expect("unable to find collection decl")
            .clone();

        let validate_and_convert = |offers: Vec<fdecl::Offer>| async {
            root.lock_resolved_state()
                .await
                .expect("failed to get resolved state")
                .validate_and_convert_dynamic_component(
                    Some(offers),
                    &ChildBuilder::new().name("foo").url("http://foo").into(),
                    Some(&collection_decl),
                )
        };

        assert_eq!(
            validate_and_convert(vec![]).await.expect("failed to validate/convert dynamic offers"),
            vec![],
        );

        assert_eq!(
            validate_and_convert(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(fdecl::Ref::Parent(fdecl::ParentRef {})),
                source_name: Some("fuchsia.example.Echo".to_string()),
                target: None,
                target_name: Some("fuchsia.example.Echo".to_string()),
                dependency_type: Some(fdecl::DependencyType::Strong),
                availability: Some(fdecl::Availability::Required),
                ..Default::default()
            })])
            .await
            .expect("failed to validate/convert dynamic offers"),
            vec![OfferDecl::Protocol(OfferProtocolDecl {
                source: OfferSource::Parent,
                source_name: "fuchsia.example.Echo".parse().unwrap(),
                source_dictionary: Default::default(),
                target: OfferTarget::Child(ChildRef {
                    name: "foo".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                }),
                target_name: "fuchsia.example.Echo".parse().unwrap(),
                dependency_type: DependencyType::Strong,
                availability: Availability::Required,
            }),],
        );

        assert_eq!(
            validate_and_convert(vec![fdecl::Offer::Protocol(fdecl::OfferProtocol {
                source: Some(fdecl::Ref::VoidType(fdecl::VoidRef {})),
                source_name: Some("fuchsia.example.Echo".to_string()),
                target: None,
                target_name: Some("fuchsia.example.Echo".to_string()),
                dependency_type: Some(fdecl::DependencyType::Strong),
                availability: Some(fdecl::Availability::Optional),
                ..Default::default()
            })])
            .await
            .expect("failed to validate/convert dynamic offers"),
            vec![OfferDecl::Protocol(OfferProtocolDecl {
                source: OfferSource::Void,
                source_name: "fuchsia.example.Echo".parse().unwrap(),
                source_dictionary: Default::default(),
                target: OfferTarget::Child(ChildRef {
                    name: "foo".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                }),
                target_name: "fuchsia.example.Echo".parse().unwrap(),
                dependency_type: DependencyType::Strong,
                availability: Availability::Optional,
            }),],
        );

        assert_matches!(
            validate_and_convert(vec![
                    fdecl::Offer::Protocol(fdecl::OfferProtocol {
                        source: Some(fdecl::Ref::Child(fdecl::ChildRef {
                            name: "doesnt-exist".to_string(),
                            collection: Some("col".parse().unwrap()),
                        })),
                        source_name: Some("fuchsia.example.Echo".to_string()),
                        source_dictionary: Default::default(),
                        target: None,
                        target_name: Some("fuchsia.example.Echo".to_string()),
                        dependency_type: Some(fdecl::DependencyType::Strong),
                        availability: Some(fdecl::Availability::Optional),
                        ..Default::default()
                    })
                ])
                .await
                .expect_err("unexpected succeess in validate/convert dynamic offers"),
                AddChildError::DynamicCapabilityError { err }
            if err == DynamicCapabilityError::SourceNotFound {
                offer: OfferDecl::Protocol(OfferProtocolDecl {
                    source: OfferSource::Child(ChildRef {
                        name: "doesnt-exist".parse().unwrap(),
                        collection: Some("col".parse().unwrap()),
                    }),
                    source_name: "fuchsia.example.Echo".parse().unwrap(),
                    source_dictionary: Default::default(),
                    target: OfferTarget::Child(ChildRef {
                        name: "foo".parse().unwrap(),
                        collection: Some("col".parse().unwrap()),
                    }),
                    target_name: "fuchsia.example.Echo".parse().unwrap(),
                    dependency_type: DependencyType::Strong,
                    availability: Availability::Optional,
                })
            }
        );

        assert_matches!(
            validate_and_convert(vec![
                fdecl::Offer::Storage(fdecl::OfferStorage {
                    source: Some(fdecl::Ref::Self_(fdecl::SelfRef { })),
                    source_name: Some("data".into()),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "foo".parse().unwrap(),
                        collection: Some("col".parse().unwrap()),
                    })),
                    target_name: Some("data".into()),
                    ..Default::default()
                })
            ])
            .await,
            Err(AddChildError::DynamicCapabilityError {
                err: DynamicCapabilityError::UnsupportedType { typename },
            }) if typename == "storage"
        );
        assert_matches!(
            validate_and_convert(vec![
                fdecl::Offer::EventStream(fdecl::OfferEventStream {
                    source: Some(fdecl::Ref::Parent(fdecl::ParentRef { })),
                    source_name: Some("started".into()),
                    target: Some(fdecl::Ref::Child(fdecl::ChildRef {
                        name: "foo".parse().unwrap(),
                        collection: Some("col".parse().unwrap()),
                    })),
                    target_name: Some("started".into()),
                    ..Default::default()
                })
            ])
            .await,
            Err(AddChildError::DynamicCapabilityError {
                err: DynamicCapabilityError::UnsupportedType { typename },
            }) if typename == "event_stream"
        );
    }

    // Tests that logging in `with_logger_as_default` uses the LogSink routed to the component.
    #[fuchsia::test]
    async fn with_logger_as_default_uses_logsink() {
        const TEST_CHILD_NAME: &str = "child";

        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .protocol_default(flogger::LogSinkMarker::PROTOCOL_NAME)
                    .offer(
                        OfferBuilder::protocol()
                            .name(flogger::LogSinkMarker::PROTOCOL_NAME)
                            .source(OfferSource::Self_)
                            .target_static_child(TEST_CHILD_NAME),
                    )
                    .child_default(TEST_CHILD_NAME)
                    .build(),
            ),
            (
                TEST_CHILD_NAME,
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name(flogger::LogSinkMarker::PROTOCOL_NAME))
                    .build(),
            ),
        ];
        let test_topology = ActionsTest::new(components[0].0, components, None).await;

        let (connect_tx, mut connect_rx) = mpsc::unbounded();
        let serve_logsink = move |stream: flogger::LogSinkRequestStream| {
            connect_tx.unbounded_send(()).unwrap();
            async move {
                // There are other tests that check the actual logs. We just need to test that
                // there's a connection.
                let _stream = stream;
                let () = std::future::pending().await;
            }
        };

        // Serve LogSink from the root component.
        let mut root_out_dir = OutDir::new();
        root_out_dir.add_entry("/svc/fuchsia.logger.LogSink".parse().unwrap(), host(serve_logsink));
        test_topology.runner.add_host_fn("test:///root", root_out_dir.host_fn());

        let child = test_topology.look_up([TEST_CHILD_NAME].try_into().unwrap()).await;

        // Start the child.
        ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await
        .expect("failed to start child");

        assert!(child.is_started().await);

        // Log a message using the child's scoped logger.
        child.log(log::Level::Info, "hello world", &[]).await;

        // Wait for the logger to connect to LogSink.
        connect_rx.next().await.unwrap();
    }

    #[fuchsia::test]
    async fn find_resolved_test() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            (
                "a",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("b").eager().build())
                    .build(),
            ),
            (
                "b",
                ComponentDeclBuilder::new()
                    .child(ChildBuilder::new().name("c").eager().build())
                    .child(ChildBuilder::new().name("d").eager().build())
                    .build(),
            ),
            ("c", component_decl_with_test_runner()),
            ("d", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        let root = test.model.root();

        // Not resolved, so not found.
        assert_matches!(root.find_resolved(&["a"].try_into().unwrap()).await, None);
        assert_matches!(root.find_resolved(&["a", "b"].try_into().unwrap()).await, None);
        assert_matches!(root.find_resolved(&["a", "b", "c"].try_into().unwrap()).await, None);
        assert_matches!(root.find_resolved(&["a", "b", "d"].try_into().unwrap()).await, None);

        // Resolve each component.
        test.look_up(Moniker::root()).await;
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let _component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let _component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let _component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

        // Now they can all be found.
        assert_matches!(root.find_resolved(&["a"].try_into().unwrap()).await, Some(_));
        assert_eq!(
            root.find_resolved(&["a"].try_into().unwrap()).await.unwrap().component_url.as_str(),
            "test:///a",
        );
        assert_matches!(root.find_resolved(&["a", "b"].try_into().unwrap()).await, Some(_));
        assert_matches!(root.find_resolved(&["a", "b", "c"].try_into().unwrap()).await, Some(_));
        assert_matches!(root.find_resolved(&["a", "b", "d"].try_into().unwrap()).await, Some(_));
        assert_matches!(
            root.find_resolved(&["a", "b", "nonesuch"].try_into().unwrap()).await,
            None
        );

        // Unresolve component a, this causes it to stop having children and drop component b after
        // shutting it down.
        ActionsManager::register(component_a.clone(), UnresolveAction::new())
            .await
            .expect("unresolve failed");

        // Because component a is not resolved, it does not have children
        assert_matches!(component_a.find(&["b"].try_into().unwrap()).await, None);
        assert_matches!(component_a.find(&["b", "c"].try_into().unwrap()).await, None);
        assert_matches!(component_a.find(&["b", "d"].try_into().unwrap()).await, None);
    }

    /// If a component is not started, a call to `open_outgoing` should start the component
    /// and deliver the open request there.
    #[fuchsia::test]
    async fn open_outgoing_starts_component() {
        let components = vec![("root", ComponentDeclBuilder::new().build())];
        let test_topology = ActionsTest::new(components[0].0, components, None).await;
        let (open_request_tx, mut open_request_rx) = mpsc::unbounded();

        let mut root_out_dir = OutDir::new();
        root_out_dir.add_entry(
            "/svc/foo".parse().unwrap(),
            vfs::service::endpoint(move |_scope, channel| {
                open_request_tx.unbounded_send(channel).unwrap();
            }),
        );
        test_topology.runner.add_host_fn("test:///root", root_out_dir.host_fn());

        let root = test_topology.look_up(Moniker::default()).await;
        assert!(!root.is_started().await);

        let (client_end, server_end) = zx::Channel::create();
        let execution_scope = ExecutionScope::new();
        let mut object_request = fio::Flags::PROTOCOL_SERVICE.to_object_request(server_end);
        root.open_outgoing(OpenRequest::new(
            execution_scope.clone(),
            fio::Flags::PROTOCOL_SERVICE,
            "svc/foo".try_into().unwrap(),
            &mut object_request,
        ))
        .await
        .unwrap();
        let server_end = open_request_rx.next().await.unwrap();
        assert!(root.is_started().await);
        assert_eq!(
            client_end.as_handle_ref().basic_info().unwrap().related_koid,
            server_end.as_handle_ref().basic_info().unwrap().koid
        );
    }

    /// If a component is not started and is configured incorrectly to not be able to start,
    /// `open_outgoing` should succeed but the channel is closed.
    #[fuchsia::test]
    async fn open_outgoing_failed_to_start_component() {
        let components = vec![(
            "root",
            ComponentDeclBuilder::new_empty_component().program_runner("invalid").build(),
        )];
        let test_topology = ActionsTest::new(components[0].0, components, None).await;

        let mut root_out_dir = OutDir::new();
        root_out_dir.add_entry(
            "/svc/foo".parse().unwrap(),
            vfs::service::endpoint(move |_scope, _channel| {
                unreachable!();
            }),
        );
        test_topology.runner.add_host_fn("test:///root", root_out_dir.host_fn());

        let root = test_topology.look_up(Moniker::default()).await;
        assert!(!root.is_started().await);

        let (client_end, server_end) = zx::Channel::create();

        let execution_scope = ExecutionScope::new();
        let mut object_request = fio::Flags::PROTOCOL_SERVICE.to_object_request(server_end);
        assert_matches!(
            root.open_outgoing(OpenRequest::new(
                execution_scope.clone(),
                fio::Flags::PROTOCOL_SERVICE,
                "svc/foo".try_into().unwrap(),
                &mut object_request,
            ))
            .await,
            Ok(())
        );
        assert!(!root.is_started().await);

        fasync::OnSignals::new(&client_end, zx::Signals::CHANNEL_PEER_CLOSED).await.unwrap();
        assert!(!root.is_started().await);
    }

    /// While the provider component is stopping, opening its outgoing directory should not block.
    /// This is important to not cause deadlocks if we are draining the provider component's
    /// namespace.
    #[fuchsia::test(allow_stalls = false)]
    async fn open_outgoing_while_component_is_stopping() {
        // Use mock time in this test.
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;

        let components = vec![("root", ComponentDeclBuilder::new().build())];
        let test_topology = ActionsTest::new(components[0].0, components, None).await;

        let root_out_dir = OutDir::new();
        test_topology.runner.add_host_fn("test:///root", root_out_dir.host_fn());

        // Configure the component runner to take 3 seconds to stop the component.
        let response_delay = zx::MonotonicDuration::from_seconds(3);
        test_topology.runner.add_controller_response(
            "test:///root",
            Box::new(move || ControllerActionResponse {
                close_channel: true,
                delay: Some(response_delay),
                termination_status: Some(Ok(())),
                exit_code: Some(1),
            }),
        );

        let root = test_topology.look_up(Moniker::default()).await;
        assert!(!root.is_started().await);

        // Start the component.
        let root = root
            .start_instance(&Moniker::root(), &StartReason::Root)
            .await
            .expect("failed to start root");
        test_topology.runner.wait_for_urls(&["test:///root"]).await;

        // Start to stop the component. This will stall because the framework will be
        // waiting the controller to respond.
        let stop_fut = ActionsManager::register(root.clone(), StopAction::new(false));
        futures::pin_mut!(stop_fut);
        assert_matches!(TestExecutor::poll_until_stalled(&mut stop_fut).await, Poll::Pending);

        // Open the outgoing directory. This should not block.
        let (_, server_end) = zx::Channel::create();
        let scope = ExecutionScope::new();
        let mut object_request = FLAGS.to_object_request(server_end);
        assert_matches!(
            root.open_outgoing(OpenRequest::new(
                scope.clone(),
                FLAGS,
                VfsPath::dot(),
                &mut object_request
            ))
            .await,
            Ok(())
        );

        // Let the timer advance. The component should be stopped now.
        TestExecutor::advance_to(initial + response_delay).await;
        assert_matches!(stop_fut.await, Ok(()));

        // Open the outgoing directory. This should still not block.
        let (_, server_end) = zx::Channel::create();
        let scope = ExecutionScope::new();
        let mut object_request = FLAGS.to_object_request(server_end);
        assert_matches!(
            root.open_outgoing(OpenRequest::new(
                scope.clone(),
                FLAGS,
                VfsPath::dot(),
                &mut object_request
            ))
            .await,
            Ok(())
        );
    }
}
