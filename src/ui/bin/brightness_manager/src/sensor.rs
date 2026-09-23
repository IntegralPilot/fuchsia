// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_input_report::{
    DeviceDescriptor, InputReportsReaderV2Event, InputReportsReaderV2EventStream,
    InputReportsReaderV2Marker, InputReportsReaderV2Proxy, SensorInputDescriptor, SensorType,
    ServiceMarker,
};
use fuchsia_component::client::Service;
use futures::lock::Mutex;

#[derive(Debug)]
pub struct AmbientLightInputRpt {
    pub illuminance: f32,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

struct AmbientLightComponent {
    pub report_index: usize,
    pub exponent: i32,
    pub report_id: u8, // report ID associated with descriptor
}

struct AmbientLightInputReportReaderProxy {
    pub proxy: InputReportsReaderV2Proxy,
    pub event_stream: Mutex<InputReportsReaderV2EventStream>,

    pub illuminance: Option<AmbientLightComponent>,
    pub red: Option<AmbientLightComponent>,
    pub green: Option<AmbientLightComponent>,
    pub blue: Option<AmbientLightComponent>,
}

use futures::StreamExt;

async fn open_sensor_input_report_reader() -> Result<AmbientLightInputReportReaderProxy, Error> {
    let mut watcher = Service::open(ServiceMarker)?
        .watch()
        .await
        .context("Failed to watch for sensor service instances")?;

    while let Some(instance_result) = watcher.next().await {
        let instance = instance_result?;
        let device = instance.connect_to_input_device()?;

        fn get_sensor_input(
            descriptor: &DeviceDescriptor,
        ) -> Result<&Vec<SensorInputDescriptor>, Error> {
            let sensor = descriptor.sensor.as_ref().context("device has no sensor")?;
            let input_desc = sensor.input.as_ref().context("sensor has no input descriptor")?;
            Ok(input_desc)
        }

        if let Ok(descriptor) = device.get_descriptor().await {
            match get_sensor_input(&descriptor) {
                Ok(input_desc) => {
                    let mut illuminance = None;
                    let mut red = None;
                    let mut green = None;
                    let mut blue = None;

                    for input in input_desc {
                        let axes =
                            input.values.as_ref().context("SensorInputDescriptor has no values")?;
                        for (i, val) in axes.iter().enumerate() {
                            let component = AmbientLightComponent {
                                report_index: i,
                                exponent: val.axis.unit.exponent,
                                report_id: input.report_id.unwrap_or(0),
                            };
                            match val.type_ {
                                SensorType::LightIlluminance => illuminance = Some(component),
                                SensorType::LightRed => red = Some(component),
                                SensorType::LightGreen => green = Some(component),
                                SensorType::LightBlue => blue = Some(component),
                                _ => {}
                            }
                        }
                    }

                    if illuminance.is_some() {
                        const MAX_UNACKNOWLEDGED_REPORTS_LIMIT: u16 = 120;
                        let (proxy, server_end) =
                            fidl::endpoints::create_proxy::<InputReportsReaderV2Marker>();
                        if let Ok(_) = device
                            .get_input_reports_reader_v2(
                                server_end,
                                MAX_UNACKNOWLEDGED_REPORTS_LIMIT,
                            )
                            .await
                        {
                            let event_stream = proxy.take_event_stream();
                            return Ok(AmbientLightInputReportReaderProxy {
                                proxy,
                                event_stream: Mutex::new(event_stream),
                                illuminance,
                                red,
                                blue,
                                green,
                            });
                        }
                    }
                }
                Err(e) => {
                    log::info!("Skip device {:?}: {}", instance.instance_name(), e);
                }
            };
        }
    }
    Err(format_err!("no sensor found"))
}

/// Reads the sensor's input report and decodes it.
async fn read_sensor_input_report(
    device: &AmbientLightInputReportReaderProxy,
) -> Result<Option<AmbientLightInputRpt>, Error> {
    let mut event_stream = device.event_stream.lock().await;
    match event_stream.next().await {
        Some(Ok(InputReportsReaderV2Event::OnInputReports { reports, last_report_stamp })) => {
            if let Err(e) = device.proxy.acknowledge_reports(last_report_stamp) {
                log::warn!("Failed to acknowledge reports: {e}");
            }
            for report in reports {
                if report.report_id.unwrap_or(0) != device.illuminance.as_ref().unwrap().report_id {
                    continue;
                }

                let sensor =
                    report.sensor.context("sensor required in InputReport for sensor device")?;
                let values = sensor.values.context("values required in SensorInputReport")?;
                let f = |component: &Option<AmbientLightComponent>| match component {
                    Some(val) => match val.exponent {
                        0 => values[val.report_index] as f32,
                        _ => values[val.report_index] as f32 * f32::powf(10.0, val.exponent as f32),
                    },
                    None => 0.0,
                };

                let illuminance = f(&device.illuminance);
                let red = f(&device.red);
                let green = f(&device.green);
                let blue = f(&device.blue);

                return Ok(Some(AmbientLightInputRpt { illuminance, red, blue, green }));
            }
            Ok(None)
        }
        Some(Ok(InputReportsReaderV2Event::_UnknownEvent { ordinal, .. })) => {
            Err(format_err!("Unknown event received: {ordinal}"))
        }
        Some(Err(e)) => Err(format_err!("Event stream error: {}", e)),
        None => Err(format_err!("Event stream ended")),
    }
}

/// TODO(lingxueluo) Default and temporary report when sensor is not valid(https://fxbug.dev/42119013).
fn default_report() -> Result<Option<AmbientLightInputRpt>, Error> {
    Ok(Some(AmbientLightInputRpt { illuminance: 200.0, red: 200.0, green: 200.0, blue: 200.0 }))
}

pub struct Sensor {
    proxy: Option<AmbientLightInputReportReaderProxy>,
}

impl Sensor {
    pub async fn new() -> Sensor {
        let proxy = open_sensor_input_report_reader().await;
        match proxy {
            Ok(proxy) => return Sensor { proxy: Some(proxy) },
            Err(_e) => {
                println!("No valid sensor found.");
                return Sensor { proxy: None };
            }
        }
    }

    async fn read(&self) -> Result<Option<AmbientLightInputRpt>, Error> {
        if self.proxy.is_none() {
            default_report()
        } else {
            read_sensor_input_report(self.proxy.as_ref().unwrap()).await
        }
    }
}

#[async_trait]
pub trait SensorControl: Send {
    async fn read(&self) -> Result<Option<AmbientLightInputRpt>, Error>;
}

#[async_trait]
impl SensorControl for Sensor {
    async fn read(&self) -> Result<Option<AmbientLightInputRpt>, Error> {
        self.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_open_sensor_error() {
        let sensor = Sensor { proxy: None };
        if let Some(ambient_light_input_rpt) = sensor.read().await.unwrap() {
            assert_eq!(ambient_light_input_rpt.illuminance, 200.0);
            assert_eq!(ambient_light_input_rpt.red, 200.0);
            assert_eq!(ambient_light_input_rpt.green, 200.0);
            assert_eq!(ambient_light_input_rpt.blue, 200.0);
        }
    }
}
