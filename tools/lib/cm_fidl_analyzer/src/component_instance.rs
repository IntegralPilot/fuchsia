// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::component_model::{BuildAnalyzerModelError, Child, DynamicDictionaryConfig};
use crate::component_sandbox::{
    ProgramOutputGenerator, build_capability_sourced_capabilities_dictionary,
    build_framework_router, build_root_component_input, new_aggregate_router,
    new_event_stream_multiplexing_router, static_children_component_output_dictionary_routers,
};
use async_trait::async_trait;
use capability_source::{BuiltinCapabilities, NamespaceCapabilities};
use cm_config::RuntimeConfig;
use cm_rust::offer::OfferDecl;
use cm_rust::{CapabilityDecl, CollectionDecl, ComponentDecl, ExposeDecl, UseDecl};
use cm_types::{Name, Url};
use config_encoder::ConfigFields;
use fuchsia_sync::{Mutex, RwLock};
use moniker::{BorrowedChildName, ChildName, Moniker};
use router_error::RouterError;
use routing::bedrock::program_output_dict::build_program_output_dictionary;
use routing::bedrock::sandbox_construction::{ComponentSandbox, build_component_sandbox};
use routing::bedrock::structured_dict::ComponentInput;
use routing::component_instance::{
    ComponentInstanceInterface, ExtendedInstanceInterface, ResolvedInstanceInterface,
    TopInstanceInterface, WeakExtendedInstanceInterface,
};
use routing::error::{ComponentInstanceError, ErrorReporter, RouteRequestErrorInfo};
use routing::policy::GlobalPolicyChecker;
use routing::resolving::{ComponentAddress, ComponentResolutionContext, ResolverError};
use std::collections::HashMap;
use std::sync::Arc;

/// A representation of a v2 component instance.
#[derive(Debug)]
pub struct ComponentInstanceForAnalyzer {
    moniker: Moniker,
    pub(crate) decl: ComponentDecl,
    config: Option<ConfigFields>,
    url: Url,
    parent: WeakExtendedInstanceInterface<Self>,
    pub(crate) children: RwLock<HashMap<ChildName, Arc<Self>>>,
    policy_checker: GlobalPolicyChecker,
    component_id_index: Arc<component_id_index::Index>,
    // A mutex is used here so that we can initialize a `ComponentSandbox::default()` value and
    // then overwrite that value once `build_component_sandbox` has been called.
    pub(crate) sandbox: Mutex<ComponentSandbox>,
}

impl ComponentInstanceForAnalyzer {
    /// Exposes the component's ComponentDecl. This is referenced directly in tests.
    pub fn decl_for_testing(&self) -> &ComponentDecl {
        &self.decl
    }

    /// Creates a new root component instance.
    pub(crate) fn new_root(
        decl: ComponentDecl,
        config: Option<ConfigFields>,
        url: Url,
        top_instance: Arc<TopInstanceForAnalyzer>,
        runtime_config: Arc<RuntimeConfig>,
        policy: GlobalPolicyChecker,
        id_index: Arc<component_id_index::Index>,
        dynamic_dictionaries: Arc<DynamicDictionaryConfig>,
    ) -> Arc<Self> {
        let moniker = Moniker::root();
        let root_component_input = build_root_component_input(&runtime_config, &policy);
        let parent = WeakExtendedInstanceInterface::from(&ExtendedInstanceInterface::AboveRoot(
            top_instance,
        ));
        Self::new_internal(
            moniker,
            decl,
            config,
            url,
            parent,
            policy,
            id_index,
            root_component_input,
            dynamic_dictionaries,
        )
    }

    // Creates a new non-root component instance as a child of `parent`.
    pub(crate) fn new_for_child(
        child: &Child,
        decl: ComponentDecl,
        config: Option<ConfigFields>,
        parent: Arc<Self>,
        policy: GlobalPolicyChecker,
        id_index: Arc<component_id_index::Index>,
        dynamic_dictionaries: Arc<DynamicDictionaryConfig>,
    ) -> Result<Arc<Self>, BuildAnalyzerModelError> {
        let child_name = ChildName::try_new(
            child.child_moniker.name().as_str(),
            child.child_moniker.collection().map(|c| c.as_str()),
        )
        .expect("child moniker is guaranteed to be valid");
        let url = child.url.clone();
        let input = if let Some(collection_name) = child.child_moniker.collection() {
            let input = parent
                .sandbox
                .lock()
                .collection_inputs
                .get(collection_name)
                .expect("missing collection input");
            input
        } else {
            parent
                .sandbox
                .lock()
                .child_inputs
                .get(
                    &cm_types::Name::new(child.child_moniker.name().as_str())
                        .expect("child is static so name is not long"),
                )
                .expect("missing child input")
        };
        let moniker = parent.moniker.child(child_name);
        let parent =
            WeakExtendedInstanceInterface::from(&ExtendedInstanceInterface::Component(parent));
        Ok(Self::new_internal(
            moniker,
            decl,
            config,
            url,
            parent,
            policy,
            id_index,
            input,
            dynamic_dictionaries,
        ))
    }

