// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::{Action, ActionKey};
use crate::model::component::instance::{InstanceState, ResolvedInstanceState};
use crate::model::component::{Component, ComponentInstance, WeakComponentInstance};
use ::routing::component_instance::ComponentInstanceInterface;
use ::routing::resolving::ComponentAddress;
use async_trait::async_trait;
use cm_util::{AbortError, AbortHandle, AbortableScope};
use errors::{ActionError, ResolveActionError};
use fuchsia_trace as trace;
use hooks::EventPayload;
use std::ops::DerefMut;
use std::sync::Arc;

/// Resolves a component instance's declaration and initializes its state.
pub struct ResolveAction {
    abort_handle: AbortHandle,
    abortable_scope: AbortableScope,
}

impl ResolveAction {
    pub fn new() -> Self {
        let (abortable_scope, abort_handle) = AbortableScope::new();
        Self { abort_handle, abortable_scope }
    }
}

#[async_trait]
impl Action for ResolveAction {
    async fn handle(self, component: Arc<ComponentInstance>) -> Result<(), ActionError> {
        do_resolve(&component, self.abortable_scope).await.map_err(Into::into)
    }
    fn key(&self) -> ActionKey {
        ActionKey::Resolve
    }

    fn abort_handle(&self) -> Option<AbortHandle> {
        Some(self.abort_handle.clone())
    }
}

