// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::{Handled, InputDeviceEvent, InputDeviceType, InputEvent, InputEventType};
use crate::input_handler::{Handler, InputHandler};
use async_trait::async_trait;
use fuchsia_inspect::health::Reporter;
use fuchsia_inspect::{
    self as inspect, ExponentialHistogramParams, HistogramProperty, Inspector, NumericProperty,
    Property,
};

use fuchsia_sync::Mutex;
use futures::FutureExt;
use inspect::Node;
use sorted_vec_map::SortedVecSet;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;
use strum::EnumCount;

const MAX_RECENT_EVENT_LOG_SIZE: usize = 125;
const LATENCY_HISTOGRAM_PROPERTIES: ExponentialHistogramParams<i64> = ExponentialHistogramParams {
    floor: 0,
    initial_step: 1,
    step_multiplier: 10,
    // Seven buckets allows us to report
    // *      < 0 msec (added automatically by Inspect)
    // *      0-1 msec
    // *     1-10 msec
    // *   10-100 msec
    // * 100-1000 msec
    // *     1-10 sec
    // *   10-100 sec
    // * 100-1000 sec
    // *    >1000 sec (added automatically by Inspect)
    buckets: 7,
};

#[derive(Debug)]
struct EventCounters {
    /// A node that contains the counters below.
    _node: inspect::Node,
    /// The number of total events that this handler has seen so far.
    events_count: inspect::UintProperty,
    /// The number of events with a wake lease seen so far.
    events_with_wake_lease_count: inspect::UintProperty,
    /// The number of total handled events that this handler has seen so far.
    handled_events_count: inspect::UintProperty,
    /// The timestamp (in nanoseconds) when the last event was seen by this
    /// handler (not when the event itself was generated). 0 if unset.
    last_seen_timestamp_ns: inspect::IntProperty,
    /// The event time at which the last recorded event was generated.
    /// 0 if unset.
    last_generated_timestamp_ns: inspect::IntProperty,
}

impl EventCounters {
    fn create(root: &inspect::Node, event_type: InputEventType) -> EventCounters {
        let node = root.create_child(format!("{}", event_type));
        let events_count = node.create_uint("events_count", 0);
        let events_with_wake_lease_count = node.create_uint("events_with_wake_lease_count", 0);
        let handled_events_count = node.create_uint("handled_events_count", 0);
        let last_seen_timestamp_ns = node.create_int("last_seen_timestamp_ns", 0);
        let last_generated_timestamp_ns = node.create_int("last_generated_timestamp_ns", 0);
        EventCounters {
            _node: node,
            events_count,
            events_with_wake_lease_count,
            handled_events_count,
            last_seen_timestamp_ns,
            last_generated_timestamp_ns,
        }
    }

    pub fn count_event(
        &self,
        time: zx::MonotonicInstant,
        event_time: zx::MonotonicInstant,
        handled: &Handled,
        has_wake_lease: bool,
    ) {
        self.events_count.add(1);
        if has_wake_lease {
            self.events_with_wake_lease_count.add(1);
        }
        if *handled == Handled::Yes {
            self.handled_events_count.add(1);
        }
        self.last_seen_timestamp_ns.set(time.into_nanos());
        self.last_generated_timestamp_ns.set(event_time.into_nanos());
    }
}

#[derive(Debug)]
pub(crate) struct CircularBuffer<T> {
    // Size of CircularBuffer
    _size: usize,
    // VecDeque of recent events with capacity of `size`
    _events: VecDeque<T>,
}

pub(crate) trait BufferNode {
    fn get_name(&self) -> &'static str;
    fn record_inspect(&self, node: &Node);
}

impl<T> CircularBuffer<T>
where
    T: BufferNode,
{
    pub(crate) fn new(size: usize) -> Self {
        let events = VecDeque::with_capacity(size);
        CircularBuffer { _size: size, _events: events }
    }

    pub(crate) fn push(&mut self, event: T) {
        if self._events.len() >= self._size {
            std::mem::drop(self._events.pop_front());
        }
        self._events.push_back(event);
    }

    pub(crate) fn record_all_lazy_inspect(
        &self,
        inspector: inspect::Inspector,
    ) -> inspect::Inspector {
        self._events.iter().enumerate().for_each(|(i, event)| {
            // Include leading zeros so Inspect will display events in correct numerical order.
            // Inspect displays nodes in alphabetical order by default.
            inspector.root().record_child(format!("{:03}_{}", i, event.get_name()), move |node| {
                event.record_inspect(node)
            });
        });
        inspector
    }
}

