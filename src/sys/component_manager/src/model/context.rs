// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framework::capabilities::RemotedRuntimeCapabilities;
use crate::model::token::InstanceRegistry;
use ::diagnostics::routing::RoutingErrors;
use ::routing::policy::GlobalPolicyChecker;
use cm_config::{AbiRevisionPolicy, RuntimeConfig};
use errors::ModelError;
use flyweights::FlyStr;
use fuchsia_inspect::Inspector;
use fuchsia_sync::Mutex;
use moniker::Moniker;
use std::collections::HashMap;
use std::sync::Arc;
use vfs::execution_scope::ExecutionScope;

/// The ModelContext provides the API boundary between the Model and Realms. It
/// defines what parts of the Model or authoritative state about the tree we
/// want to share with Realms.
pub struct ModelContext {
    component_id_index: component_id_index::Index,
    policy_checker: GlobalPolicyChecker,
    runtime_config: Arc<RuntimeConfig>,
    instance_registry: Arc<InstanceRegistry>,
    config_developer_overrides: Mutex<HashMap<Moniker, HashMap<FlyStr, cm_rust::ConfigValue>>>,
    pub scope_factory: Box<dyn Fn() -> ExecutionScope + Send + Sync + 'static>,
    remote_capabilities: Arc<RemotedRuntimeCapabilities>,
    #[cfg(test)]
    pub extra_framework_capabilities:
        Mutex<HashMap<cm_types::Name, runtime_capabilities::Capability>>,
    /// Inspect routing errors.
    pub routing_errors: Arc<RoutingErrors>,
    inspector: Inspector,
    escrow_stats_node: fuchsia_inspect::Node,
}

impl ModelContext {
    /// Constructs a new ModelContext from a RuntimeConfig.
    pub fn new(
        runtime_config: Arc<RuntimeConfig>,
        instance_registry: Arc<InstanceRegistry>,
        inspector: Inspector,
        #[cfg(test)] scope_factory: Option<Box<dyn Fn() -> ExecutionScope + Send + Sync + 'static>>,
    ) -> Result<Self, ModelError> {
        #[cfg(not(test))]
        let scope_factory = Box::new(|| ExecutionScope::new());
        #[cfg(test)]
        let scope_factory = scope_factory.unwrap_or_else(|| Box::new(|| ExecutionScope::new()));

        let component_id_index = match &runtime_config.component_id_index_path {
            Some(path) => component_id_index::Index::from_fidl_file(&path)?,
            None => component_id_index::Index::default(),
        };
        let policy_checker = GlobalPolicyChecker::new(runtime_config.security_policy.clone());
        let routing_errors =
            Arc::new(RoutingErrors::new(inspector.root().create_child("routing_errors")));
        let escrow_stats_node = inspector.root().create_child("memory_saved_by_escrow");

        Ok(Self {
            component_id_index,
            policy_checker,
            runtime_config,
            instance_registry,
            config_developer_overrides: Mutex::new(HashMap::new()),
            scope_factory,
            remote_capabilities: Arc::new(RemotedRuntimeCapabilities::new()),
            routing_errors,
            #[cfg(test)]
            extra_framework_capabilities: Mutex::new(HashMap::new()),
            inspector,
            escrow_stats_node,
        })
    }

    #[cfg(test)]
    pub fn new_for_test() -> Self {
        let runtime_config = Arc::new(RuntimeConfig::default());
        let instance_registry = InstanceRegistry::new();
        let inspector = fuchsia_inspect::component::init_inspector_with_size(
            crate::builtin_environment::INSPECTOR_SIZE,
        )
        .clone();
        Self::new(runtime_config, instance_registry, inspector, None).unwrap()
    }

    /// Returns the runtime policy checker for the model.
    pub fn policy(&self) -> &GlobalPolicyChecker {
        &self.policy_checker
    }

    pub fn runtime_config(&self) -> &Arc<RuntimeConfig> {
        &self.runtime_config
    }

    pub fn component_id_index(&self) -> &component_id_index::Index {
        &self.component_id_index
    }

    pub fn abi_revision_policy(&self) -> &AbiRevisionPolicy {
        &self.runtime_config.abi_revision_policy
    }

    pub fn instance_registry(&self) -> &Arc<InstanceRegistry> {
        &self.instance_registry
    }

    pub fn remote_capabilities(&self) -> &Arc<RemotedRuntimeCapabilities> {
        &self.remote_capabilities
    }

    pub fn routing_errors(&self) -> &Arc<RoutingErrors> {
        &self.routing_errors
    }

    pub fn inspector(&self) -> &Inspector {
        &self.inspector
    }

    pub fn escrow_stats_node(&self) -> fuchsia_inspect::Node {
        self.escrow_stats_node.clone_weak()
    }

    #[cfg(all(test, feature = "src_model_tests"))]
    pub async fn add_framework_capability(
        &self,
        name: impl Into<String>,
        capability: impl Into<runtime_capabilities::Capability>,
    ) {
        let mut framework_capabilities = self.extra_framework_capabilities.lock();
        framework_capabilities.insert(cm_types::Name::new(name.into()).unwrap(), capability.into());
    }

    /// Adds a configuration override for the component identified by moniker.
    /// This override will take effect the next time the component is started.
    pub async fn add_config_developer_override(
        &self,
        moniker: Moniker,
        config_override: cm_rust::ConfigOverride,
    ) {
        self.config_developer_overrides
            .lock()
            .entry(moniker)
            .or_default()
            .insert(config_override.key, config_override.value);
    }

    /// Retrieves a map of any configuration overrides for the component
    /// identified by moniker.
    pub async fn get_config_developer_overrides(
        &self,
        moniker: &Moniker,
    ) -> HashMap<FlyStr, cm_rust::ConfigValue> {
        self.config_developer_overrides.lock().get(&moniker).cloned().unwrap_or_else(HashMap::new)
    }

    /// Removes the configuration overrides for the component identified by
    /// moniker. Returns an error if no overrides have been defined for the
    /// component.
    pub async fn remove_config_developer_override(
        &self,
        moniker: &Moniker,
    ) -> Result<(), ModelError> {
        match self.config_developer_overrides.lock().remove(moniker) {
            Some(_) => Ok(()),
            None => Err(ModelError::instance_not_found(moniker.clone())),
        }
    }

    /// Removes configuration overrides defined for any component in the realm
    /// defined by scope_moniker.
    pub async fn clear_config_developer_override(&self, scope_moniker: &Moniker) {
        let scoped_components = self
            .config_developer_overrides
            .lock()
            .keys()
            .filter(|k| k.has_prefix(scope_moniker))
            .cloned()
            .collect::<Vec<_>>();
        for scoped_component in scoped_components {
            match self.remove_config_developer_override(&scoped_component).await {
                Ok(()) => (),
                Err(e) => {
                    log::warn!("config overrides not found for component {scoped_component}: {e}")
                }
            }
        }
    }
}
