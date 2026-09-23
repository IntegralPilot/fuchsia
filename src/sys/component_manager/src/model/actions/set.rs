// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Action, ActionKey, ActionNotifier};
use crate::model::component::ComponentInstance;
use cm_util::AbortHandle;
use errors::ActionError;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::future::{BoxFuture, FutureExt, join_all};
use futures::select;
use std::collections::HashMap;
use std::pin::pin;
use std::sync::Arc;

/// Represents a task that implements an action.
pub struct ActionTask {
    pub tx: oneshot::Sender<Result<(), ActionError>>,
    fut: BoxFuture<'static, Result<(), ActionError>>,
}

impl ActionTask {
    fn new(
        tx: oneshot::Sender<Result<(), ActionError>>,
        fut: BoxFuture<'static, Result<(), ActionError>>,
    ) -> Self {
        Self { tx, fut }
    }

    /// Runs the action in a separate task and signals the `ActionNotifier` when it completes.
    pub fn spawn(self) {
        fasync::Task::spawn(async move {
            self.tx.send(self.fut.await).unwrap_or(()); // Ignore closed receiver.
        })
        .detach();
    }
}

struct ActionController {
    /// The notifier by which clients will be informed when the action returns.
    notifier: ActionNotifier,
    /// If supported, a handle to abort the action.
    maybe_abort_handle: Option<AbortHandle>,
}

/// A set of actions on a component that must be completed.
///
/// Each action is mapped to a future that returns when the action is complete.
pub struct ActionSet {
    rep: HashMap<ActionKey, ActionController>,
    passive_waiters: HashMap<ActionKey, Vec<oneshot::Sender<()>>>,
}

impl ActionSet {
    pub fn new() -> Self {
        ActionSet { rep: HashMap::new(), passive_waiters: HashMap::new() }
    }

    pub fn contains(&self, key: ActionKey) -> bool {
        self.rep.contains_key(&key)
    }

    /// Registers an action in the set, but does not wait for it to complete, instead returning a
    /// future that can be used to wait on the task. This function is a no-op if the task is
    /// already registered.
    ///
    /// REQUIRES: `self` is the `ActionSet` contained in `component`.
    pub fn register_no_wait<A>(
        action_set: Arc<Mutex<ActionSet>>,
        component: &Arc<ComponentInstance>,
        action: A,
    ) -> ActionNotifier
    where
        A: Action,
    {
        let (task, rx) = Self::register_inner(action_set, component, action);
        if let Some(task) = task {
            task.spawn();
        }
        rx
    }

    /// Returns a future that waits for the given action to complete, if one exists.
    pub fn wait(&self, action_key: ActionKey) -> Option<ActionNotifier> {
        self.rep.get(&action_key).map(|controller| controller.notifier.clone())
    }

