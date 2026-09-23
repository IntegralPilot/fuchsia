// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::consumer_controls_binding::ConsumerControlsEvent;
use crate::dispatcher::TimeoutExt;
use crate::input_handler::{Handler, InputHandlerStatus, UnhandledInputHandler};
use crate::{Dispatcher, Incoming, MonotonicInstant, dispatcher, input_device, metrics};
use anyhow::{Context as _, Error, anyhow};
use async_trait::async_trait;
use async_utils::hanging_get::server::HangingGet;
use fidl::endpoints::{DiscoverableProtocolMarker as _, Proxy};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_media::AudioRenderUsage2;
use fidl_fuchsia_media_sounds::{PlaySoundError, PlayerMarker, PlayerProxy};
use fidl_fuchsia_recovery::{FactoryResetMarker, FactoryResetProxy};
use fidl_fuchsia_recovery_policy::{DeviceRequest, DeviceRequestStream};
use fidl_fuchsia_recovery_ui::{
    FactoryResetCountdownRequestStream, FactoryResetCountdownState,
    FactoryResetCountdownWatchResponder,
};
use fuchsia_async::MonotonicDuration;
use fuchsia_inspect::health::Reporter;
use futures::StreamExt;
use metrics_registry::*;
use std::cell::{Cell, RefCell};
use std::fs::{self, File};
use std::path::Path;
use std::rc::Rc;

/// FactoryResetState tracks the state of the device through the factory reset
/// process.
///
/// # Values
/// ## Disallowed
/// Factory reset of the device is not allowed. This is used to
/// keep public devices from being reset, such as when being used in kiosk mode.
///
/// ### Transitions
/// Disallowed → Idle
///
/// ## Idle
/// This is the default state for a device when factory resets are allowed but
/// is not currently being reset.
///
/// ### Transitions
/// Idle → Disallowed
/// Idle → ButtonCountdown
///
/// ## ButtonCountdown
/// This state represents the fact that the reset button has been pressed and a
/// countdown has started to verify that the button was pressed intentionally.
///
/// ### Transitions
/// ButtonCountdown → Disallowed
/// ButtonCountdown → Idle
/// ButtonCountdown → ResetCountdown
///
/// ## ResetCountdown
/// The button countdown has completed indicating that this was a purposeful
/// action so a reset countdown is started to give the user a chance to cancel
/// the factory reset.
///
/// ### Transitions
/// ResetCountdown → Disallowed
/// ResetCountdown → Idle
/// ResetCountdown → Resetting
///
/// ## Resetting
/// Once the device is in this state a factory reset is imminent and can no
/// longer be cancelled.
#[derive(Clone, Copy, Debug, PartialEq)]
enum FactoryResetState {
    Disallowed,
    Idle,
    ButtonCountdown { deadline: MonotonicInstant },
    ResetCountdown { deadline: MonotonicInstant },
    Resetting,
}

const FACTORY_RESET_DISALLOWED_PATH: &'static str = "/data/factory_reset_disallowed";
const FACTORY_RESET_SOUND_PATH: &'static str = "/config/data/chirp-start-tone.wav";

const BUTTON_TIMEOUT: MonotonicDuration = MonotonicDuration::from_millis(500);
const RESET_TIMEOUT: MonotonicDuration = MonotonicDuration::from_seconds(10);
/// Maximum length of time to wait for the reset earcon to play (after `RESET_TIMEOUT` elapses).
const EARCON_TIMEOUT: MonotonicDuration = MonotonicDuration::from_millis(2000);

type NotifyFn = Box<
    dyn Fn(
            &(FactoryResetState, metrics::MetricsLogger),
            FactoryResetCountdownWatchResponder,
        ) -> bool
        + Send,
>;
type ResetCountdownHangingGet = HangingGet<
    (FactoryResetState, metrics::MetricsLogger),
    FactoryResetCountdownWatchResponder,
    NotifyFn,
>;

/// A [`FactoryResetHandler`] tracks the state of the consumer control buttons
/// and starts the factory reset process after appropriate timeouts.
pub struct FactoryResetHandler {
    incoming: Incoming,
    factory_reset_state: RefCell<FactoryResetState>,
    countdown_hanging_get: RefCell<ResetCountdownHangingGet>,

    /// The inventory of this handler's Inspect status.
    pub inspect_status: InputHandlerStatus,

