// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::framework::{controller, resolve_with_pinned_url};
use crate::model::actions::{Action, ActionKey};
use crate::model::component::instance::{
    InstanceState, ResolvedInstanceState, StartedInstanceState,
};
use crate::model::component::{ComponentInstance, IncomingCapabilities, StartReason};
use crate::model::namespace::create_namespace;
use crate::model::program::{Program, StartInfo};
use crate::runner::RemoteRunner;
use ::namespace::Entry as NamespaceEntry;
use ::routing::DictExt;
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::error::RoutingError;
use async_trait::async_trait;
use cm_rust::ComponentDecl;
use cm_types::{Name, RelativePath};
use cm_util::{AbortError, AbortFutureExt, AbortHandle, AbortableScope};
use config_encoder::ConfigFields;
use errors::{
    ActionError, CreateNamespaceError, ResolveActionError, StartActionError, StructuredConfigError,
};
use fidl::Vmo;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_component_runtime::RouteRequest;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_mem as fmem;
use fidl_fuchsia_process as fprocess;
use fuchsia_runtime::{HandleInfo, HandleType};
use fuchsia_trace as trace;
use futures::channel::oneshot;
use hooks::{EventPayload, RuntimeInfo};
use log::warn;
use moniker::Moniker;
use num_traits::cast::FromPrimitive;
use router_error::RouterError;
use routing::bedrock::request_metadata::runner_metadata;
use runtime_capabilities::{
    Capability, Connector, Dictionary, DirConnector, Router, WeakInstanceToken,
};
use serve_processargs::NamespaceBuilder;
use std::sync::Arc;

/// Starts a component instance.
pub struct StartAction {
    start_reason: StartReason,
    execution_controller_task: Option<controller::ExecutionControllerTask>,
    incoming: IncomingCapabilities,
    abort_handle: AbortHandle,
    abortable_scope: AbortableScope,
}

impl StartAction {
    pub fn new(
        start_reason: StartReason,
        execution_controller_task: Option<controller::ExecutionControllerTask>,
        incoming: IncomingCapabilities,
    ) -> Self {
        let (abortable_scope, abort_handle) = AbortableScope::new();
        Self { start_reason, execution_controller_task, incoming, abort_handle, abortable_scope }
    }
}

#[async_trait]
impl Action for StartAction {
    async fn handle(self, component: Arc<ComponentInstance>) -> Result<(), ActionError> {
        do_start(
            &component,
            &self.start_reason,
            self.execution_controller_task,
            self.incoming,
            self.abortable_scope,
        )
        .await
        .map_err(Into::into)
    }

    fn key(&self) -> ActionKey {
        ActionKey::Start
    }

    fn abort_handle(&self) -> Option<AbortHandle> {
        Some(self.abort_handle.clone())
    }
}

struct StartContext {
    runner: Option<RemoteRunner>,
    namespace_builder: NamespaceBuilder,
    numbered_handles: Vec<fprocess::HandleInfo>,
    encoded_config: Option<fmem::Data>,
    start_reason: StartReason,
    execution_controller_task: Option<controller::ExecutionControllerTask>,
}

fn open_protocols_with_numbered_handle(
    resolved_instance_state: &ResolvedInstanceState,
    decl: &cm_rust::ComponentDecl,
    target: Arc<WeakInstanceToken>,
) -> Vec<fprocess::HandleInfo> {
    let numbered_handle_dict = resolved_instance_state.sandbox.program_input.numbered_handles();
    decl.uses
        .iter()
        .filter_map(|use_| match use_ {
            cm_rust::UseDecl::Protocol(cm_rust::UseProtocolDecl {
                numbered_handle: Some(numbered_handle),
                ..
            }) => {
                fn handle_info_from(
                    client: fidl::Channel,
                    numbered_handle: &cm_types::HandleType,
                ) -> Option<fprocess::HandleInfo> {
                    Some(fprocess::HandleInfo {
                        handle: client.into(),
                        id: HandleInfo::from(HandleType::from_u8((*numbered_handle).into())?)
                            .as_raw(),
                    })
                }
                let numbered_handle_key = Name::from(*numbered_handle);
                let cap = numbered_handle_dict.get_capability(&numbered_handle_key);
                match cap {
                    Some(Capability::ConnectorRouter(router)) => {
                        let scope = &resolved_instance_state.execution_scope;
                        let (client, server) = fidl::Channel::create();
                        let target = target.clone();
                        scope.spawn(async move {
                            // the router should handle logging in the event of an error
                            if let Ok(Some(c)) = router.route(RouteRequest::default(), target).await
                            {
                                let _ = c.send(server);
                            }
                        });
                        handle_info_from(client, numbered_handle)
                    }
                    Some(Capability::Connector(c)) => {
                        let (client, server) = fidl::Channel::create();
                        let _ = c.send(server);
                        handle_info_from(client, numbered_handle)
                    }
                    Some(_) => {
                        warn!(
                            "open_protocols_with_numbered_handle: Protocol capability was not a \
                             ConnectorRouter. This should never happen."
                        );
                        None
                    }
                    None => {
                        warn!(
                            "open_protocols_with_numbered_handle: {:?} not found in \
                            program_input_dict. This should never happen.",
                            numbered_handle
                        );
                        None
                    }
                }
            }
            _ => None,
        })
        .collect()
}