    fn new_internal(
        moniker: Moniker,
        decl: ComponentDecl,
        config: Option<ConfigFields>,
        url: Url,
        parent: WeakExtendedInstanceInterface<ComponentInstanceForAnalyzer>,
        policy_checker: GlobalPolicyChecker,
        component_id_index: Arc<component_id_index::Index>,
        input: ComponentInput,
        dynamic_dictionaries: Arc<DynamicDictionaryConfig>,
    ) -> Arc<Self> {
        let self_ = Arc::new(Self {
            moniker: moniker,
            decl: decl.clone(),
            config,
            url,
            parent,
            children: RwLock::new(HashMap::new()),
            policy_checker,
            component_id_index,
            sandbox: Mutex::new(Default::default()),
        });
        let children_component_output_dictionary_routers =
            static_children_component_output_dictionary_routers(&self_, &decl);
        let (program_output_dict, declared_dictionaries) = build_program_output_dictionary(
            &self_,
            &decl,
            &input,
            &children_component_output_dictionary_routers,
            &ProgramOutputGenerator {
                dynamic_dictionaries,
                executable: decl.get_runner().is_some(),
            },
        );
        #[derive(Clone)]
        struct NullErrorReporter {}
        #[async_trait]
        impl ErrorReporter for NullErrorReporter {
            async fn report(
                &self,
                _: &RouteRequestErrorInfo,
                _: &RouterError,
                _: Arc<runtime_capabilities::WeakInstanceToken>,
            ) {
            }
        }
        let sandbox = build_component_sandbox(
            &self_,
            children_component_output_dictionary_routers,
            &decl,
            input,
            program_output_dict,
            build_framework_router(&self_),
            build_capability_sourced_capabilities_dictionary(&self_, &decl),
            declared_dictionaries,
            NullErrorReporter {},
            &new_aggregate_router,
            &new_event_stream_multiplexing_router,
        );
        *self_.sandbox.lock() = sandbox;
        self_
    }

    // Returns all children of the component instance.
    pub(crate) fn get_children(&self) -> Vec<Arc<Self>> {
        self.children.read().values().map(|c| Arc::clone(c)).collect()
    }

    // Adds a new child to this component instance.
    pub(crate) fn add_child(&self, child_moniker: ChildName, child: Arc<Self>) {
        self.children.write().insert(child_moniker, child);
    }

    pub(crate) fn moniker(&self) -> &Moniker {
        &self.moniker
    }

    // A (nearly) no-op sync function used to implement the async trait method `lock_resolved_instance`
    // for `ComponentInstanceInterface`.
    pub(crate) fn resolve<'a>(
        self: &'a Arc<Self>,
    ) -> Result<Box<dyn ResolvedInstanceInterface<Component = Self> + 'a>, ComponentInstanceError>
    {
        Ok(Box::new(&**self))
    }

    pub fn config_fields(&self) -> Option<&ConfigFields> {
        self.config.as_ref()
    }
}

impl PartialEq for ComponentInstanceForAnalyzer {
    fn eq(&self, other: &Self) -> bool {
        self.moniker == other.moniker
    }
}

#[async_trait]
impl ComponentInstanceInterface for ComponentInstanceForAnalyzer {
    type TopInstance = TopInstanceForAnalyzer;

    fn moniker(&self) -> &Moniker {
        &self.moniker
    }

    fn url(&self) -> &Url {
        &self.url
    }

    fn try_get_parent(&self) -> Result<ExtendedInstanceInterface<Self>, ComponentInstanceError> {
        Ok(self.parent.upgrade()?)
    }

    fn policy_checker(&self) -> &GlobalPolicyChecker {
        &self.policy_checker
    }

    fn config_parent_overrides(&self) -> Option<&[cm_rust::ConfigOverride]> {
        // TODO(https://fxbug.dev/42077231) ensure static parent overrides are captured
        None
    }

