// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The "Action" concept represents an asynchronous activity on a component that should eventually
//! complete.
//!
//! Actions decouple the "what" of what needs to happen to a component from the "how". Several
//! client APIs may induce operations on a component's state that complete asynchronously. These
//! operations could depend on each other in various ways.
//!
//! A key property of actions is idempotency. If two equal actions are registered on a component,
//! the work for that action is performed only once. This means that two distinct call sites can
//! register the same action, and be guaranteed the work is not repeated.
//!
//! Here are a couple examples:
//! - A `Shutdown` FIDL call must shut down every component instance in the tree, in
//!   dependency order. For this to happen every component must shut down, but not before its
//!   downstream dependencies have shut down.
//! - A `Realm.DestroyChild` FIDL call returns right after a child component is destroyed.
//!   However, in order to actually delete the child, a sequence of events must happen:
//!     * All instances in the component must be shut down (see above)
//!     * The component instance's persistent storage must be erased, if any.
//!     * The component's parent must remove it as a child.
//!
//! Note the interdependencies here -- destroying a component also requires shutdown, for example.
//!
//! These processes could be implemented through a chain of futures in the vicinity of the API
//! call. However, this doesn't scale well, because it requires distributed state handling and is
//! prone to races. Actions solve this problem by allowing client code to just specify the actions
//! that need to eventually be fulfilled. The actual business logic to perform the actions can be
//! implemented by the component itself in a coordinated manner.
//!
//! `DestroyChild()` is an example of how this can work. For simplicity, suppose it's called on a
//! component with no children of its own. This might cause a chain of events like the following:
//!
//! - Before it returns, the `DestroyChild` FIDL handler registers the `DeleteChild` action on the
//!   parent component for child being destroyed.
//! - This results in a call to `Action::handle` for the component. In response to
//!   `DestroyChild`, `Action::handle()` spawns a future that sets a `Destroy` action on the child.
//!   Note that `Action::handle()` is not async, it always spawns any work that might block
//!   in a future.
//! - `Action::handle()` is called on the child. In response to `Destroy`, it sets a `Shutdown`
//!   action on itself (the component instance must be stopped before it is destroyed).
//! - `Action::handle()` is called on the child again, in response to `Shutdown`. It turns out the
//!   instance is still running, so the `Shutdown` future tells the instance to stop. When this
//!   completes, the `Shutdown` action is finished.
//! - The future that was spawned for `Destroy` is notified that `Shutdown` completes, so it cleans
//!   up the instance's resources and finishes the `Destroy` action.
//! - When the work for `Destroy` completes, the future spawned for `DestroyChild` deletes the
//!   child and marks `DestroyChild` finished, which will notify the client that the action is
//!   complete.

mod coordinator;
mod destroy;
pub mod resolve;
mod set;
pub mod shutdown;
pub mod start;
mod stop;
mod unresolve;

// Re-export the actions
pub use destroy::DestroyAction;
pub use resolve::ResolveAction;
pub use shutdown::{ShutdownAction, ShutdownType};
pub use start::StartAction;
pub use stop::StopAction;
pub use unresolve::UnresolveAction;

use crate::model::component::{ComponentInstance, WeakComponentInstance};
use async_trait::async_trait;
use cm_util::AbortHandle;
use coordinator::{ActionCoordinator, ActionExecutor, Command};
use errors::ActionError;
use fuchsia_async as fasync;
use futures::channel::{mpsc, oneshot};
use futures::future::{BoxFuture, FutureExt, Shared};
use futures::task::{Context, Poll};
use futures::{Future, SinkExt};
use std::fmt::Debug;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;

/// A action on a component that must eventually be fulfilled.
#[async_trait]
pub trait Action: Send + Sync + 'static {
    /// Run the action.
    async fn handle(self, component: Arc<ComponentInstance>) -> Result<(), ActionError>;

    /// `key` identifies the action.
    fn key(&self) -> ActionKey;

    /// If the action supports cooperative cancellation, return a handle for this purpose.
    ///
    /// The action may monitor the handle and bail early when it is safe to do so.
    fn abort_handle(&self) -> Option<AbortHandle> {
        None
    }
}

/// A key that uniquely identifies an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKey {
    Resolve,
    Unresolve,
    Start,
    Stop,
    Shutdown,
    Destroy,
}

/// A future bound to a particular action that completes when that action completes.
///
/// Cloning this type will not duplicate the action, but generate another future that waits on the
/// same action.
#[derive(Debug, Clone)]
pub struct ActionNotifier {
    fut: Shared<BoxFuture<'static, Result<(), ActionError>>>,
}

impl ActionNotifier {
    /// Instantiate an `ActionNotifier` that will complete when a message is received on
    /// `receiver`.
    pub fn new(receiver: oneshot::Receiver<Result<(), ActionError>>) -> Self {
        Self {
            fut: receiver
                .map(|res| res.expect("ActionNotifier sender was unexpectedly closed"))
                .boxed()
                .shared(),
        }
    }

    /// Returns the number of references that exist to the shared future in this notifier. Returns
    /// None if the future has completed.
    #[cfg(all(test, not(feature = "src_model_tests")))]
    pub fn get_reference_count(&self) -> Option<usize> {
        self.fut.strong_count()
    }
}

impl Future for ActionNotifier {
    type Output = Result<(), ActionError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let fut = Pin::new(&mut self.fut);
        fut.poll(cx)
    }
}

pub struct ActionsManager {
    _coordinator_task: fasync::Task<()>,
    command_sender: mpsc::UnboundedSender<Command>,
}