    metrics_logger: metrics::MetricsLogger,

    /// Tasks running in the background.
    _tasks: RefCell<Vec<(dispatcher::TaskHandle<()>, Rc<Cell<bool>>)>>,
}

/// Uses the `ConsumerControlsEvent` to determine whether the device should
/// start the Factory Reset process. The driver will turn special button
/// combinations into a `FactoryReset` signal so this code only needs to
/// listen for that.
fn is_reset_requested(event: &ConsumerControlsEvent) -> bool {
    event.pressed_buttons.iter().any(|button| match button {
        fidl_fuchsia_input::ConsumerControlButton::FactoryReset => true,
        _ => false,
    })
}

impl FactoryResetHandler {
    /// Creates a new [`FactoryResetHandler`] that listens for the reset button
    /// and handles timing down and, ultimately, factory resetting the device.
    pub fn new(
        incoming: Incoming,
        input_handlers_node: &fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Rc<Self> {
        let initial_state = if Path::new(FACTORY_RESET_DISALLOWED_PATH).exists() {
            FactoryResetState::Disallowed
        } else {
            FactoryResetState::Idle
        };

        let countdown_hanging_get =
            FactoryResetHandler::init_hanging_get(initial_state, metrics_logger.clone());
        let inspect_status = InputHandlerStatus::new(
            input_handlers_node,
            "factory_reset_handler",
            /* generates_events */ false,
        );

        Rc::new(Self {
            incoming,
            factory_reset_state: RefCell::new(initial_state),
            countdown_hanging_get: RefCell::new(countdown_hanging_get),
            inspect_status,
            metrics_logger,
            _tasks: RefCell::new(vec![]),
        })
    }

    /// Handles the request stream for FactoryResetCountdown
    ///
    /// # Parameters
    /// `stream`: The `FactoryResetCountdownRequestStream` to be handled.
    pub fn handle_factory_reset_countdown_request_stream(
        self: Rc<Self>,
        mut stream: FactoryResetCountdownRequestStream,
    ) -> impl futures::Future<Output = Result<(), Error>> {
        let subscriber = self.countdown_hanging_get.borrow_mut().new_subscriber();

        async move {
            while let Some(request_result) = stream.next().await {
                let watcher = request_result?
                    .into_watch()
                    .ok_or_else(|| anyhow!("Failed to get FactoryResetCoundown Watcher"))?;
                subscriber.register(watcher)?;
            }

            Ok(())
        }
    }

    /// Handles the request stream for fuchsia.recovery.policy.Device
    ///
    /// # Parameters
    /// `stream`: The `DeviceRequestStream` to be handled.
    pub fn handle_recovery_policy_device_request_stream(
        self: Rc<Self>,
        mut stream: DeviceRequestStream,
    ) -> impl futures::Future<Output = Result<(), Error>> {
        async move {
            while let Some(request_result) = stream.next().await {
                let DeviceRequest::SetIsLocalResetAllowed { allowed, .. } = request_result?;
                match self.factory_reset_state() {
                    FactoryResetState::Disallowed if allowed => {
                        // Update state and delete file
                        self.set_factory_reset_state(FactoryResetState::Idle);
                        fs::remove_file(FACTORY_RESET_DISALLOWED_PATH).with_context(|| {
                            format!("failed to remove {}", FACTORY_RESET_DISALLOWED_PATH)
                        })?
                    }
                    _ if !allowed => {
                        // Update state and create file
                        self.set_factory_reset_state(FactoryResetState::Disallowed);
                        let _: File =
                            File::create(FACTORY_RESET_DISALLOWED_PATH).with_context(|| {
                                format!("failed to create {}", FACTORY_RESET_DISALLOWED_PATH)
                            })?;
                    }
                    _ => (),
                }
            }

            Ok(())
        }
    }

    /// Handles `ConsumerControlEvent`s when the device is in the
    /// `FactoryResetState::Idle` state
    async fn handle_allowed_event(self: &Rc<Self>, event: &ConsumerControlsEvent) {
        if is_reset_requested(event) {
            if let Err(error) = self.start_button_countdown().await {
                self.metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::FactoryResetFailedToReset,
                    std::format!("Failed to factory reset device: {:?}", error),
                );
            }
        }
    }

