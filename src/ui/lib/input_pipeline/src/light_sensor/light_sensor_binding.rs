// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::types::Rgbc;
use crate::input_device::{
    self, Handled, InputDeviceBinding, InputDeviceDescriptor, InputDeviceStatus, InputEvent,
};
use crate::metrics;
use anyhow::{Error, format_err};
use async_trait::async_trait;
use derivative::Derivative;
use fidl_next_fuchsia_input_report::SensorType;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use metrics_registry::*;

#[derive(Derivative, Clone)]
#[derivative(Debug)]
pub struct LightSensorEvent {
    #[derivative(Debug = "ignore")]
    pub(crate) device_proxy:
        fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, crate::Transport>,
    pub(crate) rgbc: Rgbc<u16>,
}

impl PartialEq for LightSensorEvent {
    fn eq(&self, other: &Self) -> bool {
        self.rgbc == other.rgbc
    }
}

impl Eq for LightSensorEvent {}

impl LightSensorEvent {
    pub fn record_inspect(&self, node: &fuchsia_inspect::Node) {
        node.record_uint("red", u64::from(self.rgbc.red));
        node.record_uint("green", u64::from(self.rgbc.green));
        node.record_uint("blue", u64::from(self.rgbc.blue));
        node.record_uint("clear", u64::from(self.rgbc.clear));
    }
}

/// A [`LightSensorBinding`] represents a connection to a light sensor input device.
///
/// TODO more details
pub(crate) struct LightSensorBinding {
    /// The channel to stream InputEvents to.
    event_sender: UnboundedSender<Vec<InputEvent>>,

    /// Holds information about this device.
    device_descriptor: LightSensorDeviceDescriptor,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct LightSensorDeviceDescriptor {
    /// The vendor id of the connected light sensor input device.
    pub(crate) vendor_id: u32,

    /// The product id of the connected light sensor input device.
    pub(crate) product_id: u32,

    /// The device id of the connected light sensor input device.
    pub(crate) device_id: u32,

    /// Layout of the color channels in the sensor report.
    pub(crate) sensor_layout: Rgbc<usize>,
}

#[async_trait]
impl InputDeviceBinding for LightSensorBinding {
    fn input_event_sender(&self) -> UnboundedSender<Vec<InputEvent>> {
        self.event_sender.clone()
    }

