// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use futures::StreamExt;
use log::{debug, info, warn};
use std::sync::{Arc, LazyLock};
use zx;

use fidl::endpoints::create_request_stream;
use fidl_fuchsia_ui_input::MediaButtonsEvent;
use {
    fidl_fuchsia_feedback as ffeedback, fidl_fuchsia_hardware_power_statecontrol as fpower,
    fidl_fuchsia_ui_policy as fuipolicy,
};

static MAX_PRESS_INTERVAL_NS: LazyLock<i64> = LazyLock::new(|| 500 * 1_000_000); // 500ms in nanoseconds
const REQUIRED_PRESS_COUNT: u32 = 5;

pub trait Clock: Send + Sync {
    fn now_ns(&self) -> i64;
}

pub struct BootClock;

impl Clock for BootClock {
    fn now_ns(&self) -> i64 {
        zx::BootInstant::get().into_nanos()
    }
}

#[derive(Debug)]
struct ButtonPressState {
    count: u32,
    last_press_time_ns: i64,
    power_was_pressed: bool,
    crash_report_in_progress: bool,
    #[cfg(test)]
    action_triggered_count: u32,
}

impl ButtonPressState {
    fn new() -> Self {
        Self {
            count: 0,
            last_press_time_ns: 0,
            power_was_pressed: false,
            crash_report_in_progress: false,
            #[cfg(test)]
            action_triggered_count: 0,
        }
    }
}

pub struct DebugState<C: Clock> {
    /// Whether the system supports 5-button press to debug.
    debug_enabled: bool,
    button_press_state: Mutex<ButtonPressState>,
    clock: C,
    crash_reporter_source: Option<ffeedback::CrashReporterProxy>,
    power_statecontrol_admin_source: Option<fpower::AdminProxy>,
}

/// The concrete `DebugState` used in production.
pub type DebugManager = DebugState<BootClock>;

impl DebugState<BootClock> {
    pub fn new(debug_enabled: bool) -> Self {
        Self {
            debug_enabled,
            button_press_state: Mutex::new(ButtonPressState::new()),
            clock: BootClock,
            crash_reporter_source: fuchsia_component::client::connect_to_protocol::<
                ffeedback::CrashReporterMarker,
            >()
            .map_err(|e| warn!("Failed to connect to CrashReporter: {e}"))
            .ok(),
            power_statecontrol_admin_source: fuchsia_component::client::connect_to_protocol::<
                fpower::AdminMarker,
            >()
            .map_err(|e| warn!("Failed to connect to PowerStateControlAdmin: {e}"))
            .ok(),
        }
    }
}

