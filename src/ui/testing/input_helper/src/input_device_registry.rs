// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input_device::InputDevice;
use crate::new_fake_device_info;
use anyhow::{Context as _, Error};
use async_utils::event::Event as AsyncEvent;
use fidl::endpoints;
use fidl_fuchsia_input::Key;
use fidl_fuchsia_input_injection::InputDeviceRegistryProxy;
use fidl_fuchsia_input_report::{
    Axis, ConsumerControlButton, ConsumerControlDescriptor, ConsumerControlInputDescriptor,
    ContactInputDescriptor, DeviceDescriptor, InputDeviceMarker, KeyboardDescriptor,
    KeyboardInputDescriptor, MouseDescriptor, MouseInputDescriptor, Range, TOUCH_MAX_CONTACTS,
    TouchDescriptor, TouchInputDescriptor, TouchType, Unit, UnitType,
};
use fidl_fuchsia_ui_test_input::MouseButton;

/// Implements the client side of the `fuchsia.input.injection.InputDeviceRegistry` protocol.
pub(crate) struct InputDeviceRegistry {
    proxy: InputDeviceRegistryProxy,
    got_input_reports_reader: AsyncEvent,
}

impl InputDeviceRegistry {
    pub fn new(proxy: InputDeviceRegistryProxy, got_input_reports_reader: AsyncEvent) -> Self {
        Self { proxy, got_input_reports_reader }
    }

