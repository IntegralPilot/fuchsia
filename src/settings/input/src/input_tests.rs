// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::input_test_environment::{TestInputEnvironment, TestInputEnvironmentBuilder};
use crate::input_device_configuration::{
    InputConfiguration, InputDeviceConfiguration, SourceState,
};
use crate::types::{
    DeviceState, DeviceStateSource, InputCategory, InputDeviceType, InputInfoSources, InputState,
};
use fidl_fuchsia_settings::{
    DeviceState as FidlDeviceState, DeviceType, InputProxy, InputSettings,
    InputState as FidlInputState, ToggleStateFlags,
};
use fuchsia_async::TestExecutor;
use futures::channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures::pin_mut;
use futures::stream::StreamExt;
use futures::task::Poll;
use settings_media_buttons::MediaButtonsEventBuilder;
use settings_test_common::helpers::{
    clone_media_buttons_event_without_wake_lease, move_executor_forward,
};
use std::collections::HashMap;

const DEFAULT_MIC_STATE: bool = false;
const DEFAULT_CAMERA_STATE: bool = false;
const MUTED_BITS: u64 = 4;
const AVAILABLE_BITS: u64 = 1;
const MUTED_DISABLED_BITS: u64 = 12;
const DEFAULT_MIC_NAME: &str = "microphone";
const DEFAULT_CAMERA_NAME: &str = "camera";

fn create_default_input_info() -> InputInfoSources {
    InputInfoSources {
        input_device_state: InputState {
            input_categories: HashMap::<InputDeviceType, InputCategory>::new(),
        },
    }
}

// An InputConfiguration with an available microphone.
fn default_mic_config() -> InputConfiguration {
    InputConfiguration {
        devices: vec![InputDeviceConfiguration {
            device_name: DEFAULT_MIC_NAME.to_string(),
            device_type: InputDeviceType::MICROPHONE,
            source_states: vec![
                SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                SourceState { source: DeviceStateSource::SOFTWARE, state: AVAILABLE_BITS },
            ],
            mutable_toggle_state: MUTED_DISABLED_BITS,
        }],
    }
}

// An InputConfiguration with a muted microphone.
fn default_mic_config_muted() -> InputConfiguration {
    InputConfiguration {
        devices: vec![InputDeviceConfiguration {
            device_name: DEFAULT_MIC_NAME.to_string(),
            device_type: InputDeviceType::MICROPHONE,
            source_states: vec![
                SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                SourceState { source: DeviceStateSource::SOFTWARE, state: MUTED_BITS },
            ],
            mutable_toggle_state: MUTED_DISABLED_BITS,
        }],
    }
}

// An InputConfiguration with an available microphone and camera.
fn default_mic_cam_config() -> InputConfiguration {
    InputConfiguration {
        devices: vec![
            InputDeviceConfiguration {
                device_name: DEFAULT_MIC_NAME.to_string(),
                device_type: InputDeviceType::MICROPHONE,
                source_states: vec![
                    SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                    SourceState { source: DeviceStateSource::SOFTWARE, state: AVAILABLE_BITS },
                ],
                mutable_toggle_state: MUTED_DISABLED_BITS,
            },
            InputDeviceConfiguration {
                device_name: DEFAULT_CAMERA_NAME.to_string(),
                device_type: InputDeviceType::CAMERA,
                source_states: vec![
                    SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                    SourceState { source: DeviceStateSource::SOFTWARE, state: AVAILABLE_BITS },
                ],
                mutable_toggle_state: MUTED_DISABLED_BITS,
            },
        ],
    }
}

// An InputConfiguration with a microphone and sw disabled camera.
fn default_mic_cam_config_cam_sw_disabled() -> InputConfiguration {
    InputConfiguration {
        devices: vec![
            InputDeviceConfiguration {
                device_name: DEFAULT_MIC_NAME.to_string(),
                device_type: InputDeviceType::MICROPHONE,
                source_states: vec![
                    SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                    SourceState { source: DeviceStateSource::SOFTWARE, state: AVAILABLE_BITS },
                ],
                mutable_toggle_state: MUTED_DISABLED_BITS,
            },
            InputDeviceConfiguration {
                device_name: DEFAULT_CAMERA_NAME.to_string(),
                device_type: InputDeviceType::CAMERA,
                source_states: vec![
                    SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                    SourceState { source: DeviceStateSource::SOFTWARE, state: MUTED_BITS },
                ],
                mutable_toggle_state: MUTED_DISABLED_BITS,
            },
        ],
    }
}

