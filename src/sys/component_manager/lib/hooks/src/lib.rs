// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use async_trait::async_trait;
use cm_types::{Name, Url};
use errors::ModelError;
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_runner as fcrunner;
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use log::warn;
use moniker::{ExtendedMoniker, Moniker};
use runtime_capabilities::{Connector, Receiver, WeakInstanceToken};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Weak};

pub trait HasEventType {
    fn event_type(&self) -> EventType;
}

/// Transfers any move-only state out of self into a new event that is otherwise
/// a clone.
pub trait TransferEvent: Send + Sync {
    fn transfer(&self) -> impl std::future::Future<Output = Self> + Send;
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum EventType {
    /// After a CapabilityProvider has been selected, the CapabilityRequested event is dispatched
    /// with the ServerEnd of the channel for the capability.
    CapabilityRequested,
    /// Destruction of an instance has begun. The instance may/may not be stopped by this point.
    /// The instance still exists in the parent's realm but will soon be removed.
    Destroyed,
    /// An instance's declaration was resolved successfully for the first time.
    Resolved,
    /// An instance is about to be started.
    Started,
    /// An instance was stopped successfully.
    /// This event must occur before Destroyed.
    Stopped,
    /// Similar to the Started event, except the payload will carry an eventpair
    /// that the subscriber could use to defer the launch of the component.
    DebugStarted,
    /// A component instance was unresolved.
    Unresolved,
}

impl EventType {
    pub fn as_str(&self) -> &str {
        match self {
            EventType::CapabilityRequested => "capability_requested",
            EventType::Destroyed => "destroyed",
            EventType::Resolved => "resolved",
            EventType::Started => "started",
            EventType::Stopped => "stopped",
            EventType::DebugStarted => "debug_started",
            EventType::Unresolved => "unresolved",
        }
    }

    /// Returns all available event types.
    pub fn values() -> Vec<EventType> {
        vec![
            EventType::CapabilityRequested,
            EventType::Destroyed,
            EventType::Resolved,
            EventType::Started,
            EventType::Stopped,
            EventType::DebugStarted,
            EventType::Unresolved,
        ]
    }
}

impl From<EventType> for Name {
    fn from(event_type: EventType) -> Name {
        event_type.as_str().parse().unwrap()
    }
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl TryFrom<String> for EventType {
    type Error = anyhow::Error;
    fn try_from(string: String) -> Result<EventType, Self::Error> {
        for value in EventType::values() {
            if value.as_str() == string {
                return Ok(value);
            }
        }
        Err(format_err!("invalid string for event type: {:?}", string))
    }
}

impl HasEventType for EventPayload {
    fn event_type(&self) -> EventType {
        match self {
            EventPayload::CapabilityRequested { .. } => EventType::CapabilityRequested,
            EventPayload::Destroyed => EventType::Destroyed,
            EventPayload::Resolved { .. } => EventType::Resolved,
            EventPayload::Started { .. } => EventType::Started,
            EventPayload::Stopped { .. } => EventType::Stopped,
            EventPayload::DebugStarted { .. } => EventType::DebugStarted,
            EventPayload::Unresolved => EventType::Unresolved,
        }
    }
}

impl From<fcomponent::EventType> for EventType {
    fn from(fidl_event_type: fcomponent::EventType) -> Self {
        match fidl_event_type {
            fcomponent::EventType::CapabilityRequested => EventType::CapabilityRequested,
            fcomponent::EventType::Discovered => unreachable!("This isn't used anymore"),
            fcomponent::EventType::Destroyed => EventType::Destroyed,
            fcomponent::EventType::Resolved => EventType::Resolved,
            fcomponent::EventType::Started => EventType::Started,
            fcomponent::EventType::Stopped => EventType::Stopped,
            fcomponent::EventType::DebugStarted => EventType::DebugStarted,
            fcomponent::EventType::Unresolved => EventType::Unresolved,
        }
    }
}

impl From<EventType> for fcomponent::EventType {
    fn from(event_type: EventType) -> Self {
        match event_type {
            EventType::CapabilityRequested => fcomponent::EventType::CapabilityRequested,
            EventType::Destroyed => fcomponent::EventType::Destroyed,
            EventType::Resolved => fcomponent::EventType::Resolved,
            EventType::Started => fcomponent::EventType::Started,
            EventType::Stopped => fcomponent::EventType::Stopped,
            EventType::DebugStarted => fcomponent::EventType::DebugStarted,
            EventType::Unresolved => fcomponent::EventType::Unresolved,
        }
    }
}

/// The component manager calls out to objects that implement the `Hook` trait on registered
/// component manager events. Hooks block the flow of a task, and can mutate, decorate and replace
/// capabilities. This permits `Hook` to serve as a point of extensibility for the component
/// manager.
/// IMPORTANT: Hooks must not block on completion of an Action since Hooks are often called while
/// executing an Action. Waiting on an Action in a Hook could cause a deadlock.
/// IMPORTANT: Hooks should avoid causing event dispatch because we do not guarantee serialization
/// between Hooks. Therefore the order a receiver see events in may be unexpected.
#[async_trait]
pub trait Hook: Send + Sync {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError>;
}

/// An object registers a hook into a component manager event via a `HooksRegistration` object.
/// A single object may register for multiple events through a vector of `EventType`. `Hooks`
/// does not retain the callback. The hook is lazily removed when the callback object loses
/// strong references.
#[derive(Clone)]
pub struct HooksRegistration {
    events: Vec<EventType>,
    callback: Weak<dyn Hook>,
}

impl HooksRegistration {
    pub fn new(
        _name: &'static str,
        events: Vec<EventType>,
        callback: Weak<dyn Hook>,
    ) -> HooksRegistration {
        Self { events, callback }
    }
}

/// A [`CapabilityReceiver`] lets a `CapabilityRequested` event subscriber take the
/// opportunity to monitor requests for the corresponding capability.
#[derive(Clone)]
pub struct CapabilityReceiver {
    inner: Arc<Mutex<Option<Receiver>>>,
}

impl CapabilityReceiver {
    /// Creates a [`CapabilityReceiver`] that receives connection requests sent via the
    /// [`Sender`] capability.
    pub fn new() -> (Self, Arc<Connector>) {
        let (receiver, sender) = Connector::new();
        let inner = Arc::new(Mutex::new(Some(receiver)));
        (Self { inner }, sender)
    }