    /// Handles `ConsumerControlEvent`s when the device is in the
    /// `FactoryResetState::Disallowed` state
    fn handle_disallowed_event(self: &Rc<Self>, event: &ConsumerControlsEvent) {
        if is_reset_requested(event) {
            self.metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::FactoryResetNotAllowedReset,
                "Attempted to factory reset a device that is not allowed to reset",
            );
        }
    }

    /// Handles `ConsumerControlEvent`s when the device is in the
    /// `FactoryResetState::ButtonCountdown` state
    fn handle_button_countdown_event(self: &Rc<Self>, event: &ConsumerControlsEvent) {
        if !is_reset_requested(event) {
            // Cancel button timeout
            self.set_factory_reset_state(FactoryResetState::Idle);
        }
    }

    /// Handles `ConsumerControlEvent`s when the device is in the
    /// `FactoryResetState::ResetCountdown` state
    fn handle_reset_countdown_event(self: &Rc<Self>, event: &ConsumerControlsEvent) {
        if !is_reset_requested(event) {
            // Cancel reset timeout
            self.set_factory_reset_state(FactoryResetState::Idle);
        }
    }

    fn init_hanging_get(
        initial_state: FactoryResetState,
        metrics_logger: metrics::MetricsLogger,
    ) -> ResetCountdownHangingGet {
        let notify_fn: NotifyFn = Box::new(|(state, metrics_logger), responder| {
            let deadline = match state {
                FactoryResetState::ResetCountdown { deadline } => {
                    Some(deadline.into_nanos() as i64)
                }
                _ => None,
            };

            let countdown_state =
                FactoryResetCountdownState { scheduled_reset_time: deadline, ..Default::default() };

            if responder.send(&countdown_state).is_err() {
                metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::FactoryResetFailedToSendCountdown,
                    "Failed to send factory reset countdown state",
                );
            }

            true
        });

        ResetCountdownHangingGet::new((initial_state, metrics_logger), notify_fn)
    }

    /// Sets the state of FactoryResetHandler and notifies watchers of the updated state.
    fn set_factory_reset_state(self: &Rc<Self>, state: FactoryResetState) {
        *self.factory_reset_state.borrow_mut() = state;
        self.countdown_hanging_get
            .borrow_mut()
            .new_publisher()
            .set((state, self.metrics_logger.clone()));
    }

    fn factory_reset_state(self: &Rc<Self>) -> FactoryResetState {
        *self.factory_reset_state.borrow()
    }

    /// Handles waiting for the reset button to be held down long enough to start
    /// the factory reset countdown.
    async fn start_button_countdown(self: &Rc<Self>) -> Result<(), Error> {
        let deadline = MonotonicInstant::after(BUTTON_TIMEOUT);
        self.set_factory_reset_state(FactoryResetState::ButtonCountdown { deadline });

        // Wait for button timeout
        Dispatcher::after_deadline(MonotonicInstant::after(BUTTON_TIMEOUT)).await;

        // Make sure the buttons are still held
        match self.factory_reset_state() {
            FactoryResetState::ButtonCountdown { deadline: state_deadline }
                if state_deadline == deadline =>
            {
                // Proceed with reset.
                self.start_reset_countdown().await?;
            }
            _ => {
                log::info!("Factory reset request cancelled");
            }
        }

        Ok(())
    }

    /// Handles waiting for the reset countdown to complete before resetting the
    /// device.
    async fn start_reset_countdown(self: &Rc<Self>) -> Result<(), Error> {
        let deadline = MonotonicInstant::after(RESET_TIMEOUT);
        self.set_factory_reset_state(FactoryResetState::ResetCountdown { deadline });

        // Wait for reset timeout
        Dispatcher::after_deadline(MonotonicInstant::after(RESET_TIMEOUT)).await;

        // Make sure the buttons are still held
        match self.factory_reset_state() {
            FactoryResetState::ResetCountdown { deadline: state_deadline }
                if state_deadline == deadline =>
            {
                // Proceed with reset.
                self.reset().await?;
            }
            _ => {
                log::info!("Factory reset request cancelled");
            }
        }

        Ok(())
    }

    /// Retrieves and plays the sound associated with factory resetting the device.
    async fn play_reset_sound(self: &Rc<Self>) -> Result<(), Error> {
        log::debug!("Getting sound");
        // Get sound
        let sound_endpoint = fuchsia_component::directory::open_file_async(
            &self.incoming,
            FACTORY_RESET_SOUND_PATH,
            fio::R_STAR_DIR,
        )
        .context("Failed to open factory reset sound file")?;

        log::debug!("Playing sound");
        // Play sound
        let sound_player = self
            .incoming
            .connect_protocol::<PlayerProxy>()
            .with_context(|| format!("failed to connect to {}", PlayerMarker::PROTOCOL_NAME))?;

        log::debug!("Connected to player");
        let sound_id = 0;
        let _duration: i64 = sound_player
            .add_sound_from_file(sound_id, sound_endpoint.into_client_end().unwrap())
            .await
            .context("AddSoundFromFile error")?
            .map_err(zx::Status::err_from_raw)
            .context("AddSoundFromFile failed")?;
        log::debug!("Added sound from file");

        sound_player
            .play_sound2(sound_id, AudioRenderUsage2::Media)
            .await
            .context("PlaySound2 error")?
            .map_err(|err: PlaySoundError| anyhow!("PlaySound2 failed: {:?}", err))?;

        log::debug!("Played sound");

        Ok(())
    }

    /// Performs the actual factory reset.
    async fn reset(self: &Rc<Self>) -> Result<(), Error> {
        log::info!("Beginning reset sequence");
        if let Err(error) = self
            .play_reset_sound()
            .on_timeout(Dispatcher::after_deadline(MonotonicInstant::after(EARCON_TIMEOUT)), || {
                Err(anyhow!("play_reset_sound took too long"))
            })
            .await
        {
            log::warn!("Failed to play reset sound: {:?}", error);
        }

        // Trigger reset
        self.set_factory_reset_state(FactoryResetState::Resetting);
        log::info!("Calling {}.Reset", FactoryResetMarker::PROTOCOL_NAME);
        let factory_reset =
            self.incoming.connect_protocol::<FactoryResetProxy>().with_context(|| {
                format!("failed to connect to {}", FactoryResetMarker::PROTOCOL_NAME)
            })?;
        factory_reset.reset().await.context("failed while calling Reset")?;
        Ok(())
    }
}

