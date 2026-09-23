// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::events::names_from_filter;
use ::routing::bedrock::sandbox_construction::EventStreamFilter;
use async_trait::async_trait;
use cm_rust::EventScope;
use cm_types::Name;
use errors::ModelError;
use fidl::endpoints::Proxy;
use fidl_fuchsia_component as fcomponent;
use futures::channel::mpsc;
use hooks::{Event, EventPayload, EventType, HasEventType, Hook, HooksRegistration, TransferEvent};
use moniker::{ExtendedMoniker, Moniker};
use std::sync::{Arc, Weak};
use vfs::WeakExecutionScope;

/// A HookObserver will register itself to receive hooks (through which events are dispatched), and
/// watch the stream of events. When it discovers an event that matches the scope described in the
/// scope and its filter, it will convert that event into a `fcomponent::Event` and send
/// it over its `mpsc::UnboundedSender`.
pub struct HookObserver {
    pub event_type: EventType,
    pub subscriber: Moniker,
    pub sender: mpsc::UnboundedSender<fcomponent::Event>,
    pub weak_scope: WeakExecutionScope,
    pub filter: EventStreamFilter,
    pub native_scope: Option<Vec<cm_rust::EventScope>>,
    pub parsed_scope_moniker: ExtendedMoniker,
}

impl HookObserver {
    /// Returns the hooks registration for this HookObserver that can be used to install this as a
    /// hook on a component. If this is not done, then this HookObserver will not be invoked when
    /// there are new events.
    pub fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
        vec![HooksRegistration::new(
            "EventStream",
            vec![self.event_type],
            Arc::downgrade(self) as Weak<dyn Hook>,
        )]
    }

    fn is_type(&self, event: &Event) -> bool {
        event.payload.event_type() == self.event_type
    }

    fn is_for_us(&self, event: &Event) -> bool {
        match &event.payload {
            EventPayload::CapabilityRequested { source_moniker, .. } => {
                source_moniker == &self.subscriber
            }
            _ => true,
        }
    }

    /// is_in_scope returns true if this event is within the scope described by the scope.
    ///
    /// For capability requested events, they are in scope if the source_moniker of the event
    /// matches our subscriber moniker, and the scope field is ignored.
    fn is_in_scope(&self, event: &Event) -> bool {
        scope_down_moniker(
            &event.target_moniker,
            &self.parsed_scope_moniker,
            self.native_scope.as_deref(),
        )
        .is_some()
    }

    /// is_in_filter returns true if this event is within our filter. Returns true if this is not
    /// a capability requested event, because that is the only event to which filters apply.
    fn is_in_filter(&self, event: &Event) -> bool {
        match &event.payload {
            EventPayload::CapabilityRequested { name, .. } => {
                // TODO: should we log if the filter is unset?
                match names_from_filter(&self.filter) {
                    Some(names) if names.contains(&Name::new(name).unwrap()) => true,
                    _ => false,
                }
            }
            _ => true,
        }
    }

    /// Converts a `hooks::Event` into a `fcomponent::Event`. If the hooks::Event is a capability
    /// requested event, then we return `None` and spawn a new task that will observe the receiver
    /// in the event for new channels and send a new `fcomponent::Event` for each channel on
    /// `self.sender`.
    fn convert_event_to_fidl(&self, event: Event) -> Option<fcomponent::Event> {
        let smaller_moniker = scope_down_moniker(
            &event.target_moniker,
            &self.parsed_scope_moniker,
            self.native_scope.as_deref(),
        )
        // TODO: Capability requested event scope checking is weird, and probably broken,
        // because we ignore the scope and only check the subscriber. Because of this, we can end
        // up here in contexts where scope_down_moniker will fail.
        .unwrap_or_else(|| event.target_moniker.to_string());
        let header = fcomponent::EventHeader {
            event_type: Some(event.event_type().into()),
            moniker: Some(smaller_moniker),
            component_url: Some(event.component_url.to_string()),
            timestamp: Some(event.timestamp),
            ..Default::default()
        };
        let payload = match &event.payload {
            EventPayload::CapabilityRequested { name, receiver, .. } => {
                if let Some(receiver) = receiver.take() {
                    let name = name.clone();
                    let sender = self.sender.clone();
                    self.weak_scope.spawn(async move {
                        while let Some(channel) = receiver.receive().await {
                            let payload = fcomponent::EventPayload::CapabilityRequested(
                                fcomponent::CapabilityRequestedPayload {
                                    name: Some(name.clone()),
                                    capability: Some(channel),
                                    ..Default::default()
                                },
                            );
                            let _ = sender.unbounded_send(fcomponent::Event {
                                header: Some(header.clone()),
                                payload: Some(payload),
                                ..Default::default()
                            });
                        }
                    });
                    return None;
                } else {
                    fcomponent::EventPayload::CapabilityRequested(
                        fcomponent::CapabilityRequestedPayload {
                            name: Some(name.clone()),
                            capability: None,
                            ..Default::default()
                        },
                    )
                }
            }
            EventPayload::Stopped { status, exit_code, .. } => {
                fcomponent::EventPayload::Stopped(fcomponent::StoppedPayload {
                    status: Some(zx::Status::result_into_raw(*status)),
                    exit_code: *exit_code,
                    ..Default::default()
                })
            }
            EventPayload::Destroyed { .. } => {
                fcomponent::EventPayload::Destroyed(fcomponent::DestroyedPayload::default())
            }
            EventPayload::Resolved { .. } => {
                fcomponent::EventPayload::Resolved(fcomponent::ResolvedPayload::default())
            }
            EventPayload::Unresolved { .. } => {
                fcomponent::EventPayload::Unresolved(fcomponent::UnresolvedPayload::default())
            }
            EventPayload::Started { .. } => {
                fcomponent::EventPayload::Started(fcomponent::StartedPayload::default())
            }
            EventPayload::DebugStarted { runtime_dir, break_on_start } => {
                fcomponent::EventPayload::DebugStarted(fcomponent::DebugStartedPayload {
                    runtime_dir: runtime_dir
                        .as_ref()
                        .and_then(|proxy| fuchsia_fs::directory::clone(proxy).ok())
                        .map(|proxy| proxy.into_client_end().unwrap()),
                    break_on_start: break_on_start.duplicate_handle(zx::Rights::SAME_RIGHTS).ok(),
                    ..Default::default()
                })
            }
        };
        Some(fcomponent::Event {
            header: Some(header),
            payload: Some(payload),
            ..Default::default()
        })
    }
}

