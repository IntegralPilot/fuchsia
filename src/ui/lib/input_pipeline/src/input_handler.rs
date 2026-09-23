// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device;
use async_trait::async_trait;
use fuchsia_inspect::health::Reporter;
use fuchsia_inspect::{NumericProperty, Property};
use std::any::Any;
use std::cell::RefCell;
use std::fmt::{Debug, Formatter};
use std::rc::Rc;

pub trait AsRcAny {
    fn as_rc_any(self: Rc<Self>) -> Rc<dyn Any>;
}

impl<T: Any> AsRcAny for T {
    fn as_rc_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

pub trait Handler: AsRcAny {
    fn set_handler_healthy(self: std::rc::Rc<Self>);

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str);

    /// Returns the name of the input handler.
    fn get_name(&self) -> &'static str;

    /// Returns the types of input events this handler is interested in.
    fn interest(&self) -> Vec<input_device::InputEventType>;
}

/// An [`InputHandler`] dispatches InputEvents to an external service. It maintains
/// service connections necessary to handle the events.
///
/// For example, an [`ImeInputHandler`] holds a proxy to IME and keyboard services.
///
/// [`InputHandler`]s process individual input events through [`handle_input_event()`], which can
/// produce multiple events as an outcome. If the [`InputHandler`] sends an [`InputEvent`] to a
/// service that consumes the event, then the [`InputHandler`] updates the [`InputEvent.handled`]
/// accordingly.
///
/// # Notes
/// * _Callers_ should not invoke [`handle_input_event()`] concurrently since sequences of events
///   must be preserved. The state created by event n may affect the interpretation of event n+1.
/// * _Callees_ should avoid blocking unnecessarily, as that prevents `InputEvent`s from
///   propagating to downstream handlers in a timely manner. See
///   [further discussion of blocking](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/lib/input_pipeline/docs/coding.md).
#[async_trait(?Send)]
pub trait InputHandler: Handler {
    /// Returns a vector of InputEvents to propagate to the next InputHandler.
    ///
    /// * The vector may be empty if, e.g., the handler chose to buffer the
    ///   event.
    /// * The vector may have multiple events if, e.g.,
    ///   * the handler chose to release previously buffered events, or
    ///   * the handler unpacked a single event into multiple events
    ///
    /// # Parameters
    /// `input_event`: The InputEvent to be handled.
    async fn handle_input_event(
        self: std::rc::Rc<Self>,
        input_event: input_device::InputEvent,
    ) -> Vec<input_device::InputEvent>;
}

/// A [`BatchInputHandler`] dispatches multiple InputEvents to an external service.
#[async_trait(?Send)]
pub trait BatchInputHandler: Handler {
    /// Returns a vector of InputEvents to propagate to the next InputHandler.
    ///
    /// # Parameters
    /// `input_events`: The InputEvents to be handled.
    async fn handle_input_events(
        self: std::rc::Rc<Self>,
        input_events: Vec<input_device::InputEvent>,
    ) -> Vec<input_device::InputEvent>;
}

#[async_trait(?Send)]
impl<T> BatchInputHandler for T
where
    T: InputHandler,
{
    async fn handle_input_events(
        self: std::rc::Rc<Self>,
        input_events: Vec<input_device::InputEvent>,
    ) -> Vec<input_device::InputEvent> {
        // Pre-allocate the output vector to avoid reallocations when the handler generates
        // the same number of events or fewer. It will still reallocate if the handler
        // produces more events.
        let mut out_events = Vec::with_capacity(input_events.len());
        for input_event in input_events {
            out_events.extend(self.clone().handle_input_event(input_event).await);
        }
        out_events
    }
}

/// An [`UnhandledInputHandler`] is like an [`InputHandler`], but only deals in unhandled events.
#[async_trait(?Send)]
pub trait UnhandledInputHandler: Handler {
    /// Returns a vector of InputEvents to propagate to the next InputHandler.
    ///
    /// * The vector may be empty if, e.g., the handler chose to buffer the
    ///   event.
    /// * The vector may have multiple events if, e.g.,
    ///   * the handler chose to release previously buffered events, or
    ///   * the handler unpacked a single event into multiple events
    ///
    /// # Parameters
    /// `input_event`: The InputEvent to be handled.
    async fn handle_unhandled_input_event(
        self: std::rc::Rc<Self>,
        unhandled_input_event: input_device::UnhandledInputEvent,
    ) -> Vec<input_device::InputEvent>;
}