impl Handler for FactoryResetHandler {
    fn set_handler_healthy(self: std::rc::Rc<Self>) {
        self.inspect_status.health_node.borrow_mut().set_ok();
    }

    fn set_handler_unhealthy(self: std::rc::Rc<Self>, msg: &str) {
        self.inspect_status.health_node.borrow_mut().set_unhealthy(msg);
    }

    fn get_name(&self) -> &'static str {
        "FactoryResetHandler"
    }

    fn interest(&self) -> Vec<input_device::InputEventType> {
        vec![input_device::InputEventType::ConsumerControls]
    }
}

#[async_trait(?Send)]
impl UnhandledInputHandler for FactoryResetHandler {
    /// This InputHandler doesn't consume any input events. It just passes them on to the next handler in the pipeline.
    /// Since it doesn't need exclusive access to the events this seems like the best way to avoid handlers further
    /// down the pipeline missing events that they need.
    async fn handle_unhandled_input_event(
        self: Rc<Self>,
        unhandled_input_event: input_device::UnhandledInputEvent,
    ) -> Vec<input_device::InputEvent> {
        fuchsia_trace::duration!("input", "factory_reset_handler");
        match unhandled_input_event {
            input_device::UnhandledInputEvent {
                device_event: input_device::InputDeviceEvent::ConsumerControls(ref event),
                device_descriptor:
                    input_device::InputDeviceDescriptor::ConsumerControls(ref descriptor),
                event_time,
                trace_id: _,
            } => {
                fuchsia_trace::duration!("input", "factory_reset_handler[processing]");
                self.inspect_status.count_received_event(&event_time);
                if descriptor.is_injected {
                    return vec![unhandled_input_event.into()];
                }
                match self.factory_reset_state() {
                    FactoryResetState::Idle => {
                        let event_clone = event.clone();
                        let self_weak = Rc::downgrade(&self);
                        let completed = Rc::new(Cell::new(false));
                        let completed_clone = completed.clone();
                        let task = Dispatcher::spawn_local(async move {
                            if let Some(self_tg) = self_weak.upgrade() {
                                self_tg.handle_allowed_event(&event_clone).await;
                            }
                            completed_clone.set(true);
                        });
                        let mut tasks = self._tasks.borrow_mut();
                        tasks.retain(|(_, completed)| !completed.get());
                        tasks.push((task, completed));
                    }
                    FactoryResetState::Disallowed => self.handle_disallowed_event(event),
                    FactoryResetState::ButtonCountdown { deadline: _ } => {
                        self.handle_button_countdown_event(event)
                    }
                    FactoryResetState::ResetCountdown { deadline: _ } => {
                        self.handle_reset_countdown_event(event)
                    }
                    FactoryResetState::Resetting => {
                        log::warn!("Recieved an input event while factory resetting the device")
                    }
                };
            }
            _ => {
                self.metrics_logger.log_error(
                    InputPipelineErrorMetricDimensionEvent::HandlerReceivedUninterestedEvent,
                    std::format!(
                        "{} uninterested input event: {:?}",
                        self.get_name(),
                        unhandled_input_event.get_event_type()
                    ),
                );
            }
        };

        vec![input_device::InputEvent::from(unhandled_input_event)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consumer_controls_binding::ConsumerControlsDeviceDescriptor;
    use crate::input_handler::InputHandler;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_recovery_policy::{DeviceMarker, DeviceProxy};
    use fidl_fuchsia_recovery_ui::{FactoryResetCountdownMarker, FactoryResetCountdownProxy};
    use fuchsia_async::TestExecutor;
    use pretty_assertions::assert_eq;
    use std::pin::pin;
    use std::task::Poll;

    fn create_factory_reset_countdown_proxy(
        reset_handler: Rc<FactoryResetHandler>,
    ) -> FactoryResetCountdownProxy {
        let (countdown_proxy, countdown_stream) =
            create_proxy_and_stream::<FactoryResetCountdownMarker>();

        let stream_fut =
            reset_handler.clone().handle_factory_reset_countdown_request_stream(countdown_stream);

        let completed = Rc::new(Cell::new(false));
        let completed_clone = completed.clone();
        let task = Dispatcher::spawn_local(async move {
            if stream_fut.await.is_err() {
                log::warn!("Failed to handle factory reset countdown request stream");
            }
            completed_clone.set(true);
        });
        reset_handler._tasks.borrow_mut().push((task, completed));

        countdown_proxy
    }

    fn create_recovery_policy_proxy(reset_handler: Rc<FactoryResetHandler>) -> DeviceProxy {
        let (device_proxy, device_stream) = create_proxy_and_stream::<DeviceMarker>();

        let reset_handler_clone = reset_handler.clone();
        let completed = Rc::new(Cell::new(false));
        let completed_clone = completed.clone();
        let task = Dispatcher::spawn_local(async move {
            if reset_handler_clone
                .handle_recovery_policy_device_request_stream(device_stream)
                .await
                .is_err()
            {
                log::warn!("Failed to handle recovery policy device request stream");
            }
            completed_clone.set(true);
        });
        reset_handler._tasks.borrow_mut().push((task, completed));

        device_proxy
    }

    fn create_input_device_descriptor() -> input_device::InputDeviceDescriptor {
        input_device::InputDeviceDescriptor::ConsumerControls(ConsumerControlsDeviceDescriptor {
            buttons: vec![
                fidl_fuchsia_input::ConsumerControlButton::CameraDisable,
                fidl_fuchsia_input::ConsumerControlButton::FactoryReset,
                fidl_fuchsia_input::ConsumerControlButton::MicMute,
                fidl_fuchsia_input::ConsumerControlButton::Pause,
                fidl_fuchsia_input::ConsumerControlButton::VolumeDown,
                fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
            ],
            device_id: 0,
            is_injected: false,
        })
    }

    fn create_reset_consumer_controls_event() -> ConsumerControlsEvent {
        ConsumerControlsEvent::new(
            vec![fidl_fuchsia_input::ConsumerControlButton::FactoryReset],
            None,
        )
    }

    fn create_non_reset_consumer_controls_event() -> ConsumerControlsEvent {
        ConsumerControlsEvent::new(
            vec![
                fidl_fuchsia_input::ConsumerControlButton::CameraDisable,
                fidl_fuchsia_input::ConsumerControlButton::MicMute,
                fidl_fuchsia_input::ConsumerControlButton::Pause,
                fidl_fuchsia_input::ConsumerControlButton::VolumeDown,
                fidl_fuchsia_input::ConsumerControlButton::VolumeUp,
            ],
            None,
        )
    }

    fn create_non_reset_input_event() -> input_device::UnhandledInputEvent {
        let device_event = input_device::InputDeviceEvent::ConsumerControls(
            create_non_reset_consumer_controls_event(),
        );

        input_device::UnhandledInputEvent {
            device_event,
            device_descriptor: create_input_device_descriptor(),
            event_time: zx::MonotonicInstant::get(),
            trace_id: None,
        }
    }

    fn create_reset_input_event() -> input_device::UnhandledInputEvent {
        let device_event = input_device::InputDeviceEvent::ConsumerControls(
            create_reset_consumer_controls_event(),
        );

        input_device::UnhandledInputEvent {
            device_event,
            device_descriptor: create_input_device_descriptor(),
            event_time: zx::MonotonicInstant::get(),
            trace_id: None,
        }
    }

    fn create_injected_reset_input_event() -> input_device::UnhandledInputEvent {
        let device_event = input_device::InputDeviceEvent::ConsumerControls(
            create_reset_consumer_controls_event(),
        );

        let mut descriptor = match create_input_device_descriptor() {
            input_device::InputDeviceDescriptor::ConsumerControls(desc) => desc,
            _ => panic!("Expected ConsumerControls"),
        };
        descriptor.is_injected = true;

        input_device::UnhandledInputEvent {
            device_event,
            device_descriptor: input_device::InputDeviceDescriptor::ConsumerControls(descriptor),
            event_time: zx::MonotonicInstant::get(),
            trace_id: None,
        }
    }

    #[fuchsia::test]
    async fn injected_reset_event_is_ignored() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );

        // Send an injected reset event
        let reset_event = create_injected_reset_input_event();
        let events = reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // The handler doesn't consume events, but explicitly check that it remained in Idle state,
        // proving that the injected event was ignored and the countdown was not started!
        assert_eq!(events.len(), 1);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    async fn is_reset_requested_looks_for_reset_signal() {
        let reset_event = create_reset_consumer_controls_event();
        let non_reset_event = create_non_reset_consumer_controls_event();

        assert!(
            is_reset_requested(&reset_event),
            "Should reset when the reset signal is received."
        );
        assert!(
            !is_reset_requested(&non_reset_event),
            "Should only reset when the reset signal is received."
        );
    }

    #[fuchsia::test]
    async fn factory_reset_countdown_listener_gets_initial_state() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    fn factory_reset_countdown_listener_is_notified_on_state_change() -> Result<(), Error> {
        let mut executor = TestExecutor::new_with_fake_time();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        let get_countdown_state = |executor: &mut TestExecutor| {
            let mut fut = countdown_proxy.watch();
            loop {
                // NB: cannot call run_singlethreaded on an executor with a fake clock.
                match executor.run_until_stalled(&mut fut) {
                    Poll::Pending => continue,
                    Poll::Ready(state) => {
                        return state.expect("Failed to get countdown state");
                    }
                }
            }
        };

        // The initial state should be no scheduled reset time and the
        // FactoryRestHandler state should be FactoryResetState::Idle
        let countdown_state = get_countdown_state(&mut executor);
        let handler_state = reset_handler.factory_reset_state();
        assert_eq!(countdown_state.scheduled_reset_time, None);
        assert_eq!(handler_state, FactoryResetState::Idle);

        // Send a reset event
        let reset_event = create_reset_input_event();
        let mut handle_input_event_fut =
            pin!(reset_handler.clone().handle_unhandled_input_event(reset_event));
        assert_matches!(executor.run_until_stalled(&mut handle_input_event_fut), Poll::Ready(events) => {
            assert_matches!(events.as_slice(), [input_device::InputEvent { .. }]);
        });

        // The next state will be FactoryResetState::ButtonCountdown with no scheduled reset
        let countdown_state = get_countdown_state(&mut executor);
        let handler_state = reset_handler.factory_reset_state();
        assert_eq!(countdown_state.scheduled_reset_time, None);
        assert_matches!(handler_state, FactoryResetState::ButtonCountdown { deadline: _ });

        // Skip ahead 500ms for the ButtonCountdown
        executor.set_fake_time(MonotonicInstant::after(MonotonicDuration::from_millis(500)));
        executor.wake_expired_timers();

        // After the ButtonCountdown the reset_handler enters the
        // FactoryResetState::ResetCountdown state WITH a scheduled reset time.
        let countdown_state = get_countdown_state(&mut executor);
        let handler_state = reset_handler.factory_reset_state();
        assert_matches!(countdown_state.scheduled_reset_time, Some(_));
        assert_matches!(handler_state, FactoryResetState::ResetCountdown { deadline: _ });

        // Skip ahead 10s for the ResetCountdown
        executor.set_fake_time(MonotonicInstant::after(MonotonicDuration::from_seconds(10)));
        executor.wake_expired_timers();

        // After the ResetCountdown the reset_handler enters the
        // FactoryResetState::Resetting state with no scheduled reset time.
        let countdown_state = get_countdown_state(&mut executor);
        let handler_state = reset_handler.factory_reset_state();
        assert_eq!(countdown_state.scheduled_reset_time, None);
        assert_eq!(handler_state, FactoryResetState::Resetting);

        Ok(())
    }

    #[fuchsia::test]
    async fn recovery_policy_requests_update_reset_handler_state() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        // Set FactoryResetState to Disallow
        let device_proxy = create_recovery_policy_proxy(reset_handler.clone());
        device_proxy.set_is_local_reset_allowed(false).expect("Failed to set recovery policy");

        // State should now be in Disallow and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Disallowed);

        // Send reset event
        let reset_event = create_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // State should still be Disallow
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Disallowed);

