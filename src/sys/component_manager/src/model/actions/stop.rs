// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::{Action, ActionKey};
use crate::model::component::ComponentInstance;
use async_trait::async_trait;
use errors::ActionError;
use std::sync::Arc;

/// Stops a component instance.
pub struct StopAction {
    shut_down: bool,
}

impl StopAction {
    pub fn new(shut_down: bool) -> Self {
        Self { shut_down }
    }
}

#[async_trait]
impl Action for StopAction {
    async fn handle(self, component: Arc<ComponentInstance>) -> Result<(), ActionError> {
        component.stop_instance_internal(self.shut_down).await.map_err(Into::into)
    }
    fn key(&self) -> ActionKey {
        ActionKey::Stop
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use super::*;
    use crate::model::actions::test_utils::is_stopped;
    use crate::model::actions::ActionsManager;
    use crate::model::testing::test_helpers::{component_decl_with_test_runner, ActionsTest};
    use crate::model::testing::test_hook::Lifecycle;
    use cm_rust_testing::ComponentDeclBuilder;
    use errors::ModelError;
    use futures::channel::oneshot;
    use futures::lock::Mutex;
    use hooks::{Event, EventPayload, EventType, Hook, HooksRegistration};
    use moniker::Moniker;
    use std::sync::Weak;

    #[fuchsia::test]
    async fn stopped() {
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new("root", components, None).await;

        // Start component so we can witness it getting stopped.
        test.start(["a"].try_into().unwrap()).await;

        // Register `stopped` action, and wait for it. Component should be stopped.
        let component_root = test.look_up(Moniker::root()).await;
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        ActionsManager::register(component_a.clone(), StopAction::new(false))
            .await
            .expect("stop failed");
        assert!(is_stopped(&component_root, &"a".try_into().unwrap()).await);
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            assert_eq!(events, vec![Lifecycle::Stop(["a"].try_into().unwrap())],);
        }

        // Execute action again, same state and no new events.
        ActionsManager::register(component_a.clone(), StopAction::new(false))
            .await
            .expect("stop failed");
        assert!(is_stopped(&component_root, &"a".try_into().unwrap()).await);
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            assert_eq!(events, vec![Lifecycle::Stop(["a"].try_into().unwrap())],);
        }
    }

    #[fuchsia::test]
    async fn stopped_cancels_watcher() {
        struct StopHook {
            moniker: Moniker,
            stopped_tx: Mutex<Option<oneshot::Sender<()>>>,
            continue_rx: Mutex<Option<oneshot::Receiver<()>>>,
        }

        impl StopHook {
            fn new(
                moniker: Moniker,
                stopped_tx: oneshot::Sender<()>,
                continue_rx: oneshot::Receiver<()>,
            ) -> Self {
                Self {
                    moniker,
                    stopped_tx: Mutex::new(Some(stopped_tx)),
                    continue_rx: Mutex::new(Some(continue_rx)),
                }
            }

            fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
                vec![HooksRegistration::new(
                    "StopHook",
                    vec![EventType::Stopped],
                    Arc::downgrade(self) as Weak<dyn Hook>,
                )]
            }

            async fn on_stopped_async(&self, target_moniker: &Moniker) -> Result<(), ModelError> {
                if *target_moniker == self.moniker {
                    let stopped_tx = self.stopped_tx.lock().await.take().unwrap();
                    stopped_tx.send(()).unwrap();
                    let continue_rx = self.continue_rx.lock().await.take().unwrap();
                    continue_rx.await.unwrap();
                }
                Ok(())
            }
        }

        #[async_trait]
        impl Hook for StopHook {
            async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
                let target_moniker = event
                    .target_moniker
                    .unwrap_instance_moniker_or(ModelError::UnexpectedComponentManagerMoniker)?;
                if let EventPayload::Stopped { .. } = event.payload {
                    self.on_stopped_async(target_moniker).await?;
                }
                Ok(())
            }
        }

        // Cause Stop to be interrupted so we can inspect state.
        let (stopped_tx, stopped_rx) = oneshot::channel::<()>();
        let (continue_tx, continue_rx) = oneshot::channel::<()>();
        let stop_hook = Arc::new(StopHook::new(["a"].try_into().unwrap(), stopped_tx, continue_rx));
        let components = vec![
            ("root", ComponentDeclBuilder::new().child_default("a").build()),
            ("a", component_decl_with_test_runner()),
        ];
        let test = ActionsTest::new_with_hooks("root", components, None, stop_hook.hooks()).await;

        // Start component so we can witness it getting stopped.
        test.start(["a"].try_into().unwrap()).await;

        // Register `stopped` action, and make sure there are two clients waiting on the action
        // (the test, and the task spawned by the exit listener), plus the reference in
        // ActionsManager, plus a reference we take to check the count from, for a total of 4
        let component_root = test.look_up(Moniker::root()).await;
        let component_a = test.look_up(["a"].try_into().unwrap()).await;
        let nf = component_a.actions().register_no_wait(StopAction::new(false)).await;
        stopped_rx.await.unwrap();

        let action_notifier = component_a.actions().wait(ActionKey::Stop).await.unwrap();
        assert_eq!(action_notifier.get_reference_count().unwrap(), 4);

        // Let the stop continue.
        continue_tx.send(()).unwrap();
        nf.await.unwrap();
        assert!(is_stopped(&component_root, &"a".try_into().unwrap()).await);
        {
            let events: Vec<_> = test
                .test_hook
                .lifecycle()
                .into_iter()
                .filter(|e| match e {
                    Lifecycle::Stop(_) => true,
                    _ => false,
                })
                .collect();
            assert_eq!(events, vec![Lifecycle::Stop(["a"].try_into().unwrap())],);
        }
    }
}
