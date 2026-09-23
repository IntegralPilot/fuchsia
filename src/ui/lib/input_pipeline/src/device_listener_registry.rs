// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Dispatcher;
use crate::dispatcher::Transport;
use fidl::endpoints::RequestStream as _;
use fidl_fuchsia_ui_input as fidl_ui_input_legacy;
use fidl_next::{Request, Responder, ServerEnd};
use fidl_next_fuchsia_ui_input as fidl_ui_input;
use log::{error, info};
use sorted_vec_map::SortedVecMap;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct DeviceListenerRegistry {
    listeners: Rc<RefCell<Vec<fidl_next::Client<fidl_ui_input::DeviceListener, Transport>>>>,
    active_devices:
        Rc<RefCell<SortedVecMap<u32, fidl_next_fuchsia_input_report::DeviceDescriptor>>>,
    scope: Rc<fuchsia_async::Scope>,
}

impl Default for DeviceListenerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceListenerRegistry {
    pub fn new() -> Self {
        Self {
            listeners: Rc::new(RefCell::new(Vec::new())),
            active_devices: Rc::new(RefCell::new(SortedVecMap::new())),
            scope: Rc::new(fuchsia_async::Scope::new()),
        }
    }

    pub fn add_listener(
        &self,
        listener: fidl_next::Client<fidl_ui_input::DeviceListener, Transport>,
    ) -> Vec<fidl_ui_input::DeviceEvent> {
        self.listeners.borrow_mut().push(listener);

        let active_devices = self.active_devices.borrow();
        active_devices
            .iter()
            .map(|(&device_id, descriptor)| fidl_ui_input::DeviceEvent {
                action: Some(fidl_ui_input::Action::Added),
                device_id: Some(device_id),
                descriptor: Some(shim_device_descriptor(descriptor)),
                ..Default::default()
            })
            .collect()
    }

    pub fn notify_device_changed(
        &self,
        action: fidl_ui_input::Action,
        device_id: u32,
        descriptor: fidl_next_fuchsia_input_report::DeviceDescriptor,
    ) {
        match action {
            fidl_ui_input::Action::Added | fidl_ui_input::Action::Updated => {
                self.active_devices.borrow_mut().insert(device_id, descriptor.clone());
            }
            fidl_ui_input::Action::Removed => {
                self.active_devices.borrow_mut().remove(&device_id);
            }
            _ => {}
        }

        let event = fidl_ui_input::DeviceEvent {
            action: Some(action),
            device_id: Some(device_id),
            descriptor: Some(shim_device_descriptor(&descriptor)),
            ..Default::default()
        };

        self.listeners.borrow_mut().retain(|listener| {
            match listener.on_device_changed(&event).send_immediately() {
                Ok(()) => true,
                Err(e) => {
                    info!("Removing device listener due to error: {:?}", e);
                    false
                }
            }
        });
    }
}

pub fn shim_mouse_descriptor(
    mouse: &fidl_next_fuchsia_input_report::MouseDescriptor,
) -> fidl_ui_input::MouseDescriptor {
    fidl_ui_input::MouseDescriptor {
        input: mouse.input.as_ref().map(|input| fidl_ui_input::MouseInputDescriptor {
            movement_x: input.movement_x.clone(),
            movement_y: input.movement_y.clone(),
            scroll_v: input.scroll_v.clone(),
            scroll_h: input.scroll_h.clone(),
            buttons: input.buttons.clone(),
            position_x: input.position_x.clone(),
            position_y: input.position_y.clone(),
        }),
    }
}

pub fn shim_sensor_descriptor(
    sensor: &fidl_next_fuchsia_input_report::SensorDescriptor,
) -> fidl_ui_input::SensorDescriptor {
    fidl_ui_input::SensorDescriptor {
        input: sensor.input.as_ref().and_then(|inputs| inputs.first()).map(|input| {
            fidl_ui_input::SensorInputDescriptor {
                values: input.values.as_ref().map(|values| {
                    values
                        .iter()
                        .map(|v| fidl_ui_input::SensorAxis {
                            axis: v.axis.clone(),
                            type_: fidl_ui_input::SensorType::from(u32::from(v.type_)),
                        })
                        .collect()
                }),
            }
        }),
    }
}