impl BufferNode for InputEvent {
    fn get_name(&self) -> &'static str {
        self.get_event_type()
    }

    fn record_inspect(&self, node: &Node) {
        InputEvent::record_inspect(self, node);
    }
}

/// A [InputHandler] that records various metrics about the flow of events.
/// All events are passed through unmodified.  Some properties of those events
/// may be exposed in the metrics.  No PII information should ever be exposed
/// this way.
pub struct InspectHandler<F> {
    /// A function that obtains the current timestamp.
    now: RefCell<F>,
    /// A node that contains the statistics about this particular handler.
    node: inspect::Node,
    /// The number of total events that this handler has seen so far.
    events_count: inspect::UintProperty,
    /// The number of events with a wake lease seen so far.
    events_with_wake_lease_count: inspect::UintProperty,
    /// The timestamp (in nanoseconds) when the last event was seen by this
    /// handler (not when the event itself was generated). 0 if unset.
    last_seen_timestamp_ns: inspect::IntProperty,
    /// The event time at which the last recorded event was generated.
    /// 0 if unset.
    last_generated_timestamp_ns: inspect::IntProperty,
    /// An inventory of event counters by type.
    events_by_type: [Option<EventCounters>; InputEventType::COUNT],
    /// Log of recent events in the order they were received.
    recent_events_log: Option<Arc<Mutex<CircularBuffer<InputEvent>>>>,
    /// Histogram of latency from the binding timestamp for an `InputEvent` until
    /// the time the `InputEvent` was observed by this handler. Reported in milliseconds,
    /// because values less than 1 msec aren't especially interesting.
    pipeline_latency_ms: inspect::IntExponentialHistogramProperty,
    // This node records the health status of `InspectHandler`.
    health_node: RefCell<fuchsia_inspect::health::Node>,
}

impl<F: FnMut() -> zx::MonotonicInstant + 'static> Debug for InspectHandler<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InspectHandler")
            .field("node", &self.node)
            .field("events_count", &self.events_count)
            .field("events_with_wake_lease_count", &self.events_with_wake_lease_count)
            .field("last_seen_timestamp_ns", &self.last_seen_timestamp_ns)
            .field("last_generated_timestamp_ns", &self.last_generated_timestamp_ns)
            .field("events_by_type", &self.events_by_type)
            .field("recent_events_log", &self.recent_events_log)
            .field("pipeline_latency_ms", &self.pipeline_latency_ms)
            .finish()
    }
}

impl<F: FnMut() -> zx::MonotonicInstant + 'static> Handler for InspectHandler<F> {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        self.health_node.borrow_mut().set_ok();
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str) {
        self.health_node.borrow_mut().set_unhealthy(msg);
    }

    fn get_name(&self) -> &'static str {
        "InspectHandler"
    }

    fn interest(&self) -> Vec<InputEventType> {
        vec![
            InputEventType::Keyboard,
            InputEventType::LightSensor,
            InputEventType::ConsumerControls,
            InputEventType::Mouse,
            InputEventType::TouchScreen,
            InputEventType::Touchpad,
            #[cfg(test)]
            InputEventType::Fake,
        ]
    }
}

#[async_trait(?Send)]
impl<F: FnMut() -> zx::MonotonicInstant + 'static> InputHandler for InspectHandler<F> {
    async fn handle_input_event(self: Rc<Self>, input_event: InputEvent) -> Vec<InputEvent> {
        fuchsia_trace::duration!("input", "inspect_handler");
        let tracing_id = input_event.trace_id.unwrap_or_else(|| 0.into());
        fuchsia_trace::flow_step!("input", "event_in_input_pipeline", tracing_id);

        let event_time = input_event.event_time;
        let now = (self.now.borrow_mut())();
        self.events_count.add(1);

        let has_wake_lease = match &input_event.device_event {
            InputDeviceEvent::ConsumerControls(e) => e.wake_lease.is_some(),
            InputDeviceEvent::Mouse(e) => e.wake_lease.is_some(),
            InputDeviceEvent::TouchScreen(e) => e.wake_lease.is_some(),
            _ => false,
        };
        if has_wake_lease {
            self.events_with_wake_lease_count.add(1);
        }
        self.last_seen_timestamp_ns.set(now.into_nanos());
        self.last_generated_timestamp_ns.set(event_time.into_nanos());
        let event_type = InputEventType::from(&input_event.device_event);
        self.events_by_type[event_type as usize]
            .as_ref()
            .unwrap_or_else(|| panic!("no event counters for {}", event_type))
            .count_event(now, event_time, &input_event.handled, has_wake_lease);
        if let Some(recent_events_log) = &self.recent_events_log {
            recent_events_log.lock().push(input_event.clone());
        }
        self.pipeline_latency_ms.insert((now - event_time).into_millis());
        vec![input_event]
    }
}