#[async_trait]
impl Hook for HookObserver {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
        if self.is_type(&event) && self.is_in_filter(&event) && self.is_for_us(&event) {
            if self.is_in_scope(&event) {
                if let Some(fidl_event) = self.convert_event_to_fidl(event.transfer().await) {
                    let _ = self.sender.unbounded_send(fidl_event);
                }
            } else if event.payload.event_type() == EventType::CapabilityRequested {
                // This event is a capability requested event addressed to us, but it's not in our
                // scope. If we don't take the event, then normal capability routing will deliver
                // the handle to the component's outgoing directory. We disallow this to be
                // behavior compatible with prior versions of this code.
                let _ = event.transfer().await;
            }
        }
        Ok(())
    }
}

/// When a scope is applied to an event stream, the monikers for emitted events have the prefixes
/// from that scope stripped off, so that the stream consumer does not have to care about and
/// cannot learn about components outside of its scope.
///
/// This function returns the moniker limited to the scope that's described, the full event moniker
/// if no scope is set, and `None` if a scope is set and the event moniker is outside of that
/// scope.
fn scope_down_moniker(
    event_moniker: &ExtendedMoniker,
    scope_moniker: &ExtendedMoniker,
    scope: Option<&[EventScope]>,
) -> Option<String> {
    if scope.is_none() || *scope_moniker == ExtendedMoniker::ComponentManager {
        // If we have no scope, or we're scoped to the root, then we don't need to reduce anything
        return Some(event_moniker.to_string());
    }

    let ExtendedMoniker::ComponentInstance(scope_moniker_instance) = scope_moniker else {
        unreachable!();
    };
    let ExtendedMoniker::ComponentInstance(event_moniker_instance) = event_moniker else {
        panic!("component manager can't emit events");
    };

    for s in scope.unwrap() {
        let smaller_moniker = match s {
            EventScope::Child(child_ref) => {
                let child_scope_moniker = scope_moniker_instance.child(child_ref.clone().into());
                if event_moniker_instance.has_prefix(&child_scope_moniker) {
                    Some(event_moniker_instance.strip_prefix(&child_scope_moniker).unwrap())
                } else {
                    None
                }
            }
            EventScope::Collection(collection_name) => {
                if event_moniker_instance.path().len() > scope_moniker_instance.path().len()
                    && event_moniker_instance.has_prefix(&scope_moniker_instance)
                    && event_moniker_instance.path()[scope_moniker_instance.path().len()]
                        .collection()
                        == Some(collection_name)
                {
                    let start_idx = scope_moniker_instance.path().len() + 1;
                    let child_names_slice = &event_moniker_instance.path()[start_idx..];
                    Some(Moniker::new_from_borrowed(child_names_slice))
                } else {
                    None
                }
            }
        };
        if let Some(smaller_moniker) = smaller_moniker {
            return Some(smaller_moniker.to_string());
        }
    }
    None
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use cm_rust::ChildRef;

    #[fuchsia::test]
    fn scope_down_moniker_tests() {
        for (i, (event_moniker, scope_moniker, scope, expected_output)) in [
            // When no scope is set, the event moniker is unchanged
            ("a/b/c/d", None, None, Some("a/b/c/d")),
            // When a scope moniker and no scope is set, the event moniker is unchanged. These test
            // cases don't happen in practice (we never set the scope_moniker without setting the
            // scope), but we might as well ensure the logic is sound here in case that changes.
            ("a/b/c/d", Some("<component_manager>"), None, Some("a/b/c/d")),
            ("a/b/c/d", Some("e/f/g/h"), None, Some("a/b/c/d")),
            ("a/b/c/d", Some("a"), None, Some("a/b/c/d")),
            ("a", Some("a/b/c/d"), None, Some("a")),
            // When the scope is a static child
            (
                "a",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                })]),
                None,
            ),
            (
                "a/b",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                })]),
                None,
            ),
            (
                "a/b/c",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                })]),
                Some("."),
            ),
            (
                "a/b/c/d",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                })]),
                Some("d"),
            ),
            (
                "e/f/g/h",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: None,
                })]),
                None,
            ),
            // When the scope is a dynamic child
            (
                "a",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                None,
            ),
            (
                "a/b",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                None,
            ),
            (
                "a/b/c",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                None,
            ),
            (
                "a/b/col:c",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                Some("."),
            ),
            (
                "a/b/col:q",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                None,
            ),
            (
                "a/b/col:c/d",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                Some("d"),
            ),
            (
                "e/f/g/h",
                Some("a/b"),
                Some(vec![EventScope::Child(ChildRef {
                    name: "c".parse().unwrap(),
                    collection: Some("col".parse().unwrap()),
                })]),
                None,
            ),
            // When the scope is a collection
            ("a", Some("a/b"), Some(vec![EventScope::Collection("col".parse().unwrap())]), None),
            ("a/b", Some("a/b"), Some(vec![EventScope::Collection("col".parse().unwrap())]), None),
            (
                "a/b/c",
                Some("a/b"),
                Some(vec![EventScope::Collection("col".parse().unwrap())]),
                None,
            ),
            (
                "a/b/c",
                Some("a/b"),
                Some(vec![EventScope::Collection("col".parse().unwrap())]),
                None,
            ),
            (
                "a/b/col:c",
                Some("a/b"),
                Some(vec![EventScope::Collection("col".parse().unwrap())]),
                Some("."),
            ),
            (
                "a/b/col:c/d",
                Some("a/b"),
                Some(vec![EventScope::Collection("col".parse().unwrap())]),
                Some("d"),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let parsed_event_moniker: ExtendedMoniker = event_moniker.parse().unwrap();

            let parsed_scope_moniker: ExtendedMoniker = scope_moniker
                .map(|s| s.parse().unwrap())
                .unwrap_or(ExtendedMoniker::ComponentManager);

            assert_eq!(
                scope_down_moniker(&parsed_event_moniker, &parsed_scope_moniker, scope.as_deref(),),
                expected_output.map(|s: &'static str| s.to_string()),
                "test case {i} failed for event: {event_moniker}"
            );
        }
    }
}
