// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device_configuration::{
    InputConfiguration, InputDeviceConfiguration, SourceState,
};
use crate::input_test_environment::TestInputEnvironmentBuilder;
use crate::types::{DeviceStateSource, InputDeviceType};
use fidl_fuchsia_settings::{
    DeviceState as FidlDeviceState, DeviceType, Error as FidlSettingsError,
    InputState as FidlInputState, MAX_INPUT_DEVICES, ToggleStateFlags,
};

const MUTED_DISABLED_BITS: u64 = 12;
const AVAILABLE_BITS: u64 = 1;

fn create_limit_config(count: usize) -> InputConfiguration {
    let mut devices = Vec::new();
    for i in 0..count {
        devices.push(InputDeviceConfiguration {
            device_name: format!("mic{i}"),
            device_type: InputDeviceType::MICROPHONE,
            source_states: vec![
                SourceState { source: DeviceStateSource::HARDWARE, state: AVAILABLE_BITS },
                SourceState { source: DeviceStateSource::SOFTWARE, state: AVAILABLE_BITS },
            ],
            mutable_toggle_state: MUTED_DISABLED_BITS,
        });
    }
    InputConfiguration { devices }
}

#[fuchsia::test]
async fn test_input_device_limit_enforcement() {
    let env = TestInputEnvironmentBuilder::new()
        .set_input_device_config(create_limit_config(MAX_INPUT_DEVICES as usize))
        .build()
        .await;
    let proxy = env.input_service;

    let overflow_state = vec![FidlInputState {
        name: Some(format!("mic{}", MAX_INPUT_DEVICES)),
        device_type: Some(DeviceType::Microphone),
        state: Some(FidlDeviceState {
            toggle_flags: ToggleStateFlags::from_bits(AVAILABLE_BITS),
            ..Default::default()
        }),
        ..Default::default()
    }];

    let res = proxy.set(&overflow_state).await.expect("set completed");
    assert_eq!(res, Err(FidlSettingsError::Failed));
}

#[fuchsia::test]
async fn test_input_device_add_under_limit() {
    let env = TestInputEnvironmentBuilder::new()
        .set_input_device_config(create_limit_config(5))
        .build()
        .await;
    let proxy = env.input_service;

    let new_state = vec![FidlInputState {
        name: Some("mic5".to_string()),
        device_type: Some(DeviceType::Microphone),
        state: Some(FidlDeviceState {
            toggle_flags: ToggleStateFlags::from_bits(AVAILABLE_BITS),
            ..Default::default()
        }),
        ..Default::default()
    }];

    let res = proxy.set(&new_state).await.expect("set completed");
    assert_eq!(res, Ok(()));
}
