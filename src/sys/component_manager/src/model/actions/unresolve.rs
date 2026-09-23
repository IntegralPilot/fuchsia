// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::shutdown::do_shutdown;
use crate::model::actions::{Action, ActionKey, ShutdownType};
use crate::model::component::ComponentInstance;
use crate::model::component::instance::InstanceState;
use async_trait::async_trait;
use errors::{ActionError, UnresolveActionError};
use hooks::EventPayload;
use std::sync::Arc;

/// Returns a resolved component to the discovered state. The result is that the component can be
/// restarted, updating both the code and the manifest with destroying its resources. Unresolve can
/// only be applied to a resolved, stopped, component. This action supports the `ffx component
/// reload` command.
pub struct UnresolveAction {}

impl UnresolveAction {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl Action for UnresolveAction {
    async fn handle(self, component: Arc<ComponentInstance>) -> Result<(), ActionError> {
        do_unresolve(&component).await
    }
    fn key(&self) -> ActionKey {
        ActionKey::Unresolve
    }
}

// Implement the UnresolveAction by resetting the state from unresolved to unresolved and emitting
// an Unresolved event.
async fn do_unresolve(component: &Arc<ComponentInstance>) -> Result<(), ActionError> {
    // We want to shut down the component without invoking a full shutdown action, as the
    // unresolved state of a component is viewed as unreachable from the shutdown state by the
    // actions system.
    //
    // TODO(fxbug.dev/TODO): refactor shutdown such that the component is not put in an
    // InstanceState::Shutdown when invoked from here. This will allow us to remove
    // UnresolvedInstanceState from InstanceState::Shutdown.
    do_shutdown(component, ShutdownType::Instance).await?;

    // Move this component back to the Unresolved state. The state may have changed during the time
    // between the do_shutdown call and now, so recheck here.
    {
        let mut state = component.lock_state().await;
        if let InstanceState::Destroyed = &*state {
            return Err(UnresolveActionError::InstanceDestroyed {
                moniker: component.moniker.clone(),
            }
            .into());
        }
        if !state.is_shut_down() {
            return Err(UnresolveActionError::InstanceRunning {
                moniker: component.moniker.clone(),
            }
            .into());
        }
        state.replace(|instance_state| match instance_state {
            InstanceState::Shutdown(_, unresolved_state) => {
                InstanceState::Unresolved(*unresolved_state)
            }
            instance_state => panic!(
                "component {} was shutdown, but then moved to unexpected state {:?}",
                component.moniker, instance_state
            ),
        });
    };

    // Drop any tasks that might be running in the component's execution scope.  We don't need to
    // wait for old tasks to complete naturally; we just force them to stop.  It's possible that
    // waiting for them to complete would block forever anyway e.g. routing a storage capability can
    // block if the backing directory isn't responding for some reason.
    component.execution_scope.force_shutdown();
    component.execution_scope.wait().await;
    component.execution_scope.resurrect();

    let event = component.new_event(EventPayload::Unresolved);
    component.hooks.dispatch(&event).await;
    Ok(())
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use crate::model::actions::test_utils::{
        is_destroyed, is_discovered, is_resolved, is_shutdown,
    };
    use crate::model::actions::{ActionsManager, ShutdownAction, ShutdownType, UnresolveAction};
    use crate::model::component::{ComponentInstance, StartReason};
    use crate::model::testing::test_helpers::{ActionsTest, component_decl_with_test_runner};
    use assert_matches::assert_matches;
    use cm_rust_testing::*;
    use errors::{ActionErrorKind, UnresolveActionError};
    use fidl_fuchsia_component as fcomponent;
    use fidl_fuchsia_component_decl as fdecl;
    use fuchsia_async as fasync;
    use hooks::EventType;
    use moniker::Moniker;
    use std::sync::Arc;

    /// Check unresolve for _recursive_ case. The system has a root with the child `a` and `a` has
    /// descendants as shown in the diagram below.
    ///  a
    ///   \
    ///    b
    ///     \
    ///      c
    ///
    /// Also tests UnresolveAction on InstanceState::Unresolved.
    #[fuchsia::test]
    async fn unresolve_action_recursive_test() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child_default("b").build()),
            ("b", ComponentDeclBuilder::new().child_default("c").build()),
            ("c", component_decl_with_test_runner()),
        ];
        // Resolve components without starting them.
        let test = ActionsTest::new("root", components, None).await;
        let component_root = test.look_up(Moniker::root()).await;
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        assert!(is_resolved(&component_root).await);
        assert!(is_resolved(&component_a).await);
        assert!(is_resolved(&component_b).await);
        assert!(is_resolved(&component_c).await);

        // Unresolve, recursively.
        ActionsManager::register(component_a.clone(), UnresolveAction::new())
            .await
            .expect("unresolve failed");
        assert!(is_resolved(&component_root).await);
        // Component a is back in the discovered state
        assert!(is_discovered(&component_a).await);
        // The original descendents of component a have been placed in a Shutdown state.
        assert!(is_shutdown(&component_b).await);
        assert!(is_shutdown(&component_c).await);
        // Component a itself is now unresolved, and thus does not have children anymore.
        assert_matches!(component_a.find(&["b"].try_into().unwrap()).await, None);

        // Unresolve again, which is ok because UnresolveAction is idempotent.
        assert_matches!(
            ActionsManager::register(component_a.clone(), UnresolveAction::new()).await,
            Ok(())
        );
        // Still Discovered.
        assert!(is_discovered(&component_a).await);
    }

    /// Check unresolve with recursion on eagerly-loaded peer children. The system has a root with
    /// the child `a` and `a` has descendants as shown in the diagram below.
    ///  a
    ///   \
    ///    b
    ///   / \
    ///  c   d
    #[fuchsia::test]
    async fn unresolve_action_recursive_test2() {
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
        let component_b = test.look_up(["a", "b"].try_into().unwrap()).await;
        let component_c = test.look_up(["a", "b", "c"].try_into().unwrap()).await;
        let component_d = test.look_up(["a", "b", "d"].try_into().unwrap()).await;

        // Unresolve, recursively.
        ActionsManager::register(component_a.clone(), UnresolveAction::new())
            .await
            .expect("unresolve failed");

        // Component a is back in the discovered state
        assert!(is_discovered(&component_a).await);
        // The original descendents of component a have been placed in a Shutdown state.
        assert!(is_shutdown(&component_b).await);
        assert!(is_shutdown(&component_c).await);
        assert!(is_shutdown(&component_d).await);
        // Component a itself is now unresolved, and thus does not have children anymore.
        assert_matches!(component_a.find(&["b"].try_into().unwrap()).await, None);
    }

    async fn setup_unresolve_test_event_stream(test: &ActionsTest) -> fcomponent::EventStreamProxy {
        let unresolved_event_stream = test.new_event_stream(vec![EventType::Unresolved]).await;
        let model = test.model.clone();
        fasync::Task::spawn(async move { model.start().await }).detach();
        unresolved_event_stream
    }

    #[fuchsia::test]
    async fn unresolve_action_registers_unresolve_event_test() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", ComponentDeclBuilder::new().child_default("b").build()),
            ("b", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;
        test.start(Moniker::root()).await;

        let a_moniker: Moniker = ["a"].try_into().unwrap();
        let component_a = test.start(a_moniker.clone()).await;
        test.start(["a", "b"].try_into().unwrap()).await;

        let event_stream = setup_unresolve_test_event_stream(&test).await;

        // Register the UnresolveAction.
        let nf = component_a.actions().register_no_wait(UnresolveAction::new()).await;

        // Component a is then unresolved.
        let unresolved_events = event_stream.get_next().await.unwrap();
        assert_eq!(
            unresolved_events[0].header.as_ref().unwrap().moniker,
            Some(a_moniker.to_string())
        );
        nf.await.unwrap();

        // Now attempt to unresolve again with another UnresolveAction.
        let nf2 = component_a.actions().register_no_wait(UnresolveAction::new()).await;
        // The component is not resolved anymore, so the unresolve will have no effect.
        nf2.await.unwrap();
        assert!(is_discovered(&component_a).await);
    }

    /// Start a collection with the given durability. The system has a root with a container that
    /// has a collection containing children `a` and `b` as shown in the diagram below.
    ///    root
    ///      \
    ///    container
    ///     /     \
    ///  coll:a   coll:b
    ///
    async fn start_collection(
        durability: fdecl::Durability,
    ) -> (ActionsTest, Arc<ComponentInstance>, Arc<ComponentInstance>, Arc<ComponentInstance>) {
        let collection =
            CollectionBuilder::new().name("coll").durability(durability).allow_long_names().build();

        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("container").build()),
            ("container", ComponentDeclBuilder::new().collection(collection).build()),
            ("a", component_decl_with_test_runner()),
            ("b", component_decl_with_test_runner()),
        ];
        let test =
            ActionsTest::new("root", components, Some(["container"].try_into().unwrap())).await;
        let root = test.model.root();

        // Create dynamic instances in "coll".
        test.create_dynamic_child("coll", "a").await;
        test.create_dynamic_child("coll", "b").await;

        // Start the components. This should cause them to have an `Execution`.
        let component_container = test.look_up(["container"].try_into().unwrap()).await;
        let component_a = test.look_up(["container", "coll:a"].try_into().unwrap()).await;
        let component_b = test.look_up(["container", "coll:b"].try_into().unwrap()).await;
        root.start_instance(&component_container.moniker, &StartReason::Eager)
            .await
            .expect("could not start container");
        root.start_instance(&component_a.moniker, &StartReason::Eager)
            .await
            .expect("could not start coll:a");
        root.start_instance(&component_b.moniker, &StartReason::Eager)
            .await
            .expect("could not start coll:b");
        assert!(component_container.is_started().await);
        assert!(is_resolved(&component_a).await);
        assert!(is_resolved(&component_b).await);
        assert!(component_a.is_started().await);
        assert!(component_b.is_started().await);
        (test, component_container, component_a, component_b)
    }

    /// Test a collection with the given durability.
    /// Also tests UnresolveAction on InstanceState::Destroyed.
    async fn test_collection(durability: fdecl::Durability) {
        let (_test, component_container, component_a, component_b) =
            start_collection(durability).await;

        // Stop the collection.
        ActionsManager::register(
            component_container.clone(),
            ShutdownAction::new(ShutdownType::Instance),
        )
        .await
        .expect("shutdown failed");
        assert!(is_destroyed(&component_a).await);
        assert!(is_destroyed(&component_b).await);
        ActionsManager::register(component_container.clone(), UnresolveAction::new())
            .await
            .expect("unresolve failed");
        assert!(is_discovered(&component_container).await);

        // Trying to unresolve a child fails because the children of a collection are destroyed when
        // the collection is stopped. Then it's an error to unresolve a Destroyed component.
        assert_matches!(
            ActionsManager::register(component_a.clone(), UnresolveAction::new()).await,
            Err(e) if matches!(e.kind(), ActionErrorKind::UnresolveError {
                err: UnresolveActionError::InstanceDestroyed { .. }
            })
        );
        // Still Destroyed.
        assert!(is_destroyed(&component_a).await);
        assert!(is_destroyed(&component_b).await);
    }

    /// Test a collection whose children have transient durability.
    #[fuchsia::test]
    async fn unresolve_action_on_transient_collection() {
        test_collection(fdecl::Durability::Transient).await;
    }

    /// Test a collection whose children have single-run durability.
    #[fuchsia::test]
    async fn unresolve_action_on_single_run_collection() {
        test_collection(fdecl::Durability::SingleRun).await;
    }
}