async fn do_start(
    component: &Arc<ComponentInstance>,
    start_reason: &StartReason,
    execution_controller_task: Option<controller::ExecutionControllerTask>,
    mut incoming: IncomingCapabilities,
    abortable_scope: AbortableScope,
) -> Result<(), StartActionError> {
    trace::duration!("component_manager", "Actions::Start", "moniker" => component.moniker.as_str());

    // Translates the error when a long running future is aborted.
    let abort_error =
        |_: AbortError| StartActionError::Aborted { moniker: component.moniker.clone() };

    // Resolve the component and find the runner to use.
    let (runner_router, runner_name, resolved_component, mut program_input_dict, decl) = {
        trace::duration!("component_manager", "Actions::Start::resolve_runner", "moniker" => component.moniker.as_str());

        // Obtain a clone of the decl under a short lock. If the decl has
        // already been dropped from the ResolvedInstanceState, then we will
        // need to reacquire the lock in order to reresolve the decl.  As a
        // result, we keep the lock tightly scoped here.
        let maybe_decl = {
            let resolved_state = component
                .lock_resolved_state()
                .with(&abortable_scope)
                .await
                .map_err(abort_error)?
                .map_err(|err| StartActionError::ResolveActionError {
                    moniker: component.moniker.clone(),
                    err: Box::new(err),
                })?;
            resolved_state.decl().cloned()
        };
        let decl = match maybe_decl {
            Some(decl) => decl,
            None => resolve_with_pinned_url(component).await.map_err(|err| {
                StartActionError::ResolveActionError {
                    moniker: component.moniker.clone(),
                    err: Box::new(ActionError::from(ResolveActionError::from(err))),
                }
            })?,
        };
        let resolved_state = component
            .lock_resolved_state()
            .with(&abortable_scope)
            .await
            .map_err(abort_error)?
            .map_err(|err| StartActionError::ResolveActionError {
                moniker: component.moniker.clone(),
                err: Box::new(err),
            })?;
        let mut numbered_handles = open_protocols_with_numbered_handle(
            &resolved_state,
            &decl,
            component.clone().as_weak().into(),
        );
        incoming.numbered_handles.append(&mut numbered_handles);

        let runner = resolved_state.sandbox.program_input.runner();
        (
            runner,
            decl.get_runner().as_ref().map(|r| r.source_name.clone()),
            resolved_state.resolved_component.clone(),
            resolved_state.sandbox.program_input.namespace(),
            decl,
        )
    };
    let runner = match runner_router {
        Some(runner_router) => open_runner(
            component,
            runner_router,
            runner_name.expect(
                "runner is set in sandbox but is missing use decl, this should be impossible",
            ),
        )
        .with(&abortable_scope)
        .await
        .map_err(abort_error)?
        .map_err(|err| {
            warn!(moniker:% = component.moniker, err:%; "Failed to resolve runner.");
            err
        })?,
        None => None,
    };

    let IncomingCapabilities {
        numbered_handles,
        additional_namespace_entries,
        dict: program_input_dict_additions,
    } = incoming;

    // Create the component's namespace. If there are additions, it's necessary to install the
    // additions into a copy of the `program_input_dict` so the original remains unchanged if the
    // component is started again.
    if let Some(additions) = program_input_dict_additions {
        program_input_dict = dict_deep_copy(&program_input_dict);
        dict_merge(&program_input_dict, additions);
    }
    let mut namespace_builder = create_namespace(
        resolved_component.package.as_ref(),
        component,
        &decl.uses,
        &program_input_dict,
        component.execution_scope.clone(),
    )
    .await
    .map_err(|err| StartActionError::CreateNamespaceError(err))?;
    for NamespaceEntry { directory, path } in additional_namespace_entries {
        let dir_connector = DirConnector::from_proxy(
            directory.into_proxy(),
            RelativePath::dot(),
            fio::PERM_READABLE | fio::Flags::PERM_INHERIT_WRITE | fio::Flags::PERM_INHERIT_EXECUTE,
        );
        namespace_builder.add_entry(dir_connector.into(), &path).map_err(|err| {
            StartActionError::CreateNamespaceError(CreateNamespaceError::BuildNamespaceError {
                moniker: component.moniker.clone(),
                err,
            })
        })?;
    }

    // Check policy.
    // TODO(https://fxbug.dev/42071809): Consider moving this check to ComponentInstance::add_child
    match component.on_terminate {
        fdecl::OnTerminate::Reboot => {
            component.policy_checker().reboot_on_terminate_allowed(&component.moniker).map_err(
                |err| StartActionError::RebootOnTerminateForbidden {
                    moniker: component.moniker.clone(),
                    err,
                },
            )?;
        }
        fdecl::OnTerminate::None => {}
    }

    let encoded_config = match decl.config {
        None => None,
        Some(ref config_decl) => match config_decl.value_source {
            cm_rust::ConfigValueSource::PackagePath(_) => {
                let Some(mut config) = resolved_component.config.clone() else {
                    return Err(StartActionError::StructuredConfigError {
                        moniker: component.moniker.clone(),
                        err: StructuredConfigError::ConfigValuesMissing,
                    });
                };
                update_config_fields(Arc::make_mut(&mut config), &decl, &component)
                    .with(&abortable_scope)
                    .await
                    .map_err(abort_error)??;
                update_component_config(&component, config.clone()).await?;
                Some(encode_config(config, &component.moniker).await?)
            }
            cm_rust::ConfigValueSource::Capabilities(_) => {
                let config = create_config_with_capabilities(&decl, &component)
                    .with(&abortable_scope)
                    .await
                    .map_err(abort_error)??;
                match config {
                    Some(c) => {
                        let arc_config = Arc::new(c);
                        update_component_config(&component, Arc::clone(&arc_config)).await?;
                        Some(encode_config(arc_config, &component.moniker).await?)
                    }
                    None => None,
                }
            }
        },
    };
    let start_context = StartContext {
        runner,
        namespace_builder,
        numbered_handles,
        encoded_config,
        start_reason: start_reason.clone(),
        execution_controller_task,
    };

    start_component(&component, decl.into(), start_context).await
}