/// Creates a new inspect handler instance.
///
/// `node` is the inspect node that will receive the stats.
pub fn make_inspect_handler(
    node: inspect::Node,
    supported_input_devices: &SortedVecSet<&InputDeviceType>,
    displays_recent_events: bool,
) -> Rc<InspectHandler<fn() -> zx::MonotonicInstant>> {
    InspectHandler::new_internal(
        node,
        zx::MonotonicInstant::get,
        supported_input_devices,
        displays_recent_events,
    )
}

impl<F> InspectHandler<F> {
    /// Creates a new inspect handler instance, using `now` to supply the current timestamp.
    /// Expected to be useful in testing mainly.
    fn new_internal(
        node: inspect::Node,
        now: F,
        supported_input_devices: &SortedVecSet<&InputDeviceType>,
        displays_recent_events: bool,
    ) -> Rc<Self> {
        let event_count = node.create_uint("events_count", 0);
        let events_with_wake_lease_count_node = node.create_uint("events_with_wake_lease_count", 0);
        let last_seen_timestamp_ns = node.create_int("last_seen_timestamp_ns", 0);
        let last_generated_timestamp_ns = node.create_int("last_generated_timestamp_ns", 0);

        let recent_events_log = match displays_recent_events {
            true => {
                let recent_events =
                    Arc::new(Mutex::new(CircularBuffer::new(MAX_RECENT_EVENT_LOG_SIZE)));
                record_lazy_recent_events(&node, Arc::clone(&recent_events));
                Some(recent_events)
            }
            false => None,
        };

        let pipeline_latency_ms = node
            .create_int_exponential_histogram("pipeline_latency_ms", LATENCY_HISTOGRAM_PROPERTIES);

        let mut health_node = fuchsia_inspect::health::Node::new(&node);
        health_node.set_starting_up();

        let mut events_by_type: [Option<EventCounters>; InputEventType::COUNT] = Default::default();
        if supported_input_devices.contains(&InputDeviceType::Keyboard) {
            events_by_type[InputEventType::Keyboard as usize] =
                Some(EventCounters::create(&node, InputEventType::Keyboard));
        }
        if supported_input_devices.contains(&InputDeviceType::ConsumerControls) {
            events_by_type[InputEventType::ConsumerControls as usize] =
                Some(EventCounters::create(&node, InputEventType::ConsumerControls));
        }
        if supported_input_devices.contains(&InputDeviceType::LightSensor) {
            events_by_type[InputEventType::LightSensor as usize] =
                Some(EventCounters::create(&node, InputEventType::LightSensor));
        }
        if supported_input_devices.contains(&InputDeviceType::Mouse) {
            events_by_type[InputEventType::Mouse as usize] =
                Some(EventCounters::create(&node, InputEventType::Mouse));
        }
        if supported_input_devices.contains(&InputDeviceType::Touch) {
            events_by_type[InputEventType::TouchScreen as usize] =
                Some(EventCounters::create(&node, InputEventType::TouchScreen));
            events_by_type[InputEventType::Touchpad as usize] =
                Some(EventCounters::create(&node, InputEventType::Touchpad));
        }
        #[cfg(test)]
        {
            events_by_type[InputEventType::Fake as usize] =
                Some(EventCounters::create(&node, InputEventType::Fake));
        }

        Rc::new(Self {
            now: RefCell::new(now),
            node,
            events_count: event_count,
            events_with_wake_lease_count: events_with_wake_lease_count_node,
            last_seen_timestamp_ns,
            last_generated_timestamp_ns,
            events_by_type,
            recent_events_log,
            pipeline_latency_ms,
            health_node: RefCell::new(health_node),
        })
    }
}