    fn component_id_index(&self) -> &component_id_index::Index {
        &self.component_id_index
    }

    // The trait definition requires this function to be async, but `ComponentInstanceForAnalyzer`'s
    // implementation must not await. This method is called by `route_capability`, which must
    // return immediately for `ComponentInstanceForAnalyzer` (see
    // `ComponentModelForAnalyzer::route_capability_sync()`).
    //
    // TODO(https://fxbug.dev/42168300): Remove this comment when Scrutiny's `DataController` can make async
    // function calls.
    async fn lock_resolved_state<'a>(
        self: &'a Arc<Self>,
    ) -> Result<
        Box<dyn ResolvedInstanceInterface<Component = ComponentInstanceForAnalyzer> + 'a>,
        ComponentInstanceError,
    > {
        self.resolve()
    }

    async fn component_sandbox(
        self: &Arc<Self>,
    ) -> Result<ComponentSandbox, ComponentInstanceError> {
        Ok(self.sandbox.lock().clone())
    }
}

#[async_trait]
impl ResolvedInstanceInterface for ComponentInstanceForAnalyzer {
    type Component = Self;

    fn try_uses(&self) -> Option<Box<[UseDecl]>> {
        Some(self.decl.uses.clone())
    }

    fn try_exposes(&self) -> Option<Box<[ExposeDecl]>> {
        Some(self.decl.exposes.clone())
    }

    fn try_offers(&self) -> Option<Box<[OfferDecl]>> {
        Some(self.decl.offers.clone())
    }

    fn try_capabilities(&self) -> Option<Box<[CapabilityDecl]>> {
        Some(self.decl.capabilities.clone())
    }

    fn try_collections(&self) -> Option<Box<[CollectionDecl]>> {
        Some(self.decl.collections.clone())
    }

    fn get_child(&self, moniker: &BorrowedChildName) -> Option<Arc<Self>> {
        self.children.read().get(moniker).map(Arc::clone)
    }

    // This is a static model with no notion of a collection.
    fn children_in_collection(&self, _collection: &Name) -> Vec<(ChildName, Arc<Self>)> {
        vec![]
    }

    async fn address(&self) -> Result<ComponentAddress, ResolverError> {
        Ok(ComponentAddress::from_absolute_url(&"none://not_used".parse().unwrap()).unwrap())
    }

    fn context_to_resolve_children(&self) -> Option<ComponentResolutionContext> {
        None
    }
}

/// A representation of `ComponentManager`'s instance, providing a set of capabilities to
/// the root component instance.
#[derive(Debug, Default)]
pub struct TopInstanceForAnalyzer {
    namespace_capabilities: NamespaceCapabilities,
    builtin_capabilities: BuiltinCapabilities,
}

impl TopInstanceForAnalyzer {
    pub fn new(
        namespace_capabilities: NamespaceCapabilities,
        builtin_capabilities: BuiltinCapabilities,
    ) -> Arc<Self> {
        Arc::new(Self { namespace_capabilities, builtin_capabilities })
    }
}

impl TopInstanceInterface for TopInstanceForAnalyzer {
    fn namespace_capabilities(&self) -> &NamespaceCapabilities {
        &self.namespace_capabilities
    }

    fn builtin_capabilities(&self) -> &BuiltinCapabilities {
        &self.builtin_capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cm_rust_testing::ComponentDeclBuilder;
    use futures::FutureExt;

    // Spot-checks that `ComponentInstanceForAnalyzer`'s implementation of the `ComponentInstanceInterface`
    // trait method `lock_resolved_state()` returns immediately. In addition, updates to that method should
    // be reviewed to make sure that this property holds; otherwise, `ComponentModelForAnalyzer`'s sync
    // methods may panic.
    #[test]
    fn lock_resolved_state_is_sync() {
        let decl = ComponentDeclBuilder::new().build();
        let url = "base://some_url";

        let instance = ComponentInstanceForAnalyzer::new_root(
            decl,
            None,
            url.parse().unwrap(),
            TopInstanceForAnalyzer::new(vec![], vec![]),
            Arc::new(RuntimeConfig::default()),
            GlobalPolicyChecker::default(),
            Arc::new(component_id_index::Index::default()),
            Arc::new(DynamicDictionaryConfig::default()),
        );

        assert!(instance.lock_resolved_state().now_or_never().is_some())
    }
}
