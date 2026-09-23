// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{ComponentInstance, RouterError};
use ::routing::component_instance::TopInstanceInterface;
use anyhow::format_err;
use async_trait::async_trait;
use capability_source::{BuiltinCapabilities, CapabilitySource, NamespaceCapabilities};
use clonable_error::ClonableError;
use errors::RebootError;
use fidl::endpoints::{self};
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_hardware_power_statecontrol as fstatecontrol;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_component::client;
use fuchsia_sync::Mutex;
use log::warn;
use moniker::Moniker;
use routing::error::{ComponentInstanceError, RoutingError};
use runtime_capabilities::{Capability, Connector, Routable, Router, WeakInstanceToken};
use std::sync::Arc;
use vfs::directory::entry::OpenRequest;
use vfs::path::Path;
use vfs::{ExecutionScope, ToObjectRequest};

/// A special instance identified with component manager, at the top of the tree.
#[derive(Debug)]
pub struct ComponentManagerInstance {
    /// The list of capabilities offered from component manager's namespace.
    pub namespace_capabilities: NamespaceCapabilities,

    /// The list of capabilities offered from component manager as built-in capabilities.
    pub builtin_capabilities: BuiltinCapabilities,

    /// Scope owned by component manager's instance.
    execution_scope: ExecutionScope,

    /// Mutable state for component manager's instance.
    state: Mutex<ComponentManagerInstanceState>,
}

/// Mutable state for component manager's instance.
#[derive(Debug)]
pub struct ComponentManagerInstanceState {
    /// The root component instance, this instance's only child.
    root: Option<Arc<ComponentInstance>>,

    /// Task that is rebooting the system, if any.
    reboot_task: Option<fasync::Task<()>>,
}

impl ComponentManagerInstance {
    pub fn new(
        namespace_capabilities: NamespaceCapabilities,
        builtin_capabilities: BuiltinCapabilities,
    ) -> Self {
        Self {
            namespace_capabilities,
            builtin_capabilities,
            state: Mutex::new(ComponentManagerInstanceState::new()),
            execution_scope: ExecutionScope::new(),
        }
    }

    /// Returns a scope for component manager's instance
    pub fn execution_scope(&self) -> &ExecutionScope {
        &self.execution_scope
    }

    #[cfg(all(test, feature = "src_model_tests"))]
    pub fn has_reboot_task(&self) -> bool {
        self.state.lock().reboot_task.is_some()
    }

    /// Returns the root component instance.
    ///
    /// REQUIRES: The root has already been set. Otherwise panics.
    pub fn root(&self) -> Arc<ComponentInstance> {
        self.state.lock().root.as_ref().expect("root not set").clone()
    }

    /// Returns a connector that lazily resolves the given protocol exposed by `/`.
    ///
    /// REQUIRES: The root has already been set. Otherwise panics.
    pub fn get_root_exposed_capability_router(
        &self,
        source_name: cm_types::Name,
    ) -> impl Routable<Connector> + use<> {
        struct RootCapabilityRouter {
            root: Arc<ComponentInstance>,
            source_name: cm_types::Name,
        }

        impl RootCapabilityRouter {
            async fn get_router(&self) -> Result<Arc<Router<Connector>>, RouterError> {
                let component_output = self
                    .root
                    .lock_resolved_state()
                    .await
                    .map_err(|e| {
                        RoutingError::from(ComponentInstanceError::ResolveFailed {
                            moniker: Moniker::root(),
                            err: ClonableError::from(format_err!("{:?}", e)),
                        })
                    })?
                    .sandbox
                    .component_output
                    .clone();
                match component_output.capabilities().get(&self.source_name) {
                    Some(Capability::ConnectorRouter(router)) => Ok(router),
                    _ => Err(RouterError::NotFound(Arc::new(
                        RoutingError::UseFromRootExposeNotFound {
                            capability_id: self.source_name.to_string(),
                        },
                    ))),
                }
            }
        }

        #[async_trait]
        impl Routable<Connector> for RootCapabilityRouter {
            async fn route(
                &self,
                request: RouteRequest,
                target: Arc<WeakInstanceToken>,
            ) -> Result<Option<Arc<Connector>>, RouterError> {
                let router = self.get_router().await?;
                router.route(request, target).await
            }
            async fn route_debug(
                &self,
                request: RouteRequest,
                target: Arc<WeakInstanceToken>,
            ) -> Result<CapabilitySource, RouterError> {
                let router = self.get_router().await?;
                router.route_debug(request, target).await
            }
        }

        RootCapabilityRouter { root: self.root(), source_name }
    }

