// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::ActionKey;
use crate::model::component::manager::ComponentManagerInstance;
use crate::model::component::{ComponentInstance, StartReason};
use crate::model::context::ModelContext;
use crate::model::start::Start;
use crate::model::token::InstanceRegistry;
use cm_config::RuntimeConfig;
use cm_types::Url;
use errors::ModelError;
use fuchsia_inspect::Inspector;
use log::warn;
use routing::bedrock::structured_dict::ComponentInput;
use std::sync::Arc;

/// Parameters for initializing a component model, particularly the root of the component
/// instance tree.
pub struct ModelParams {
    // TODO(viktard): Merge into RuntimeConfig
    /// The URL of the root component.
    pub root_component_url: Url,
    /// Global runtime configuration for the component_manager.
    pub runtime_config: Arc<RuntimeConfig>,
    /// The instance at the top of the tree, representing component manager.
    pub top_instance: Arc<ComponentManagerInstance>,
    /// The [`InstanceRegistry`] to attach to the model.
    pub instance_registry: Arc<InstanceRegistry>,
    /// The inspector instance which controls what information is exposed by component manager over
    /// inspect.
    pub inspector: Inspector,
    /// The execution scope to assign to components, for dependency injection in tests.
    /// If `None`, use the default.
    #[cfg(test)]
    pub scope_factory:
        Option<Box<dyn Fn() -> vfs::execution_scope::ExecutionScope + Send + Sync + 'static>>,
}

/// The component model holds authoritative state about a tree of component instances, including
/// each instance's identity, lifecycle, capabilities, and topological relationships.  It also
/// provides operations for instantiating, destroying, querying, and controlling component
/// instances at runtime.
pub struct Model {
    /// The instance at the top of the tree, i.e. the instance representing component manager
    /// itself.
    top_instance: Arc<ComponentManagerInstance>,
    /// The instance representing the root component. Owned by `top_instance`, but cached here for
    /// efficiency.
    root: Arc<ComponentInstance>,
    /// The context shared across the model.
    context: Arc<ModelContext>,
}

impl Model {
    /// Creates a new component model and initializes its topology.
    pub async fn new(
        params: ModelParams,
        root_component_input: ComponentInput,
    ) -> Result<Arc<Model>, ModelError> {
        let context = Arc::new(ModelContext::new(
            params.runtime_config,
            params.instance_registry,
            params.inspector,
            #[cfg(test)]
            params.scope_factory,
        )?);
        let root = ComponentInstance::new_root(
            root_component_input,
            context.clone(),
            Arc::downgrade(&params.top_instance),
            params.root_component_url,
        )
        .await;
        let top_instance = params.top_instance;
        top_instance.init(root.clone());
        Ok(Arc::new(Model { root, context, top_instance }))
    }

    /// Returns a reference to the instance at the top of the tree (component manager's own
    /// instance).
    pub fn top_instance(&self) -> &Arc<ComponentManagerInstance> {
        &self.top_instance
    }

    /// Returns a reference to the root component instance.
    pub fn root(&self) -> &Arc<ComponentInstance> {
        &self.root
    }

    pub fn context(&self) -> &ModelContext {
        &self.context
    }

    pub fn component_id_index(&self) -> &component_id_index::Index {
        self.context.component_id_index()
    }

    /// Starts root, starting the component tree.
    ///
    /// If `discover_root_component` has already been called, then `input_for_root` is unused.
    pub async fn start(self: &Arc<Model>) {
        // In debug mode, we don't start the component root. It must be started manually from
        // the lifecycle controller.
        if self.context.runtime_config().debug {
            warn!(
                "In debug mode, the root component will not be started. Use the LifecycleController
                protocol to start the root component."
            );
        } else {
            if let Err(e) = self.root.ensure_started(&StartReason::Root).await {
                // Starting root may take a long time as it will be resolving and starting
                // eager children. If graceful shutdown is initiated, that will cause those
                // children to fail to resolve or fail to start, and for `start` to fail.
                //
                // If we fail to start the root, but the root is being shutdown, or already
                // shutdown, that's ok. The system is tearing down, so it doesn't matter any more
                // if we never got everything started that we wanted to.
                if !self.root.actions().contains(ActionKey::Shutdown).await {
                    if !self.root.lock_state().await.is_shut_down() {
                        panic!(
                            "failed to start root component {}: {:?}",
                            self.root.component_url, e
                        );
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use crate::model::actions::{ActionsManager, ShutdownAction, ShutdownType};
    use crate::model::testing::test_helpers::{TestEnvironmentBuilder, TestModelResult};

    #[fuchsia::test]
    async fn already_shut_down_when_start_fails() {
        // Omit root declaration to force root component to fail starting
        let TestModelResult { model, .. } =
            TestEnvironmentBuilder::new().set_components(vec![]).build().await;

        let _ = ActionsManager::register(
            model.root.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .unwrap();

        model.start().await;
    }

    #[fuchsia::test]
    async fn shutting_down_when_start_fails() {
        // Omit root declaration to force root component to fail starting
        let TestModelResult { model, .. } =
            TestEnvironmentBuilder::new().set_components(vec![]).build().await;

        let _ = model
            .root()
            .actions()
            .register_no_wait(ShutdownAction::new(ShutdownType::Instance))
            .await;

        model.start().await;
    }

    #[should_panic]
    #[fuchsia::test]
    async fn not_shutting_down_when_start_fails() {
        // Omit root declaration to force root component to fail starting
        let TestModelResult { model, .. } =
            TestEnvironmentBuilder::new().set_components(vec![]).build().await;

        model.start().await;
    }
}