// Creates an environment with an executor for moving forward execution and
// a configuration for the input devices.
fn create_env_and_executor_with_config(
    config: InputConfiguration,
    // The pre-populated data to insert into the store before spawning
    // the environment.
    initial_input_info: Option<InputInfoSources>,
    media_button_event_tx: UnboundedSender<settings_media_buttons::Event>,
) -> (TestExecutor, TestInputEnvironment) {
    let mut executor = TestExecutor::new_with_fake_time();
    let mut builder = TestInputEnvironmentBuilder::new()
        .set_input_device_config(config)
        .add_media_buttons_event_tx(media_button_event_tx);
    if let Some(initial_info) = initial_input_info {
        builder = builder.set_starting_input_info_sources(initial_info);
    }
    let env_future = builder.build();
    pin_mut!(env_future);
    let env = match executor.run_until_stalled(&mut env_future) {
        Poll::Ready(env) => env,
        _ => panic!("Failed to create environment"),
    };

    // Return order matters here. Executor must be in first position to avoid
    // it being torn down before env.
    (executor, env)
}

// Creates an environment with an executor for moving forward execution and
// a configuration for the input devices.
async fn create_env_with_config(
    config: InputConfiguration,
    // The pre-populated data to insert into the store before spawning
    // the environment.
    initial_input_info: Option<InputInfoSources>,
    media_button_event_tx: UnboundedSender<settings_media_buttons::Event>,
) -> TestInputEnvironment {
    let mut builder = TestInputEnvironmentBuilder::new()
        .set_input_device_config(config)
        .add_media_buttons_event_tx(media_button_event_tx);
    if let Some(initial_info) = initial_input_info {
        builder = builder.set_starting_input_info_sources(initial_info);
    }

    builder.build().await
}

// Set the software mic mute state to muted = [mic_muted] for input.
async fn set_mic_mute(proxy: &InputProxy, mic_muted: bool) {
    set_device_muted(proxy, mic_muted, DEFAULT_MIC_NAME, DeviceType::Microphone).await;
}

// Set the software camera disabled state to disabled = [disabled].
async fn set_camera_disable(proxy: &InputProxy, disabled: bool) {
    set_device_muted(proxy, disabled, DEFAULT_CAMERA_NAME, DeviceType::Camera).await;
}

// Helper to set a device's muted state via the input proxy.
async fn set_device_muted(
    proxy: &InputProxy,
    muted: bool,
    device_name: &str,
    device_type: DeviceType,
) {
    let mut input_state = FidlInputState::default();
    let mut states = Vec::new();

    input_state.name = Some(device_name.to_string());
    input_state.device_type = Some(device_type);
    input_state.state = Some(FidlDeviceState {
        toggle_flags: ToggleStateFlags::from_bits(if muted { MUTED_BITS } else { AVAILABLE_BITS }),
        ..Default::default()
    });

    states.push(input_state);
    proxy.set(&states).await.expect("set completed").expect("set successful");
}

// Switch the hardware mic state to muted = [muted] for input.
async fn switch_hardware_mic_mute(env: &TestInputEnvironment, muted: bool) {
    let buttons_event = MediaButtonsEventBuilder::new().set_mic_mute(muted).build();
    env.input_button_service
        .lock()
        .await
        .send_media_button_event(clone_media_buttons_event_without_wake_lease(&buttons_event))
        .await;
}

// Switch the hardware camera disable to disabled = [disabled].
async fn switch_hardware_camera_disable(env: &TestInputEnvironment, disabled: bool) {
    let buttons_event = MediaButtonsEventBuilder::new().set_camera_disable(disabled).build();
    env.input_button_service
        .lock()
        .await
        .send_media_button_event(clone_media_buttons_event_without_wake_lease(&buttons_event))
        .await;
}

// Perform a watch and check that the mic mute state matches [expected_muted_state].
async fn get_and_check_mic_mute(input_proxy: &InputProxy, expected_muted_state: bool) {
    let settings = input_proxy.watch().await.expect("watch completed");
    verify_muted_state(&settings, expected_muted_state, DeviceType::Microphone);
}

// Perform a watch and check that the camera disabled state matches [expected_camera_disabled_state].
async fn get_and_check_camera_disable(
    input_proxy: &InputProxy,
    expected_camera_disabled_state: bool,
) {
    let settings = input_proxy.watch().await.expect("watch completed");
    verify_muted_state(&settings, expected_camera_disabled_state, DeviceType::Camera);
}