    fn get_device_descriptor(&self) -> InputDeviceDescriptor {
        InputDeviceDescriptor::LightSensor(self.device_descriptor.clone())
    }
}

impl LightSensorBinding {
    /// Creates a new [`InputDeviceBinding`] from the `device_proxy`.
    ///
    /// The binding will start listening for input reports immediately and send new InputEvents
    /// to the device binding owner over `input_event_sender`.
    ///
    /// # Parameters
    /// - `device_proxy`: The proxy to bind the new [`InputDeviceBinding`] to.
    /// - `device_id`: The unique identifier of this device.
    /// - `input_event_sender`: The channel to send new InputEvents to.
    /// - `device_node`: The inspect node for this device binding
    /// - `metrics_logger`: The metrics logger.
    ///
    /// # Errors
    /// If there was an error binding to the proxy.
    pub(crate) async fn new(
        device_proxy: fidl_next::Client<
            fidl_next_fuchsia_input_report::InputDevice,
            crate::Transport,
        >,
        device_id: u32,
        input_event_sender: UnboundedSender<Vec<InputEvent>>,
        device_node: fuchsia_inspect::Node,
        feature_flags: input_device::InputPipelineFeatureFlags,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<(Self, crate::dispatcher::TaskHandle<()>), Error> {
        let (device_descriptor, mut inspect_status) =
            Self::bind_device(&device_proxy, device_id, device_node, metrics_logger.clone())
                .await?;
        inspect_status.health_node.set_ok();
        let task = input_device::initialize_report_stream(
            device_proxy.clone(),
            InputDeviceDescriptor::LightSensor(device_descriptor.clone()),
            input_event_sender.clone(),
            inspect_status,
            metrics_logger,
            feature_flags,
            move |reports,
                  previous_state,
                  device_descriptor,
                  input_event_sender,
                  inspect_status,
                  metrics_logger,
                  _feature_flags| {
                Self::process_reports(
                    reports,
                    previous_state,
                    device_descriptor,
                    input_event_sender,
                    device_proxy.clone(),
                    inspect_status,
                    metrics_logger,
                )
            },
        );

        Ok((LightSensorBinding { event_sender: input_event_sender, device_descriptor }, task))
    }

    /// Binds the provided input device to a new instance of `LightSensorBinding`.
    ///
    /// # Parameters
    /// - `device`: The device to use to initialize the binding.
    /// - `device_id`: The device ID being bound.
    /// - `device_node`: The inspect node for this device binding
    ///
    /// # Errors
    /// If the device descriptor could not be retrieved, or the descriptor could not be parsed
    /// correctly.
    async fn bind_device(
        device: &fidl_next::Client<fidl_next_fuchsia_input_report::InputDevice, crate::Transport>,
        device_id: u32,
        device_node: fuchsia_inspect::Node,
        metrics_logger: metrics::MetricsLogger,
    ) -> Result<(LightSensorDeviceDescriptor, InputDeviceStatus), Error> {
        let mut input_device_status = InputDeviceStatus::new(device_node);
        let descriptor = match device.get_descriptor().await {
            Ok(descriptor) => descriptor.descriptor,
            Err(_) => {
                input_device_status.health_node.set_unhealthy("Could not get device descriptor.");
                return Err(format_err!("Could not get descriptor for device_id: {}", device_id));
            }
        };
        let device_info = descriptor.device_information.ok_or_else(|| {
            input_device_status.health_node.set_unhealthy("Empty device_info in descriptor.");
            // Logging in addition to returning an error, as in some test
            // setups the error may never be displayed to the user.
            metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::LightEmptyDeviceInfo,
                std::format!("DRIVER BUG: empty device_info for device_id: {}", device_id),
            );
            format_err!("empty device info for device_id: {}", device_id)
        })?;
        match descriptor.sensor {
            Some(fidl_next_fuchsia_input_report::SensorDescriptor {
                input: Some(input_descriptors),
                ..
            }) => {
                let sensor_layout = input_descriptors
                    .into_iter()
                    .filter_map(|input_descriptor| {
                        input_descriptor.values.and_then(|values| {
                            let mut red_value = None;
                            let mut green_value = None;
                            let mut blue_value = None;
                            let mut clear_value = None;
                            for (i, value) in values.iter().enumerate() {
                                let old = match value.type_ {
                                    SensorType::LightRed => {
                                        std::mem::replace(&mut red_value, Some(i))
                                    }
                                    SensorType::LightGreen => {
                                        std::mem::replace(&mut green_value, Some(i))
                                    }
                                    SensorType::LightBlue => {
                                        std::mem::replace(&mut blue_value, Some(i))
                                    }
                                    SensorType::LightIlluminance => {
                                        std::mem::replace(&mut clear_value, Some(i))
                                    }
                                    type_ => {
                                        log::warn!(
                                            "unexpected sensor type {type_:?} found on light \
                                                sensor device"
                                        );
                                        None
                                    }
                                };
                                if old.is_some() {
                                    log::warn!(
                                        "existing index for light sensor {:?} replaced",
                                        value.type_
                                    );
                                }
                            }

                            red_value.and_then(|red| {
                                green_value.and_then(|green| {
                                    blue_value.and_then(|blue| {
                                        clear_value.map(|clear| Rgbc { red, green, blue, clear })
                                    })
                                })
                            })
                        })
                    })
                    .next()
                    .ok_or_else(|| {
                        input_device_status.health_node.set_unhealthy("Missing light sensor data.");
                        format_err!("missing sensor data in device")
                    })?;
                Ok((
                    LightSensorDeviceDescriptor {
                        vendor_id: device_info.vendor_id.unwrap_or_default(),
                        product_id: device_info.product_id.unwrap_or_default(),
                        device_id,
                        sensor_layout,
                    },
                    input_device_status,
                ))
            }
            device_descriptor => {
                input_device_status
                    .health_node
                    .set_unhealthy("Light Sensor Device Descriptor failed to parse.");
                Err(format_err!(
                    "Light Sensor Device Descriptor failed to parse: \n {:?}",
                    device_descriptor
                ))
            }
        }
    }