fn dict_deep_copy(dict: &Arc<Dictionary>) -> Arc<Dictionary> {
    let out = Dictionary::new();
    for (key, cap) in dict.enumerate() {
        match cap {
            Capability::Dictionary(d) => {
                let d = dict_deep_copy(&d);
                let _ = out.insert(key, Capability::Dictionary(d));
            }
            cap => {
                let _ = out.insert(key, cap);
            }
        }
    }
    out
}

fn dict_merge(dict1: &Arc<Dictionary>, dict2: Arc<Dictionary>) {
    for (key, value) in dict2.drain() {
        match value {
            Capability::Dictionary(inner_dict2) => {
                if let Some(Capability::Dictionary(inner_dict1)) = dict1.get(&key) {
                    dict_merge(&inner_dict1, inner_dict2);
                } else {
                    let _ = dict1.insert(key, Capability::Dictionary(inner_dict2));
                }
            }
            value => {
                let _ = dict1.insert(key, value);
            }
        }
    }
}

/// Set the Runtime in the Execution and start the exit watcher. From component manager's
/// perspective, this indicates that the component has started. If this returns an error, the
/// component was shut down and the Runtime is not set, otherwise the function returns the
/// start context with the runtime set. This function acquires the state and execution locks on
/// `Component`.
async fn start_component(
    component: &Arc<ComponentInstance>,
    decl: Arc<ComponentDecl>,
    start_context: StartContext,
) -> Result<(), StartActionError> {
    let runtime_info;
    let timestamp;
    let runtime_dir;
    let break_on_start_left;
    let break_on_start_right;

    let escrowed_state_opt =
        if start_context.runner.is_some() {
            let actor_handle = {
                let state = component.lock_state().await;
                if let Some(r) = should_return_early(&state, &component.moniker) {
                    return r;
                }
                state.get_resolved_state().and_then(|r| r.program_escrow().cloned()).ok_or(
                    StartActionError::InstanceDestroyed { moniker: component.moniker.clone() },
                )?
            };

            Some(actor_handle.will_start().await.ok_or(StartActionError::InstanceDestroyed {
                moniker: component.moniker.clone(),
            })?)
        } else {
            None
        };

    {
        let mut state = component.lock_state().await;

        if let Some(r) = should_return_early(&state, &component.moniker) {
            return r;
        }

        let (diagnostics_sender, diagnostics_receiver) = oneshot::channel();
        (break_on_start_left, break_on_start_right) = zx::EventPair::create();

        let StartContext {
            runner,
            namespace_builder,
            numbered_handles,
            encoded_config,
            start_reason,
            execution_controller_task,
        } = start_context;

        let program = if let Some(runner) = runner {
            let moniker = &component.moniker;
            let component_instance = state
                .instance_token(moniker, &component.context)
                .ok_or(StartActionError::InstanceDestroyed { moniker: moniker.clone() })?;

            let start_info = StartInfo {
                url: component.component_url.clone().into(),
                program: decl
                    .program
                    .as_ref()
                    .map(|p| p.info.clone())
                    .unwrap_or_else(|| fdata::Dictionary::default()),
                namespace: namespace_builder,
                numbered_handles,
                encoded_config,
                break_on_start: Some(break_on_start_left),
                component_instance,
            };

            let escrowed_state = escrowed_state_opt.unwrap();
            let program = Program::start(
                &runner,
                start_info,
                escrowed_state,
                diagnostics_sender,
                component.moniker.clone(),
                component.context.escrow_stats_node(),
            )
            .map_err(|err| StartActionError::StartProgramError { moniker: moniker.clone(), err })?;

            Some(program)
        } else {
            None
        };

        let started = StartedInstanceState::new(
            program,
            component.as_weak(),
            start_reason,
            execution_controller_task,
        );
        timestamp = started.timestamp;
        let timestamp_monotonic = started.timestamp_monotonic;
        runtime_info = RuntimeInfo::new(timestamp, timestamp_monotonic, diagnostics_receiver);
        runtime_dir = started.runtime_dir().cloned();

        state.replace(|instance_state| match instance_state {
            #[cfg(not(test))]
            InstanceState::Resolved(mut resolved) => {
                // Drop the decl now that it's no longer needed.
                let decl = resolved.resolved_component.decl.take();
                match decl {
                    Some(d) => {
                        // Put the decl back if it has collections, so that
                        // dynamic children can have their manifest validated
                        // against their parent (this component).
                        if d.collections.len() > 0 {
                            resolved.resolved_component.decl = Some(d);
                        }
                    }
                    None => (),
                }
                InstanceState::Started(resolved, started)
            }
            #[cfg(test)]
            InstanceState::Resolved(resolved) => InstanceState::Started(resolved, started),
            other_state => panic!("starting an unresolved component: {:?}", other_state),
        });
    }

    // Dispatch Started and DebugStarted events outside of the state lock, but under the
    // actions lock, so that:
    //
    // - Hooks implementations can use the state (such as re-entrantly ensuring the component is
    //   started).
    // - The Started events will be ordered before any other component lifecycle transitions.
    //
    component
        .hooks
        .dispatch(&component.new_event_with_timestamp(
            EventPayload::Started { runtime: Box::new(runtime_info) },
            timestamp,
        ))
        .await;
    component
        .hooks
        .dispatch(&component.new_event_with_timestamp(
            EventPayload::DebugStarted {
                runtime_dir,
                break_on_start: Arc::new(break_on_start_right),
            },
            timestamp,
        ))
        .await;

    Ok(())
}