impl<C: Clock + 'static> DebugState<C> {
    #[cfg(test)]
    fn new_for_test(
        debug_enabled: bool,
        clock: C,
        crash_reporter_proxy: Option<ffeedback::CrashReporterProxy>,
        power_statecontrol_admin_proxy: Option<fpower::AdminProxy>,
    ) -> Self {
        Self {
            debug_enabled,
            button_press_state: Mutex::new(ButtonPressState::new()),
            clock,
            crash_reporter_source: crash_reporter_proxy,
            power_statecontrol_admin_source: power_statecontrol_admin_proxy,
        }
    }

    pub fn start_media_buttons_listener(self: Arc<Self>) {
        if !self.debug_enabled {
            info!("Debug mode not enabled, skipping media button listener registration.");
            return;
        }

        info!("Registering for media button events to enable 5-button press for debug.");
        fuchsia_async::Task::spawn(async move {
            match fuchsia_component::client::connect_to_protocol::<
                fuipolicy::DeviceListenerRegistryMarker,
            >() {
                Ok(device_listener_registry) => {
                    let (client_end, mut stream) =
                        create_request_stream::<fuipolicy::MediaButtonsListenerMarker>();
                    if let Err(e) = device_listener_registry.register_listener(client_end).await {
                        warn!("Failed to register media buttons listener: {e:?}");
                        return;
                    }
                    while let Some(Ok(request)) = stream.next().await {
                        if let fuipolicy::MediaButtonsListenerRequest::OnEvent {
                            event,
                            responder,
                        } = request
                        {
                            if self.process_button_event(&event) {
                                info!("Detected 5 function button presses in a row. Filing crash report.");
                                if self.file_crash_report().await {
                                    if !self.reboot_device().await {
                                        self.button_press_state.lock().crash_report_in_progress =
                                            false;
                                    }
                                } else {
                                    self.button_press_state.lock().crash_report_in_progress = false;
                                }
                            }
                            if let Err(e) = responder.send() {
                                warn!("Failed to send response for media buttons event: {e:?}");
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to connect to fuchsia.ui.policy.DeviceListenerRegistry: {e:?}");
                }
            }
        })
        .detach();
    }

    fn process_button_event(&self, event: &MediaButtonsEvent) -> bool {
        let mut state = self.button_press_state.lock();
        if state.crash_report_in_progress {
            info!("Crash report in progress, ignoring button event.");
            return false;
        }
        if let Some(power_is_pressed) = event.power
            && (power_is_pressed || state.power_was_pressed)
        {
            if state.count >= 3 {
                info!(
                    "Detected overlapping POWER button activity; resetting FUNCTION button counter."
                );
            } else {
                debug!(
                    "Detected overlapping POWER button activity; resetting FUNCTION button counter."
                );
            }
            state.power_was_pressed = power_is_pressed;
            state.count = 0;
            return false;
        }

        if event.function != Some(true) {
            // Function button could have been released. Ignore it.
            return false;
        }

        // At this point, we have a pure function press event.
        let now_ns = self.clock.now_ns();

        if now_ns - state.last_press_time_ns > *MAX_PRESS_INTERVAL_NS {
            state.count = 1;
        } else {
            state.count += 1;
        }

        state.last_press_time_ns = now_ns;

        if state.count >= 3 {
            info!(
                "Identified {:?} side button presses in a row. {:?} in a row will trigger a crash report and reboot sequence.",
                state.count, REQUIRED_PRESS_COUNT
            );
        }

        if state.count >= REQUIRED_PRESS_COUNT {
            state.count = 0;
            state.crash_report_in_progress = true;
            #[cfg(test)]
            {
                state.action_triggered_count += 1;
            }
            return true;
        }
        false
    }

    pub fn stop(&self) {
        if !self.debug_enabled {
            return;
        }
        info!("Resetting debug button press state.");
        *self.button_press_state.lock() = ButtonPressState::new();
    }

    #[cfg(test)]
    pub(crate) fn get_button_press_state_count(&self) -> u32 {
        self.button_press_state.lock().count
    }

    #[cfg(test)]
    pub(crate) fn set_button_press_state_count(&self, count: u32) {
        self.button_press_state.lock().count = count;
    }

    async fn file_crash_report(&self) -> bool {
        if let Some(reporter) = &self.crash_reporter_source {
            let report = ffeedback::CrashReport {
                program_name: Some("session_manager".to_string()),
                crash_signature: Some("fuchsia-user-SOS-device-stuck".to_string()),
                is_fatal: Some(false),
                ..Default::default()
            };
            match reporter.file_report(report).await {
                Ok(Ok(_)) => {
                    info!("Successfully filed crash report.");
                    true
                }
                Ok(Err(e)) => {
                    warn!("Failed to file crash report: {e:?}");
                    false
                }
                Err(e) => {
                    warn!("Failed to file crash report: {e:?}");
                    false
                }
            }
        } else {
            warn!("Failed to get fuchsia.feedback.CrashReporter");
            false
        }
    }

    async fn reboot_device(&self) -> bool {
        info!("Rebooting device due to 5-button press.");
        if let Some(proxy) = self.power_statecontrol_admin_source.clone() {
            let options = fpower::ShutdownOptions {
                action: Some(fpower::ShutdownAction::Reboot),
                reasons: Some(vec![fpower::ShutdownReason::UserRequestDeviceStuck]),
                ..Default::default()
            };
            if let Err(e) = proxy.shutdown(&options).await {
                warn!("Failed to reboot device: {e:?}");
                false
            } else {
                true
            }
        } else {
            warn!("Failed to connect to fuchsia.hardware.power.statecontrol.Admin");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_ui_input as fui_input;
    use futures::{TryStreamExt, join};

    struct FakeClock {
        now: Mutex<i64>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self { now: Mutex::new(0) }
        }

        fn advance_ns(&self, duration_ns: i64) {
            *self.now.lock() += duration_ns;
        }
    }

    impl Clock for FakeClock {
        fn now_ns(&self) -> i64 {
            *self.now.lock()
        }
    }

    // By implementing Clock for Arc<T>, we can pass a clone of the Arc to the DebugState,
    // while the test retains ownership of the original Arc. This allows the test to call
    // `advance_ns` on the FakeClock.
    impl<T: Clock> Clock for Arc<T> {
        fn now_ns(&self) -> i64 {
            self.as_ref().now_ns()
        }
    }

    #[fuchsia::test]
    fn test_successful_press_sequence() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };

        // Press the button REQUIRED_PRESS_COUNT - 1 times, which should not trigger the debug state.
        for i in 1..REQUIRED_PRESS_COUNT {
            assert!(
                !debug_state.process_button_event(&press_event),
                "Incorrectly triggered action after {i} presses"
            );
            let state = debug_state.button_press_state.lock();
            assert_eq!(
                state.action_triggered_count, 0,
                "Incorrectly incremented trigger count after {i} presses. State: {state:?}"
            );
        }

        assert!(
            debug_state.process_button_event(&press_event),
            "The final press should trigger the sequence."
        );
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.action_triggered_count, 1, "State: {state:?}");
    }

    #[fuchsia::test]
    fn test_counter_resets_after_successful_sequence() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };

        // Test that the counter resets after a successful sequence.
        for _ in 1..REQUIRED_PRESS_COUNT {
            assert!(!debug_state.process_button_event(&press_event));
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
        }
        assert!(debug_state.process_button_event(&press_event));
        {
            let mut state = debug_state.button_press_state.lock();
            assert_eq!(state.action_triggered_count, 1, "State: {state:?}");
            assert!(state.crash_report_in_progress);
            // Manually reset for test purposes.
            state.crash_report_in_progress = false;
        }

        assert!(
            !debug_state.process_button_event(&press_event),
            "The next press should start a new sequence."
        );
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.count, 1, "State: {state:?}");
        assert_eq!(state.action_triggered_count, 1, "State: {state:?}");
    }

    #[fuchsia::test]
    fn test_ignores_presses_while_crash_report_in_progress() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };

        // Trigger the crash report.
        for _ in 1..REQUIRED_PRESS_COUNT {
            assert!(!debug_state.process_button_event(&press_event));
        }
        assert!(debug_state.process_button_event(&press_event));
        {
            let state = debug_state.button_press_state.lock();
            assert!(state.crash_report_in_progress);
            assert_eq!(state.action_triggered_count, 1);
        }

        // Try to press again, it should be ignored.
        assert!(!debug_state.process_button_event(&press_event));
        {
            let state = debug_state.button_press_state.lock();
            assert!(state.crash_report_in_progress);
            assert_eq!(state.action_triggered_count, 1);
            assert_eq!(state.count, 0);
        }

        // Manually reset the flag.
        debug_state.button_press_state.lock().crash_report_in_progress = false;

        // The next press should start a new sequence.
        assert!(!debug_state.process_button_event(&press_event));
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.count, 1);
    }

    #[fuchsia::test]
    fn test_counter_resets_after_timeout() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };

        // Test that the counter resets after a timeout.
        for _ in 1..REQUIRED_PRESS_COUNT - 1 {
            assert!(!debug_state.process_button_event(&press_event));
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
        }
        // Wait for longer than the max interval.
        clock.advance_ns(*MAX_PRESS_INTERVAL_NS + 1);

        assert!(
            !debug_state.process_button_event(&press_event),
            "This press should reset the counter to 1, not increment it."
        );
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.count, 1, "State: {state:?}");
        assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
    }

    #[fuchsia::test]
    fn test_power_button_press_resets_counter() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };
        let power_press_event =
            fui_input::MediaButtonsEvent { power: Some(true), ..Default::default() };

        assert!(
            !debug_state.process_button_event(&press_event),
            "first function press should not trigger action"
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.count, 1, "should have count of 1. State: {state:?}");
        }
        assert!(
            !debug_state.process_button_event(&power_press_event),
            "power press should not trigger action"
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(
                state.count, 0,
                "A non-function-button press should reset the counter. State: {state:?}"
            );
            assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
        }

        assert!(
            !debug_state.process_button_event(&press_event),
            "The next press should start a new sequence."
        );
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.count, 1, "State: {state:?}");
        assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
    }

    #[fuchsia::test]
    fn test_power_button_release_resets_counter() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let function_press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };
        let function_release_event =
            fui_input::MediaButtonsEvent { function: Some(false), ..Default::default() };
        let power_press_event =
            fui_input::MediaButtonsEvent { power: Some(true), ..Default::default() };
        let power_release_event =
            fui_input::MediaButtonsEvent { power: Some(false), ..Default::default() };

        assert!(
            !debug_state.process_button_event(&power_press_event),
            "power press should not trigger action"
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.count, 0, "count should be 0 after power press. State: {state:?}");
        }
        assert!(
            !debug_state.process_button_event(&function_press_event),
            "function press should not trigger action"
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.count, 1, "count should be 1 after function press. State: {state:?}");
        }
        assert!(
            !debug_state.process_button_event(&power_release_event),
            "A non-function-button release should reset the counter."
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.count, 0, "count should be 0 after power release. State: {state:?}");
            assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
        }
        assert!(
            !debug_state.process_button_event(&function_release_event),
            "function release should not trigger action"
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(
                state.count, 0,
                "count should be 0 after function release. State: {state:?}"
            );
            assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
        }

        assert!(
            !debug_state.process_button_event(&function_press_event),
            "The next press should start a new sequence."
        );
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.count, 1, "State: {state:?}");
        assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
    }

    #[fuchsia::test]
    fn test_ignores_function_button_releases() {
        let clock = Arc::new(FakeClock::new());
        let debug_state = DebugState::new_for_test(true, clock.clone(), None, None);

        let press_event =
            fui_input::MediaButtonsEvent { function: Some(true), ..Default::default() };
        let function_release_event =
            fui_input::MediaButtonsEvent { function: Some(false), ..Default::default() };

        assert!(
            !debug_state.process_button_event(&press_event),
            "first function press should not trigger action"
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.count, 1, "count should be 1. State: {state:?}");
        }
        assert!(
            !debug_state.process_button_event(&function_release_event),
            "A button release should be ignored and not affect the counter."
        );
        {
            let state = debug_state.button_press_state.lock();
            assert_eq!(state.count, 1, "count should still be 1. State: {state:?}");
            assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
        }
        assert!(
            !debug_state.process_button_event(&press_event),
            "second function press should not trigger action"
        );
        let state = debug_state.button_press_state.lock();
        assert_eq!(state.count, 2, "count should be 2. State: {state:?}");
        assert_eq!(state.action_triggered_count, 0, "State: {state:?}");
    }

    async fn run_report_server(mut stream: ffeedback::CrashReporterRequestStream) {
        let request =
            stream.try_next().await.expect("failed to read from stream").expect("stream is empty");
        match request {
            ffeedback::CrashReporterRequest::FileReport { report, responder } => {
                assert_eq!(
                    report,
                    ffeedback::CrashReport {
                        program_name: Some("session_manager".to_string()),
                        crash_signature: Some("fuchsia-user-SOS-device-stuck".to_string()),
                        is_fatal: Some(false),
                        ..Default::default()
                    }
                );
                responder
                    .send(Ok(&ffeedback::FileReportResults::default()))
                    .expect("failed to send response");
            }
        }
    }

    #[fuchsia::test]
    async fn test_file_crash_report() {
        let clock = Arc::new(FakeClock::new());
        let (crash_reporter_proxy, stream) =
            create_proxy_and_stream::<ffeedback::CrashReporterMarker>();
        let (power_statecontrol_admin_proxy, _) = create_proxy_and_stream::<fpower::AdminMarker>();

        let server_fut = run_report_server(stream);

        // File the crash report.
        let debug_state = DebugState::new_for_test(
            true,
            clock.clone(),
            Some(crash_reporter_proxy),
            Some(power_statecontrol_admin_proxy),
        );
        let client_fut = async {
            assert!(debug_state.file_crash_report().await);
        };

        join!(client_fut, server_fut);
    }

    async fn run_reboot_server(mut stream: fpower::AdminRequestStream) {
        let request =
            stream.try_next().await.expect("failed to read from stream").expect("stream is empty");
        match request {
            fpower::AdminRequest::Shutdown { options, responder } => {
                assert_eq!(
                    options,
                    fpower::ShutdownOptions {
                        action: Some(fpower::ShutdownAction::Reboot),
                        reasons: Some(vec![fpower::ShutdownReason::UserRequestDeviceStuck]),
                        ..Default::default()
                    }
                );
                responder.send(Ok(())).expect("failed to send response");
            }
            _ => panic!("unexpected request"),
        }
    }

    #[fuchsia::test]
    async fn test_reboot_device() {
        let clock = Arc::new(FakeClock::new());
        let (crash_reporter_proxy, _) = create_proxy_and_stream::<ffeedback::CrashReporterMarker>();
        let (power_statecontrol_admin_proxy, stream) =
            create_proxy_and_stream::<fpower::AdminMarker>();

        let server_fut = run_reboot_server(stream);

        // Reboot the device.
        let debug_state = DebugState::new_for_test(
            true,
            clock.clone(),
            Some(crash_reporter_proxy),
            Some(power_statecontrol_admin_proxy),
        );
        let reboot_fut = debug_state.reboot_device();

        // Wait for the server to have received the call.
        let (reboot_succeeded, ()) = join!(reboot_fut, server_fut);
        assert!(reboot_succeeded);
    }
}