        // Set the state back to Allow
        let device_proxy = create_recovery_policy_proxy(reset_handler.clone());
        device_proxy.set_is_local_reset_allowed(true).expect("Failed to set recovery policy");

        // State should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    fn handle_allowed_event_changes_state_with_reset() {
        let mut executor = TestExecutor::new();

        let reset_event = create_reset_consumer_controls_event();
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = executor
            .run_singlethreaded(countdown_proxy.watch())
            .expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        let handle_allowed_event_fut = reset_handler.handle_allowed_event(&reset_event);
        futures::pin_mut!(handle_allowed_event_fut);
        assert_eq!(executor.run_until_stalled(&mut handle_allowed_event_fut), Poll::Pending);

        // This should result in the reset handler entering the ButtonCountdown state
        assert_matches!(
            executor.run_singlethreaded(countdown_proxy.watch()),
            Ok(FactoryResetCountdownState { scheduled_reset_time: None, .. })
        );
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ButtonCountdown { deadline: _ }
        );
    }

    #[fuchsia::test]
    async fn handle_allowed_event_wont_change_state_without_reset() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        let non_reset_event = create_non_reset_consumer_controls_event();
        reset_handler.clone().handle_allowed_event(&non_reset_event).await;

        // This should result in the reset handler staying in the Allowed state
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    async fn handle_disallowed_event_wont_change_state() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        *reset_handler.factory_reset_state.borrow_mut() = FactoryResetState::Disallowed;

        // Calling handle_disallowed_event shouldn't change the state no matter
        // what the contents of the event are
        let reset_event = create_reset_consumer_controls_event();
        reset_handler.handle_disallowed_event(&reset_event);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Disallowed);

        let non_reset_event = create_non_reset_consumer_controls_event();
        reset_handler.handle_disallowed_event(&non_reset_event);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Disallowed);
    }

    #[fuchsia::test]
    async fn handle_button_countdown_event_changes_state_when_reset_no_longer_requested() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );

        let deadline = MonotonicInstant::after(BUTTON_TIMEOUT);
        *reset_handler.factory_reset_state.borrow_mut() =
            FactoryResetState::ButtonCountdown { deadline };

        // Calling handle_button_countdown_event should reset the handler
        // to the idle state
        let non_reset_event = create_non_reset_consumer_controls_event();
        reset_handler.handle_button_countdown_event(&non_reset_event);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    async fn handle_reset_countdown_event_changes_state_when_reset_no_longer_requested() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );

        *reset_handler.factory_reset_state.borrow_mut() =
            FactoryResetState::ResetCountdown { deadline: MonotonicInstant::now() };

        // Calling handle_reset_countdown_event should reset the handler
        // to the idle state
        let non_reset_event = create_non_reset_consumer_controls_event();
        reset_handler.handle_reset_countdown_event(&non_reset_event);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    async fn factory_reset_disallowed_during_button_countdown() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        // Send reset event
        let reset_event = create_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // State should now be ButtonCountdown and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ButtonCountdown { deadline: _ }
        );

        // Set FactoryResetState to Disallow
        let device_proxy = create_recovery_policy_proxy(reset_handler.clone());
        device_proxy.set_is_local_reset_allowed(false).expect("Failed to set recovery policy");

        // State should now be in Disallow and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Disallowed);
    }

    #[fuchsia::test]
    async fn factory_reset_disallowed_during_reset_countdown() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        // Send reset event
        let reset_event = create_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // State should now be ButtonCountdown and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ButtonCountdown { deadline: _ }
        );

        // State should now be ResetCountdown and scheduled_reset_time should be Some
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_matches!(reset_state.scheduled_reset_time, Some(_));
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ResetCountdown { deadline: _ }
        );

        // Set FactoryResetState to Disallow
        let device_proxy = create_recovery_policy_proxy(reset_handler.clone());
        device_proxy.set_is_local_reset_allowed(false).expect("Failed to set recovery policy");

        // State should now be in Disallow and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Disallowed);
    }

    #[fuchsia::test]
    async fn factory_reset_cancelled_during_button_countdown() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        // Send reset event
        let reset_event = create_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // State should now be ButtonCountdown and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ButtonCountdown { deadline: _ }
        );

        // Pass in an event to simulate releasing the reset button
        let non_reset_event = create_non_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(non_reset_event).await;

        // State should now be in Idle and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    async fn factory_reset_cancelled_during_reset_countdown() {
        let inspector = fuchsia_inspect::Inspector::default();
        let test_node = inspector.root().create_child("test_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &test_node,
            metrics::MetricsLogger::default(),
        );
        let countdown_proxy = create_factory_reset_countdown_proxy(reset_handler.clone());

        // Initial state should be FactoryResetState::Idle with no scheduled reset
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);

        // Send reset event
        let reset_event = create_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // State should now be ButtonCountdown and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ButtonCountdown { deadline: _ }
        );

        // State should now be ResetCountdown and scheduled_reset_time should be Some
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_matches!(reset_state.scheduled_reset_time, Some(_));
        assert_matches!(
            reset_handler.factory_reset_state(),
            FactoryResetState::ResetCountdown { deadline: _ }
        );

        // Pass in an event to simulate releasing the reset button
        let non_reset_event = create_non_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(non_reset_event).await;

        // State should now be in Idle and scheduled_reset_time should be None
        let reset_state = countdown_proxy.watch().await.expect("Failed to get countdown state");
        assert_eq!(reset_state.scheduled_reset_time, None);
        assert_eq!(reset_handler.factory_reset_state(), FactoryResetState::Idle);
    }

    #[fuchsia::test]
    async fn factory_reset_handler_initialized_with_inspect_node() {
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let _handler = FactoryResetHandler::new(
            Incoming::new(),
            &fake_handlers_node,
            metrics::MetricsLogger::default(),
        );
        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                factory_reset_handler: {
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
        });
    }

    #[fuchsia::test]
    async fn factory_reset_handler_inspect_counts_events() {
        let inspector = fuchsia_inspect::Inspector::default();
        let fake_handlers_node = inspector.root().create_child("input_handlers_node");
        let reset_handler = FactoryResetHandler::new(
            Incoming::new(),
            &fake_handlers_node,
            metrics::MetricsLogger::default(),
        );

        // Send reset event
        let reset_event = create_reset_input_event();
        reset_handler.clone().handle_unhandled_input_event(reset_event).await;

        // Send handled event that should be ignored.
        let mut handled_event = input_device::InputEvent::from(create_reset_input_event());
        handled_event.handled = input_device::Handled::Yes;
        reset_handler.clone().handle_input_event(handled_event).await;

        // Send event to simulate releasing the reset button
        let non_reset_event = create_non_reset_input_event();
        let last_event_timestamp: u64 =
            non_reset_event.clone().event_time.into_nanos().try_into().unwrap();
        reset_handler.clone().handle_unhandled_input_event(non_reset_event).await;

        diagnostics_assertions::assert_data_tree!(inspector, root: {
            input_handlers_node: {
                factory_reset_handler: {
                    events_received_count: 2u64,
                    events_handled_count: 0u64,
                    last_received_timestamp_ns: last_event_timestamp,
                    "fuchsia.inspect.Health": {
                        status: "STARTING_UP",
                        // Timestamp value is unpredictable and not relevant in this context,
                        // so we only assert that the property is present.
                        start_timestamp_nanos: diagnostics_assertions::AnyProperty
                    },
                }
            }
        });
    }
}