    /// Removes an action from the set, completing it.
    fn finish<'a>(&mut self, key: &'a ActionKey) {
        self.rep.remove(key);

        if let Some(waiters) = self.passive_waiters.remove(key) {
            for sender in waiters {
                let _ = sender.send(());
            }
        }
    }

    /// Registers, but does not execute, an action.
    ///
    /// Returns:
    /// - An object that implements the action if it was scheduled for the first time. The caller
    ///   should call spawn() on it.
    /// - A future to listen on the completion of the action.
    #[must_use]
    fn register_inner<'a, A>(
        action_set: Arc<Mutex<ActionSet>>,
        component: &Arc<ComponentInstance>,
        action: A,
    ) -> (Option<ActionTask>, ActionNotifier)
    where
        A: Action,
    {
        let mut self_ = action_set.lock();
        let key = action.key();
        // If this Action is already running, just subscribe to the result
        if let Some(action_controller) = self_.rep.get(&key) {
            return (None, action_controller.notifier.clone());
        }

        // Otherwise we spin up the new Action
        let maybe_abort_handle = action.abort_handle();
        let prereqs = self_.get_prereq_action(action.key());
        let abort_handles = self_.get_abort_action(action.key());

        let component = component.clone();

        let action_set_clone = action_set.clone();
        let action_fut = async move {
            for abort in abort_handles {
                abort.abort();
            }
            _ = join_all(prereqs).await;
            let key = action.key();
            let mut action_fut = action.handle(component.clone()).fuse();
            let mut timer = pin!(fasync::Timer::new(std::time::Duration::from_secs(300)));
            let res = select! {
                res = action_fut => {
                    res
                }
                _ = timer => {
                    log::warn!("action {:?} has been running for over 5 minutes on component {}", key, component.moniker);
                    action_fut.await
                }
            };
            action_set_clone.lock().finish(&key);
            res
        }
        .boxed();

        let (notifier_completer, notifier_receiver) = oneshot::channel();
        let task = ActionTask::new(notifier_completer, action_fut);
        let notifier = ActionNotifier::new(notifier_receiver);
        self_.rep.insert(
            key.clone(),
            ActionController { notifier: notifier.clone(), maybe_abort_handle },
        );
        (Some(task), notifier)
    }

    /// Return futures that waits for any Action that must be waited on before
    /// executing the target Action. If none is required the returned vector is
    /// empty.
    fn get_prereq_action(&self, key: ActionKey) -> Vec<ActionNotifier> {
        // Start, Stop, and Shutdown are all serialized with respect to one another.
        match key {
            ActionKey::Shutdown => vec![
                self.rep.get(&ActionKey::Stop).map(|c| c.notifier.clone()),
                self.rep.get(&ActionKey::Start).map(|c| c.notifier.clone()),
            ],
            ActionKey::Stop => vec![
                self.rep.get(&ActionKey::Shutdown).map(|c| c.notifier.clone()),
                self.rep.get(&ActionKey::Start).map(|c| c.notifier.clone()),
            ],
            ActionKey::Start => vec![
                self.rep.get(&ActionKey::Stop).map(|c| c.notifier.clone()),
                self.rep.get(&ActionKey::Shutdown).map(|c| c.notifier.clone()),
            ],
            _ => vec![],
        }
        .into_iter()
        .flatten()
        .collect()
    }

    /// Return abort handles for any Action that may be canceled by the target Action.
    ///
    /// This is useful for stopping unnecessary work e.g. if Stop is requested while
    /// Start is running.
    fn get_abort_action(&self, key: ActionKey) -> Vec<AbortHandle> {
        // Stop and Shutdown will attempt to cancel an in-progress Start.
        match key {
            ActionKey::Shutdown | ActionKey::Stop => self
                .rep
                .get(&ActionKey::Start)
                .and_then(|action_controller| action_controller.maybe_abort_handle.clone())
                .into_iter()
                .collect(),
            _ => vec![],
        }
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub mod tests {
    use super::*;
    use crate::model::actions::{DestroyAction, ShutdownAction, ShutdownType};
    use crate::model::testing::test_helpers::ActionsTest;
    use assert_matches::assert_matches;
    use errors::{ActionErrorKind, StopActionError};
    use fuchsia_async as fasync;

    async fn register_action_in_new_task<A>(
        action: A,
        component: Arc<ComponentInstance>,
        action_set: Arc<Mutex<ActionSet>>,
        responder: oneshot::Sender<Result<(), ActionError>>,
        res: Result<(), ActionError>,
    ) where
        A: Action,
    {
        // Register action, and get the future. Use `register_inner` so that we can control
        // when to notify the listener.
        let (task, rx) = ActionSet::register_inner(action_set.clone(), &component, action);

        fasync::Task::spawn(async move {
            if let Some(task) = task {
                // Notify the listeners, but don't actually run the action since this test tests
                // action registration and not the actions themselves.
                task.tx.send(res).unwrap();
            }
            let res = rx.await;

            // If the future completed successfully then we will get to this point.
            responder.send(res).expect("failed to send response");
        })
        .detach();
    }

    #[fuchsia::test]
    async fn action_set() {
        let test = ActionsTest::new("root", vec![], None).await;
        let component = test.model.root().clone();

        // It's not possible to get at the action set used for a given component, so we create a
        // new one here. This works because no actions are scheduled on the component's `Actions`
        // and thus collisions are avoided, making our action set here the de-facto only action set
        // for the component, .
        let action_set = Arc::new(Mutex::new(ActionSet::new()));

        let (tx1, rx1) = oneshot::channel();
        register_action_in_new_task(
            DestroyAction::new(),
            component.clone(),
            action_set.clone(),
            tx1,
            Ok(()),
        )
        .await;
        let (tx2, rx2) = oneshot::channel();
        register_action_in_new_task(
            ShutdownAction::new(ShutdownType::Instance),
            component.clone(),
            action_set.clone(),
            tx2,
            Err(ActionError::from(StopActionError::GetParentFailed)), // Some random error.
        )
        .await;
        let (tx3, rx3) = oneshot::channel();
        register_action_in_new_task(
            DestroyAction::new(),
            component.clone(),
            action_set.clone(),
            tx3,
            Ok(()),
        )
        .await;

        // Complete actions, while checking notifications.
        action_set.lock().finish(&ActionKey::Destroy);
        assert_matches!(rx1.await.expect("Unable to receive result of Notification"), Ok(()));
        assert_matches!(rx3.await.expect("Unable to receive result of Notification"), Ok(()));

        action_set.lock().finish(&ActionKey::Shutdown);
        assert_matches!(
            rx2.await.expect("Unable to receive result of Notification"),
            Err(e) if matches!(e.kind(), ActionErrorKind::StopError { err: StopActionError::GetParentFailed })
        );
    }
}