#[async_trait(?Send)]
impl<T> InputHandler for T
where
    T: UnhandledInputHandler,
{
    async fn handle_input_event(
        self: std::rc::Rc<Self>,
        input_event: input_device::InputEvent,
    ) -> Vec<input_device::InputEvent> {
        match input_event.handled {
            input_device::Handled::Yes => return vec![input_event],
            input_device::Handled::No => {
                T::handle_unhandled_input_event(
                    self,
                    input_device::UnhandledInputEvent {
                        device_event: input_event.device_event,
                        device_descriptor: input_event.device_descriptor,
                        event_time: input_event.event_time,
                        trace_id: input_event.trace_id,
                    },
                )
                .await
            }
        }
    }
}

pub struct InputHandlerStatus {
    /// A node that contains the state below.
    pub inspect_node: fuchsia_inspect::Node,

    /// The number of unhandled events received by the handler.
    events_received_count: fuchsia_inspect::UintProperty,

    /// The number of reports handled by the handler.
    events_handled_count: fuchsia_inspect::UintProperty,

    /// The event time the last received InputEvent was received.
    last_received_timestamp_ns: fuchsia_inspect::UintProperty,

    // This node records the health status of the `InputHandler`.
    pub health_node: RefCell<fuchsia_inspect::health::Node>,
}

impl PartialEq for InputHandlerStatus {
    fn eq(&self, other: &Self) -> bool {
        self.inspect_node == other.inspect_node
            && self.events_received_count == other.events_received_count
            && self.events_handled_count == other.events_handled_count
            && self.last_received_timestamp_ns == other.last_received_timestamp_ns
    }
}

impl Debug for InputHandlerStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.inspect_node.fmt(f)
    }
}

impl Default for InputHandlerStatus {
    fn default() -> Self {
        let inspector = fuchsia_inspect::Inspector::default();
        Self::new(&inspector.root(), "default", false)
    }
}

impl InputHandlerStatus {
    pub fn new(node: &fuchsia_inspect::Node, name: &str, _generates_events: bool) -> Self {
        let handler_node = node.create_child(name);
        let events_received_count = handler_node.create_uint("events_received_count", 0);
        let events_handled_count = handler_node.create_uint("events_handled_count", 0);
        let last_received_timestamp_ns = handler_node.create_uint("last_received_timestamp_ns", 0);
        let mut health_node = fuchsia_inspect::health::Node::new(&handler_node);
        health_node.set_starting_up();
        Self {
            inspect_node: handler_node,
            events_received_count: events_received_count,
            events_handled_count: events_handled_count,
            last_received_timestamp_ns: last_received_timestamp_ns,
            health_node: RefCell::new(health_node),
        }
    }

    pub fn count_received_event(&self, event_time: &zx::MonotonicInstant) {
        self.events_received_count.add(1);
        self.last_received_timestamp_ns.set(event_time.into_nanos().try_into().unwrap());
    }

    pub fn count_handled_event(&self) {
        self.events_handled_count.add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_device::{
        Handled, InputDeviceDescriptor, InputDeviceEvent, InputEvent, InputEventType,
        UnhandledInputEvent,
    };
    use crate::input_handler::InputHandlerStatus;

    use futures::StreamExt as _;
    use futures::channel::mpsc;
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    struct FakeUnhandledInputHandler {
        event_sender: mpsc::UnboundedSender<UnhandledInputEvent>,
        mark_events_handled: bool,
    }

    impl super::Handler for FakeUnhandledInputHandler {
        fn set_handler_healthy(self: std::rc::Rc<Self>) {
            // No inspect data on FakeUnhandledInputHandler. Do nothing.
        }

        fn set_handler_unhealthy(self: std::rc::Rc<Self>, _msg: &str) {
            // No inspect data on FakeUnhandledInputHandler. Do nothing.
        }

        fn get_name(&self) -> &'static str {
            "FakeUnhandledInputHandler"
        }

        fn interest(&self) -> Vec<InputEventType> {
            vec![InputEventType::Fake]
        }
    }