    /// Take the opportunity to monitor requests.
    pub fn take(&self) -> Option<Receiver> {
        self.inner.lock().take()
    }

    /// Did someone call `take` on this capability receiver.
    pub fn is_taken(&self) -> bool {
        self.inner.lock().is_none()
    }
}

impl TransferEvent for CapabilityReceiver {
    fn transfer(&self) -> impl std::future::Future<Output = Self> + Send {
        async move {
            let receiver = self.take();
            let inner = Arc::new(Mutex::new(receiver));
            Self { inner }
        }
    }
}

#[derive(Clone)]
pub enum EventPayload {
    // Keep the events listed below in alphabetical order!
    CapabilityRequested {
        source_moniker: Moniker,
        name: String,
        receiver: CapabilityReceiver,
    },
    Destroyed,
    Resolved {
        component: Arc<WeakInstanceToken>,
    },
    Unresolved,
    Started {
        runtime: Box<RuntimeInfo>,
    },
    Stopped {
        status: Result<(), zx::Status>,
        exit_code: Option<i64>,
        stop_time: zx::BootInstant,
        stop_time_monotonic: zx::MonotonicInstant,
        execution_duration: zx::MonotonicDuration,
        requested_escrow: bool,
    },
    DebugStarted {
        runtime_dir: Option<fio::DirectoryProxy>,
        break_on_start: Arc<zx::EventPair>,
    },
}

/// Information about a component's runtime provided to `Started`.
#[derive(Clone)]
pub struct RuntimeInfo {
    pub diagnostics_receiver: Arc<Mutex<Option<oneshot::Receiver<fcrunner::ComponentDiagnostics>>>>,
    pub start_time: zx::BootInstant,
    pub start_time_monotonic: zx::MonotonicInstant,
}

impl RuntimeInfo {
    pub fn new(
        timestamp: zx::BootInstant,
        timestamp_monotonic: zx::MonotonicInstant,
        diagnostics_receiver: oneshot::Receiver<fcrunner::ComponentDiagnostics>,
    ) -> Self {
        let diagnostics_receiver = Arc::new(Mutex::new(Some(diagnostics_receiver)));
        Self {
            diagnostics_receiver,
            start_time: timestamp,
            start_time_monotonic: timestamp_monotonic,
        }
    }
}

impl fmt::Debug for EventPayload {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut formatter = fmt.debug_struct("EventPayload");
        formatter.field("type", &self.event_type());
        match self {
            EventPayload::CapabilityRequested { name, .. } => {
                formatter.field("name", &name).finish()
            }
            EventPayload::Stopped { status, exit_code, .. } => {
                formatter.field("status", status).field("exit_code", exit_code).finish()
            }
            EventPayload::Unresolved
            | EventPayload::Destroyed
            | EventPayload::Started { .. }
            | EventPayload::Resolved { .. }
            | EventPayload::DebugStarted { .. } => formatter.finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    /// Moniker of component that this event applies to
    pub target_moniker: ExtendedMoniker,

    /// Component url of the component that this event applies to
    pub component_url: Url,

    /// Payload of the event
    pub payload: EventPayload,

    /// Time when this event was created
    pub timestamp: zx::BootInstant,
}

impl Event {
    pub fn new_builtin(payload: EventPayload) -> Self {
        Self {
            target_moniker: ExtendedMoniker::ComponentManager,
            component_url: "file:///bin/component_manager".parse().unwrap(),
            payload,
            timestamp: zx::BootInstant::get(),
        }
    }
}

impl TransferEvent for EventPayload {
    fn transfer(&self) -> impl std::future::Future<Output = Self> + Send {
        async move {
            match self {
                EventPayload::CapabilityRequested { source_moniker, name, receiver } => {
                    EventPayload::CapabilityRequested {
                        source_moniker: source_moniker.clone(),
                        name: name.to_string(),
                        receiver: receiver.transfer().await,
                    }
                }
                result => result.clone(),
            }
        }
    }
}

impl HasEventType for Event {
    fn event_type(&self) -> EventType {
        self.payload.event_type()
    }
}

impl TransferEvent for Event {
    fn transfer(&self) -> impl std::future::Future<Output = Self> + Send {
        async move {
            Self {
                target_moniker: self.target_moniker.clone(),
                component_url: self.component_url.clone(),
                payload: self.payload.transfer().await,
                timestamp: self.timestamp,
            }
        }
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let payload = match &self.payload {
            EventPayload::CapabilityRequested { source_moniker, name, .. } => {
                format!("requested '{}' from '{}'", name, source_moniker)
            }
            EventPayload::Stopped { status, .. } => {
                format!("with status: {status:?}")
            }
            EventPayload::Destroyed { .. }
            | EventPayload::Resolved { .. }
            | EventPayload::DebugStarted { .. }
            | EventPayload::Started { .. }
            | EventPayload::Unresolved => "".to_string(),
        };
        write!(f, "[{}] '{}' {}", self.event_type(), self.target_moniker, payload)
    }
}

/// This is a collection of hooks to component manager events.
pub struct Hooks {
    hooks_map: Mutex<HashMap<EventType, Vec<Weak<dyn Hook>>>>,
}

impl Hooks {
    pub fn new() -> Self {
        Self { hooks_map: Mutex::new(HashMap::new()) }
    }