async fn do_resolve(
    component: &Arc<ComponentInstance>,
    abortable_scope: AbortableScope,
) -> Result<(), ResolveActionError> {
    match &*component.lock_state().await {
        InstanceState::Unresolved(_) => (),
        InstanceState::Resolved(_) | InstanceState::Started(_, _) => {
            // The component is already resolved, there's no work for us to do.
            return Ok(());
        }
        InstanceState::Shutdown(_, _) => {
            return Err(ResolveActionError::InstanceShutDown {
                moniker: component.moniker.clone(),
            });
        }
        InstanceState::Destroyed => {
            return Err(ResolveActionError::InstanceDestroyed {
                moniker: component.moniker.clone(),
            });
        }
    }
    trace::duration!("component_manager", "Actions::Resolve", "moniker" => component.moniker.as_str());

    let component_url = &component.component_url;
    let component_address =
        ComponentAddress::from_url(component_url, component).await.map_err(|err| {
            ResolveActionError::ComponentAddressParseError {
                url: component.component_url.clone(),
                moniker: component.moniker.clone(),
                err: Box::new(err),
            }
        })?;
    let component_info = abortable_scope
        .run(async {
            let component_info = component
                .perform_resolve(None, &component_address)
                .await
                .map_err(|err| ResolveActionError::ResolverError {
                    url: component.component_url.clone(),
                    err: Box::new(err),
                })?;
            Component::resolve_with_config(component_info, component.config_parent_overrides())
        })
        .await
        .map_err(|_: AbortError| ResolveActionError::Aborted {
            moniker: component.moniker.clone(),
        })??;
    let policy = component.context.abi_revision_policy();
    policy
        .check_compatibility(
            &version_history_data::HISTORY,
            &component.moniker,
            component_info.abi_revision,
        )
        .map_err(|err| ResolveActionError::AbiCompatibilityError {
            url: component_url.clone(),
            err: Box::new(err),
        })?;
    {
        let mut state = component.lock_state().await;
        let (instance_token_state, component_input_dict) = match state.deref_mut() {
            InstanceState::Resolved(_) => {
                panic!("Component was marked Resolved during Resolve action?");
            }
            InstanceState::Started(_, _) => {
                panic!("Component was marked Started during Resolve action?");
            }
            InstanceState::Shutdown(_, _) => {
                return Err(ResolveActionError::InstanceShutDown {
                    moniker: component.moniker.clone(),
                });
            }
            InstanceState::Destroyed => {
                return Err(ResolveActionError::InstanceDestroyed {
                    moniker: component.moniker.clone(),
                });
            }
            InstanceState::Unresolved(unresolved_state) => unresolved_state.to_resolved(),
        };
        let resolved_state = ResolvedInstanceState::new(
            component,
            component_info.clone(),
            component_address.into(),
            instance_token_state,
            component_input_dict,
        )
        .await?;
        state.set(InstanceState::Resolved(Box::new(resolved_state)));
    }

    let weak = WeakComponentInstance::new(component).into();
    let event = component.new_event(EventPayload::Resolved { component: weak });
    component.hooks.dispatch(&event).await;
    Ok(())
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use crate::model::actions::test_utils::{is_resolved, is_stopped};
    use crate::model::actions::{
        Action, ActionKey, ActionsManager, ResolveAction, ShutdownAction, ShutdownType,
        StartAction, StopAction,
    };
    use crate::model::component::{IncomingCapabilities, StartReason};
    use crate::model::testing::test_helpers::{ActionsTest, component_decl_with_test_runner};
    use assert_matches::assert_matches;
    use cm_rust_testing::ComponentDeclBuilder;
    use errors::{ActionErrorKind, ResolveActionError};
    use futures::FutureExt;
    use futures::channel::oneshot;
    use moniker::Moniker;

    #[fuchsia::test]
    async fn resolve_action_test() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", component_decl_with_test_runner()),
        ];
        // Resolve and start the components.
        let test = ActionsTest::new("root", components, None).await;
        let component_root = test.look_up(Moniker::root()).await;
        let component_a = test.start(["a"].try_into().unwrap()).await;
        assert!(component_a.is_started().await);
        assert!(is_resolved(&component_root).await);
        assert!(is_resolved(&component_a).await);

        // Stop, then it's ok to resolve again.
        ActionsManager::register(component_a.clone(), StopAction::new(false)).await.unwrap();
        assert!(is_resolved(&component_a).await);
        assert!(is_stopped(&component_root, &"a".try_into().unwrap()).await);

        ActionsManager::register(component_a.clone(), ResolveAction::new()).await.unwrap();
        assert!(is_resolved(&component_a).await);
        assert!(is_stopped(&component_root, &"a".try_into().unwrap()).await);

        // Start it again then shut it down.
        ActionsManager::register(
            component_a.clone(),
            StartAction::new(StartReason::Debug, None, IncomingCapabilities::default()),
        )
        .await
        .unwrap();
        ActionsManager::register(component_a.clone(), ShutdownAction::new(ShutdownType::Instance))
            .await
            .unwrap();

        // Error to resolve a shut-down component.
        assert_matches!(
            ActionsManager::register(component_a.clone(), ResolveAction::new()).await,
            Err(e) if matches!(e.kind(), ActionErrorKind::ResolveError { err: ResolveActionError::InstanceShutDown { .. } })
        );
        assert!(!is_resolved(&component_a).await);
        assert!(is_stopped(&component_root, &"a".try_into().unwrap()).await);
    }

    /// Tests that a resolve action can be cancelled while it's waiting on the resolver.
    #[fuchsia::test]
    async fn cancel_resolve_test() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", component_decl_with_test_runner()),
        ];

        let test = ActionsTest::new("root", components, None).await;

        let (resolved_tx, resolved_rx) = oneshot::channel::<()>();
        let (_continue_tx, continue_rx) = oneshot::channel::<()>();
        test.resolver.add_blocker("a", resolved_tx, continue_rx).await;

        let component_root = test.look_up(Moniker::root()).await;
        let component_a = component_root.find(&["a"].try_into().unwrap()).await.unwrap();
        let resolve_action = ResolveAction::new();
        let resolve_abort_handle = resolve_action.abort_handle().unwrap();
        let resolve_fut = component_a.actions().register_no_wait(resolve_action).await;
        let resolve_fut_2 = component_a.actions().wait(ActionKey::Resolve).await.unwrap();

        // Wait until routing reaches resolution.
        let _ = resolved_rx.await.unwrap();

        // Resolution should not be finished yet.
        assert!(resolve_fut_2.now_or_never().is_none());

        // Cancel the resolve action.
        resolve_abort_handle.abort();

        // We should now see the abort error from the action.
        assert_matches!(
            resolve_fut.await,
            Err(e) if matches!(e.kind(), ActionErrorKind::ResolveError { err: ResolveActionError::Aborted { .. } })
        );
    }
}
