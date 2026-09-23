// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::modern_backend::input_device::InputDevice;
use crate::synthesizer;
use anyhow::{Context as _, Error};
use fidl::endpoints;
use fidl_fuchsia_input::Key;
use fidl_fuchsia_input_injection::InputDeviceRegistryProxy;
use fidl_fuchsia_input_report::{
    Axis, ConsumerControlButton, ConsumerControlDescriptor, ConsumerControlInputDescriptor,
    ContactInputDescriptor, DeviceDescriptor, DeviceInformation, InputDeviceMarker,
    KeyboardDescriptor, KeyboardInputDescriptor, MouseDescriptor, MouseInputDescriptor, Range,
    TouchDescriptor, TouchInputDescriptor, TouchType, Unit, UnitType, TOUCH_MAX_CONTACTS,
};

// Use this to place required DeviceInfo into DeviceDescriptor.
fn new_fake_device_info() -> DeviceInformation {
    DeviceInformation {
        product_id: Some(42),
        vendor_id: Some(43),
        version: Some(u32::MAX),
        polling_rate: Some(1000),
        ..Default::default()
    }
}

/// Implements the `synthesizer::InputDeviceRegistry` trait, and the client side
/// of the `fuchsia.input.injection.InputDeviceRegistry` protocol.
pub struct InputDeviceRegistry {
    proxy: InputDeviceRegistryProxy,
}

