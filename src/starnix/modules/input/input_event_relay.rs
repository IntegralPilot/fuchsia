// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{InputDeviceStatus, InputFile, uinput};
use fidl::endpoints::{ClientEnd, RequestStream};
use fidl_fuchsia_ui_input::TouchDeviceInfo;
use fidl_fuchsia_ui_input3::{
    KeyEventStatus, KeyboardListenerMarker, KeyboardListenerRequest, KeyboardListenerRequestStream,
    KeyboardSynchronousProxy,
};
use fidl_fuchsia_ui_pointer::{
    MouseEvent as FidlMouseEvent, TouchEvent as FidlTouchEvent, TouchPointerSample,
    {self as fuipointer},
};
use fidl_fuchsia_ui_policy as fuipolicy;
use fidl_fuchsia_ui_views as fuiviews;
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot::{self, Sender};
use futures::executor::block_on;
use sorted_vec_map::SortedVecMap;
use starnix_core::power::{ContainerWakingStream, create_proxy_for_wake_events_counter};
use starnix_core::task::dynamic_thread_spawner::SpawnRequestBuilder;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_logging::log_warn;
use starnix_modules_input_event_conversion::button_fuchsia_to_linux::{
    new_touch_buttons_bitvec, parse_fidl_media_button_event, parse_fidl_touch_button_event,
};
use starnix_modules_input_event_conversion::key_fuchsia_to_linux::parse_fidl_keyboard_event_to_linux_input_event;
use starnix_modules_input_event_conversion::mouse_fuchsia_to_linux::FuchsiaMouseEventToLinuxMouseEventConverter;
use starnix_modules_input_event_conversion::touch_fuchsia_to_linux::FuchsiaTouchEventToLinuxTouchEventConverter;
use starnix_sync::{InputEventRelayOpenedFilesLock, LockDepMutex};
use starnix_uapi::uapi;
use std::collections::VecDeque;
use std::sync::{Arc, Weak};

const INPUT_RELAY_ROLE_NAME: &str = "fuchsia.starnix.kthread.input_relay";

#[derive(Clone, Copy)]
pub enum EventProxyMode {
    /// Don't proxy input events at all.
    None,

    /// Have the Starnix runner proxy events such that the container
    /// will wake up if events are received while the container is
    /// suspended.
    WakeContainer,
}

pub type OpenedFiles = Arc<LockDepMutex<Vec<Weak<InputFile>>, InputEventRelayOpenedFilesLock>>;

pub enum InputDeviceType {
    Touch(FuchsiaTouchEventToLinuxTouchEventConverter),
    Keyboard,
    Mouse(FuchsiaMouseEventToLinuxMouseEventConverter),
}

impl std::fmt::Display for InputDeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InputDeviceType::Touch(_) => write!(f, "touch"),
            InputDeviceType::Keyboard => write!(f, "keyboard"),
            InputDeviceType::Mouse(_) => write!(f, "mouse"),
        }
    }
}

pub struct DeviceState {
    device_type: InputDeviceType,
    open_files: OpenedFiles,
    inspect_status: Option<Arc<InputDeviceStatus>>,
}

pub struct TrackedWakeLease {
    _lease: fidl::EventPair,
    device_status: Arc<InputDeviceStatus>,
}

impl TrackedWakeLease {
    pub fn new(lease: fidl::EventPair, device_status: Arc<InputDeviceStatus>) -> Self {
        device_status.increment_active_wake_leases(1);
        device_status.count_events_with_wake_lease(1);
        Self { _lease: lease, device_status }
    }
}

impl Drop for TrackedWakeLease {
    fn drop(&mut self) {
        self.device_status.decrement_active_wake_leases(1);
    }
}

pub type DeviceId = u32;

pub const DEFAULT_TOUCH_DEVICE_ID: DeviceId = 0;
pub const DEFAULT_KEYBOARD_DEVICE_ID: DeviceId = 1;
pub const DEFAULT_MOUSE_DEVICE_ID: DeviceId = 2;

enum DeviceStateChange {
    Add(DeviceId, DeviceState, Sender<()>),
    Remove(DeviceId, Sender<()>),
}

pub fn new_input_relay() -> (InputEventsRelay, Arc<InputEventsRelayHandle>) {
    let (sender, receiver) = unbounded();

    (
        InputEventsRelay { devices: SortedVecMap::new(), receiver },
        Arc::new(InputEventsRelayHandle { sender }),
    )
}

pub struct InputEventsRelayHandle {
    sender: UnboundedSender<DeviceStateChange>,
}

impl InputEventsRelayHandle {
    pub fn add_touch_device(
        self: &Arc<Self>,
        device_id: DeviceId,
        open_files: OpenedFiles,
        inspect_status: Option<Arc<InputDeviceStatus>>,
    ) {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.unbounded_send(DeviceStateChange::Add(
            device_id,
            DeviceState {
                device_type: InputDeviceType::Touch(
                    FuchsiaTouchEventToLinuxTouchEventConverter::create(),
                ),
                open_files,
                inspect_status,
            },
            sender,
        ));
        let _ = block_on(receiver);
    }

    pub fn add_keyboard_device(
        &self,
        device_id: DeviceId,
        open_files: OpenedFiles,
        inspect_status: Option<Arc<InputDeviceStatus>>,
    ) {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.unbounded_send(DeviceStateChange::Add(
            device_id,
            DeviceState { device_type: InputDeviceType::Keyboard, open_files, inspect_status },
            sender,
        ));
        let _ = block_on(receiver);
    }

    pub fn add_mouse_device(
        &self,
        device_id: DeviceId,
        open_files: OpenedFiles,
        inspect_status: Option<Arc<InputDeviceStatus>>,
    ) {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.unbounded_send(DeviceStateChange::Add(
            device_id,
            DeviceState {
                device_type: InputDeviceType::Mouse(
                    FuchsiaMouseEventToLinuxMouseEventConverter::create(),
                ),
                open_files,
                inspect_status,
            },
            sender,
        ));
        let _ = block_on(receiver);
    }

    pub fn remove_device(&self, device_id: DeviceId) {
        let (sender, receiver) = oneshot::channel();
        let _ = self.sender.unbounded_send(DeviceStateChange::Remove(device_id, sender));
        let _ = block_on(receiver);
    }
}

pub struct InputEventsRelay {
    devices: SortedVecMap<DeviceId, DeviceState>,
    receiver: UnboundedReceiver<DeviceStateChange>,
}