    /// Registers a touchscreen device, with in injection coordinate space that spans [-1000, 1000]
    /// on both axes.
    /// # Returns
    /// A `input_device::InputDevice`, which can be used to send events to the
    /// `fuchsia.input.report.InputDevice` that has been registered with the
    /// `fuchsia.input.injection.InputDeviceRegistry` service.
    pub async fn add_touchscreen_device(
        &mut self,
        min_x: i64,
        max_x: i64,
        min_y: i64,
        max_y: i64,
    ) -> Result<InputDevice, Error> {
        self.add_device(DeviceDescriptor {
            touch: Some(TouchDescriptor {
                input: Some(TouchInputDescriptor {
                    contacts: Some(
                        std::iter::repeat(ContactInputDescriptor {
                            position_x: Some(Axis {
                                range: Range { min: min_x, max: max_x },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            position_y: Some(Axis {
                                range: Range { min: min_y, max: max_y },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            contact_width: Some(Axis {
                                range: Range { min: min_x, max: max_x },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            contact_height: Some(Axis {
                                range: Range { min: min_y, max: max_y },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            ..Default::default()
                        })
                        .take(
                            usize::try_from(TOUCH_MAX_CONTACTS)
                                .context("usize is impossibly small")?,
                        )
                        .collect(),
                    ),
                    max_contacts: Some(TOUCH_MAX_CONTACTS),
                    touch_type: Some(TouchType::Touchscreen),
                    buttons: Some(vec![]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    }

    /// Registers a media buttons device.
    /// # Returns
    /// A `input_device::InputDevice`, which can be used to send events to the
    /// `fuchsia.input.report.InputDevice` that has been registered with the
    /// `fuchsia.input.injection.InputDeviceRegistry` service.
    pub async fn add_media_buttons_device(&mut self) -> Result<InputDevice, Error> {
        self.add_device(DeviceDescriptor {
            consumer_control: Some(ConsumerControlDescriptor {
                input: Some(ConsumerControlInputDescriptor {
                    buttons: Some(vec![
                        ConsumerControlButton::VolumeUp,
                        ConsumerControlButton::VolumeDown,
                        ConsumerControlButton::Pause,
                        ConsumerControlButton::FactoryReset,
                        ConsumerControlButton::MicMute,
                        ConsumerControlButton::Reboot,
                        ConsumerControlButton::CameraDisable,
                        ConsumerControlButton::Power,
                        ConsumerControlButton::Function,
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    }

    /// Registers a keyboard device.
    /// # Returns
    /// An `input_device::InputDevice`, which can be used to send events to the
    /// `fuchsia.input.report.InputDevice` that has been registered with the
    /// `fuchsia.input.injection.InputDeviceRegistry` service.
    pub async fn add_keyboard_device(&mut self) -> Result<InputDevice, Error> {
        // Generate a `Vec` of all known keys.
        // * Because there is no direct way to iterate over enum values, we iterate
        //   over the values corresponding to `Key::A` and `Key::MediaVolumeDecrement`.
        // * Some values in the range have no corresponding enum value. For example,
        //   the value 0x00070065 sits between `NonUsBackslash` (0x00070064), and
        //   `KeypadEquals` (0x00070067). Such primitives are removed by `filter_map()`.
        //
        // TODO(https://fxbug.dev/42059900): Extend to include all values of the Key enum.
        let all_keys: Vec<Key> = (Key::A.into_primitive()
            ..=Key::MediaVolumeDecrement.into_primitive())
            .filter_map(Key::from_primitive)
            .collect();
        self.add_device(DeviceDescriptor {
            // Required for DeviceDescriptor.
            device_information: Some(new_fake_device_info()),
            keyboard: Some(KeyboardDescriptor {
                input: Some(KeyboardInputDescriptor {
                    keys3: Some(all_keys),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    }

    pub async fn add_mouse_device(&mut self) -> Result<InputDevice, Error> {
        self.add_device(DeviceDescriptor {
            // Required for DeviceDescriptor.
            device_information: Some(new_fake_device_info()),
            mouse: Some(MouseDescriptor {
                input: Some(MouseInputDescriptor {
                    movement_x: Some(Axis {
                        range: Range { min: -1000, max: 1000 },
                        unit: Unit { type_: UnitType::Other, exponent: 0 },
                    }),
                    movement_y: Some(Axis {
                        range: Range { min: -1000, max: 1000 },
                        unit: Unit { type_: UnitType::Other, exponent: 0 },
                    }),
                    // `scroll_v` and `scroll_h` are range of tick number on
                    // driver's report. [-100, 100] should be enough for
                    // testing.
                    scroll_v: Some(Axis {
                        range: Range { min: -100, max: 100 },
                        unit: Unit { type_: UnitType::Other, exponent: 0 },
                    }),
                    scroll_h: Some(Axis {
                        range: Range { min: -100, max: 100 },
                        unit: Unit { type_: UnitType::Other, exponent: 0 },
                    }),
                    // Match to the values of fuchsia.ui.test.input.MouseButton.
                    buttons: Some(
                        (MouseButton::First.into_primitive()..=MouseButton::Third.into_primitive())
                            .map(|b| {
                                b.try_into().expect("failed to convert mouse button to primitive")
                            })
                            .collect(),
                    ),
                    position_x: None,
                    position_y: None,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
    }

    /// Adds a device to the `InputDeviceRegistry` FIDL server connected to this
    /// `InputDeviceRegistry` struct.
    ///
    /// # Returns
    /// A `input_device::InputDevice`, which can be used to send events to the
    /// `fuchsia.input.report.InputDevice` that has been registered with the
    /// `fuchsia.input.injection.InputDeviceRegistry` service.
    async fn add_device(&self, descriptor: DeviceDescriptor) -> Result<InputDevice, Error> {
        let (client_end, request_stream) = endpoints::create_request_stream::<InputDeviceMarker>();
        let mut device: InputDevice =
            InputDevice::new(request_stream, descriptor, self.got_input_reports_reader.clone());

        let res = self.proxy.register_and_get_device_info(client_end).await?;
        let device_id = res.device_id.expect("missing device_id");
        device.device_id = device_id;

        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_input_injection::{
        InputDeviceRegistryMarker, InputDeviceRegistryRegisterAndGetDeviceInfoResponse,
        InputDeviceRegistryRequest,
    };
    use fidl_fuchsia_input_report::InputReportsReaderV2Marker;
    use futures::StreamExt;
    use test_case::test_case;

    const MAX_UNACKNOWLEDGED_REPORTS_LIMIT: u16 = 120;

    enum TestDeviceType {
        TouchScreen,
        MediaButtons,
        Keyboard,
        Mouse,
    }

    async fn add_device_for_test(
        registry: &mut InputDeviceRegistry,
        ty: TestDeviceType,
    ) -> Result<InputDevice, Error> {
        match ty {
            TestDeviceType::TouchScreen => registry.add_touchscreen_device(1, 1000, 1, 1000).await,
            TestDeviceType::MediaButtons => registry.add_media_buttons_device().await,
            TestDeviceType::Keyboard => registry.add_keyboard_device().await,
            TestDeviceType::Mouse => registry.add_mouse_device().await,
        }
    }

    #[test_case(TestDeviceType::TouchScreen =>
                matches Ok(DeviceDescriptor {
                    touch: Some(TouchDescriptor {
                        input: Some(TouchInputDescriptor { .. }),
                        ..
                    }),
                    .. });
                "touchscreen_device")]
    #[test_case(TestDeviceType::MediaButtons =>
                matches Ok(DeviceDescriptor {
                    consumer_control: Some(ConsumerControlDescriptor {
                        input: Some(ConsumerControlInputDescriptor { .. }),
                        ..
                    }),
                    .. });
                "media_buttons_device")]
    #[test_case(TestDeviceType::Keyboard =>
                matches Ok(DeviceDescriptor {
                    keyboard: Some(KeyboardDescriptor { .. }),
                    ..
                });
                "keyboard_device")]
    #[test_case(TestDeviceType::Mouse =>
                matches Ok(DeviceDescriptor {
                    mouse: Some(MouseDescriptor { .. }),
                    ..
                });
                "mouse_device")]
    #[fuchsia::test]
    async fn add_device_registers_correct_device_type(
        device_type: TestDeviceType,
    ) -> Result<DeviceDescriptor, Error> {
        let (registry_proxy, mut registry_request_stream) =
            endpoints::create_proxy_and_stream::<InputDeviceRegistryMarker>();
        let mut input_device_registry = InputDeviceRegistry {
            proxy: registry_proxy,
            got_input_reports_reader: AsyncEvent::new(),
        };

        let add_device_fut = add_device_for_test(&mut input_device_registry, device_type);

        let input_device_proxy_fut = async {
            // `input_device_registry` should send a `Register` messgage to `registry_request_stream`.
            // Use `registry_request_stream` to grab the `ClientEnd` of the device added above,
            // and convert the `ClientEnd` into an `InputDeviceProxy`.
            //
            // Here only handle InputDeviceRegistryRequest once.
            let input_device_proxy = match registry_request_stream
                .next()
                .await
                .expect("stream read should yield Some")
                .expect("fidl read")
            {
                InputDeviceRegistryRequest::Register { .. } => {
                    unreachable!("InputDeviceRegistryRequest::Register should not be called");
                }
                InputDeviceRegistryRequest::RegisterAndGetDeviceInfo {
                    device, responder, ..
                } => {
                    responder
                        .send(InputDeviceRegistryRegisterAndGetDeviceInfoResponse {
                            device_id: Some(1),
                            ..Default::default()
                        })
                        .expect("RegisterAndGetDeviceInfo send response failed");

                    device
                }
            }
            .into_proxy();

            input_device_proxy
        };

        let (add_device_res, input_device_proxy) =
            futures::join!(add_device_fut, input_device_proxy_fut);

        let input_device = add_device_res.expect("add_device failed");
        assert_ne!(input_device.device_id, 0);

        let input_device_get_descriptor = input_device_proxy.get_descriptor().await;

        let input_device_server_fut = input_device.flush();

        // Avoid unrelated `panic()`: `InputDevice` requires clients to get an input
        // reports reader, to help debug integration test failures where no component
        // read events from the fake device.
        let (_input_reports_reader_proxy, input_reports_reader_server_end) =
            endpoints::create_proxy::<InputReportsReaderV2Marker>();
        let _ = input_device_proxy
            .get_input_reports_reader_v2(
                input_reports_reader_server_end,
                MAX_UNACKNOWLEDGED_REPORTS_LIMIT,
            )
            .await;

        std::mem::drop(input_device_proxy); // Terminate stream served by `input_device_server_fut`.
        input_device_server_fut.await;

        input_device_get_descriptor.map_err(anyhow::Error::from)
    }
}