/// Determines if `start` should return early. Returns the following values:
/// - None: `start` should not return early because the component is not currently running
/// - Some(Ok(())): `start` should return early because the component has already been started
/// - Some(Err(err)): `start` should return early because it will be unable to start the component
/// due to `err` (for example, perhaps the component is destroyed or shut down).
pub fn should_return_early(
    component: &InstanceState,
    moniker: &Moniker,
) -> Option<Result<(), StartActionError>> {
    match component {
        InstanceState::Started(_, _) => Some(Ok(())),
        InstanceState::Unresolved(_) | InstanceState::Resolved(_) => None,
        InstanceState::Shutdown(_, _) => {
            Some(Err(StartActionError::InstanceShutDown { moniker: moniker.clone() }))
        }
        InstanceState::Destroyed => {
            Some(Err(StartActionError::InstanceDestroyed { moniker: moniker.clone() }))
        }
    }
}

/// Returns a RemoteRunner routed to the component's runner, if it specifies one.
///
/// Returns None if the component's decl does not specify a runner.
async fn open_runner(
    component: &Arc<ComponentInstance>,
    runner_router: Arc<Router<Connector>>,
    runner_name: Name,
) -> Result<Option<RemoteRunner>, StartActionError> {
    // Open up a channel to the runner.
    let request = runner_metadata(cm_rust::Availability::Required);
    let runner_capability = runner_router
        .route(request, component.as_weak().into())
        .await
        .map_err(|err| StartActionError::ResolveRunnerError {
            moniker: component.moniker.clone(),
            err: Box::new(err),
            runner: runner_name.clone(),
        })?;
    match &runner_capability {
        // Built-in runners are hosted by a LaunchTaskOnReceive, which returns a Connector
        // capability for new routes.
        Some(runner_connector) => {
            let (proxy, server_end) = create_proxy::<fcrunner::ComponentRunnerMarker>();
            runner_connector.send(server_end.into_channel()).map_err(|_| {
                StartActionError::ResolveRunnerError {
                    moniker: component.moniker.clone(),
                    err: Box::new(RouterError::from(RoutingError::BedrockFailedToSend {
                        moniker: component.moniker.clone().into(),
                        capability_id: runner_name.to_string(),
                    })),
                    runner: runner_name.clone(),
                }
            })?;
            Ok(Some(RemoteRunner::new(proxy)))
        }
        _ => Err(StartActionError::ResolveRunnerError {
            moniker: component.moniker.clone(),
            err: Box::new(
                RoutingError::BedrockWrongCapabilityType {
                    actual: "Debug".into(),
                    expected: "Connector".into(),
                    moniker: component.moniker.clone().into(),
                }
                .into(),
            ),
            runner: runner_name,
        }),
    }
}