    /// For every hook in `hooks`, add it to the list of hooks that are executed when `dispatch`
    /// is called for `hook.event`.
    pub fn install(&self, hooks: Vec<HooksRegistration>) {
        let mut hooks_map = self.hooks_map.lock();
        for hook in hooks {
            for event in hook.events {
                let existing_hooks = hooks_map.entry(event).or_insert(vec![]);
                existing_hooks.push(hook.callback.clone());
            }
        }
    }

    /// Same as `install`, but adds the hook to the front of the queue.
    ///
    /// This is test-only because in general it shouldn't matter what order hooks are executed
    /// in. This is useful for tests that need guarantees about hook execution order.
    pub fn install_front_for_test(&self, hooks: Vec<HooksRegistration>) {
        let mut hooks_map = self.hooks_map.lock();
        for hook in hooks {
            for event in hook.events {
                let existing_hooks = hooks_map.entry(event).or_insert(vec![]);
                existing_hooks.insert(0, hook.callback.clone());
            }
        }
    }

    pub async fn dispatch(&self, event: &Event) {
        let strong_hooks = {
            let mut hooks_map = self.hooks_map.lock();
            if let Some(hooks) = hooks_map.get_mut(&event.event_type()) {
                // We must upgrade our weak references to hooks to strong ones before we can
                // call out to them.
                let mut strong_hooks = vec![];
                hooks.retain(|hook| {
                    if let Some(hook) = hook.upgrade() {
                        strong_hooks.push(hook);
                        true
                    } else {
                        false
                    }
                });
                strong_hooks
            } else {
                vec![]
            }
        };
        for hook in strong_hooks {
            if let Err(err) = hook.on(event).await {
                warn!(err:%, event:%; "Hook produced error for event");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // This test verifies that the payload of the CapabilityRequested event will be transferred.
    #[fuchsia::test]
    async fn capability_requested_transfer() {
        let (receiver, _sender) = CapabilityReceiver::new();
        let event = Event {
            target_moniker: ExtendedMoniker::ComponentInstance(Moniker::root()),
            component_url: "fuchsia-pkg://root".parse().unwrap(),
            payload: EventPayload::CapabilityRequested {
                source_moniker: Moniker::root(),
                name: "foo".to_string(),
                receiver,
            },
            timestamp: zx::BootInstant::get(),
        };

        // Verify the transferred event carries the capability.
        let transferred_event = event.transfer().await;
        match transferred_event.payload {
            EventPayload::CapabilityRequested { receiver, .. } => {
                assert!(!receiver.is_taken());
            }
            _ => panic!("Event type unexpected"),
        }

        // Verify that the original event no longer carries the capability.
        match &event.payload {
            EventPayload::CapabilityRequested { receiver, .. } => {
                assert!(receiver.is_taken());
            }
            _ => panic!("Event type unexpected"),
        }

        // Transferring the original event again should give an empty capability provider.
        let second_transferred_event = event.transfer().await;
        match &second_transferred_event.payload {
            EventPayload::CapabilityRequested { receiver, .. } => {
                assert!(receiver.is_taken());
            }
            _ => panic!("Event type unexpected"),
        }
    }
}