fn record_lazy_recent_events(
    node: &inspect::Node,
    recent_events: Arc<Mutex<CircularBuffer<InputEvent>>>,
) {
    node.record_lazy_child("recent_events_log", move || {
        let recent_events_clone = Arc::clone(&recent_events);
        async move {
            let inspector = Inspector::default();
            let events = recent_events_clone.lock();
            Ok(events.record_all_lazy_inspect(inspector))
        }
        .boxed()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input_device::{self, InputDeviceDescriptor, InputDeviceEvent};
    use crate::keyboard_binding::KeyboardDeviceDescriptor;
    use crate::light_sensor::types::Rgbc;
    use crate::light_sensor_binding::{LightSensorDeviceDescriptor, LightSensorEvent};
    use crate::mouse_binding::{
        MouseDeviceDescriptor, MouseLocation, MousePhase, PrecisionScroll, WheelDelta,
    };
    use crate::testing_utilities::{
        consumer_controls_device_descriptor, create_consumer_controls_event,
        create_fake_handled_input_event, create_fake_input_event, create_keyboard_event,
        create_mouse_event, create_touch_contact, create_touch_screen_event, create_touchpad_event,
        next_client_old_stream,
    };
    use crate::touch_binding::{TouchScreenDeviceDescriptor, TouchpadDeviceDescriptor};
    use crate::utils::Position;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fidl_fuchsia_input_report::InputDeviceMarker;
    use sorted_vec_map::SortedVecMap;
    use test_case::test_case;

    fn fixed_now() -> zx::MonotonicInstant {
        zx::MonotonicInstant::ZERO + zx::MonotonicDuration::from_nanos(42)
    }

    #[fuchsia::test]
    async fn circular_buffer_no_overflow() {
        let mut circular_buffer = CircularBuffer::new(MAX_RECENT_EVENT_LOG_SIZE);
        assert_eq!(circular_buffer._size, MAX_RECENT_EVENT_LOG_SIZE);

        let first_event_time = zx::MonotonicInstant::get();
        circular_buffer.push(create_fake_input_event(first_event_time));
        let second_event_time = zx::MonotonicInstant::get();
        circular_buffer.push(create_fake_input_event(second_event_time));

        // Fill up `events` VecDeque
        for _i in 2..MAX_RECENT_EVENT_LOG_SIZE {
            let curr_event_time = zx::MonotonicInstant::get();
            circular_buffer.push(create_fake_input_event(curr_event_time));
            match circular_buffer._events.back() {
                Some(event) => assert_eq!(event.event_time, curr_event_time),
                None => assert!(false),
            }
        }

        // Verify first event at the front
        match circular_buffer._events.front() {
            Some(event) => assert_eq!(event.event_time, first_event_time),
            None => assert!(false),
        }

        // CircularBuffer `events` should be full, pushing another event should remove the first event.
        let last_event_time = zx::MonotonicInstant::get();
        circular_buffer.push(create_fake_input_event(last_event_time));
        match circular_buffer._events.front() {
            Some(event) => assert_eq!(event.event_time, second_event_time),
            None => assert!(false),
        }
        match circular_buffer._events.back() {
            Some(event) => assert_eq!(event.event_time, last_event_time),
            None => assert!(false),
        }
    }

    #[fuchsia::test]
    async fn recent_events_log_records_inspect() {
        let inspector = fuchsia_inspect::Inspector::default();

        let recent_events_log =
            Arc::new(Mutex::new(CircularBuffer::new(MAX_RECENT_EVENT_LOG_SIZE)));
        record_lazy_recent_events(inspector.root(), Arc::clone(&recent_events_log));

        let keyboard_descriptor = InputDeviceDescriptor::Keyboard(KeyboardDeviceDescriptor {
            keys: vec![fidl_fuchsia_input::Key::A, fidl_fuchsia_input::Key::B],
            ..Default::default()
        });
        let mouse_descriptor = InputDeviceDescriptor::Mouse(MouseDeviceDescriptor {
            device_id: 1u32,
            absolute_x_range: None,
            absolute_y_range: None,
            wheel_v_range: None,
            wheel_h_range: None,
            buttons: None,
        });
        let touch_screen_descriptor =
            InputDeviceDescriptor::TouchScreen(TouchScreenDeviceDescriptor {
                device_id: 1,
                contacts: vec![],
            });
        let touchpad_descriptor = InputDeviceDescriptor::Touchpad(TouchpadDeviceDescriptor {
            device_id: 1,
            contacts: vec![],
        });

        let pressed_buttons = SortedVecSet::from(vec![1u8, 21u8, 15u8]);
        let mut pressed_buttons_vec: Vec<u64> = vec![];
        pressed_buttons.iter().for_each(|button| {
            pressed_buttons_vec.push(*button as u64);
        });

        let (light_sensor_proxy, _) = next_client_old_stream::<
            InputDeviceMarker,
            fidl_next_fuchsia_input_report::InputDevice,
        >();

        let recent_events = vec![
            create_keyboard_event(
                fidl_fuchsia_input::Key::A,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                &keyboard_descriptor,
                None,
            ),
            create_consumer_controls_event(
                vec![
                    fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
                    fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
                    fidl_fuchsia_input::ConsumerControlButton::Pause,
                    fidl_fuchsia_input::ConsumerControlButton::VolumeDown,
                    fidl_fuchsia_input::ConsumerControlButton::MicMute,
                    fidl_fuchsia_input::ConsumerControlButton::CameraDisable,
                    fidl_fuchsia_input::ConsumerControlButton::FactoryReset,
                    fidl_fuchsia_input::ConsumerControlButton::Reboot,
                ],
                zx::MonotonicInstant::get(),
                &consumer_controls_device_descriptor(),
            ),
            create_mouse_event(
                MouseLocation::Absolute(Position { x: 7.0f32, y: 15.0f32 }),
                Some(WheelDelta { ticks: 5i64, physical_pixel: Some(8.0f32) }),
                Some(WheelDelta { ticks: 10i64, physical_pixel: Some(8.0f32) }),
                Some(PrecisionScroll::Yes),
                MousePhase::Move,
                SortedVecSet::from(vec![1u8]),
                pressed_buttons.clone(),
                zx::MonotonicInstant::get(),
                &mouse_descriptor,
            ),
            create_touch_screen_event(
                SortedVecMap::from_iter(vec![
                    (
                        fidl_fuchsia_ui_input::PointerEventPhase::Add,
                        vec![create_touch_contact(1u32, Position { x: 10.0, y: 30.0 })],
                    ),
                    (
                        fidl_fuchsia_ui_input::PointerEventPhase::Move,
                        vec![create_touch_contact(1u32, Position { x: 11.0, y: 31.0 })],
                    ),
                ]),
                zx::MonotonicInstant::get(),
                &touch_screen_descriptor,
            ),
            create_touchpad_event(
                vec![
                    create_touch_contact(1u32, Position { x: 0.0, y: 0.0 }),
                    create_touch_contact(2u32, Position { x: 10.0, y: 10.0 }),
                ],
                SortedVecSet::new(),
                zx::MonotonicInstant::get(),
                &touchpad_descriptor,
            ),
            InputEvent {
                device_event: InputDeviceEvent::LightSensor(LightSensorEvent {
                    device_proxy: light_sensor_proxy,
                    rgbc: Rgbc { red: 1, green: 2, blue: 3, clear: 14747 },
                }),
                device_descriptor: InputDeviceDescriptor::LightSensor(
                    LightSensorDeviceDescriptor {
                        vendor_id: 1,
                        product_id: 2,
                        device_id: 3,
                        sensor_layout: Rgbc { red: 1, green: 2, blue: 3, clear: 4 },
                    },
                ),
                event_time: zx::MonotonicInstant::get(),
                handled: input_device::Handled::No,
                trace_id: None,
            },
            create_keyboard_event(
                fidl_fuchsia_input::Key::B,
                fidl_fuchsia_ui_input3::KeyEventType::Pressed,
                None,
                &keyboard_descriptor,
                None,
            ),
        ];

        for event in recent_events.into_iter() {
            recent_events_log.lock().push(event);
        }

        assert_data_tree!(inspector, root: {
            recent_events_log: {
                "000_keyboard_event": {
                    event_time: AnyProperty,
                },
                "001_consumer_controls_event": {
                    event_time: AnyProperty,
                    pressed_buttons: vec!["volume_up", "volume_up", "pause", "volume_down", "mic_mute", "camera_disable", "factory_reset", "reboot"],
                },
                "002_mouse_event": {
                    event_time: AnyProperty,
                    wheel_delta_v: {
                        ticks: 5i64,
                        physical_pixel: 8.0f64,
                    },
                    wheel_delta_h: {
                        ticks: 10i64,
                        physical_pixel: 8.0f64,
                    },
                    is_precision_scroll: "yes",
                    phase: "move",
                    affected_buttons: vec![1u64],
                    pressed_buttons: pressed_buttons_vec.clone(),
                },
                "003_touch_screen_event": {
                    event_time: AnyProperty,
                    injector_contacts: {
                        add: {
                            "1": {},
                        },
                        change: {
                            "1": {},
                        },
                        remove: {},
                    },
                    pressed_buttons: Vec::<String>::new(),
                },
                "004_touchpad_event": {
                    event_time: AnyProperty,
                    pressed_buttons: Vec::<u64>::new(),
                    injector_contacts: {
                        "1": {},
                        "2": {},
                    },
                },
                "005_light_sensor_event": {
                    event_time: AnyProperty,
                    red: 1u64,
                    green: 2u64,
                    blue: 3u64,
                    clear: 14747u64,
                },
                "006_keyboard_event": {
                    event_time: AnyProperty,
                },
            }
        });
    }

    #[fuchsia::test]
    async fn verify_inspect_no_recent_events_log() {
        let inspector = inspect::Inspector::default();
        let root = inspector.root();
        let test_node = root.create_child("test_node");
        let supported_input_devices: SortedVecSet<&InputDeviceType> = SortedVecSet::from([
            &input_device::InputDeviceType::Keyboard,
            &input_device::InputDeviceType::ConsumerControls,
            &input_device::InputDeviceType::LightSensor,
            &input_device::InputDeviceType::Mouse,
            &input_device::InputDeviceType::Touch,
        ]);

        let handler = super::InspectHandler::new_internal(
            test_node,
            fixed_now,
            &supported_input_devices,
            /* displays_recent_events = */ false,
        );
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 0u64,
                last_seen_timestamp_ns: 0i64,
                last_generated_timestamp_ns: 0i64,
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
           }
        });

        handler
            .clone()
            .handle_input_event(create_fake_input_event(zx::MonotonicInstant::from_nanos(43i64)))
            .await;
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 1u64,
                last_seen_timestamp_ns: 42i64,
                last_generated_timestamp_ns: 43i64,
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 1u64,
                     events_with_wake_lease_count: 0u64, // Fake event doesn't have wake lease in this test setup
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 43i64,
                     last_seen_timestamp_ns: 42i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
            }
        });

        handler
            .clone()
            .handle_input_event(create_fake_input_event(zx::MonotonicInstant::from_nanos(44i64)))
            .await;
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 2u64,
                last_seen_timestamp_ns: 42i64,
                last_generated_timestamp_ns: 44i64,
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 2u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 44i64,
                     last_seen_timestamp_ns: 42i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
            }
        });

        handler
            .clone()
            .handle_input_event(create_fake_handled_input_event(zx::MonotonicInstant::from_nanos(
                44,
            )))
            .await;
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 3u64,
                last_seen_timestamp_ns: 42i64,
                last_generated_timestamp_ns: 44i64,
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 3u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 1u64,
                     last_generated_timestamp_ns: 44i64,
                     last_seen_timestamp_ns: 42i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
            }
        });
    }

    #[fuchsia::test]
    async fn verify_inspect_with_recent_events_log() {
        let inspector = inspect::Inspector::default();
        let root = inspector.root();
        let test_node = root.create_child("test_node");
        let supported_input_devices: SortedVecSet<&InputDeviceType> = SortedVecSet::from([
            &input_device::InputDeviceType::Keyboard,
            &input_device::InputDeviceType::ConsumerControls,
            &input_device::InputDeviceType::LightSensor,
            &input_device::InputDeviceType::Mouse,
            &input_device::InputDeviceType::Touch,
        ]);

        let handler = super::InspectHandler::new_internal(
            test_node,
            fixed_now,
            &supported_input_devices,
            /* displays_recent_events = */ true,
        );
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 0u64,
                last_seen_timestamp_ns: 0i64,
                last_generated_timestamp_ns: 0i64,
                recent_events_log: {},
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
           }
        });

        handler
            .clone()
            .handle_input_event(create_fake_input_event(zx::MonotonicInstant::from_nanos(43i64)))
            .await;
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 1u64,
                last_seen_timestamp_ns: 42i64,
                last_generated_timestamp_ns: 43i64,
                recent_events_log: {
                    "000_fake_event": {
                        event_time: 43i64,
                    },
                },
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 1u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 43i64,
                     last_seen_timestamp_ns: 42i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
            }
        });

        handler
            .clone()
            .handle_input_event(create_fake_input_event(zx::MonotonicInstant::from_nanos(44i64)))
            .await;
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 2u64,
                last_seen_timestamp_ns: 42i64,
                last_generated_timestamp_ns: 44i64,
                recent_events_log: {
                    "000_fake_event": {
                        event_time: 43i64,
                    },
                    "001_fake_event": {
                        event_time: 44i64,
                    },
                },
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 2u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 44i64,
                     last_seen_timestamp_ns: 42i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
            }
        });

        handler
            .clone()
            .handle_input_event(create_fake_handled_input_event(zx::MonotonicInstant::from_nanos(
                44,
            )))
            .await;
        assert_data_tree!(inspector, root: {
            test_node: contains {
                events_count: 3u64,
                last_seen_timestamp_ns: 42i64,
                last_generated_timestamp_ns: 44i64,
                recent_events_log: {
                    "000_fake_event": {
                        event_time: 43i64,
                    },
                    "001_fake_event": {
                        event_time: 44i64,
                    },
                    "002_fake_event": {
                        event_time: 44i64,
                    },
                },
                consumer_controls: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                fake: {
                     events_count: 3u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 1u64,
                     last_generated_timestamp_ns: 44i64,
                     last_seen_timestamp_ns: 42i64,
                },
                keyboard: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                light_sensor: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                mouse: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touch_screen: {
                     events_count: 0u64,
                     events_with_wake_lease_count: 0u64,
                     handled_events_count: 0u64,
                     last_generated_timestamp_ns: 0i64,
                     last_seen_timestamp_ns: 0i64,
                },
                touchpad: {
                    events_count: 0u64,
                    events_with_wake_lease_count: 0u64,
                    handled_events_count: 0u64,
                    last_generated_timestamp_ns: 0i64,
                    last_seen_timestamp_ns: 0i64,
               },
            }
        });
    }

    #[test_case([i64::MIN]; "min value")]
    #[test_case([-1]; "negative value")]
    #[test_case([0]; "zero")]
    #[test_case([1]; "positive value")]
    #[test_case([i64::MAX]; "max value")]
    #[test_case([1_000_000, 10_000_000, 100_000_000, 1000_000_000]; "multiple values")]
    #[fuchsia::test(allow_stalls = false)]
    async fn updates_latency_histogram(
        latencies_nsec: impl IntoIterator<Item = i64> + Clone + 'static,
    ) {
        let inspector = inspect::Inspector::default();
        let root = inspector.root();
        let test_node = root.create_child("test_node");

        let mut seen_timestamps =
            latencies_nsec.clone().into_iter().map(zx::MonotonicInstant::from_nanos);
        let now = move || {
            seen_timestamps.next().expect("internal error: test has more events than latencies")
        };
        let handler = super::InspectHandler::new_internal(
            test_node,
            now,
            &SortedVecSet::new(),
            /* displays_recent_events = */ false,
        );
        for _latency in latencies_nsec.clone() {
            handler
                .clone()
                .handle_input_event(create_fake_input_event(zx::MonotonicInstant::ZERO))
                .await;
        }

        let mut histogram_assertion = diagnostics_assertions::HistogramAssertion::exponential(
            super::LATENCY_HISTOGRAM_PROPERTIES,
        );
        histogram_assertion
            .insert_values(latencies_nsec.into_iter().map(|nsec| nsec / 1000 / 1000));
        assert_data_tree!(inspector, root: {
            test_node: contains {
                pipeline_latency_ms: histogram_assertion
            }
        })
    }
}