/// Returns true if the decl uses configuration capabilities.
fn has_config_capabilities(decl: &cm_rust::ComponentDecl) -> bool {
    decl.uses.iter().find(|u| matches!(u, cm_rust::UseDecl::Config(_))).is_some()
}

/// Update the component's configuration fields.
async fn update_component_config(
    component: &Arc<ComponentInstance>,
    config: Arc<ConfigFields>,
) -> Result<(), StartActionError> {
    let mut resolved_state = component.lock_resolved_state().await.unwrap();
    resolved_state.resolved_component.config = Some(config);
    Ok(())
}

async fn create_config_with_capabilities(
    decl: &cm_rust::ComponentDecl,
    component: &Arc<ComponentInstance>,
) -> Result<Option<ConfigFields>, StartActionError> {
    let Some(ref config_decl) = decl.config else {
        return Ok(None);
    };

    let mut fields = ConfigFields {
        fields: Vec::with_capacity(config_decl.fields.len()),
        checksum: config_decl.checksum.clone(),
    };
    let mut overrides = component.context.get_config_developer_overrides(component.moniker()).await;
    for field in &config_decl.fields {
        let Some(use_config) = ::routing::config::get_use_config_from_key(&field.key, decl) else {
            return Err(StartActionError::StructuredConfigError {
                moniker: component.moniker.clone(),
                err: StructuredConfigError::KeyNotFound { key: field.key.to_string() },
            });
        };
        let value = match ::routing::config::route_config_value(use_config, component).await {
            Ok(Some(v)) => v,
            Ok(None) => {
                return Err(StartActionError::StructuredConfigError {
                    moniker: component.moniker.clone(),
                    err: StructuredConfigError::KeyNotFound { key: field.key.to_string() },
                });
            }
            Err(err) => {
                return Err(StartActionError::StructuredConfigError {
                    moniker: component.moniker.clone(),
                    err: err.into(),
                });
            }
        };
        // If developer overrides have been set for this field, use the override value.
        let value = if let Some(v) = overrides.remove(&field.key) { v } else { value };

        let field = config_encoder::ConfigField::resolve(value, field).map_err(|e| {
            StartActionError::StructuredConfigError {
                moniker: component.moniker.clone(),
                err: StructuredConfigError::ConfigResolutionFailed(
                    config_encoder::ResolutionError::InvalidValue {
                        key: field.key.clone(),
                        source: e,
                    },
                ),
            }
        })?;
        fields.fields.push(field);
    }

    Ok(Some(fields))
}

/// Update config fields with the values received through configuration
/// capabilities or developer overrides.  If there is no developer override
/// present for a field, this will perform routing on each of the configuration
/// `use` decls to get the values. Updating the fields is fine because developer
/// overrides and configuration capabilities take precedence over both the CVF
/// value and "mutability: parent".
async fn update_config_fields(
    config: &mut ConfigFields,
    decl: &cm_rust::ComponentDecl,
    component: &Arc<ComponentInstance>,
) -> Result<(), StartActionError> {
    let mut overrides = component.context.get_config_developer_overrides(component.moniker()).await;
    let has_config_caps = has_config_capabilities(decl);
    for field in config.fields.iter_mut() {
        // If developer overrides have been set for this field, use the override
        // value. Otherwise attempt to route the configuration capability.
        let value = if let Some(v) = overrides.remove(&field.key) {
            v
        } else {
            if !has_config_caps {
                continue;
            }
            let Some(use_config) = ::routing::config::get_use_config_from_key(&field.key, decl)
            else {
                continue;
            };
            let value = ::routing::config::route_config_value(use_config, component)
                .await
                .map_err(|e| StartActionError::StructuredConfigError {
                    moniker: component.moniker.clone(),
                    err: e.into(),
                })?;
            let Some(value) = value else {
                continue;
            };
            value
        };

        if !field.value.matches_type(&value) {
            return Err(StartActionError::StructuredConfigError {
                moniker: component.moniker.clone(),
                err: StructuredConfigError::ValueMismatch { key: field.key.to_string() },
            });
        }
        field.value = value;
    }
    Ok(())
}