// Waits for the media buttons receiver to receive an update, so that
// following code can be sure that the media buttons event was handled
// before continuing.
async fn wait_for_media_button_event(
    media_buttons_rx: &mut UnboundedReceiver<settings_media_buttons::Event>,
) {
    if let Some(event) = media_buttons_rx.next().await {
        // This is really a no-op, but ensures the code will fail to compile if
        // another variant is ever added to settings_media_buttons::Event.
        let settings_media_buttons::Event::OnButton(_) = event;
    } else {
        panic!("received None from media button rx. Tx closed before event received");
    }
}

// Perform a watch and check that the mic mute state matches [expected_mic_mute_state]
// and the camera disabled state matches [expected_camera_disabled_state].
async fn get_and_check_state(
    input_proxy: &InputProxy,
    expected_mic_mute_state: bool,
    expected_camera_disabled_state: bool,
) {
    let settings = input_proxy.watch().await.expect("watch completed");
    verify_muted_state(&settings, expected_camera_disabled_state, DeviceType::Camera);
    verify_muted_state(&settings, expected_mic_mute_state, DeviceType::Microphone);
}

// Helper for checking the returned muted state for a given
// device type in the watch results.
fn verify_muted_state(
    settings: &InputSettings,
    expected_muted_state: bool,
    device_type: DeviceType,
) {
    assert_eq!(
        settings
            .devices
            .as_ref()
            .unwrap()
            .iter()
            .find(|x| x.device_type == Some(device_type))
            .expect("Device not found in results")
            .state,
        if expected_muted_state {
            Some(FidlDeviceState {
                toggle_flags: ToggleStateFlags::from_bits(MUTED_BITS),
                ..Default::default()
            })
        } else {
            Some(FidlDeviceState {
                toggle_flags: ToggleStateFlags::from_bits(AVAILABLE_BITS),
                ..Default::default()
            })
        },
    );
}

// Test that a watch is executed correctly.
#[fuchsia::test(allow_stalls = false)]
async fn test_watch() {
    let env = TestInputEnvironmentBuilder::new()
        .set_input_device_config(default_mic_config())
        .build()
        .await;
    let input_proxy = env.input_service.clone();

    get_and_check_mic_mute(&input_proxy, false).await;
}