impl InputEventsRelay {
    // TODO(https://fxbug.dev/371602479): Use `fuchsia.ui.SupportedInputDevices` to create
    // relays.
    // start_relays will take over the ownership of InputEventsRelay.
    pub fn start_relays(
        mut self: Self,
        kernel: &Kernel,
        event_proxy_mode: EventProxyMode,
        touch_source_client_end: ClientEnd<fuipointer::TouchSourceV2Marker>,
        keyboard: KeyboardSynchronousProxy,
        mouse_source_client_end: ClientEnd<fuipointer::MouseSourceV2Marker>,
        view_ref: fuiviews::ViewRef,
        registry_proxy: fuipolicy::DeviceListenerRegistrySynchronousProxy,
        default_touch_device_opened_files: OpenedFiles,
        default_keyboard_device_opened_files: OpenedFiles,
        default_mouse_device_opened_files: OpenedFiles,
        default_touch_device_inspect: Option<Arc<InputDeviceStatus>>,
        default_keyboard_device_inspect: Option<Arc<InputDeviceStatus>>,
        default_mouse_device_inspect: Option<Arc<InputDeviceStatus>>,
    ) {
        let f = async move |current_task: &CurrentTask| {
            let kernel = current_task.kernel();
            // touch
            let (mut default_touch_device, touch_source_proxy, mut touch_waking_stream) =
                setup_touch_relay(
                    kernel,
                    event_proxy_mode,
                    touch_source_client_end,
                    default_touch_device_opened_files,
                    default_touch_device_inspect,
                );
            let mut touch_future = touch_waking_stream.next();

            // mouse
            let (mut default_mouse_device, mouse_source_proxy, mut mouse_waking_stream) =
                setup_mouse_relay(
                    kernel,
                    event_proxy_mode,
                    mouse_source_client_end,
                    default_mouse_device_opened_files,
                    default_mouse_device_inspect,
                );
            let mut mouse_future = mouse_waking_stream.next();

            // keyboard
            // `_keyboard_proxy` is load-bearing despite being unused: it holds the channel to
            // text_manager open for the lifetime of the relay loop below. Dropping it closes
            // the channel, which deregisters our `KeyboardListener` and silently stops all
            // key delivery. Do not remove it as an unused binding.
            let (mut default_keyboard_device, mut keyboard_event_stream, _keyboard_proxy) =
                setup_keyboard_relay(
                    keyboard,
                    view_ref,
                    default_keyboard_device_opened_files.clone(),
                    default_keyboard_device_inspect.clone(),
                );

            // button
            let (
                mut default_button_device,
                mut media_buttons_waking_stream,
                mut touch_buttons_waking_stream,
            ) = setup_button_relay(
                kernel,
                registry_proxy,
                event_proxy_mode,
                default_keyboard_device_opened_files,
                default_keyboard_device_inspect,
            );
            let mut media_buttons_future = media_buttons_waking_stream.next();
            let mut touch_buttons_future = touch_buttons_waking_stream.next();

            let mut power_was_pressed = false;
            let mut function_was_pressed = false;
            let mut volume_up_was_pressed = false;
            let mut volume_down_was_pressed = false;
            let mut touch_buttons_were_pressed = new_touch_buttons_bitvec();

            loop {
                futures::select! {
                    touch_res = touch_future => {
                        match touch_res {
                            Some(Ok(fuipointer::TouchSourceV2Event::OnTouchEvents {
                                events,
                                last_event_stamp,
                            })) => {
                                self.process_touch_event(
                                    &mut default_touch_device,
                                    events,
                                );
                                if let Err(e) = touch_source_proxy.acknowledge_events(last_event_stamp) {
                                    log_warn!("error acknowledging touch events: {:?}", e);
                                }
                                touch_future = touch_waking_stream.next();
                            }
                            Some(Ok(fuipointer::TouchSourceV2Event::_UnknownEvent { ordinal, .. })) => {
                                log_warn!("unknown event on TouchSourceV2: {}", ordinal);
                                touch_future = touch_waking_stream.next();
                            }
                            Some(Err(e)) => {
                                log_warn!(
                                    "error {:?} reading from TouchSourceV2Proxy; input is stopped",
                                    e
                                );
                            }
                            None => {}
                        }
                    }
                    mouse_res = mouse_future => {
                        match mouse_res {
                            Some(Ok(fuipointer::MouseSourceV2Event::OnMouseEvents {
                                events,
                                last_event_stamp,
                            })) => {
                                self.process_mouse_event(&mut default_mouse_device, events);
                                if let Err(e) = mouse_source_proxy.acknowledge_events(last_event_stamp) {
                                    log_warn!("error acknowledging mouse events: {:?}", e);
                                }
                                mouse_future = mouse_waking_stream.next();
                            }
                            Some(Ok(fuipointer::MouseSourceV2Event::_UnknownEvent { ordinal, .. })) => {
                                log_warn!("unknown event on MouseSourceV2: {}", ordinal);
                                mouse_future = mouse_waking_stream.next();
                            }
                            Some(Err(e)) => {
                                log_warn!(
                                    "error {:?} reading from MouseSourceV2Proxy; input is stopped",
                                    e
                                );
                            }
                            None => {}
                        }
                    }
                    media_buttons_res = media_buttons_future => {
                        match media_buttons_res {
                            Some(Ok(event)) => {
                                (
                                    power_was_pressed,
                                    function_was_pressed,
                                    volume_up_was_pressed,
                                    volume_down_was_pressed,
                                ) = self.process_media_button_event(
                                    &mut default_button_device,
                                    event,
                                    power_was_pressed,
                                    function_was_pressed,
                                    volume_up_was_pressed,
                                    volume_down_was_pressed,
                                );
                                media_buttons_future = media_buttons_waking_stream.next();
                            }
                            _ => {}
                        }
                    }
                    touch_buttons_res = touch_buttons_future => {
                        match touch_buttons_res {
                            Some(Ok(event)) => {
                                touch_buttons_were_pressed = self.process_touch_button_event(
                                    &mut default_touch_device,
                                    event,
                                    &touch_buttons_were_pressed,
                                );
                                touch_buttons_future = touch_buttons_waking_stream.next();
                            }
                            _ => {}
                        }
                    }
                    e = keyboard_event_stream.next() => {
                        match e  {
                            Some(Ok(request)) => {
                                self.process_keyboard(&mut default_keyboard_device, request);
                            }
                            _ => {}
                        }
                    }
                    e = self.receiver.next() => {
                        match e {
                            Some(event) => {
                                match event {
                                    DeviceStateChange::Add(id, device_state, sender) => {
                                        self.devices.insert(id, device_state);
                                        let _ = sender.send(());
                                    }
                                    DeviceStateChange::Remove(id, sender) => {
                                        self.devices.remove(&id);
                                        let _ = sender.send(());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    complete => break,
                }
            }
        };
        let req = SpawnRequestBuilder::new()
            .with_debug_name("input-event-relay")
            .with_role(INPUT_RELAY_ROLE_NAME)
            .with_async_closure(f)
            .build();
        kernel.kthreads.spawner().spawn_from_request(req);
    }

    fn process_touch_event(
        self: &mut Self,
        default_touch_device: &mut DeviceState,
        touch_events: Vec<FidlTouchEvent>,
    ) {
        fuchsia_trace::duration!("input", "starnix_process_touch_event");
        for e in &touch_events {
            match e.trace_flow_id {
                Some(trace_flow_id) => {
                    fuchsia_trace::flow_end!(
                        "input",
                        "dispatch_event_to_client",
                        trace_flow_id.into()
                    );
                }
                None => {
                    log_warn!("touch event has not tracing id");
                }
            }
        }
        let num_received_events: u64 = touch_events.len().try_into().unwrap();

        let mut num_ignored_events: u64 = 0;

        // 1 vec may contains events from different device.
        let (events_by_device, ignored_events) = group_touch_events_by_device_id(touch_events);
        num_ignored_events += ignored_events;

        for (device_id, mut events) in events_by_device {
            fuchsia_trace::duration_begin!("input", "starnix_process_per_device_touch_event");

            let dev = self.devices.get_mut(&device_id).unwrap_or(default_touch_device);

            let mut num_converted_events: u64 = 0;
            let mut num_unexpected_events: u64 = 0;
            let mut new_events: VecDeque<uapi::input_event> = VecDeque::new();

            #[allow(clippy::collection_is_never_read)]
            let mut tracked_leases = vec![];
            for event in &mut events {
                if let Some(lease) = event.wake_lease.take() {
                    if let Some(status) = &dev.inspect_status {
                        tracked_leases.push(TrackedWakeLease::new(lease, status.clone()));
                    }
                }
            }

            let last_event_time_ns: i64;
            if let InputDeviceType::Touch(ref mut converter) = dev.device_type {
                let mut batch = converter.handle(events);
                new_events.append(&mut batch.events);
                num_converted_events += batch.count_converted_fidl_events;
                num_ignored_events += batch.count_ignored_fidl_events;
                num_unexpected_events += batch.count_unexpected_fidl_events;
                last_event_time_ns = batch.last_event_time_ns;
            } else {
                fuchsia_trace::duration_end!("input", "starnix_process_per_device_touch_event");
                log_warn!(
                    "Non touch device received touch events: device_id = {}, device_type = {}",
                    device_id,
                    dev.device_type
                );
                continue;
            }

            if let Some(dev_inspect_status) = &dev.inspect_status {
                dev_inspect_status.count_total_received_events(num_received_events);
                dev_inspect_status.count_total_ignored_events(num_ignored_events);
                dev_inspect_status.count_total_unexpected_events(num_unexpected_events);
                dev_inspect_status.count_total_converted_events(num_converted_events);
                dev_inspect_status.count_total_generated_events(
                    new_events.len().try_into().unwrap(),
                    last_event_time_ns,
                );
            } else {
                log_warn!(
                    "unable to record inspect for device_id: {}, device_type: {}",
                    device_id,
                    dev.device_type
                );
            }

            fuchsia_trace::duration_end!("input", "starnix_process_per_device_touch_event");
            dev.open_files.lock().retain(|f| {
                let Some(file) = f.upgrade() else {
                    log_warn!("Dropping input file for touch that failed to upgrade");
                    return false;
                };
                match &file.inspect_status {
                    Some(file_inspect_status) => {
                        file_inspect_status.count_received_events(num_received_events);
                        file_inspect_status.count_ignored_events(num_ignored_events);
                        file_inspect_status.count_unexpected_events(num_unexpected_events);
                        file_inspect_status.count_converted_events(num_converted_events);
                    }
                    None => {
                        log_warn!("unable to record inspect within the input file")
                    }
                }
                if !new_events.is_empty() {
                    // TODO(https://fxbug.dev/42075438): Reading from an `InputFile` should
                    // not provide access to events that occurred before the file was
                    // opened.
                    if let Some(file_inspect_status) = &file.inspect_status {
                        file_inspect_status.count_generated_events(
                            new_events.len().try_into().unwrap(),
                            last_event_time_ns,
                        );
                    }
                    file.add_events(new_events.clone().into_iter().collect());
                }

                true
            });
        }
    }

    fn process_keyboard(
        self: &mut Self,
        default_keyboard_device: &mut DeviceState,
        request: KeyboardListenerRequest,
    ) {
        match request {
            KeyboardListenerRequest::OnKeyEvent { event, responder } => {
                fuchsia_trace::duration!("input", "starnix_process_keyboard_event");

                let new_events = parse_fidl_keyboard_event_to_linux_input_event(
                    &event,
                    uinput::uinput_running(),
                );

                let dev = match event.device_id {
                    Some(device_id) => {
                        self.devices.get_mut(&device_id).unwrap_or(default_keyboard_device)
                    }
                    None => default_keyboard_device,
                };

                // These counters are denominated in FIDL events, not uapi events: the
                // documented invariant is received = ignored + unexpected + converted (see
                // `InputDeviceStatus`). One FIDL key event converts to several uapi events
                // (the key itself plus a SYN), so only the *generated* counters take
                // `new_events.len()`.
                let (converted_events, ignored_events, generated_events) = match new_events.len() {
                    0 => (0u64, 1u64, 0u64),
                    len => (1u64, 0u64, len as u64),
                };
                let last_time = event.timestamp.unwrap_or(0);

                if let Some(dev_inspect_status) = &dev.inspect_status {
                    dev_inspect_status.count_total_received_events(1);
                    dev_inspect_status.count_total_ignored_events(ignored_events);
                    dev_inspect_status.count_total_converted_events(converted_events);
                    // Guarded because `count_total_generated_events` *stores* the timestamp:
                    // calling it with a count of 0 would move
                    // `last_generated_uapi_event_timestamp_ns` on an event that generated
                    // nothing.
                    if generated_events > 0 {
                        dev_inspect_status
                            .count_total_generated_events(generated_events, last_time);
                    }
                } else {
                    log_warn!("unable to record inspect for keyboard device");
                }

                dev.open_files.lock().retain(|f| {
                    let Some(file) = f.upgrade() else {
                        log_warn!("Dropping input file for keyboard that failed to upgrade");
                        return false;
                    };
                    match &file.inspect_status {
                        Some(file_inspect_status) => {
                            file_inspect_status.count_received_events(1);
                            file_inspect_status.count_ignored_events(ignored_events);
                            file_inspect_status.count_converted_events(converted_events);
                            if generated_events > 0 {
                                file_inspect_status
                                    .count_generated_events(generated_events, last_time);
                            }
                        }
                        None => {
                            log_warn!("unable to record inspect within the input file")
                        }
                    }
                    if !new_events.is_empty() {
                        file.add_events(new_events.clone().into_iter().collect());
                    }

                    true
                });

                responder.send(KeyEventStatus::Handled).expect("");
            }
        }
    }

    fn process_media_button_event(
        &mut self,
        default_button_device: &mut DeviceState,
        button_event: fuipolicy::MediaButtonsListenerRequest,
        power_was_pressed: bool,
        function_was_pressed: bool,
        volume_up_was_pressed: bool,
        volume_down_was_pressed: bool,
    ) -> (bool, bool, bool, bool) {
        let mut power_was_pressed_after = false;
        let mut function_was_pressed_after = false;
        let mut volume_up_was_pressed_after = false;
        let mut volume_down_was_pressed_after = false;
        match button_event {
            fuipolicy::MediaButtonsListenerRequest::OnEvent { mut event, responder } => {
                if let Some(trace_flow_id) = event.trace_flow_id {
                    fuchsia_trace::flow_end!(
                        "input",
                        "dispatch_media_buttons_to_listeners",
                        trace_flow_id.into()
                    );
                }
                fuchsia_trace::duration!("input", "starnix_process_media_button_event");

                let batch = parse_fidl_media_button_event(
                    &event,
                    power_was_pressed,
                    function_was_pressed,
                    volume_up_was_pressed,
                    volume_down_was_pressed,
                );

                power_was_pressed_after = batch.power_is_pressed;
                function_was_pressed_after = batch.function_is_pressed;
                volume_up_was_pressed_after = batch.volume_up_is_pressed;
                volume_down_was_pressed_after = batch.volume_down_is_pressed;

                let (converted_events, ignored_events, generated_events) = match batch.events.len()
                {
                    0 => (0u64, 1u64, 0u64),
                    len => {
                        if len % 2 == 1 {
                            log_warn!(
                                "unexpectedly received {} events: there should always be an even number of non-empty events.",
                                len
                            );
                        }
                        (1u64, 0u64, len as u64)
                    }
                };

                let dev = match event.device_id {
                    Some(device_id) => {
                        self.devices.get_mut(&device_id).unwrap_or(default_button_device)
                    }
                    None => default_button_device,
                };

                #[allow(clippy::collection_is_never_read)]
                let mut tracked_leases = vec![];
                if let Some(lease) = event.wake_lease.take() {
                    if let Some(status) = &dev.inspect_status {
                        tracked_leases.push(TrackedWakeLease::new(lease, status.clone()));
                    }
                }

                if let Some(dev_inspect_status) = &dev.inspect_status {
                    dev_inspect_status.count_total_received_events(1);
                    dev_inspect_status.count_total_ignored_events(ignored_events);
                    dev_inspect_status.count_total_converted_events(converted_events);
                    dev_inspect_status.count_total_generated_events(
                        generated_events,
                        batch.event_time.into_nanos().try_into().unwrap(),
                    );
                } else {
                    log_warn!("unable to record inspect for button device");
                }

                dev.open_files.lock().retain(|f| {
                    let Some(file) = f.upgrade() else {
                        log_warn!("Dropping input file for buttons that failed to upgrade");
                        return false;
                    };
                    match &file.inspect_status {
                        Some(file_inspect_status) => {
                            file_inspect_status.count_received_events(1);
                            file_inspect_status.count_ignored_events(ignored_events);
                            file_inspect_status.count_converted_events(converted_events);
                        }
                        None => {
                            log_warn!("unable to record inspect within the input file")
                        }
                    }
                    if !batch.events.is_empty() {
                        if let Some(file_inspect_status) = &file.inspect_status {
                            file_inspect_status.count_generated_events(
                                generated_events,
                                batch.event_time.into_nanos().try_into().unwrap(),
                            );
                        }
                        file.add_events(batch.events.clone());
                    }

                    true
                });

                responder.send().expect("media buttons responder failed to respond");
            }
            _ => { /* Ignore deprecated OnMediaButtonsEvent */ }
        }

        (
            power_was_pressed_after,
            function_was_pressed_after,
            volume_up_was_pressed_after,
            volume_down_was_pressed_after,
        )
    }

    fn process_touch_button_event(
        &mut self,
        default_touch_device: &mut DeviceState,
        button_event: fuipolicy::TouchButtonsListenerRequest,
        touch_buttons_were_pressed: &bit_vec::BitVec,
    ) -> bit_vec::BitVec {
        fuchsia_trace::duration!("input", "starnix_process_touch_button_event");
        match button_event {
            fuipolicy::TouchButtonsListenerRequest::OnEvent { mut event, responder } => {
                if let Some(trace_flow_id) = event.trace_flow_id {
                    fuchsia_trace::flow_end!(
                        "input",
                        "dispatch_touch_button_to_listeners",
                        trace_flow_id.into()
                    );
                }
                let batch = parse_fidl_touch_button_event(&event, touch_buttons_were_pressed);

                let (converted_events, ignored_events, generated_events) = match batch.events.len()
                {
                    0 => (0u64, 1u64, 0u64),
                    len => {
                        if len % 2 == 1 {
                            log_warn!(
                                "unexpectedly received {} events: there should always be an even number of non-empty events.",
                                len
                            );
                        }
                        (1u64, 0u64, len as u64)
                    }
                };

                let device_id = match &event.device_info {
                    Some(TouchDeviceInfo { id: Some(id), .. }) => Some(*id),
                    _ => None,
                };

                let dev = match device_id {
                    Some(id) => self.devices.get_mut(&id).unwrap_or(default_touch_device),
                    None => default_touch_device,
                };

                #[allow(clippy::collection_is_never_read)]
                let mut tracked_leases = vec![];
                if let Some(lease) = event.wake_lease.take() {
                    if let Some(status) = &dev.inspect_status {
                        tracked_leases.push(TrackedWakeLease::new(lease, status.clone()));
                    }
                }

                if let Some(dev_inspect_status) = &dev.inspect_status {
                    dev_inspect_status.count_total_received_events(1);
                    dev_inspect_status.count_total_ignored_events(ignored_events);
                    dev_inspect_status.count_total_converted_events(converted_events);
                    dev_inspect_status.count_total_generated_events(
                        generated_events,
                        batch.event_time.into_nanos().try_into().unwrap(),
                    );
                } else {
                    log_warn!("unable to record inspect for touch device");
                }

                dev.open_files.lock().retain(|f| {
                    let Some(file) = f.upgrade() else {
                        log_warn!("Dropping input file for touch that failed to upgrade");
                        return false;
                    };
                    match &file.inspect_status {
                        Some(file_inspect_status) => {
                            file_inspect_status.count_received_events(1);
                            file_inspect_status.count_ignored_events(ignored_events);
                            file_inspect_status.count_converted_events(converted_events);
                        }
                        None => {
                            log_warn!("unable to record inspect within the input file")
                        }
                    }
                    if !batch.events.is_empty() {
                        if let Some(file_inspect_status) = &file.inspect_status {
                            file_inspect_status.count_generated_events(
                                generated_events,
                                batch.event_time.into_nanos().try_into().unwrap(),
                            );
                        }
                        file.add_events(batch.events.clone());
                    }

                    true
                });

                responder.send().expect("touch buttons responder failed to respond");

                batch.touch_buttons
            }
            fuipolicy::TouchButtonsListenerRequest::_UnknownMethod { ordinal, .. } => {
                log_warn!("Received an unknown method with ordinal {ordinal}");
                touch_buttons_were_pressed.clone()
            }
        }
    }

    fn process_mouse_event(
        self: &mut Self,
        default_mouse_device: &mut DeviceState,
        mouse_events: Vec<FidlMouseEvent>,
    ) {
        fuchsia_trace::duration!("input", "starnix_process_mouse_event");
        for e in &mouse_events {
            if let Some(trace_flow_id) = e.trace_flow_id {
                fuchsia_trace::flow_end!("input", "dispatch_event_to_client", trace_flow_id.into());
            }
        }
        // TODO(https://fxbug.dev/563345995): `num_received_events` counts the whole
        // batch and `num_ignored_events` accumulates across devices, yet both are
        // recorded against every device in the loop below. With more than one mouse
        // device in use simultaneously these counters over-report and violate the
        // inspect invariants. Scope them per device before multi-mouse is supported.
        let num_received_events: u64 = mouse_events.len().try_into().unwrap();
        let mut num_ignored_events: u64 = 0;

        let (events_by_device, ignored_events) = group_mouse_events_by_device_id(mouse_events);
        num_ignored_events += ignored_events;

        for (device_id, mut events) in events_by_device {
            fuchsia_trace::duration_begin!("input", "starnix_process_per_device_mouse_event");

            let dev = self.devices.get_mut(&device_id).unwrap_or(default_mouse_device);

            let mut num_converted_events: u64 = 0;
            let mut num_unexpected_events: u64 = 0;
            let mut new_events: VecDeque<uapi::input_event> = VecDeque::new();

            #[allow(clippy::collection_is_never_read)]
            let mut tracked_leases = vec![];
            for event in &mut events {
                if let Some(lease) = event.wake_lease.take() {
                    if let Some(status) = &dev.inspect_status {
                        tracked_leases.push(TrackedWakeLease::new(lease, status.clone()));
                    }
                }
            }

            let last_event_time_ns: i64;
            if let InputDeviceType::Mouse(ref mut converter) = dev.device_type {
                let mut batch = converter.handle(events);
                new_events.append(&mut batch.events);
                num_converted_events += batch.count_converted_events;
                num_ignored_events += batch.count_ignored_events;
                num_unexpected_events += batch.count_unexpected_events;
                last_event_time_ns = batch.last_event_time_ns;
            } else {
                fuchsia_trace::duration_end!("input", "starnix_process_per_device_mouse_event");
                log_warn!(
                    "Non mouse device received mouse events: device_id = {}, device_type = {}",
                    device_id,
                    dev.device_type
                );
                continue;
            }

            if let Some(dev_inspect_status) = &dev.inspect_status {
                dev_inspect_status.count_total_received_events(num_received_events);
                dev_inspect_status.count_total_ignored_events(num_ignored_events);
                dev_inspect_status.count_total_unexpected_events(num_unexpected_events);
                dev_inspect_status.count_total_converted_events(num_converted_events);
                if !new_events.is_empty() {
                    dev_inspect_status.count_total_generated_events(
                        new_events.len().try_into().unwrap(),
                        last_event_time_ns,
                    );
                }
            } else {
                log_warn!(
                    "unable to record inspect for device_id: {}, device_type: {}",
                    device_id,
                    dev.device_type
                );
            }

            fuchsia_trace::duration_end!("input", "starnix_process_per_device_mouse_event");
            dev.open_files.lock().retain(|f| {
                let Some(file) = f.upgrade() else {
                    log_warn!("Dropping input file for mouse that failed to upgrade");
                    return false;
                };
                if let Some(file_inspect_status) = &file.inspect_status {
                    file_inspect_status.count_received_events(num_received_events);
                    file_inspect_status.count_ignored_events(num_ignored_events);
                    file_inspect_status.count_unexpected_events(num_unexpected_events);
                    file_inspect_status.count_converted_events(num_converted_events);
                }
                if !new_events.is_empty() {
                    if let Some(file_inspect_status) = &file.inspect_status {
                        file_inspect_status.count_generated_events(
                            new_events.len().try_into().unwrap(),
                            last_event_time_ns,
                        );
                    }
                    file.add_events(new_events.clone().into_iter().collect());
                }
                true
            });
        }
    }
}

fn setup_touch_relay(
    kernel: &Arc<Kernel>,
    event_proxy_mode: EventProxyMode,
    touch_source_client_end: ClientEnd<fuipointer::TouchSourceV2Marker>,
    default_touch_device_opened_files: OpenedFiles,
    device_inspect_status: Option<Arc<InputDeviceStatus>>,
) -> (
    DeviceState,
    fuipointer::TouchSourceV2Proxy,
    ContainerWakingStream<fuipointer::TouchSourceV2EventStream>,
) {
    let touch_counter_name = "touch";
    let default_touch_device = DeviceState {
        device_type: InputDeviceType::Touch(FuchsiaTouchEventToLinuxTouchEventConverter::create()),
        open_files: default_touch_device_opened_files,
        inspect_status: device_inspect_status,
    };
    let (touch_source_proxy, counter) = match event_proxy_mode {
        EventProxyMode::WakeContainer => {
            // Proxy the touch events through the Starnix runner. This allows touch events to
            // wake the container when it is suspended.
            let (touch_source_channel, counter) = create_proxy_for_wake_events_counter(
                touch_source_client_end.into_channel(),
                touch_counter_name.to_string(),
            );
            (
                fuipointer::TouchSourceV2Proxy::new(fidl::AsyncChannel::from_channel(
                    touch_source_channel,
                )),
                Some(counter),
            )
        }
        EventProxyMode::None => (touch_source_client_end.into_proxy(), None),
    };
    let waking_stream = ContainerWakingStream::new(
        kernel.suspend_resume_manager.add_message_counter(touch_counter_name, counter),
        touch_source_proxy.take_event_stream(),
    );
    (default_touch_device, touch_source_proxy, waking_stream)
}

fn setup_keyboard_relay(
    keyboard: KeyboardSynchronousProxy,
    view_ref: fuiviews::ViewRef,
    default_keyboard_device_opened_files: OpenedFiles,
    device_inspect_status: Option<Arc<InputDeviceStatus>>,
) -> (DeviceState, KeyboardListenerRequestStream, KeyboardSynchronousProxy) {
    let default_keyboard_device = DeviceState {
        device_type: InputDeviceType::Keyboard,
        open_files: default_keyboard_device_opened_files,
        inspect_status: device_inspect_status,
    };
    let (keyboard_listener, event_stream) =
        fidl::endpoints::create_request_stream::<KeyboardListenerMarker>();
    if let Err(e) =
        keyboard.add_listener(view_ref, keyboard_listener, zx::MonotonicInstant::INFINITE)
    {
        log_warn!("Could not register keyboard listener: {:?}", e);
    }

    (default_keyboard_device, event_stream, keyboard)
}

fn setup_button_relay(
    kernel: &Arc<Kernel>,
    registry_proxy: fuipolicy::DeviceListenerRegistrySynchronousProxy,
    event_proxy_mode: EventProxyMode,
    default_keyboard_device_opened_files: OpenedFiles,
    device_inspect_status: Option<Arc<InputDeviceStatus>>,
) -> (
    DeviceState,
    ContainerWakingStream<fuipolicy::MediaButtonsListenerRequestStream>,
    ContainerWakingStream<fuipolicy::TouchButtonsListenerRequestStream>,
) {
    let default_keyboard_device = DeviceState {
        device_type: InputDeviceType::Keyboard,
        open_files: default_keyboard_device_opened_files,
        inspect_status: device_inspect_status,
    };
    let media_buttons_name = "media buttons";
    let touch_buttons_name = "touch buttons";

    let (remote_media_button_client, remote_media_button_server) =
        fidl::endpoints::create_endpoints::<fuipolicy::MediaButtonsListenerMarker>();
    if let Err(e) =
        registry_proxy.register_listener(remote_media_button_client, zx::MonotonicInstant::INFINITE)
    {
        log_warn!("Failed to register media buttons listener: {:?}", e);
    }

    let (remote_touch_button_client, remote_touch_button_server) =
        fidl::endpoints::create_endpoints::<fuipolicy::TouchButtonsListenerMarker>();
    if let Err(e) = registry_proxy
        .register_touch_buttons_listener(remote_touch_button_client, zx::MonotonicInstant::INFINITE)
    {
        log_warn!("Failed to register touch buttons listener: {:?}", e);
    }

    let (
        local_media_buttons_listener_stream,
        media_buttons_counter,
        local_touch_buttons_listener_stream,
        touch_buttons_counter,
    ) = match event_proxy_mode {
        EventProxyMode::WakeContainer => {
            let (local_media_buttons_channel, media_buttons_counter) =
                create_proxy_for_wake_events_counter(
                    remote_media_button_server.into_channel(),
                    media_buttons_name.to_string(),
                );
            let local_media_buttons_listener_stream =
                fuipolicy::MediaButtonsListenerRequestStream::from_channel(
                    fidl::AsyncChannel::from_channel(local_media_buttons_channel),
                );

            let (local_touch_buttons_channel, touch_buttons_counter) =
                create_proxy_for_wake_events_counter(
                    remote_touch_button_server.into_channel(),
                    touch_buttons_name.to_string(),
                );
            let local_touch_buttons_listener_stream =
                fuipolicy::TouchButtonsListenerRequestStream::from_channel(
                    fidl::AsyncChannel::from_channel(local_touch_buttons_channel),
                );
            (
                local_media_buttons_listener_stream,
                Some(media_buttons_counter),
                local_touch_buttons_listener_stream,
                Some(touch_buttons_counter),
            )
        }
        EventProxyMode::None => (
            remote_media_button_server.into_stream(),
            None,
            remote_touch_button_server.into_stream(),
            None,
        ),
    };

    (
        default_keyboard_device,
        ContainerWakingStream::new(
            kernel
                .suspend_resume_manager
                .add_message_counter(media_buttons_name, media_buttons_counter),
            local_media_buttons_listener_stream,
        ),
        ContainerWakingStream::new(
            kernel
                .suspend_resume_manager
                .add_message_counter(touch_buttons_name, touch_buttons_counter),
            local_touch_buttons_listener_stream,
        ),
    )
}

fn setup_mouse_relay(
    kernel: &Arc<Kernel>,
    event_proxy_mode: EventProxyMode,
    mouse_source_client_end: ClientEnd<fuipointer::MouseSourceV2Marker>,
    default_mouse_device_opened_files: OpenedFiles,
    device_inspect_status: Option<Arc<InputDeviceStatus>>,
) -> (
    DeviceState,
    fuipointer::MouseSourceV2Proxy,
    ContainerWakingStream<fuipointer::MouseSourceV2EventStream>,
) {
    let mouse_counter_name = "mouse";
    let default_mouse_device = DeviceState {
        device_type: InputDeviceType::Mouse(FuchsiaMouseEventToLinuxMouseEventConverter::create()),
        open_files: default_mouse_device_opened_files,
        inspect_status: device_inspect_status,
    };
    let (mouse_source_proxy, counter) = match event_proxy_mode {
        EventProxyMode::WakeContainer => {
            // Proxy the mouse events through the Starnix runner. This allows mouse events to
            // wake the container when it is suspended.
            let (mouse_source_channel, resume_event) = create_proxy_for_wake_events_counter(
                mouse_source_client_end.into_channel(),
                "mouse".to_string(),
            );
            (
                fuipointer::MouseSourceV2Proxy::new(fidl::AsyncChannel::from_channel(
                    mouse_source_channel,
                )),
                Some(resume_event),
            )
        }
        EventProxyMode::None => (mouse_source_client_end.into_proxy(), None),
    };

    let waking_stream = ContainerWakingStream::new(
        kernel.suspend_resume_manager.add_message_counter(mouse_counter_name, counter),
        mouse_source_proxy.take_event_stream(),
    );

    (default_mouse_device, mouse_source_proxy, waking_stream)
}

fn group_mouse_events_by_device_id(
    events: Vec<FidlMouseEvent>,
) -> (SortedVecMap<DeviceId, Vec<FidlMouseEvent>>, u64) {
    let mut events_by_device: SortedVecMap<u32, Vec<FidlMouseEvent>> = SortedVecMap::new();
    let mut ignored_events: u64 = 0;
    for e in events {
        match e {
            FidlMouseEvent { pointer_sample: Some(ref sample), .. } => {
                let id = sample.device_id.unwrap_or(DEFAULT_MOUSE_DEVICE_ID);
                if let Some(vec) = events_by_device.get_mut(&id) {
                    vec.push(e);
                } else {
                    events_by_device.insert(id, vec![e]);
                }
            }
            _ => {
                ignored_events += 1;
            }
        }
    }

    (events_by_device, ignored_events)
}

fn group_touch_events_by_device_id(
    events: Vec<FidlTouchEvent>,
) -> (SortedVecMap<DeviceId, Vec<FidlTouchEvent>>, u64) {
    let mut events_by_device: SortedVecMap<u32, Vec<FidlTouchEvent>> = SortedVecMap::new();
    let mut ignored_events: u64 = 0;
    for e in events {
        match e {
            FidlTouchEvent {
                pointer_sample: Some(TouchPointerSample { interaction: Some(id), .. }),
                ..
            } => {
                if let Some(vec) = events_by_device.get_mut(&id.device_id) {
                    vec.push(e);
                } else {
                    events_by_device.insert(id.device_id, vec![e]);
                }
            }
            _ => {
                ignored_events += 1;
            }
        }
    }

    (events_by_device, ignored_events)
}

#[cfg(test)]
pub async fn start_input_relays_for_test(
    current_task: &starnix_core::task::CurrentTask,
    event_proxy_mode: EventProxyMode,
) -> (
    Arc<InputEventsRelayHandle>,
    crate::InputDevice,
    crate::InputDevice,
    crate::InputDevice,
    starnix_core::vfs::FileHandle,
    starnix_core::vfs::FileHandle,
    starnix_core::vfs::FileHandle,
    fuipointer::TouchSourceV2RequestStream,
    fuipointer::MouseSourceV2RequestStream,
    fidl_fuchsia_ui_input3::KeyboardListenerProxy,
    fuipolicy::MediaButtonsListenerProxy,
    fuipolicy::TouchButtonsListenerProxy,
) {
    let inspector = fuchsia_inspect::Inspector::default();

    let touch_device = crate::InputDevice::new_touch(700, 1200, inspector.root());
    let touch_file = touch_device.open_test(current_task).expect("Failed to create input file");

    let keyboard_device = crate::InputDevice::new_keyboard(inspector.root());
    let keyboard_file =
        keyboard_device.open_test(current_task).expect("Failed to create input file");

    let mouse_device = crate::InputDevice::new_mouse(inspector.root());
    let mouse_file = mouse_device.open_test(current_task).expect("Failed to create input file");

    let (touch_source_client_end, touch_source_stream) =
        fidl::endpoints::create_request_stream::<fuipointer::TouchSourceV2Marker>();
    let (mouse_source_client_end, mouse_stream) =
        fidl::endpoints::create_request_stream::<fuipointer::MouseSourceV2Marker>();
    let (keyboard_proxy, mut keyboard_stream) =
        fidl::endpoints::create_sync_proxy_and_stream::<fidl_fuchsia_ui_input3::KeyboardMarker>();
    let view_ref_pair = fuchsia_scenic::ViewRefPair::new().expect("Failed to create ViewRefPair");
    let (device_registry_proxy, mut device_listener_stream) =
        fidl::endpoints::create_sync_proxy_and_stream::<fuipolicy::DeviceListenerRegistryMarker>();

    let (relay, relay_handle) = new_input_relay();
    relay.start_relays(
        &current_task.kernel(),
        event_proxy_mode,
        touch_source_client_end,
        keyboard_proxy,
        mouse_source_client_end,
        view_ref_pair.view_ref,
        device_registry_proxy,
        touch_device.open_files.clone(),
        keyboard_device.open_files.clone(),
        mouse_device.open_files.clone(),
        Some(touch_device.inspect_status.clone()),
        Some(keyboard_device.inspect_status.clone()),
        Some(mouse_device.inspect_status.clone()),
    );

    let keyboard_listener = match keyboard_stream.next().await {
        Some(Ok(fidl_fuchsia_ui_input3::KeyboardRequest::AddListener {
            view_ref: _,
            listener,
            responder,
        })) => {
            let _ = responder.send();
            listener.into_proxy()
        }
        _ => {
            panic!("Failed to get event");
        }
    };

    let media_buttons_listener = match device_listener_stream.next().await {
        Some(Ok(fuipolicy::DeviceListenerRegistryRequest::RegisterListener {
            listener,
            responder,
        })) => {
            let _ = responder.send();
            listener.into_proxy()
        }
        _ => {
            panic!("Failed to get event");
        }
    };

    let touch_buttons_listener = match device_listener_stream.next().await {
        Some(Ok(fuipolicy::DeviceListenerRegistryRequest::RegisterTouchButtonsListener {
            listener,
            responder,
        })) => {
            let _ = responder.send();
            listener.into_proxy()
        }
        _ => {
            panic!("Failed to get event");
        }
    };

    (
        relay_handle,
        touch_device,
        keyboard_device,
        mouse_device,
        touch_file,
        keyboard_file,
        mouse_file,
        touch_source_stream,
        mouse_stream,
        keyboard_listener,
        media_buttons_listener,
        touch_buttons_listener,
    )
}

#[cfg(test)]
mod test {
    use super::*;
    use anyhow::anyhow;
    use fidl_fuchsia_ui_input::{
        MediaButtonsEvent, TouchButton, TouchButtonsEvent, TouchDeviceInfo,
    };
    use fidl_fuchsia_ui_input3 as fuiinput;
    use fuipointer::{
        EventPhase, MouseEvent, MousePointerSample, TouchEvent, TouchInteractionId,
        TouchPointerSample, TouchSourceV2Request, TouchSourceV2RequestStream,
    };
    use starnix_core::task::CurrentTask;
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_core::vfs::{FileHandle, FileObject, VecOutputBuffer};

    use starnix_types::time::timeval_from_time;
    use starnix_uapi::errors::{EAGAIN, Errno};
    use starnix_uapi::input_id;
    use starnix_uapi::open_flags::OpenFlags;
    use zerocopy::FromBytes as _;

    const INPUT_EVENT_SIZE: usize = std::mem::size_of::<uapi::input_event>();

    // Sends `touch_events` to the client stream and waits for `AcknowledgeEvents`.
    async fn answer_next_touch_watch_request(
        request_stream: &mut TouchSourceV2RequestStream,
        touch_events: Vec<TouchEvent>,
    ) {
        let control_handle = request_stream.control_handle();
        control_handle
            .send_on_touch_events(touch_events, 1)
            .expect("failure sending OnTouchEvents");
        match request_stream.next().await {
            Some(Ok(TouchSourceV2Request::AcknowledgeEvents { .. })) => {}
            unexpected_request => panic!("unexpected request {:?}", unexpected_request),
        }
    }

    // Sends `mouse_events` to the client stream and waits for `AcknowledgeEvents`.
    async fn answer_next_mouse_watch_request(
        request_stream: &mut fuipointer::MouseSourceV2RequestStream,
        mouse_events: Vec<MouseEvent>,
    ) {
        let control_handle = request_stream.control_handle();
        control_handle
            .send_on_mouse_events(mouse_events, 1)
            .expect("failure sending OnMouseEvents");
        match request_stream.next().await {
            Some(Ok(fuipointer::MouseSourceV2Request::AcknowledgeEvents { .. })) => {}
            unexpected_request => panic!("unexpected request {:?}", unexpected_request),
        }
    }

    fn make_empty_touch_event(device_id: u32) -> TouchEvent {
        TouchEvent {
            pointer_sample: Some(TouchPointerSample {
                interaction: Some(TouchInteractionId {
                    pointer_id: 0,
                    device_id,
                    interaction_id: 0,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_touch_event_with_phase_device_id(
        phase: EventPhase,
        pointer_id: u32,
        device_id: u32,
    ) -> TouchEvent {
        make_touch_event_with_phase_device_id_position(phase, pointer_id, device_id, 0.0, 0.0)
    }

    fn make_touch_event_with_phase_device_id_position(
        phase: EventPhase,
        pointer_id: u32,
        device_id: u32,
        x: f32,
        y: f32,
    ) -> TouchEvent {
        TouchEvent {
            timestamp: Some(0),
            pointer_sample: Some(TouchPointerSample {
                position_in_viewport: Some([x, y]),
                phase: Some(phase),
                interaction: Some(TouchInteractionId { pointer_id, device_id, interaction_id: 0 }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_mouse_wheel_event(scroll_v_ticks: i64, device_id: u32) -> MouseEvent {
        MouseEvent {
            timestamp: Some(0),
            pointer_sample: Some(MousePointerSample {
                device_id: Some(device_id),
                scroll_v: Some(scroll_v_ticks),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn read_uapi_events(file: &FileHandle, current_task: &CurrentTask) -> Vec<uapi::input_event> {
        std::iter::from_fn(|| {
            let mut event_bytes = VecOutputBuffer::new(INPUT_EVENT_SIZE);
            match file.read(current_task, &mut event_bytes) {
                Ok(INPUT_EVENT_SIZE) => Some(
                    uapi::input_event::read_from_bytes(Vec::from(event_bytes).as_slice())
                        .map_err(|_| anyhow!("failed to read input_event from buffer")),
                ),
                Ok(other_size) => {
                    Some(Err(anyhow!("got {} bytes (expected {})", other_size, INPUT_EVENT_SIZE)))
                }
                Err(Errno { code: EAGAIN, .. }) => None,
                Err(other_error) => Some(Err(anyhow!("read failed: {:?}", other_error))),
            }
        })
        .enumerate()
        .map(|(i, read_res)| match read_res {
            Ok(event) => event,
            Err(e) => panic!("unexpected result {:?} on iteration {}", e, i),
        })
        .collect()
    }

    fn create_test_touch_device(
        current_task: &CurrentTask,
        input_relay: Arc<InputEventsRelayHandle>,
        device_id: u32,
    ) -> FileHandle {
        let open_files: OpenedFiles = Default::default();
        input_relay.add_touch_device(device_id, open_files.clone(), None);
        let inspector = fuchsia_inspect::Inspector::default();
        let device_file = Arc::new(InputFile::new_touch(
            input_id { bustype: 0, vendor: 0, product: 0, version: 0 },
            1000,
            1000,
            inspector.root(),
        ));
        open_files.lock().push(Arc::downgrade(&device_file));

        let root_namespace_node = current_task
            .lookup_path_from_root(".".into())
            .expect("failed to get namespace node for root");

        FileObject::new(
            &current_task,
            Box::new(crate::input_file::ArcInputFile(device_file)),
            root_namespace_node,
            OpenFlags::empty(),
        )
        .expect("FileObject::new failed")
    }

    fn make_uapi_input_event(ty: u32, code: u32, value: i32) -> uapi::input_event {
        uapi::input_event {
            time: timeval_from_time(zx::MonotonicInstant::from_nanos(0)),
            type_: ty as u16,
            code: code as u16,
            value,
        }
    }

    #[::fuchsia::test]
    async fn route_touch_event_by_device_id() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                input_relay,
                _touch_device,
                _keyboard_device,
                _mouse_device,
                input_file,
                _keyboard_file,
                _mouse_file,
                mut touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            const DEVICE_ID: u32 = 10;

            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase_device_id(EventPhase::Add, 1, DEVICE_ID)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event creates unexpected
            // `uapi::input_event`s.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event(DEVICE_ID)],
            )
            .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events = read_uapi_events(&input_file, &current_task);
            // Default device receive events because no matched device.
            assert_ne!(events.len(), 0);

            // add a device, mock uinput.
            let device_id_10_file =
                create_test_touch_device(&current_task, input_relay.clone(), DEVICE_ID);

            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase_device_id(EventPhase::Add, 1, DEVICE_ID)],
            )
            .await;

            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event(DEVICE_ID)],
            )
            .await;

            let events = read_uapi_events(&input_file, &current_task);
            // Default device should not receive events because they matched device id 10.
            assert_eq!(events.len(), 0);

            let events = read_uapi_events(&device_id_10_file, &current_task);
            // file of device id 10 should receive events.
            assert_ne!(events.len(), 0);
        })
        .await;
    }

    #[::fuchsia::test]
    async fn route_touch_event_with_wake_lease() {
        spawn_kernel_and_run(async move |current_task| {
            let (
                _input_relay,
                touch_device,
                _keyboard_device,
                _mouse_device,
                _input_file,
                _keyboard_file,
                _mouse_file,
                mut touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            const DEVICE_ID: u32 = 10;
            let mut event = make_touch_event_with_phase_device_id(EventPhase::Add, 1, DEVICE_ID);
            let (p1, _p2) = fidl::EventPair::create();
            event.wake_lease = Some(p1);

            answer_next_touch_watch_request(&mut touch_source_stream, vec![event]).await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event(DEVICE_ID)],
            )
            .await;

            let status = &touch_device.inspect_status;
            assert_eq!(
                status
                    .total_events_with_wake_lease_count
                    .load(std::sync::atomic::Ordering::Relaxed),
                1
            );
            assert_eq!(
                status.active_wake_leases_count.load(std::sync::atomic::Ordering::Relaxed),
                0
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn route_touch_event_by_device_id_multi_device_events_in_one_sequence() {
        spawn_kernel_and_run(async move |current_task| {
            let (
                input_relay,
                _touch_device,
                _keyboard_device,
                _mouse_device,
                _input_file,
                _keyboard_file,
                _mouse_file,
                mut touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            const DEVICE_ID_10: u32 = 10;
            const DEVICE_ID_11: u32 = 11;

            let device_id_10_file =
                create_test_touch_device(&current_task, input_relay.clone(), DEVICE_ID_10);

            let device_id_11_file =
                create_test_touch_device(&current_task, input_relay.clone(), DEVICE_ID_11);

            // 2 pointer down on different touch device.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![
                    make_touch_event_with_phase_device_id_position(
                        EventPhase::Add,
                        1,
                        DEVICE_ID_10,
                        10.0,
                        20.0,
                    ),
                    make_touch_event_with_phase_device_id_position(
                        EventPhase::Add,
                        2,
                        DEVICE_ID_11,
                        30.0,
                        40.0,
                    ),
                ],
            )
            .await;

            answer_next_touch_watch_request(&mut touch_source_stream, vec![]).await;

            let events_10 = read_uapi_events(&device_id_10_file, &current_task);
            let events_11 = read_uapi_events(&device_id_11_file, &current_task);
            assert_eq!(events_10.len(), events_11.len());

            assert_eq!(
                events_10,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 1),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 10),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 20),
                    make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 1),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );

            assert_eq!(
                events_11,
                vec![
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_SLOT, 0),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_TRACKING_ID, 2),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_X, 30),
                    make_uapi_input_event(uapi::EV_ABS, uapi::ABS_MT_POSITION_Y, 40),
                    make_uapi_input_event(uapi::EV_KEY, uapi::BTN_TOUCH, 1),
                    make_uapi_input_event(uapi::EV_SYN, uapi::SYN_REPORT, 0),
                ]
            );
        })
        .await;
    }

    #[::fuchsia::test]
    async fn route_key_event_by_device_id() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                input_relay,
                _touch_device,
                _keyboard_device,
                _mouse_device,
                _touch_file,
                keyboard_file,
                _mouse_file,
                _touch_source_stream,
                _mouse_source_stream,
                keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            const DEVICE_ID: u32 = 10;

            let key_event = fuiinput::KeyEvent {
                timestamp: Some(0),
                type_: Some(fuiinput::KeyEventType::Pressed),
                key: Some(fidl_fuchsia_input::Key::A),
                device_id: Some(DEVICE_ID),
                ..Default::default()
            };

            let _ = keyboard_listener.on_key_event(&key_event).await;

            let events = read_uapi_events(&keyboard_file, &current_task);
            // Default device should receive events because no device device id is 10.
            assert_ne!(events.len(), 0);

            // add a device, mock uinput.
            let open_files: OpenedFiles = Default::default();
            input_relay.add_keyboard_device(DEVICE_ID, open_files.clone(), None);
            let inspector = fuchsia_inspect::Inspector::default();
            let device_id_10_file = Arc::new(InputFile::new_keyboard(
                input_id { bustype: 0, vendor: 0, product: 0, version: 0 },
                inspector.root(),
            ));
            open_files.lock().push(Arc::downgrade(&device_id_10_file));
            let root_namespace_node = current_task
                .lookup_path_from_root(".".into())
                .expect("failed to get namespace node for root");
            let device_id_10_file_object = FileObject::new(
                &current_task,
                Box::new(crate::input_file::ArcInputFile(device_id_10_file)),
                root_namespace_node,
                OpenFlags::empty(),
            )
            .expect("FileObject::new failed");

            let _ = keyboard_listener.on_key_event(&key_event).await;

            let events = read_uapi_events(&keyboard_file, &current_task);
            // Default device should not receive events because they matched device id 10.
            assert_eq!(events.len(), 0);

            let events = read_uapi_events(&device_id_10_file_object, &current_task);
            // file of device id 10 should receive events.
            assert_ne!(events.len(), 0);

            std::mem::drop(keyboard_listener); // Close Zircon channel.
        })
        .await;
    }

    #[::fuchsia::test]
    async fn route_media_button_event_by_device_id() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                input_relay,
                _touch_device,
                _keyboard_device,
                _mouse_device,
                _touch_file,
                keyboard_file,
                _mouse_file,
                _touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            const DEVICE_ID: u32 = 10;

            let power_pressed_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(false),
                device_id: Some(DEVICE_ID),
                ..Default::default()
            };

            let _ = media_buttons_listener.on_event(power_pressed_event).await;

            let events = read_uapi_events(&keyboard_file, &current_task);
            // Default device should receive events because no device device id is 10.
            assert_ne!(events.len(), 0);

            // add a device, mock uinput.
            let open_files: OpenedFiles = Default::default();
            input_relay.add_keyboard_device(DEVICE_ID, open_files.clone(), None);
            let inspector = fuchsia_inspect::Inspector::default();
            let device_id_10_file = Arc::new(InputFile::new_keyboard(
                input_id { bustype: 0, vendor: 0, product: 0, version: 0 },
                inspector.root(),
            ));
            open_files.lock().push(Arc::downgrade(&device_id_10_file));
            let root_namespace_node = current_task
                .lookup_path_from_root(".".into())
                .expect("failed to get namespace node for root");
            let device_id_10_file_object = FileObject::new(
                &current_task,
                Box::new(crate::input_file::ArcInputFile(device_id_10_file)),
                root_namespace_node,
                OpenFlags::empty(),
            )
            .expect("FileObject::new failed");

            let power_released_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(false),
                function: Some(false),
                device_id: Some(DEVICE_ID),
                ..Default::default()
            };

            let _ = media_buttons_listener.on_event(power_released_event).await;

            let events = read_uapi_events(&keyboard_file, &current_task);
            // Default device should not receive events because they matched device id 10.
            assert_eq!(events.len(), 0);

            let events = read_uapi_events(&device_id_10_file_object, &current_task);
            // file of device id 10 should receive events.
            assert_ne!(events.len(), 0);

            std::mem::drop(media_buttons_listener); // Close Zircon channel.
        })
        .await;
    }

    #[::fuchsia::test]
    async fn route_touch_button_event_by_device_id() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                input_relay,
                _touch_device,
                _keyboard_device,
                _mouse_device,
                touch_file,
                _keyboard_file,
                _mouse_file,
                _touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                _media_buttons_listener,
                touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            const DEVICE_ID: u32 = 10;

            let palm_pressed_event: TouchButtonsEvent = TouchButtonsEvent {
                pressed_buttons: Some(vec![TouchButton::Palm]),
                device_info: Some(TouchDeviceInfo { id: Some(DEVICE_ID), ..Default::default() }),
                ..Default::default()
            };

            let _ = touch_buttons_listener.on_event(palm_pressed_event).await;

            let events = read_uapi_events(&touch_file, &current_task);
            // Default device should receive events because no device device id is 10.
            assert_ne!(events.len(), 0);

            // add a device, mock uinput.
            let open_files: OpenedFiles = Default::default();
            input_relay.add_touch_device(DEVICE_ID, open_files.clone(), None);
            let device_id_10_file =
                create_test_touch_device(&current_task, input_relay.clone(), DEVICE_ID);

            let palm_released_event: TouchButtonsEvent = TouchButtonsEvent {
                pressed_buttons: Some(vec![]),
                device_info: Some(TouchDeviceInfo { id: Some(DEVICE_ID), ..Default::default() }),
                ..Default::default()
            };

            let _ = touch_buttons_listener.on_event(palm_released_event).await;

            let events = read_uapi_events(&touch_file, &current_task);
            // Default device should not receive events because they matched device id 10.
            assert_eq!(events.len(), 0);

            let events = read_uapi_events(&device_id_10_file, &current_task);
            // file of device id 10 should receive events.
            assert_ne!(events.len(), 0);

            std::mem::drop(touch_buttons_listener); // Close Zircon channel.
        })
        .await;
    }

    #[::fuchsia::test]
    async fn touch_device_multi_reader() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                _input_relay,
                touch_device,
                _keyboard_device,
                _mouse_device,
                touch_reader1,
                _keyboard_file,
                _mouse_file,
                mut touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            let touch_reader2 =
                touch_device.open_test(&current_task).expect("Failed to create input file");

            const DEVICE_ID: u32 = 10;

            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_touch_event_with_phase_device_id(EventPhase::Add, 1, DEVICE_ID)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `TouchEvent`, to minimize the chance that this event creates unexpected
            // `uapi::input_event`s.
            answer_next_touch_watch_request(
                &mut touch_source_stream,
                vec![make_empty_touch_event(DEVICE_ID)],
            )
            .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events_from_reader1 = read_uapi_events(&touch_reader1, &current_task);
            let events_from_reader2 = read_uapi_events(&touch_reader2, &current_task);
            assert_ne!(events_from_reader1.len(), 0);
            assert_eq!(events_from_reader1.len(), events_from_reader2.len());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn keyboard_device_multi_reader() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                _input_relay,
                _touch_device,
                keyboard_device,
                _mouse_device,
                _touch_file,
                keyboard_reader1,
                _mouse_file,
                _touch_source_stream,
                _mouse_source_stream,
                keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            let keyboard_reader2 =
                keyboard_device.open_test(&current_task).expect("Failed to create input file");

            const DEVICE_ID: u32 = 10;

            let key_event = fuiinput::KeyEvent {
                timestamp: Some(0),
                type_: Some(fuiinput::KeyEventType::Pressed),
                key: Some(fidl_fuchsia_input::Key::A),
                device_id: Some(DEVICE_ID),
                ..Default::default()
            };

            let _ = keyboard_listener.on_key_event(&key_event).await;

            // Consume all of the `uapi::input_event`s that are available.
            let events_from_reader1 = read_uapi_events(&keyboard_reader1, &current_task);
            let events_from_reader2 = read_uapi_events(&keyboard_reader2, &current_task);
            assert_ne!(events_from_reader1.len(), 0);
            assert_eq!(events_from_reader1.len(), events_from_reader2.len());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn button_device_multi_reader() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                _input_relay,
                _touch_device,
                keyboard_device,
                _mouse_device,
                _touch_file,
                keyboard_reader1,
                _mouse_file,
                _touch_source_stream,
                _mouse_source_stream,
                _keyboard_listener,
                media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            let keyboard_reader2 =
                keyboard_device.open_test(&current_task).expect("Failed to create input file");

            const DEVICE_ID: u32 = 10;

            let power_pressed_event = MediaButtonsEvent {
                volume: Some(0),
                mic_mute: Some(false),
                pause: Some(false),
                camera_disable: Some(false),
                power: Some(true),
                function: Some(false),
                device_id: Some(DEVICE_ID),
                ..Default::default()
            };

            let _ = media_buttons_listener.on_event(power_pressed_event).await;

            // Consume all of the `uapi::input_event`s that are available.
            let events_from_reader1 = read_uapi_events(&keyboard_reader1, &current_task);
            let events_from_reader2 = read_uapi_events(&keyboard_reader2, &current_task);
            assert_ne!(events_from_reader1.len(), 0);
            assert_eq!(events_from_reader1.len(), events_from_reader2.len());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn mouse_device_multi_reader() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.

            let (
                _input_relay,
                _touch_device,
                _keyboard_device,
                mouse_device,
                _touch_file,
                _keyboard_file,
                mouse_reader1,
                _touch_stream,
                mut mouse_stream,
                _keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::None).await;

            let mouse_reader2 =
                mouse_device.open_test(&current_task).expect("Failed to create input file");

            const DEVICE_ID: u32 = 10;

            answer_next_mouse_watch_request(
                &mut mouse_stream,
                vec![make_mouse_wheel_event(1, DEVICE_ID)],
            )
            .await;

            // Wait for another `Watch` to ensure input_file done processing the first reply.
            // Use an empty `MouseEvent`, to minimize the chance that this event creates unexpected
            // `uapi::input_event`s.
            answer_next_mouse_watch_request(
                &mut mouse_stream,
                vec![make_mouse_wheel_event(0, DEVICE_ID)],
            )
            .await;

            // Consume all of the `uapi::input_event`s that are available.
            let events_from_reader1 = read_uapi_events(&mouse_reader1, &current_task);
            let events_from_reader2 = read_uapi_events(&mouse_reader2, &current_task);
            assert_ne!(events_from_reader1.len(), 0);
            assert_eq!(events_from_reader1.len(), events_from_reader2.len());
        })
        .await;
    }

    #[::fuchsia::test]
    async fn input_message_counters() {
        spawn_kernel_and_run(async move |current_task| {
            // Set up resources.
            let kernel = current_task.kernel().clone();
            let (
                _input_relay,
                _touch_device,
                _keyboard_device,
                _mouse_device,
                _touch_file,
                keyboard_file,
                _mouse_file,
                _touch_source_stream,
                _mouse_source_stream,
                keyboard_listener,
                _media_buttons_listener,
                _touch_buttons_listener,
            ) = start_input_relays_for_test(&current_task, EventProxyMode::WakeContainer).await;

            const DEVICE_ID: u32 = 10;

            let key_event = fuiinput::KeyEvent {
                timestamp: Some(0),
                type_: Some(fuiinput::KeyEventType::Pressed),
                key: Some(fidl_fuchsia_input::Key::A),
                device_id: Some(DEVICE_ID),
                ..Default::default()
            };

            let _ = keyboard_listener.on_key_event(&key_event).await;

            let events = read_uapi_events(&keyboard_file, &current_task);
            assert_ne!(events.len(), 0);

            assert!(!kernel.suspend_resume_manager.has_nonzero_message_counter());
        })
        .await;
    }
}