    /// Parses an [`InputReport`] into one or more [`InputEvent`]s.
    ///
    /// The [`InputEvent`]s are sent to the device binding owner via [`input_event_sender`].
    ///
    /// # Parameters
    /// `reports`: The incoming [`InputReport`].
    /// `previous_report`: The previous [`InputReport`] seen for the same device.
    /// `device_descriptor`: The descriptor for the input device generating the input reports.
    /// `input_event_sender`: The sender for the device binding's input event stream.
    ///
    /// # Returns
    /// An [`InputReport`] which will be passed to the next call to [`process_reports`], as
    /// [`previous_report`]. If `None`, the next call's [`previous_report`] will be `None`.
    /// A [`UnboundedReceiver<InputEvent>`] which will poll asynchronously generated events to be
    /// recorded by `inspect_status` in `input_device::initialize_report_stream()`. If device
    /// binding does not generate InputEvents asynchronously, this will be `None`.
    ///
    /// The returned [`InputReport`] is guaranteed to have no `wake_lease`.
    fn process_reports(
        reports: &[fidl_next_fuchsia_input_report::wire::InputReport<'_>],
        mut previous_state: Option<input_device::PreviousDeviceState>,
        device_descriptor: &input_device::InputDeviceDescriptor,
        input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
        device_proxy: fidl_next::Client<
            fidl_next_fuchsia_input_report::InputDevice,
            crate::Transport,
        >,
        inspect_status: &InputDeviceStatus,
        metrics_logger: &metrics::MetricsLogger,
    ) -> (Option<input_device::PreviousDeviceState>, Option<UnboundedReceiver<InputEvent>>) {
        fuchsia_trace::duration!("input", "light-sensor-binding-process-reports", "num_reports" => reports.len());
        for report in reports {
            previous_state = Self::process_report(
                report,
                previous_state,
                device_descriptor,
                input_event_sender,
                device_proxy.clone(),
                inspect_status,
                metrics_logger,
            );
        }
        (previous_state, None)
    }

    fn process_report(
        report: &fidl_next_fuchsia_input_report::wire::InputReport<'_>,
        previous_state: Option<input_device::PreviousDeviceState>,
        device_descriptor: &input_device::InputDeviceDescriptor,
        input_event_sender: &mut UnboundedSender<Vec<InputEvent>>,
        device_proxy: fidl_next::Client<
            fidl_next_fuchsia_input_report::InputDevice,
            crate::Transport,
        >,
        inspect_status: &InputDeviceStatus,
        metrics_logger: &metrics::MetricsLogger,
    ) -> Option<input_device::PreviousDeviceState> {
        if let Some(trace_id) = report.trace_id() {
            fuchsia_trace::flow_end!("input", "input_report", trace_id.0.into());
        }

        inspect_status.count_received_report_wire(report);
        let light_sensor_descriptor =
            if let input_device::InputDeviceDescriptor::LightSensor(light_sensor_descriptor) =
                device_descriptor
            {
                light_sensor_descriptor
            } else {
                unreachable!()
            };

        // Input devices can have multiple types so ensure `report` is a KeyboardInputReport.
        let sensor = match report.sensor() {
            None => {
                inspect_status.count_filtered_report();
                return previous_state;
            }
            Some(sensor) => sensor,
        };

        let values = match sensor.values() {
            None => {
                inspect_status.count_filtered_report();
                return None;
            }
            Some(values) => values,
        };

        let event = input_device::InputEvent {
            device_event: input_device::InputDeviceEvent::LightSensor(LightSensorEvent {
                device_proxy,
                rgbc: Rgbc {
                    red: values[light_sensor_descriptor.sensor_layout.red].0 as u16,
                    green: values[light_sensor_descriptor.sensor_layout.green].0 as u16,
                    blue: values[light_sensor_descriptor.sensor_layout.blue].0 as u16,
                    clear: values[light_sensor_descriptor.sensor_layout.clear].0 as u16,
                },
            }),
            device_descriptor: device_descriptor.clone(),
            event_time: zx::MonotonicInstant::get(),
            handled: Handled::No,
            trace_id: None,
        };

        let events = vec![event];
        inspect_status.count_generated_events(&events);

        if let Err(e) = input_event_sender.unbounded_send(events) {
            metrics_logger.log_error(
                InputPipelineErrorMetricDimensionEvent::LightFailedToSendEvent,
                std::format!("Failed to send LightSensorEvent with error: {e:?}"),
            );
        }

        Some(input_device::PreviousDeviceState::LightSensor)
    }
}