pub fn shim_touch_descriptor(
    touch: &fidl_next_fuchsia_input_report::TouchDescriptor,
) -> fidl_ui_input::TouchDescriptor {
    fidl_ui_input::TouchDescriptor {
        input: touch.input.as_ref().map(|input| fidl_ui_input::TouchInputDescriptor {
            contacts: input.contacts.as_ref().map(|contacts| {
                contacts
                    .iter()
                    .map(|c| fidl_ui_input::ContactInputDescriptor {
                        position_x: c.position_x.clone(),
                        position_y: c.position_y.clone(),
                        pressure: c.pressure.clone(),
                        contact_width: c.contact_width.clone(),
                        contact_height: c.contact_height.clone(),
                    })
                    .collect()
            }),
            max_contacts: input.max_contacts,
            touch_type: input.touch_type.map(|tt| fidl_ui_input::TouchType::from(u32::from(tt))),
            buttons: input.buttons.as_ref().map(|buttons| {
                buttons
                    .iter()
                    .map(|b| fidl_ui_input::TouchButton::from(u32::from(u8::from(*b))))
                    .collect()
            }),
        }),
        feature: touch.feature.as_ref().map(|feature| fidl_ui_input::TouchFeatureDescriptor {
            supports_input_mode: feature.supports_input_mode,
            supports_selective_reporting: feature.supports_selective_reporting,
        }),
    }
}