// Test that a set then watch for the mic is executed correctly.
#[fuchsia::test]
fn test_set_watch_mic_mute() {
    let (tx, mut rx) = mpsc::unbounded();
    let (mut executor, env) =
        create_env_and_executor_with_config(default_mic_config_muted(), None, tx);
    let input_proxy = env.input_service.clone();

    // SW muted, HW unmuted.
    move_executor_forward(
        &mut executor,
        async {
            set_mic_mute(&input_proxy, true).await;
        },
        "Failed to set mic mute",
    );
    move_executor_forward(
        &mut executor,
        async {
            get_and_check_mic_mute(&input_proxy, true).await;
        },
        "Failed to watch mic mute",
    );

    // SW unmuted, HW muted.
    move_executor_forward(
        &mut executor,
        async {
            switch_hardware_mic_mute(&env, true).await;
            set_mic_mute(&input_proxy, false).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to switch mic mute state",
    );
    move_executor_forward(
        &mut executor,
        async {
            get_and_check_mic_mute(&input_proxy, true).await;
        },
        "Failed to watch mic mute",
    );

    // SW unmuted, HW unmuted.
    move_executor_forward(
        &mut executor,
        async {
            switch_hardware_mic_mute(&env, false).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to switch hardware mic mute",
    );
    move_executor_forward(
        &mut executor,
        async {
            get_and_check_mic_mute(&input_proxy, false).await;
        },
        "Failed to watch mic mute",
    );
}

// Test that a set then watch for the camera is executed correctly.
#[fuchsia::test]
fn test_set_watch_camera_disable() {
    let (tx, mut rx) = mpsc::unbounded();
    let (mut executor, env) =
        create_env_and_executor_with_config(default_mic_cam_config(), None, tx);
    let input_proxy = env.input_service.clone();

    // SW muted, HW unmuted.
    move_executor_forward(
        &mut executor,
        async {
            set_camera_disable(&input_proxy, true).await;
            get_and_check_camera_disable(&input_proxy, true).await;
        },
        "Failed to switch camera state",
    );

    // SW unmuted, HW muted.
    move_executor_forward(
        &mut executor,
        async {
            switch_hardware_camera_disable(&env, true).await;
            set_camera_disable(&input_proxy, false).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to switch camera state",
    );
    move_executor_forward(
        &mut executor,
        async {
            get_and_check_camera_disable(&input_proxy, true).await;
        },
        "Failed to get camera mute state",
    );

    // SW unmuted, HW unmuted.
    move_executor_forward(
        &mut executor,
        async {
            switch_hardware_camera_disable(&env, false).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to switch camera mute state",
    );
    move_executor_forward(
        &mut executor,
        async {
            get_and_check_camera_disable(&input_proxy, false).await;
        },
        "Failed to get camera mute state",
    );
}

// Test to ensure mic input change events are received.
#[fuchsia::test]
fn test_mic_input() {
    let (tx, mut rx) = mpsc::unbounded();
    let (mut executor, env) = create_env_and_executor_with_config(default_mic_config(), None, tx);
    let input_proxy = env.input_service.clone();
    // Call watch() to ensure the watch() call below will return the results of the mic mute toggle.
    let Poll::Ready(..) = executor.run_until_stalled(&mut input_proxy.watch()) else {
        panic!("Initial watch failed to return");
    };

    move_executor_forward(
        &mut executor,
        async {
            switch_hardware_mic_mute(&env, true).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to switch hardware mic mute",
    );
    move_executor_forward(
        &mut executor,
        async {
            get_and_check_mic_mute(&input_proxy, true).await;
        },
        "Failed to watch mic mute",
    );
}

// Test to ensure camera input change events are received.
#[fuchsia::test]
fn test_camera_input() {
    let (tx, mut rx) = mpsc::unbounded();
    let (mut executor, env) =
        create_env_and_executor_with_config(default_mic_cam_config(), None, tx);
    let input_proxy = env.input_service.clone();

    move_executor_forward(
        &mut executor,
        async {
            get_and_check_camera_disable(&input_proxy, false).await;
            switch_hardware_camera_disable(&env, true).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to get and switch hardware camera mute state",
    );

    move_executor_forward(
        &mut executor,
        async {
            get_and_check_camera_disable(&input_proxy, true).await;
        },
        "Failed to get camera mute state",
    );
}

// Test to ensure camera sw state is not changed on camera3 api
// when the hw state is changed.
#[fuchsia::test]
fn test_camera3_hw_change() {
    let (tx, mut rx) = mpsc::unbounded();
    let (mut executor, env) =
        create_env_and_executor_with_config(default_mic_cam_config(), None, tx);

    move_executor_forward(
        &mut executor,
        async {
            switch_hardware_camera_disable(&env, true).await;
            wait_for_media_button_event(&mut rx).await;
        },
        "Failed to switch camera hw state",
    );

    let camera3_service_future = env.camera3_service.lock();
    pin_mut!(camera3_service_future);
    let camera3_proxy = match executor.run_until_stalled(&mut camera3_service_future) {
        Poll::Ready(service) => service,
        _ => panic!("Could not acquire camera3 service lock"),
    };

    #[allow(clippy::bool_assert_comparison)]
    {
        assert_eq!(camera3_proxy.camera_sw_muted(), false);
    }
}

// Test to ensure camera sw state is changed on camera3 api
// when the sw state is changed.
#[fuchsia::test(allow_stalls = false)]
async fn test_camera3_sw_change() {
    let env = TestInputEnvironmentBuilder::new()
        .set_input_device_config(default_mic_cam_config())
        .build()
        .await;
    let input_proxy = env.input_service.clone();

    set_camera_disable(&input_proxy, true).await;
    assert!(env.camera3_service.lock().await.camera_sw_muted());
}

// Test that when either hardware or software is muted, the service
// reports the microphone as muted.
#[fuchsia::test]
async fn test_mic_mute_combinations() {
    let (tx, mut rx) = mpsc::unbounded();
    let env = create_env_with_config(default_mic_config(), None, tx).await;
    let input_proxy = env.input_service.clone();

    // Throw away initial watch so we can sync with updates.
    let _ = input_proxy.watch().await;

    // Hardware muted, software unmuted.
    switch_hardware_mic_mute(&env, true).await;
    set_mic_mute(&input_proxy, false).await;
    wait_for_media_button_event(&mut rx).await;
    get_and_check_mic_mute(&input_proxy, true).await;

    // Hardware muted, software muted.
    set_mic_mute(&input_proxy, true).await;
    get_and_check_mic_mute(&input_proxy, true).await;

    // Hardware unmuted, software muted.
    switch_hardware_mic_mute(&env, false).await;
    wait_for_media_button_event(&mut rx).await;
    get_and_check_mic_mute(&input_proxy, true).await;

    // Hardware unmuted, software unmuted.
    switch_hardware_mic_mute(&env, false).await;
    set_mic_mute(&input_proxy, false).await;
    wait_for_media_button_event(&mut rx).await;
    get_and_check_mic_mute(&input_proxy, false).await;
}

// Test that when either hardware or software is disabled, the service
// reports the camera as disabled.
#[fuchsia::test]
async fn test_camera_disable_combinations() {
    let (tx, mut rx) = mpsc::unbounded();
    let env = create_env_with_config(default_mic_cam_config(), None, tx).await;
    let input_proxy = env.input_service.clone();

    // Throw away initial watch so we can sync with updates.
    let _ = input_proxy.watch().await;

    // Hardware disabled, software enabled.
    switch_hardware_camera_disable(&env, true).await;
    set_camera_disable(&input_proxy, false).await;
    wait_for_media_button_event(&mut rx).await;
    get_and_check_camera_disable(&input_proxy, true).await;

    // Hardware disabled, software disabled
    set_camera_disable(&input_proxy, true).await;
    get_and_check_camera_disable(&input_proxy, true).await;

    // Hardware enabled, software disabled.
    switch_hardware_camera_disable(&env, false).await;
    wait_for_media_button_event(&mut rx).await;
    get_and_check_camera_disable(&input_proxy, true).await;

    // Hardware enabled, software enabled.
    switch_hardware_camera_disable(&env, false).await;
    set_camera_disable(&input_proxy, false).await;
    wait_for_media_button_event(&mut rx).await;
    get_and_check_camera_disable(&input_proxy, false).await;
}

// Test that the input settings are restored correctly.
#[fuchsia::test(allow_stalls = false)]
async fn test_restore() {
    let mut stored_info = create_default_input_info().clone();
    stored_info.input_device_state = default_mic_cam_config_cam_sw_disabled().into();
    let env = TestInputEnvironmentBuilder::new()
        .set_starting_input_info_sources(stored_info)
        .set_input_device_config(default_mic_cam_config_cam_sw_disabled())
        .build()
        .await;

    get_and_check_state(&env.input_service, false, true).await;
    assert!(env.camera3_service.lock().await.camera_sw_muted());
}

// Test to ensure mic input change events are received.
#[fuchsia::test(allow_stalls = false)]
async fn test_bringup_without_input_registry() {
    let env = TestInputEnvironmentBuilder::new()
        .set_input_device_config(default_mic_cam_config())
        .build()
        .await;
    let input_proxy = env.input_service.clone();

    get_and_check_state(&input_proxy, DEFAULT_MIC_STATE, DEFAULT_CAMERA_STATE).await;
}

// Test that cloning works.
#[fuchsia::test]
#[allow(clippy::redundant_clone)]
fn test_input_info_copy() {
    let input_info = create_default_input_info();
    let copy_input_info = input_info.clone();
    assert_eq!(input_info, copy_input_info);
}

// Test that the values in the persistent store are restored at the start.
#[fuchsia::test(allow_stalls = false)]
async fn test_persisted_values_applied_at_start() {
    let mut test_input_info = InputInfoSources {
        input_device_state: InputState {
            input_categories: HashMap::<InputDeviceType, InputCategory>::new(),
        },
    };

    test_input_info.input_device_state.set_source_state(
        InputDeviceType::MICROPHONE,
        DEFAULT_MIC_NAME.to_string(),
        DeviceStateSource::SOFTWARE,
        DeviceState::from_bits(MUTED_BITS).unwrap(),
    );
    test_input_info.input_device_state.set_source_state(
        InputDeviceType::MICROPHONE,
        DEFAULT_MIC_NAME.to_string(),
        DeviceStateSource::HARDWARE,
        DeviceState::from_bits(AVAILABLE_BITS).unwrap(),
    );
    test_input_info.input_device_state.set_source_state(
        InputDeviceType::CAMERA,
        DEFAULT_CAMERA_NAME.to_string(),
        DeviceStateSource::SOFTWARE,
        DeviceState::from_bits(AVAILABLE_BITS).unwrap(),
    );
    test_input_info.input_device_state.set_source_state(
        InputDeviceType::CAMERA,
        DEFAULT_CAMERA_NAME.to_string(),
        DeviceStateSource::HARDWARE,
        DeviceState::from_bits(MUTED_BITS).unwrap(),
    );
    let env = TestInputEnvironmentBuilder::new()
        .set_starting_input_info_sources(test_input_info)
        .set_input_device_config(default_mic_cam_config())
        .build()
        .await;

    get_and_check_state(&env.input_service, true, true).await;
}