    /// Initializes the state of the instance. Panics if already initialized.
    pub fn init(&self, root: Arc<ComponentInstance>) {
        let mut state = self.state.lock();
        assert!(state.root.is_none(), "child of top instance already set");
        state.root = Some(root);
    }

    /// Triggers a graceful system reboot. Panics if the reboot call fails (which will trigger a
    /// forceful reboot if this is the root component manager instance).
    ///
    /// Returns as soon as the call has been made. In the background, component_manager will wait
    /// on the `Reboot` call.
    pub(super) fn trigger_reboot(self: &Arc<Self>) {
        let mut state = self.state.lock();
        if state.reboot_task.is_some() {
            // Reboot task was already scheduled, nothing to do.
            return;
        }
        let this = self.clone();
        state.reboot_task = Some(fasync::Task::spawn(async move {
            let res = async move {
                let statecontrol_proxy = this.connect_to_statecontrol_admin().await?;
                statecontrol_proxy
                    .shutdown(&fstatecontrol::ShutdownOptions {
                        action: Some(fstatecontrol::ShutdownAction::Reboot),
                        reasons: Some(vec![
                            fstatecontrol::ShutdownReason::CriticalComponentFailure,
                        ]),
                        ..Default::default()
                    })
                    .await?
                    .map_err(|s| RebootError::AdminError(zx::Status::err_from_raw(s)))
            }
            .await;
            if let Err(RebootError::AdminError(zx::Status::ALREADY_EXISTS)) = res {
                warn!(
                    "Reboot in progress but trigger_reboot is called again. Something else is going on."
                );
                return;
            }
            if let Err(e) = res {
                // TODO(https://fxbug.dev/42161535): Instead of panicking, we could fall back more gently by
                // triggering component_manager's shutdown.
                panic!(
                    "Component with on_terminate=REBOOT terminated, but triggering \
                    reboot failed. Crashing component_manager instead: {}",
                    e
                );
            }
        }));
    }

    /// Obtains a connection to power_manager's `statecontrol` protocol.
    async fn connect_to_statecontrol_admin(
        &self,
    ) -> Result<fstatecontrol::AdminProxy, RebootError> {
        let (exposed_dir, server) = endpoints::create_proxy::<fio::DirectoryMarker>();
        let root = self.root();
        const FLAGS: fio::Flags = fio::PERM_READABLE;
        let mut object_request = FLAGS.to_object_request(server);
        root.open_exposed(OpenRequest::new(
            root.execution_scope.clone(),
            FLAGS,
            Path::dot(),
            &mut object_request,
        ))
        .await?;
        let statecontrol_proxy =
            client::connect_to_protocol_at_dir_root::<fstatecontrol::AdminMarker>(&exposed_dir)
                .map_err(RebootError::ConnectToAdminFailed)?;
        Ok(statecontrol_proxy)
    }
}

impl ComponentManagerInstanceState {
    pub fn new() -> Self {
        Self { reboot_task: None, root: None }
    }
}

impl TopInstanceInterface for ComponentManagerInstance {
    fn namespace_capabilities(&self) -> &NamespaceCapabilities {
        &self.namespace_capabilities
    }

    fn builtin_capabilities(&self) -> &BuiltinCapabilities {
        &self.builtin_capabilities
    }
}