pub fn shim_keyboard_descriptor(
    keyboard: &fidl_next_fuchsia_input_report::KeyboardDescriptor,
) -> fidl_ui_input::KeyboardDescriptor {
    fidl_ui_input::KeyboardDescriptor {
        input: keyboard.input.as_ref().map(|input| fidl_ui_input::KeyboardInputDescriptor {
            keys: input.keys3.clone(),
            ..Default::default()
        }),
        output: keyboard.output.as_ref().map(|output| fidl_ui_input::KeyboardOutputDescriptor {
            leds: output.leds.as_ref().map(|leds| {
                leds.iter().map(|l| fidl_ui_input::LedType::from(u32::from(*l))).collect()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn shim_consumer_control_descriptor(
    consumer_control: &fidl_next_fuchsia_input_report::ConsumerControlDescriptor,
) -> fidl_ui_input::ConsumerControlDescriptor {
    fidl_ui_input::ConsumerControlDescriptor {
        input: consumer_control.input.as_ref().map(|input| {
            fidl_ui_input::ConsumerControlInputDescriptor { buttons: input.buttons.clone() }
        }),
    }
}

pub fn shim_device_information(
    info: &fidl_next_fuchsia_input_report::DeviceInformation,
) -> fidl_ui_input::DeviceInformation {
    fidl_ui_input::DeviceInformation {
        vendor_id: info.vendor_id,
        product_id: info.product_id,
        version: info.version,
        polling_rate: info.polling_rate,
        manufacturer_name: info.manufacturer_name.clone(),
        product_name: info.product_name.clone(),
        serial_number: info.serial_number.clone(),
    }
}

pub fn shim_device_descriptor(
    descriptor: &fidl_next_fuchsia_input_report::DeviceDescriptor,
) -> fidl_ui_input::DeviceDescriptor {
    fidl_ui_input::DeviceDescriptor {
        mouse: descriptor.mouse.as_ref().map(shim_mouse_descriptor),
        sensor: descriptor.sensor.as_ref().map(shim_sensor_descriptor),
        touch: descriptor.touch.as_ref().map(shim_touch_descriptor),
        keyboard: descriptor.keyboard.as_ref().map(shim_keyboard_descriptor),
        consumer_control: descriptor
            .consumer_control
            .as_ref()
            .map(shim_consumer_control_descriptor),
        device_information: descriptor.device_information.as_ref().map(shim_device_information),
    }
}

struct DeviceListenerRegistryServer {
    registry: DeviceListenerRegistry,
}

impl fidl_ui_input::DeviceListenerRegistryLocalServerHandler<Transport>
    for DeviceListenerRegistryServer
{
    async fn register_listener(
        &mut self,
        request: Request<fidl_ui_input::device_listener_registry::RegisterListener, Transport>,
        responder: Responder<fidl_ui_input::device_listener_registry::RegisterListener, Transport>,
    ) {
        let payload = request.payload();
        let listener = Dispatcher::client_from_zx_channel(payload.listener).spawn();

        let (iterator_client, iterator_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceIterator>();
        let iterator_server = Dispatcher::server_from_zx_channel(iterator_server);

        let existing_devices = self.registry.add_listener(listener);

        let dispatcher = fidl_next::ServerDispatcher::new(iterator_server);
        let server = dispatcher.server();
        let iterator_handler =
            DeviceIteratorServer { server, device_iter: existing_devices.into_iter() };

        self.registry.scope.spawn_local(async move {
            if let Err(e) = dispatcher.run_local(iterator_handler).await {
                error!("Error serving DeviceIterator: {:?}", e);
            }
        });

        let _ = responder.respond(iterator_client).await;
    }
}

pub async fn handle_device_listener_registry_request_stream(
    server_end: fidl_next::ServerEnd<fidl_ui_input::DeviceListenerRegistry, Transport>,
    registry: DeviceListenerRegistry,
) -> Result<(), anyhow::Error> {
    let dispatcher = fidl_next::ServerDispatcher::new(server_end);
    let handler = DeviceListenerRegistryServer { registry };

    dispatcher
        .run_local(handler)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("DeviceListenerRegistry error: {:?}", e))
}

pub async fn handle_device_listener_registry_request_stream_legacy(
    stream: fidl_ui_input_legacy::DeviceListenerRegistryRequestStream,
    registry: DeviceListenerRegistry,
) -> Result<(), anyhow::Error> {
    let (inner, _) = stream.into_inner();
    let serve_inner = std::sync::Arc::try_unwrap(inner)
        .map_err(|_| anyhow::anyhow!("Arc has more than one reference"))?;
    let channel = serve_inner.into_channel();
    let zx_channel = channel.into_zx_channel();
    let server_end = ServerEnd::from_untyped(zx_channel);
    let server_end = Dispatcher::server_from_zx_channel(server_end);
    handle_device_listener_registry_request_stream(server_end, registry).await
}

struct DeviceIteratorServer {
    server: fidl_next::Server<fidl_ui_input::DeviceIterator, Transport>,
    device_iter: std::vec::IntoIter<fidl_ui_input::DeviceEvent>,
}

impl fidl_ui_input::DeviceIteratorLocalServerHandler<Transport> for DeviceIteratorServer {
    async fn get_next(
        &mut self,
        responder: Responder<fidl_ui_input::device_iterator::GetNext, Transport>,
    ) {
        let devices: Vec<fidl_ui_input::DeviceEvent> = self.device_iter.by_ref().collect();
        let close = devices.is_empty();
        if responder.respond(&devices).await.is_err() || close {
            self.server.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_next_fuchsia_input as fidl_input;
    use fidl_next_fuchsia_input_report as fidl_input_report;
    use fuchsia_async as fasync;
    use futures::StreamExt as _;

    struct MockDeviceListener {
        event_sender: futures::channel::mpsc::UnboundedSender<fidl_ui_input::DeviceEvent>,
    }

    impl fidl_ui_input::DeviceListenerServerHandler<Transport> for MockDeviceListener {
        async fn on_device_changed(
            &mut self,
            request: Request<fidl_ui_input::device_listener::OnDeviceChanged, Transport>,
        ) {
            let event = request.payload().event.clone();
            let _ = self.event_sender.unbounded_send(event);
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_register_and_notify() {
        let registry = DeviceListenerRegistry::new();
        let (listener_client, listener_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceListener>();

        let device_id = 42;
        let descriptor = fidl_input_report::DeviceDescriptor::default();

        // Add an active device before registering
        registry.active_devices.borrow_mut().insert(device_id, descriptor.clone());

        let listener_client = listener_client.spawn();
        let existing_devices = registry.add_listener(listener_client);
        assert_eq!(existing_devices.len(), 1);
        assert_eq!(existing_devices[0].device_id, Some(device_id));
        assert_eq!(existing_devices[0].descriptor, Some(shim_device_descriptor(&descriptor)));

        let (event_sender, mut event_receiver) = futures::channel::mpsc::unbounded();
        let _server_task = listener_server.spawn(MockDeviceListener { event_sender });

        registry.notify_device_changed(fidl_ui_input::Action::Added, device_id, descriptor.clone());

        if let Some(event) = event_receiver.next().await {
            assert_eq!(event.action, Some(fidl_ui_input::Action::Added));
            assert_eq!(event.device_id, Some(device_id));
            assert_eq!(event.descriptor, Some(shim_device_descriptor(&descriptor)));
        } else {
            panic!("Expected a request on listener stream");
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_remove_failed_listener() {
        let registry = DeviceListenerRegistry::new();
        let (listener_client, listener_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceListener>();

        let listener_client = listener_client.spawn();
        let _ = registry.add_listener(listener_client);
        assert_eq!(registry.listeners.borrow().len(), 1);

        std::mem::drop(listener_server);

        fasync::Timer::new(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(10)))
            .await;

        // Notify, which should attempt to send to the listener and remove it on failure.
        let device_id = 42;
        let descriptor = fidl_input_report::DeviceDescriptor::default();
        registry.notify_device_changed(fidl_ui_input::Action::Added, device_id, descriptor);

        assert_eq!(registry.listeners.borrow().len(), 0);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_device_iterator() {
        let registry = DeviceListenerRegistry::new();
        let (listener_client, _listener_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceListener>();

        let device_id = 42;
        let descriptor = fidl_input_report::DeviceDescriptor::default();
        registry.active_devices.borrow_mut().insert(device_id, descriptor.clone());

        let (registry_client, registry_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceListenerRegistry>();

        let handle_fut = handle_device_listener_registry_request_stream(registry_server, registry);
        let _task = fasync::Task::local(handle_fut);

        let registry_proxy = registry_client.spawn();
        let response = registry_proxy.register_listener(listener_client).await.unwrap();
        let iterator_proxy = response.iterator.spawn();

        // Get first device
        let response = iterator_proxy.get_next().await.unwrap();
        assert_eq!(response.devices.len(), 1);
        assert_eq!(response.devices[0].device_id, Some(device_id));

        // Get next (should be empty)
        let response = iterator_proxy.get_next().await.unwrap();
        assert!(response.devices.is_empty());
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_device_iterator_survives_registry_disconnect() {
        let registry = DeviceListenerRegistry::new();
        let (listener_client, _listener_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceListener>();

        let device_id = 42;
        let descriptor = fidl_input_report::DeviceDescriptor::default();
        registry.active_devices.borrow_mut().insert(device_id, descriptor.clone());

        let (registry_client, registry_server) =
            fidl_next::fuchsia::create_channel::<fidl_ui_input::DeviceListenerRegistry>();

        let handle_fut =
            handle_device_listener_registry_request_stream(registry_server, registry.clone());
        let _task = fasync::Task::local(handle_fut);

        let registry_proxy = registry_client.spawn();
        let response = registry_proxy.register_listener(listener_client).await.unwrap();
        let iterator_proxy = response.iterator.spawn();

        // Drop registry proxy (client disconnects from registry)
        std::mem::drop(registry_proxy);

        // Allow disconnect to process
        fasync::Timer::new(fasync::MonotonicInstant::after(zx::MonotonicDuration::from_millis(10)))
            .await;

        // Iterator should still work!
        let response = iterator_proxy.get_next().await.unwrap();
        assert_eq!(response.devices.len(), 1);
        assert_eq!(response.devices[0].device_id, Some(device_id));
    }

    #[fuchsia::test]
    fn test_shim_device_descriptor_full() {
        let test_axis = fidl_input::Axis {
            range: fidl_input::Range { min: -100, max: 100 },
            unit: fidl_input::Unit { type_: fidl_input::UnitType::None, exponent: 0 },
        };

        let descriptor = fidl_input_report::DeviceDescriptor {
            mouse: Some(fidl_input_report::MouseDescriptor {
                input: Some(fidl_input_report::MouseInputDescriptor {
                    movement_x: Some(test_axis),
                    movement_y: Some(test_axis),
                    scroll_v: Some(test_axis),
                    scroll_h: Some(test_axis),
                    buttons: Some(vec![1, 2, 3]),
                    position_x: Some(test_axis),
                    position_y: Some(test_axis),
                }),
            }),
            sensor: Some(fidl_input_report::SensorDescriptor {
                input: Some(vec![fidl_input_report::SensorInputDescriptor {
                    values: Some(vec![
                        fidl_input_report::SensorAxis {
                            axis: test_axis,
                            type_: fidl_input_report::SensorType::AccelerometerX,
                        },
                        fidl_input_report::SensorAxis {
                            axis: test_axis,
                            type_: fidl_input_report::SensorType::LightIlluminance,
                        },
                    ]),
                    report_id: Some(1),
                }]),
                feature: None,
            }),
            touch: Some(fidl_input_report::TouchDescriptor {
                input: Some(fidl_input_report::TouchInputDescriptor {
                    contacts: Some(vec![fidl_input_report::ContactInputDescriptor {
                        position_x: Some(test_axis),
                        position_y: Some(test_axis),
                        pressure: Some(test_axis),
                        contact_width: Some(test_axis),
                        contact_height: Some(test_axis),
                    }]),
                    max_contacts: Some(10),
                    touch_type: Some(fidl_input_report::TouchType::Touchscreen),
                    buttons: Some(vec![
                        fidl_input_report::TouchButton::Palm,
                        fidl_input_report::TouchButton::SwipeUp,
                    ]),
                }),
                feature: Some(fidl_input_report::TouchFeatureDescriptor {
                    supports_input_mode: Some(true),
                    supports_selective_reporting: Some(false),
                }),
            }),
            keyboard: Some(fidl_input_report::KeyboardDescriptor {
                input: Some(fidl_input_report::KeyboardInputDescriptor {
                    keys3: Some(vec![fidl_input::Key::A, fidl_input::Key::B]),
                }),
                output: Some(fidl_input_report::KeyboardOutputDescriptor {
                    leds: Some(vec![
                        fidl_input_report::LedType::NumLock,
                        fidl_input_report::LedType::CapsLock,
                    ]),
                }),
            }),
            consumer_control: Some(fidl_input_report::ConsumerControlDescriptor {
                input: Some(fidl_input_report::ConsumerControlInputDescriptor {
                    buttons: Some(vec![
                        fidl_input::ConsumerControlButton::VolumeUp,
                        fidl_input::ConsumerControlButton::VolumeDown,
                    ]),
                }),
            }),
            device_information: Some(fidl_input_report::DeviceInformation {
                vendor_id: Some(0x1234),
                product_id: Some(0x5678),
                version: Some(1),
                polling_rate: Some(10_000_000),
                manufacturer_name: Some("Google".to_string()),
                product_name: Some("Fuchsia Test Device".to_string()),
                serial_number: Some("SN123456".to_string()),
            }),
        };

        let shimmed = shim_device_descriptor(&descriptor);

        // Check mouse
        let mouse = shimmed.mouse.expect("mouse should be present");
        let mouse_input = mouse.input.expect("mouse input should be present");
        assert_eq!(mouse_input.movement_x, Some(test_axis));
        assert_eq!(mouse_input.movement_y, Some(test_axis));
        assert_eq!(mouse_input.scroll_v, Some(test_axis));
        assert_eq!(mouse_input.scroll_h, Some(test_axis));
        assert_eq!(mouse_input.buttons, Some(vec![1, 2, 3]));
        assert_eq!(mouse_input.position_x, Some(test_axis));
        assert_eq!(mouse_input.position_y, Some(test_axis));

        // Check sensor
        let sensor = shimmed.sensor.expect("sensor should be present");
        let sensor_input = sensor.input.expect("sensor input should be present");
        let values = sensor_input.values.expect("sensor values should be present");
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].axis, test_axis);
        assert_eq!(values[0].type_, fidl_ui_input::SensorType::AccelerometerX);
        assert_eq!(values[1].axis, test_axis);
        assert_eq!(values[1].type_, fidl_ui_input::SensorType::LightIlluminance);

        // Check touch
        let touch = shimmed.touch.expect("touch should be present");
        let touch_input = touch.input.expect("touch input should be present");
        let contacts = touch_input.contacts.expect("contacts should be present");
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].position_x, Some(test_axis));
        assert_eq!(contacts[0].position_y, Some(test_axis));
        assert_eq!(contacts[0].pressure, Some(test_axis));
        assert_eq!(contacts[0].contact_width, Some(test_axis));
        assert_eq!(contacts[0].contact_height, Some(test_axis));
        assert_eq!(touch_input.max_contacts, Some(10));
        assert_eq!(touch_input.touch_type, Some(fidl_ui_input::TouchType::Touchscreen));
        assert_eq!(
            touch_input.buttons,
            Some(vec![fidl_ui_input::TouchButton::Palm, fidl_ui_input::TouchButton::SwipeUp])
        );
        let touch_feature = touch.feature.expect("touch feature should be present");
        assert_eq!(touch_feature.supports_input_mode, Some(true));
        assert_eq!(touch_feature.supports_selective_reporting, Some(false));

        // Check keyboard
        let keyboard = shimmed.keyboard.expect("keyboard should be present");
        let kb_input = keyboard.input.expect("keyboard input should be present");
        assert_eq!(kb_input.keys, Some(vec![fidl_input::Key::A, fidl_input::Key::B]));
        let kb_output = keyboard.output.expect("keyboard output should be present");
        assert_eq!(
            kb_output.leds,
            Some(vec![fidl_ui_input::LedType::NumLock, fidl_ui_input::LedType::CapsLock])
        );

        // Check consumer control
        let cc = shimmed.consumer_control.expect("consumer control should be present");
        let cc_input = cc.input.expect("consumer control input should be present");
        assert_eq!(
            cc_input.buttons,
            Some(vec![
                fidl_input::ConsumerControlButton::VolumeUp,
                fidl_input::ConsumerControlButton::VolumeDown
            ])
        );

        // Check device information
        let info = shimmed.device_information.expect("device info should be present");
        assert_eq!(info.vendor_id, Some(0x1234));
        assert_eq!(info.product_id, Some(0x5678));
        assert_eq!(info.version, Some(1));
        assert_eq!(info.polling_rate, Some(10_000_000));
        assert_eq!(info.manufacturer_name, Some("Google".to_string()));
        assert_eq!(info.product_name, Some("Fuchsia Test Device".to_string()));
        assert_eq!(info.serial_number, Some("SN123456".to_string()));
    }
}