impl ActionsManager {
    pub fn new() -> Self {
        let (coordinator_task, command_sender) = ActionCoordinator::new();
        Self { _coordinator_task: coordinator_task, command_sender }
    }

    /// Sets the component reference against which actions on this component will be run. This must
    /// be called before any actions are scheduled on this component, or the action coordinator
    /// will panic.
    pub async fn set_component_reference(&self, component: WeakComponentInstance) {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .clone()
            .send(Command::SetComponentReference(component, sender))
            .await
            .unwrap();
        receiver.await.unwrap();
    }

    /// Returns `true` if the action coordinator is currently running or planning on running an
    /// action with a matching key.
    pub async fn contains(&self, key: ActionKey) -> bool {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.clone().send(Command::Contains(key, sender)).await.unwrap();
        receiver.await.unwrap()
    }

    /// Registers an action in the set, returning when the action is finished (which may represent
    /// a task that's already running for this action).
    pub async fn register<A>(
        component: Arc<ComponentInstance>,
        action: A,
    ) -> Result<(), ActionError>
    where
        A: Action,
    {
        let rx = component.actions().register_no_wait(action).await;
        rx.await
    }

    /// Registers an action to run with the action coordinator. Actions with matching keys may be
    /// deduplicated, such that if the action coordinator is already planning on running an action
    /// with the given key then this action will be dropped and this function will return when the
    /// other matching action completes.
    ///
    /// Returns a future which will complete once the action finishes, with the output of the
    /// action.
    pub async fn register_no_wait<A>(&self, action: A) -> ActionNotifier
    where
        A: Action,
    {
        let (sender, receiver) = oneshot::channel();
        self.command_sender
            .clone()
            .send(Command::RunAction(ActionExecutor::new(action), sender))
            .await
            .unwrap();
        let notifier = receiver.await.unwrap();
        notifier
    }

    /// Returns a future that waits for the given action to complete, if the action coordinator is
    /// currently running or planning on running an action with a matching key.
    pub async fn wait(&self, action_key: ActionKey) -> Option<ActionNotifier> {
        let (sender, receiver) = oneshot::channel();
        self.command_sender.clone().send(Command::GetNotifier(action_key, sender)).await.unwrap();
        receiver.await.unwrap()
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
pub(crate) mod test_utils {
    use super::*;
    use crate::model::component::instance::InstanceState;
    use moniker::ChildName;
    use routing::component_instance::ComponentInstanceInterface;

    /// A mock action, which can be scheduled on a component. The mock action will wait until it
    /// receives a message over a given oneshot, and then use the received value as the return
    /// value for the action.
    ///
    /// Note that the action coordinator will panic if an action returns `Ok(())` and does not
    /// change the component's state into the expected target state of that action key, and mock
    /// actions are incapable of changing the component's state.
    pub struct MockAction {
        key: ActionKey,
        completion_waiter: oneshot::Receiver<Result<(), ActionError>>,
    }

    #[async_trait]
    impl Action for MockAction {
        async fn handle(self, _component: Arc<ComponentInstance>) -> Result<(), ActionError> {
            match self.completion_waiter.await {
                // If we successfully received a value, return it.
                Ok(result) => result,

                // If we fail to receive a value, then we never will. Let's leave this action as
                // pending indefinitely.
                Err(_) => std::future::pending().await,
            }
        }

        fn key(&self) -> ActionKey {
            self.key
        }

        fn abort_handle(&self) -> Option<AbortHandle> {
            None
        }
    }

    impl MockAction {
        pub fn new(key: ActionKey) -> (oneshot::Sender<Result<(), ActionError>>, Self) {
            let (sender, completion_waiter) = oneshot::channel();
            (sender, Self { key, completion_waiter })
        }
    }

    /// Verifies that a child component is deleted by checking its InstanceState and verifying that
    /// it does not exist in the InstanceState of its parent. Assumes the parent is not destroyed
    /// yet.
    pub async fn is_child_deleted(parent: &ComponentInstance, child: &ComponentInstance) -> bool {
        let moniker = child.moniker().leaf().expect("Root component cannot be destroyed");

        // Verify the parent-child relationship
        assert_eq!(parent.moniker().child(moniker.into()), *child.moniker());

        let parent_state = parent.lock_state().await;
        let parent_resolved_state = parent_state.get_resolved_state().expect("not resolved");

        let child_state = child.lock_state().await;
        let found_child = parent_resolved_state.get_child(child.child_moniker().unwrap());

        found_child.is_none() && matches!(*child_state, InstanceState::Destroyed)
    }

    pub async fn is_stopped(component: &ComponentInstance, moniker: &ChildName) -> bool {
        if let Some(resolved_state) = component.lock_state().await.get_resolved_state() {
            if let Some(child) = resolved_state.get_child(moniker) {
                return !child.is_started().await;
            }
        }
        false
    }

    pub async fn is_destroyed(component: &ComponentInstance) -> bool {
        let state = component.lock_state().await;
        matches!(*state, InstanceState::Destroyed)
    }

    pub async fn is_resolved(component: &ComponentInstance) -> bool {
        component.lock_state().await.get_resolved_state().is_some()
    }

    pub async fn is_discovered(component: &ComponentInstance) -> bool {
        let state = component.lock_state().await;
        matches!(*state, InstanceState::Unresolved(_))
    }

    pub async fn is_shutdown(component: &ComponentInstance) -> bool {
        let state = component.lock_state().await;
        matches!(*state, InstanceState::Shutdown(_, _))
    }
}