impl synthesizer::InputDeviceRegistry for self::InputDeviceRegistry {
    fn add_touchscreen_device(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Box<dyn synthesizer::InputDevice>, Error> {
        self.add_device(DeviceDescriptor {
            // Required for DeviceDescriptor.
            device_information: Some(new_fake_device_info()),
            touch: Some(TouchDescriptor {
                input: Some(TouchInputDescriptor {
                    contacts: Some(
                        std::iter::repeat(ContactInputDescriptor {
                            position_x: Some(Axis {
                                range: Range { min: 0, max: i64::from(width) },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            position_y: Some(Axis {
                                range: Range { min: 0, max: i64::from(height) },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            contact_width: Some(Axis {
                                range: Range { min: 0, max: i64::from(width) },
                                unit: Unit { type_: UnitType::Other, exponent: 0 },
                            }),
                            contact_height: Some(Axis {
                                range: Range { min: 0, max: i64::from(height) },
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
    }

    fn add_keyboard_device(&mut self) -> Result<Box<dyn synthesizer::InputDevice>, Error> {
        // Generate a `Vec` of all known keys.
        // * Because there is no direct way to iterate over enum values, we iterate
        //   over the primitives corresponding to `Key::A` and `Key::MediaVolumeDecrement`.
        // * Some primitive values in the range have no corresponding enum value. For
        //   example, the value 0x00070065 sits between `NonUsBackslash` (0x00070064), and
        //   `KeypadEquals` (0x00070067). Such primitives are removed by `filter_map()`.
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
    }

    fn add_media_buttons_device(&mut self) -> Result<Box<dyn synthesizer::InputDevice>, Error> {
        self.add_device(DeviceDescriptor {
            // Required for DeviceDescriptor.
            device_information: Some(new_fake_device_info()),
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
                    ]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    fn add_mouse_device(
        &mut self,
        width: u32,
        height: u32,
    ) -> Result<Box<dyn synthesizer::InputDevice>, Error> {
        self.add_device(DeviceDescriptor {
            // Required for DeviceDescriptor.
            device_information: Some(new_fake_device_info()),
            mouse: Some(MouseDescriptor {
                input: Some(MouseInputDescriptor {
                    movement_x: Some(Axis {
                        range: Range { min: 0, max: i64::from(width) },
                        unit: Unit { type_: UnitType::Other, exponent: 0 },
                    }),
                    movement_y: Some(Axis {
                        range: Range { min: 0, max: i64::from(height) },
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
                    buttons: Some(vec![0, 1, 2]),
                    position_x: None,
                    position_y: None,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

impl InputDeviceRegistry {
    pub fn new(proxy: InputDeviceRegistryProxy) -> Self {
        Self { proxy }
    }

    /// Adds a device to the `InputDeviceRegistry` FIDL server connected to this
    /// `InputDeviceRegistry` struct.
    ///
    /// # Returns
    /// A `synthesizer::InputDevice`, which can be used to send events to the
    /// `fuchsia.input.report.InputDevice` that has been registered with the
    /// `fuchsia.input.injection.InputDeviceRegistry` service.
    fn add_device(
        &self,
        descriptor: DeviceDescriptor,
    ) -> Result<Box<dyn synthesizer::InputDevice>, Error> {
        let (client_end, request_stream) = endpoints::create_request_stream::<InputDeviceMarker>();
        self.proxy.register(client_end)?;
        Ok(Box::new(InputDevice::new(request_stream, descriptor)))
    }
}

#[cfg(test)]
mod tests {
    use super::synthesizer::InputDeviceRegistry as _;
    use super::*;
    use anyhow::format_err;
    use fidl_fuchsia_input_injection::{InputDeviceRegistryMarker, InputDeviceRegistryRequest};
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use futures::{pin_mut, StreamExt};
    use test_case::test_case;

    #[test_case(&super::InputDeviceRegistry::add_keyboard_device; "keyboard_device")]
    #[test_case(&super::InputDeviceRegistry::add_media_buttons_device; "media_button_device")]
    #[test_case(&|registry| InputDeviceRegistry::add_touchscreen_device(registry, 640, 480);
                "touchscreen_device")]
    #[test_case(&|registry| InputDeviceRegistry::add_mouse_device(registry, 640, 480);
                "mouse_device")]
    fn add_device_invokes_fidl_register_method_exactly_once(
        add_device_method: &dyn Fn(
            &mut super::InputDeviceRegistry,
        ) -> Result<Box<dyn synthesizer::InputDevice>, Error>,
    ) -> Result<(), Error> {
        let mut executor = fasync::TestExecutor::new();
        let (proxy, request_stream) =
            endpoints::create_proxy_and_stream::<InputDeviceRegistryMarker>();
        add_device_method(&mut InputDeviceRegistry { proxy }).context("adding device")?;

        let requests = match executor.run_until_stalled(&mut request_stream.collect::<Vec<_>>()) {
            Poll::Ready(reqs) => reqs,
            Poll::Pending => return Err(format_err!("request_stream did not terminate")),
        };
        assert_matches::assert_matches!(
            requests.as_slice(),
            [Ok(InputDeviceRegistryRequest::Register { .. })]
        );

        Ok(())
    }

    #[test_case(&super::InputDeviceRegistry::add_keyboard_device =>
                matches Ok(DeviceDescriptor { keyboard: Some(_), .. });
                "keyboard_device")]
    #[test_case(&super::InputDeviceRegistry::add_media_buttons_device =>
                matches Ok(DeviceDescriptor { consumer_control: Some(_), .. });
                "media_button_device")]
    #[test_case(&|registry| InputDeviceRegistry::add_touchscreen_device(registry, 640, 480) =>
                matches Ok(DeviceDescriptor {
                    touch: Some(TouchDescriptor {
                        input: Some(TouchInputDescriptor { .. }),
                        ..
                    }),
                    .. });
                "touchscreen_device")]
    #[test_case(&|registry| InputDeviceRegistry::add_mouse_device(registry, 640, 480) =>
                matches Ok(DeviceDescriptor {
                    mouse: Some(MouseDescriptor {
                        input: Some(MouseInputDescriptor { .. }),
                        ..
                    }),
                    .. });
                "mouse_device")]
    fn add_device_registers_correct_device_type(
        add_device_method: &dyn Fn(
            &mut super::InputDeviceRegistry,
        ) -> Result<Box<dyn synthesizer::InputDevice>, Error>,
    ) -> Result<DeviceDescriptor, Error> {
        let mut executor = fasync::TestExecutor::new();
        // Create an `InputDeviceRegistry`, and add a keyboard to it.
        let (registry_proxy, mut registry_request_stream) =
            endpoints::create_proxy_and_stream::<InputDeviceRegistryMarker>();
        let mut input_device_registry = InputDeviceRegistry { proxy: registry_proxy };
        let input_device =
            add_device_method(&mut input_device_registry).context("adding keyboard")?;

        let test_fut = async {
            // `input_device_registry` should send a `Register` messgage to `registry_request_stream`.
            // Use `registry_request_stream` to grab the `ClientEnd` of the keyboard added above,
            // and convert the `ClientEnd` into an `InputDeviceProxy`.
            let input_device_proxy = match registry_request_stream
                .next()
                .await
                .context("stream read should yield Some")?
                .context("fidl read")?
            {
                InputDeviceRegistryRequest::Register { device, .. } => device,
                InputDeviceRegistryRequest::RegisterAndGetDeviceInfo { device, .. } => device,
            }
            .into_proxy();

            // Send a `GetDescriptor` request to `input_device`, and verify that the device
            // is as keyboard.
            let input_device_get_descriptor_fut = input_device_proxy.get_descriptor();
            let input_device_server_fut = input_device.flush();
            std::mem::drop(input_device_proxy); // Terminate stream served by `input_device_server_fut`.

            let (_server_result, get_descriptor_result) =
                futures::future::join(input_device_server_fut, input_device_get_descriptor_fut)
                    .await;
            get_descriptor_result.map_err(anyhow::Error::from)
        };
        pin_mut!(test_fut);

        match executor.run_until_stalled(&mut test_fut) {
            Poll::Ready(r) => r,
            Poll::Pending => Err(format_err!("test did not complete")),
        }
    }
}
