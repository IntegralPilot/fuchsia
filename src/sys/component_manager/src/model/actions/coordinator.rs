// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::set::ActionSet;
use crate::model::actions::{Action, ActionKey, ActionNotifier};
use crate::model::component::{ComponentInstance, WeakComponentInstance};
use async_trait::async_trait;
use cm_util::AbortHandle;
use errors::ActionError;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use std::sync::Arc;

/// A command, sent to the action coordinator to request it performs some kind of work.
pub(super) enum Command {
    /// Sets the component reference used by this action set when invoking actions. This command
    /// must be received before any RunAction commands. A message is delivered on the oneshot
    /// sender when this is complete.
    SetComponentReference(WeakComponentInstance, oneshot::Sender<()>),

    /// Asks the coordinator to run a given action. An action notifier will be sent over the
    /// oneshot sender for this action. This may be a clone of the notifier for an action that we
    /// received earlier, or it could be a new notifier.
    RunAction(ActionExecutor, oneshot::Sender<ActionNotifier>),

    /// Gets the notifier for a given action, if such an action is currently in our plan of which
    /// actions we will run.
    GetNotifier(ActionKey, oneshot::Sender<Option<ActionNotifier>>),

    /// Returns true over the given oneshot if the plan of which actions we will run contains the
    /// given action.
    Contains(ActionKey, oneshot::Sender<bool>),
}

/// The action coordinator controls the running of actions.
#[allow(unused)]
pub(super) struct ActionCoordinator {
    /// The component against which actions will be run
    component: WeakComponentInstance,

    /// The action set which is used to run actions not supported by the coordinator.
    action_set: Arc<Mutex<ActionSet>>,
}

impl ActionCoordinator {
    /// Creates a new action coordinator. Returns the task in which the coordinator is running, and
    /// the mpsc channel over which commands to this coordinator may be sent.
    pub(super) fn new() -> (fasync::Task<()>, mpsc::UnboundedSender<Command>) {
        let (command_sender, command_receiver) = mpsc::unbounded();
        (
            fasync::Task::spawn(
                Self {
                    // We initially have an invalid component reference, and the reference must be
                    // set with Command::SetComponentReference before a Command::RunAction is
                    // received.
                    component: WeakComponentInstance::invalid(),
                    action_set: Arc::new(Mutex::new(ActionSet::new())),
                }
                .run_loop(command_receiver),
            ),
            command_sender,
        )
    }

    async fn run_loop(mut self, mut command_receiver: mpsc::UnboundedReceiver<Command>) {
        loop {
            match command_receiver.next().await {
                Some(Command::SetComponentReference(component, confirmation_sender)) => {
                    self.component = component;
                    let _ = confirmation_sender.send(());
                }
                Some(Command::RunAction(action, notifier_sender)) => {
                    let Ok(component) = self.component.upgrade() else {
                        continue;
                    };
                    let notifier =
                        ActionSet::register_no_wait(self.action_set.clone(), &component, action);
                    let _ = notifier_sender.send(notifier);
                }
                Some(Command::GetNotifier(key, notifier_sender)) => {
                    let _ = notifier_sender.send(self.action_set.lock().wait(key));
                }
                Some(Command::Contains(key, bool_sender)) => {
                    let _ = bool_sender.send(self.action_set.lock().contains(key));
                }
                None => return,
            }
        }
    }
}

/// This struct exists to hold an action and run it, without using generics. The actions are sent
/// over a mpsc channel, which doesn't support generics. We could move a `Box<dyn Action>` over the
/// channel, but then we can't run the action because the `handle` function takes ownership of the
/// action and we can't move the action out of the box without knowing its concrete type. We could
/// downcast to a concrete type, but that requires hardcoding a downcast attempt for each known
/// action type, which doesn't support new action types existing in testing code.
///
/// To work around these restrictions, all actions are ultimately moved into a closure which knows
/// the concrete type of the action, and the closure is then invoked to run the action. This struct
/// is used to wrap that closure, so that it can remember the values the action returns to its
/// other calls.
pub(super) struct ActionExecutor {
    action_fn: Box<
        dyn FnOnce(Arc<ComponentInstance>) -> BoxFuture<'static, Result<(), ActionError>>
            + Sync
            + Send,
    >,
    key: ActionKey,
    abort_handle: Option<AbortHandle>,
}

impl ActionExecutor {
    pub fn new<A>(action: A) -> Self
    where
        A: Action,
    {
        let key = action.key();
        let abort_handle = action.abort_handle();
        Self {
            action_fn: Box::new(move |component: Arc<ComponentInstance>| {
                action.handle(component).boxed()
            }),
            key,
            abort_handle,
        }
    }
}

#[async_trait]
impl Action for ActionExecutor {
    fn key(&self) -> ActionKey {
        self.key
    }

    fn abort_handle(&self) -> Option<AbortHandle> {
        self.abort_handle.clone()
    }

    async fn handle(mut self, component: Arc<ComponentInstance>) -> Result<(), ActionError> {
        (self.action_fn)(component).await
    }
}