    #[async_trait(?Send)]
    impl UnhandledInputHandler for FakeUnhandledInputHandler {
        async fn handle_unhandled_input_event(
            self: std::rc::Rc<Self>,
            unhandled_input_event: UnhandledInputEvent,
        ) -> Vec<InputEvent> {
            self.event_sender
                .unbounded_send(unhandled_input_event.clone())
                .expect("failed to send");
            vec![InputEvent::from(unhandled_input_event).into_handled_if(self.mark_events_handled)]
        }
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn blanket_impl_passes_unhandled_events_to_wrapped_handler() {
        let expected_trace_id: Option<fuchsia_trace::Id> = Some(1234.into());
        let (event_sender, mut event_receiver) = mpsc::unbounded();
        let handler = std::rc::Rc::new(FakeUnhandledInputHandler {
            event_sender,
            mark_events_handled: false,
        });
        handler
            .clone()
            .handle_input_event(InputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: zx::MonotonicInstant::from_nanos(1),
                handled: Handled::No,
                trace_id: expected_trace_id,
            })
            .await;
        assert_eq!(
            event_receiver.next().await,
            Some(UnhandledInputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: zx::MonotonicInstant::from_nanos(1),
                trace_id: expected_trace_id,
            })
        )
    }

    #[test_case(false; "not marked by handler")]
    #[test_case(true; "marked by handler")]
    #[fuchsia::test(allow_stalls = false)]
    async fn blanket_impl_propagates_wrapped_handlers_return_value(mark_events_handled: bool) {
        let (event_sender, _event_receiver) = mpsc::unbounded();
        let handler =
            std::rc::Rc::new(FakeUnhandledInputHandler { event_sender, mark_events_handled });
        let input_event = InputEvent {
            device_event: InputDeviceEvent::Fake,
            device_descriptor: InputDeviceDescriptor::Fake,
            event_time: zx::MonotonicInstant::from_nanos(1),
            handled: Handled::No,
            trace_id: None,
        };
        let expected_propagated_event = input_event.clone().into_handled_if(mark_events_handled);
        pretty_assertions::assert_eq!(
            handler.clone().handle_input_event(input_event).await.as_slice(),
            [expected_propagated_event]
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn blanket_impl_filters_handled_events_from_wrapped_handler() {
        let (event_sender, mut event_receiver) = mpsc::unbounded();
        let handler = std::rc::Rc::new(FakeUnhandledInputHandler {
            event_sender,
            mark_events_handled: false,
        });
        handler
            .clone()
            .handle_input_event(InputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: zx::MonotonicInstant::from_nanos(1),
                handled: Handled::Yes,
                trace_id: None,
            })
            .await;

        // Drop `handler` to dispose of `event_sender`. This ensures
        // that `event_receiver.next()` does not block.
        std::mem::drop(handler);

        assert_eq!(event_receiver.next().await, None)
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn blanket_impl_propagates_handled_events_to_next_handler() {
        let (event_sender, _event_receiver) = mpsc::unbounded();
        let handler = std::rc::Rc::new(FakeUnhandledInputHandler {
            event_sender,
            mark_events_handled: false,
        });
        assert_eq!(
            handler
                .clone()
                .handle_input_event(InputEvent {
                    device_event: InputDeviceEvent::Fake,
                    device_descriptor: InputDeviceDescriptor::Fake,
                    event_time: zx::MonotonicInstant::from_nanos(1),
                    handled: Handled::Yes,
                    trace_id: None,
                })
                .await
                .as_slice(),
            [InputEvent {
                device_event: InputDeviceEvent::Fake,
                device_descriptor: InputDeviceDescriptor::Fake,
                event_time: zx::MonotonicInstant::from_nanos(1),
                handled: Handled::Yes,
                trace_id: None,
            }]
        );
    }

    #[fuchsia::test]
    fn get_name() {
        struct NeuralInputHandler {}
        impl super::Handler for NeuralInputHandler {
            fn set_handler_healthy(self: std::rc::Rc<Self>) {
                unimplemented!()
            }

            fn set_handler_unhealthy(self: std::rc::Rc<Self>, _msg: &str) {
                unimplemented!()
            }

            fn get_name(&self) -> &'static str {
                "NeuralInputHandler"
            }

            fn interest(&self) -> Vec<InputEventType> {
                vec![InputEventType::Fake]
            }
        }

        #[async_trait(?Send)]
        impl InputHandler for NeuralInputHandler {
            async fn handle_input_event(
                self: std::rc::Rc<Self>,
                _input_event: InputEvent,
            ) -> Vec<InputEvent> {
                unimplemented!()
            }
        }

        let handler = std::rc::Rc::new(NeuralInputHandler {});
        assert_eq!(handler.get_name(), "NeuralInputHandler");
    }

    #[fuchsia::test]
    async fn input_handler_status_initialized_with_correct_properties() {
        let inspector = fuchsia_inspect::Inspector::default();
        let input_pipeline_node = inspector.root().create_child("input_pipeline");
        let input_handlers_node = input_pipeline_node.create_child("input_handlers");
        let _input_handler_status =
            InputHandlerStatus::new(&input_handlers_node, "test_handler", false);
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_pipeline: {
                input_handlers: {
                    test_handler: {
                        events_received_count: 0u64,
                        events_handled_count: 0u64,
                        last_received_timestamp_ns: 0u64,
                        "fuchsia.inspect.Health": {
                            status: "STARTING_UP",
                            // Timestamp value is unpredictable and not relevant in this context,
                            // so we only assert that the property is present.
                            start_timestamp_nanos: diagnostics_assertions::AnyProperty
                        },
                    }
                }
            }
        });
    }
}
