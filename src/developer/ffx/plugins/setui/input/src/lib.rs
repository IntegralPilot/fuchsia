// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Used to interact with the input interface.
// TODO(https://fxbug.dev/42144965): Support multiple devices.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_settings::{DeviceType, InputProxy, InputState};
use ffx_setui_input_args::Input;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct InputTool {
    #[command]
    cmd: Input,
    #[with(moniker("/core/setui_service"))]
    input_proxy: InputProxy,
}

fho::embedded_plugin!(InputTool);

#[async_trait(?Send)]
impl FfxMain for InputTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.input_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn run_command<W: std::io::Write>(
    input_proxy: InputProxy,
    input: Input,
    writer: &mut W,
) -> Result<()> {
    handle_mixed_result("Input", command(input_proxy, InputState::from(input)).await, writer).await
}

async fn command(proxy: InputProxy, mut input_state: InputState) -> WatchOrSetResult {
    if input_state == InputState::default() {
        Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
    } else {
        if input_state.device_type.is_none() {
            return Err(format_err!("Device type required"));
        }
        if input_state.state.is_none() {
            return Err(format_err!("Device state required"));
        }
        if input_state.name.is_none() {
            // Default device names.
            input_state.name = match input_state.device_type.unwrap() {
                DeviceType::Camera => Some("camera".to_string()),
                DeviceType::Microphone => Some("microphone".to_string()),
            };
        }
        let input_states = &[input_state];
        Ok(Either::Set(if let Err(err) = proxy.set(input_states).await? {
            format!("{:?}", err)
        } else {
            format!("Successfully set input states to {:#?}\n", input_states)
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::{
        DeviceState, DeviceStateSource, DeviceType, InputDevice, InputRequest, InputSettings,
        SourceState, ToggleStateFlags,
    };
    use target_holders::fake_proxy;
    use test_case::test_case;

    /// Creates a one-item list of input devices with the given properties.
    fn create_input_devices(
        device_type: DeviceType,
        device_name: &str,
        device_state: u64,
    ) -> Vec<InputDevice> {
        let mut devices = Vec::new();
        let mut source_states = Vec::new();
        source_states.push(SourceState {
            source: Some(DeviceStateSource::Hardware),
            state: Some(DeviceState {
                toggle_flags: ToggleStateFlags::from_bits(1),
                ..Default::default()
            }),
            ..Default::default()
        });
        source_states.push(SourceState {
            source: Some(DeviceStateSource::Software),
            state: Some(u64_to_state(device_state)),
            ..Default::default()
        });
        let device = InputDevice {
            device_name: Some(device_name.to_string()),
            device_type: Some(device_type),
            source_states: Some(source_states),
            mutable_toggle_state: ToggleStateFlags::from_bits(12),
            state: Some(u64_to_state(device_state)),
            ..Default::default()
        };
        devices.push(device);
        devices
    }

    /// Transforms an u64 into an fuchsia_fidl_settings::DeviceState.
    fn u64_to_state(num: u64) -> DeviceState {
        DeviceState { toggle_flags: ToggleStateFlags::from_bits(num), ..Default::default() }
    }

    #[fuchsia::test]
    async fn test_run_command() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            InputRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            InputRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let input = Input {
            device_name: None,
            device_type: Some(DeviceType::Camera),
            device_state: Some(DeviceState {
                toggle_flags: Some(ToggleStateFlags::AVAILABLE),
                ..Default::default()
            }),
        };
        let response = run_command(proxy, input, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        Input {
            device_name: Some(String::from("camera")),
            device_type: Some(DeviceType::Microphone),
            device_state: Some(DeviceState {
                toggle_flags: Some(ToggleStateFlags::MUTED),
                ..Default::default()
            }),
        };
        "Test input set() output with non-empty input."
    )]
    #[test_case(
        Input {
            device_name: None,
            device_type: Some(DeviceType::Microphone),
            device_state: Some(DeviceState {
                toggle_flags: Some(ToggleStateFlags::MUTED),
                ..Default::default()
            }),
        };
        "Test input set() output with a different input."
    )]
    #[fuchsia::test]
    async fn validate_input_set_output(mut expected_input: Input) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            InputRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            InputRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output = utils::assert_set!(command(proxy, InputState::from(expected_input.clone())));
        // Make sure the `name` is auto-filled.
        if expected_input.device_name.is_none() {
            // Default device names.
            expected_input.device_name = match expected_input.device_type.unwrap() {
                DeviceType::Camera => Some("camera".to_string()),
                DeviceType::Microphone => Some("microphone".to_string()),
            };
        }
        assert_eq!(
            output,
            format!(
                "Successfully set input states to {:#?}\n",
                vec!(InputState::from(expected_input))
            )
        );
        Ok(())
    }

    #[fuchsia::test]
    async fn validate_input_watch_output() -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            InputRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            InputRequest::Watch { responder } => {
                let _ = responder.send(&InputSettings {
                    devices: Some(create_input_devices(DeviceType::Camera, "camera", 1)),
                    ..Default::default()
                });
            }
        });

        let output = utils::assert_watch!(command(
            proxy,
            InputState::from(Input { device_name: None, device_type: None, device_state: None })
        ));
        // Just check that the output contains some key strings that confirms the watch returned.
        // The string representation may not necessarily be in the same order.
        assert!(output.contains("Software"));
        assert!(output.contains("source_states: Some"));
        assert!(output.contains("toggle_flags: Some"));
        assert!(output.contains("camera"));
        assert!(output.contains("AVAILABLE"));
        Ok(())
    }
}