/// Encode the configuration into a VMO.
async fn encode_config(
    config: Arc<ConfigFields>,
    moniker: &Moniker,
) -> Result<fmem::Data, StartActionError> {
    let (vmo, size) = (|| {
        let encoded = (*config).clone().encode_as_fidl_struct();
        let size = encoded.len() as u64;
        let vmo = Vmo::create(size)?;
        vmo.write(&encoded, 0)?;
        Ok((vmo, size))
    })()
    .map_err(|s| StartActionError::StructuredConfigError {
        err: StructuredConfigError::VmoCreateFailed(s),
        moniker: moniker.clone(),
    })?;
    Ok(fmem::Data::Buffer(fmem::Buffer { vmo, size }))
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::actions::{ActionsManager, ShutdownAction, ShutdownType, StopAction};
    use crate::model::component::instance::{ResolvedInstanceState, UnresolvedInstanceState};
    use crate::model::component::{Component, WeakComponentInstance};
    use crate::model::testing::routing_test_helpers::RoutingTestBuilder;
    use crate::model::testing::test_helpers::{self, ActionsTest};
    use crate::model::testing::test_hook::Lifecycle;
    use ::routing::bedrock::structured_dict::ComponentInput;
    use ::routing::resolving::ComponentAddress;
    use assert_matches::assert_matches;
    use async_trait::async_trait;
    use cm_rust::push_box;
    use cm_rust_testing::{ChildBuilder, ComponentDeclBuilder};
    use directed_graph::DirectedGraph;
    use errors::{ActionErrorKind, ModelError};
    use fuchsia_async as fasync;
    use fuchsia_sync::Mutex;
    use futures::channel::mpsc;
    use futures::stream::FuturesUnordered;
    use futures::{FutureExt, StreamExt};
    use hooks::{Event, EventType, Hook, HooksRegistration};
    use rand::seq::SliceRandom as _;
    use std::sync::Weak;

    // Child name for test child components instantiated during tests.
    const TEST_CHILD_NAME: &str = "child";

    struct ShutdownOnStartHook {
        component: Arc<ComponentInstance>,
        done: Mutex<mpsc::Sender<()>>,
    }

    #[async_trait]
    impl Hook for ShutdownOnStartHook {
        async fn on(self: Arc<Self>, _event: &Event) -> Result<(), ModelError> {
            fasync::Task::spawn(async move {
                ActionsManager::register(
                    self.component.clone(),
                    ShutdownAction::new(ShutdownType::Instance),
                )
                .await
                .expect("shutdown failed");
                self.done.lock().try_send(()).unwrap();
            })
            .detach();
            Ok(())
        }
    }

    #[fuchsia::test]
    /// Validate that if a start action is issued and the component shuts down
    /// the action completes we see a Stop event emitted.
    async fn start_issues_shutdown() {
        let (test_topology, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;
        let (done, mut hook_done_receiver) = mpsc::channel(1);
        let start_hook =
            Arc::new(ShutdownOnStartHook { component: child.clone(), done: Mutex::new(done) });
        child.hooks.install(vec![HooksRegistration::new(
            "my_start_hook",
            vec![EventType::Started],
            Arc::downgrade(&start_hook) as Weak<dyn Hook>,
        )]);

        ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await
        .unwrap();

        // Wait until the action in the hook is done.
        hook_done_receiver.next().await.unwrap();

        let events: Vec<_> = test_topology
            .test_hook
            .lifecycle()
            .into_iter()
            .filter(|event| match event {
                Lifecycle::Start(_) | Lifecycle::Stop(_) => true,
                _ => false,
            })
            .collect();
        assert_eq!(
            events,
            vec![
                Lifecycle::Start([TEST_CHILD_NAME].try_into().unwrap()),
                Lifecycle::Stop([TEST_CHILD_NAME].try_into().unwrap())
            ]
        );
    }

    struct StopOnStartHook {
        component: Arc<ComponentInstance>,
        done: Mutex<mpsc::Sender<()>>,
    }

    #[async_trait]
    impl Hook for StopOnStartHook {
        async fn on(self: Arc<Self>, _event: &Event) -> Result<(), ModelError> {
            fasync::Task::spawn(async move {
                ActionsManager::register(self.component.clone(), StopAction::new(false))
                    .await
                    .expect("stop failed");
                self.done.lock().try_send(()).unwrap();
            })
            .detach();
            Ok(())
        }
    }

    #[fuchsia::test]
    /// Validate that if a start action is issued and the component stops
    /// the action completes we see a Stop event emitted.
    async fn start_issues_stop() {
        let (test_topology, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;
        let (done, mut hook_done_receiver) = mpsc::channel(1);
        let start_hook =
            Arc::new(StopOnStartHook { component: child.clone(), done: Mutex::new(done) });
        child.hooks.install(vec![HooksRegistration::new(
            "my_start_hook",
            vec![EventType::Started],
            Arc::downgrade(&start_hook) as Weak<dyn Hook>,
        )]);

        ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await
        .unwrap();

        // Wait until the action in the hook is done.
        hook_done_receiver.next().await.unwrap();

        let events: Vec<_> = test_topology
            .test_hook
            .lifecycle()
            .into_iter()
            .filter(|event| match event {
                Lifecycle::Start(_) | Lifecycle::Stop(_) => true,
                _ => false,
            })
            .collect();
        assert_eq!(
            events,
            vec![
                Lifecycle::Start([TEST_CHILD_NAME].try_into().unwrap()),
                Lifecycle::Stop([TEST_CHILD_NAME].try_into().unwrap())
            ]
        );
    }

    #[fuchsia::test]
    /// If start and stop happens concurrently, the component should either end up as
    /// started or stopped, without deadlocking.
    async fn concurrent_start_stop() {
        let (_test_topology, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;

        // Run start and stop in random order.
        let start_fut = ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        );
        let stop_fut = ActionsManager::register(child.clone(), StopAction::new(false));
        let mut futs = vec![start_fut.boxed(), stop_fut.boxed()];
        futs.shuffle(&mut rand::rng());
        let stream: FuturesUnordered<_> = futs.into_iter().collect();
        let _: Vec<_> = stream.collect().await;

        // Both actions have completed, which demonstrates that the component did not deadlock.
    }

    /// If start is blocked during resolving then stop can interrupt it.
    #[fuchsia::test]
    async fn start_aborted_by_stop() {
        let root_name = "root";
        let components = vec![
            (root_name, ComponentDeclBuilder::new().child_default(TEST_CHILD_NAME).build()),
            (TEST_CHILD_NAME, test_helpers::component_decl_with_test_runner()),
        ];
        let builder = RoutingTestBuilder::new(components[0].0, components);

        // Add a blocker to the resolver that will cause resolving the child to block.
        let (resolved_tx, resolved_rx) = oneshot::channel::<()>();
        let (continue_tx, continue_rx) = oneshot::channel::<()>();
        let test_topology =
            builder.add_blocker(TEST_CHILD_NAME, resolved_tx, continue_rx).build().await;

        let _root =
            test_topology.model.root().find_and_maybe_resolve(&Moniker::default()).await.unwrap();
        let child =
            test_topology.model.root().find(&TEST_CHILD_NAME.try_into().unwrap()).await.unwrap();

        let start_fut = child
            .actions()
            .register_no_wait(StartAction::new(
                StartReason::Debug,
                None,
                IncomingCapabilities::default(),
            ))
            .await;

        // Wait until start is blocked.
        resolved_rx.await.unwrap();

        // Stop should cancel start.
        let stop_fut = child.actions().register_no_wait(StopAction::new(false)).await;
        assert_matches!(
            start_fut.await.unwrap_err().kind(),
            ActionErrorKind::StartError { err: StartActionError::Aborted { .. } }
        );

        continue_tx.send(()).unwrap();
        stop_fut.await.unwrap();
    }

    #[fuchsia::test]
    async fn start_type_checks_config_capabilities() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .child_default(TEST_CHILD_NAME)
                    .capability(
                        cm_rust_testing::CapabilityBuilder::config()
                            .name("fuchsia.MyConfig")
                            .value(cm_rust::ConfigSingleValue::Bool(false).into()),
                    )
                    .offer(
                        cm_rust_testing::OfferBuilder::config()
                            .name("fuchsia.MyConfig")
                            .source(cm_rust::offer::OfferSource::Self_)
                            .target(cm_rust::offer::OfferTarget::Child(cm_rust::ChildRef {
                                name: TEST_CHILD_NAME.parse().unwrap(),
                                collection: None,
                            })),
                    )
                    .build(),
            ),
            (
                TEST_CHILD_NAME,
                ComponentDeclBuilder::new()
                    .config(cm_rust::ConfigDecl {
                        fields: Box::from([cm_rust::ConfigField {
                            key: "my_config".into(),
                            type_: cm_rust::ConfigValueType::Int8,
                            mutability: Default::default(),
                        }]),
                        checksum: cm_rust::ConfigChecksum::Sha256([0; 32]),
                        value_source: cm_rust::ConfigValueSource::Capabilities(
                            cm_rust::ConfigSourceCapabilities {},
                        ),
                    })
                    .use_(
                        cm_rust_testing::UseBuilder::config()
                            .source(cm_rust::UseSource::Parent)
                            .name("fuchsia.MyConfig")
                            .target_name("my_config")
                            .config_type(cm_rust::ConfigValueType::Int8),
                    )
                    .build(),
            ),
        ];

        let test_topology = ActionsTest::new(components[0].0, components, None).await;
        let child = test_topology.look_up([TEST_CHILD_NAME].try_into().unwrap()).await;

        let res = ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await;

        assert_matches!(
            res.unwrap_err().kind(),
            ActionErrorKind::StartError{
                err: StartActionError::StructuredConfigError{
                    moniker,
                    err: StructuredConfigError::ConfigResolutionFailed (
                        config_encoder::ResolutionError::InvalidValue {
                            key,
                            source: config_encoder::ValueError::TypeMismatch{
                                expected,
                                received,
                            }
                        }

                    )
                }
            }
            if moniker == &"child".parse().unwrap()
                && key == "my_config"
                && expected == "Int8"
                && received == "Single(Bool(false))"
        );
    }

    #[fuchsia::test]
    async fn restart_set_execution_runtime() {
        let (_test_harness, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;

        {
            let timestamp = zx::BootInstant::get();
            ActionsManager::register(
                child.clone(),
                StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
            )
            .await
            .expect("failed to start child");
            let state = child.lock_state().await;
            let runtime = state.get_started_state().expect("child runtime is unexpectedly empty");
            assert!(runtime.timestamp > timestamp);
        }

        {
            ActionsManager::register(child.clone(), StopAction::new(false))
                .await
                .expect("failed to stop child");
            let state = child.lock_state().await;
            assert!(state.get_started_state().is_none());
        }

        {
            let timestamp = zx::BootInstant::get();
            ActionsManager::register(
                child.clone(),
                StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
            )
            .await
            .expect("failed to start child");
            let state = child.lock_state().await;
            let runtime = state.get_started_state().expect("child runtime is unexpectedly empty");
            assert!(runtime.timestamp > timestamp);
        }
    }

    #[fuchsia::test]
    async fn restart_does_not_refresh_resolved_state() {
        let (test_harness, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;

        {
            let timestamp = zx::BootInstant::get();
            ActionsManager::register(
                child.clone(),
                StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
            )
            .await
            .expect("failed to start child");
            let state = child.lock_state().await;
            let runtime = state.get_started_state().expect("child runtime is unexpectedly empty");
            assert!(runtime.timestamp > timestamp);
        }

        {
            let () = ActionsManager::register(child.clone(), StopAction::new(false))
                .await
                .expect("failed to stop child");
            let state = child.lock_state().await;
            assert!(state.get_started_state().is_none());
        }

        let original_decl = test_harness
            .resolver
            .get_component_decl(TEST_CHILD_NAME)
            .expect("child decl not stored");
        let mut modified_decl = original_decl.clone();
        push_box(&mut modified_decl.children, ChildBuilder::new().name("foo").build());
        test_harness.resolver.add_component(TEST_CHILD_NAME, modified_decl.clone());

        ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await
        .expect("failed to start child");

        let resolved_decl = get_resolved_decl(&child).await;
        assert_ne!(resolved_decl, modified_decl);
        assert_eq!(resolved_decl, original_decl);
    }

    async fn build_tree_with_single_child(
        child_name: &'static str,
    ) -> (ActionsTest, Arc<ComponentInstance>) {
        let root_name = "root";
        let components = vec![
            (root_name, ComponentDeclBuilder::new().child_default(child_name).build()),
            (child_name, test_helpers::component_decl_with_test_runner()),
        ];
        let test_topology = ActionsTest::new(components[0].0, components, None).await;

        let child = test_topology.look_up([child_name].try_into().unwrap()).await;

        (test_topology, child)
    }

    async fn get_resolved_decl(component: &Arc<ComponentInstance>) -> ComponentDecl {
        let state = component.lock_state().await;
        state.get_resolved_state().unwrap().decl().unwrap().clone()
    }

    #[fuchsia::test]
    async fn check_should_return_early() {
        let m = Moniker::try_from(["foo"]).unwrap();

        // Checks based on InstanceState:
        assert!(
            should_return_early(
                &InstanceState::Unresolved(UnresolvedInstanceState::new(ComponentInput::default())),
                &m
            )
            .is_none()
        );
        assert_matches!(
            should_return_early(&InstanceState::Destroyed, &m),
            Some(Err(StartActionError::InstanceDestroyed { moniker: _ }))
        );
        let (_, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;
        let decl = ComponentDeclBuilder::new().child_default(TEST_CHILD_NAME).build();
        let resolved_component = Component {
            context_to_resolve_children: None,
            decl: Some(Arc::new(decl)),
            package: None,
            config: None,
            abi_revision: None,
            dependencies: Arc::new(DirectedGraph::new()),
        };
        let ris = ResolvedInstanceState::new(
            &child,
            resolved_component.clone(),
            ComponentAddress::from_absolute_url(&child.component_url).unwrap().into(),
            Default::default(),
            ComponentInput::default(),
        )
        .await
        .unwrap();
        assert!(should_return_early(&InstanceState::Resolved(Box::new(ris)), &m).is_none());

        // Check for already_started:
        {
            let ris = ResolvedInstanceState::new(
                &child,
                resolved_component,
                ComponentAddress::from_absolute_url(&child.component_url).unwrap().into(),
                Default::default(),
                ComponentInput::default(),
            )
            .await
            .unwrap();
            let started_state = StartedInstanceState::new(
                None,
                WeakComponentInstance::invalid(),
                StartReason::Debug,
                None,
            );
            assert_matches!(
                should_return_early(&InstanceState::Started(Box::new(ris), started_state), &m),
                Some(Ok(()))
            );
        }

        // Check for shut_down:
        let _ = child.stop_instance_internal(true).await;
        assert!(child.lock_state().await.is_shut_down());
        let state = child.lock_state().await;
        assert_matches!(
            should_return_early(&*state, &m),
            Some(Err(StartActionError::InstanceShutDown { moniker: _ }))
        );
    }

    #[fuchsia::test]
    async fn check_already_started() {
        let (_test_harness, child) = build_tree_with_single_child(TEST_CHILD_NAME).await;

        ActionsManager::register(
            child.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await
        .expect("failed to start child");

        let m = Moniker::try_from(["TEST_CHILD_NAME"]).unwrap();
        assert_matches!(should_return_early(&*child.lock_state().await, &m), Some(Ok(())));
    }
}
