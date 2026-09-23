// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_temperature as ftemperature;
use fidl_fuchsia_thermal as fthermal;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::{StreamExt, TryStreamExt};
use log::info;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const SENSOR_NAME: &'static str = "fake-trippoint";
const DEFAULT_SENSOR_TEMPERATURE: f32 = 25.0;

struct SensorProps {
    info: fthermal::SensorInfo,
    temperature: f32,
    override_temperature: RefCell<Option<f32>>,
}

fn spawn_sensor_server(server: fthermal::SensorServer_, props: Rc<SensorProps>) {
    match server {
        fthermal::SensorServer_::Temperature(temp_server_end) => {
            let mut stream = temp_server_end.into_stream();
            let props = props.clone();
            fasync::Task::local(async move {
                while let Ok(Some(req)) = stream.try_next().await {
                    match req {
                        ftemperature::DeviceRequest::GetTemperatureCelsius { responder } => {
                            let temp_c_opt = props.override_temperature.borrow().clone();
                            let temp_c = match temp_c_opt {
                                Some(temp_c) => temp_c,
                                None => props.temperature,
                            };
                            responder.send(zx::sys::ZX_OK, temp_c as f32).unwrap();
                        }
                        ftemperature::DeviceRequest::GetSensorName { responder } => {
                            match &props.info.name {
                                Some(name) => responder.send(name).unwrap(),
                                None => unreachable!("unknown sensor name"),
                            }
                        }
                    }
                }
            })
            .detach();
        }
        _ => unreachable!(),
    }
}

fn handle_sensor_manager_stream(
    mut stream: fthermal::SensorManagerRequestStream,
    sensors: Rc<HashMap<String, Rc<SensorProps>>>,
) {
    fasync::Task::local(async move {
        while let Ok(Some(req)) = stream.try_next().await {
            match req {
                fthermal::SensorManagerRequest::ListSensors { responder } => {
                    let infos = sensors.values().map(|s| s.info.clone()).collect::<Vec<_>>();
                    responder.send(&infos).unwrap();
                }
                fthermal::SensorManagerRequest::SetTemperatureOverride {
                    name,
                    override_temperature,
                    responder,
                } => {
                    let sensor = sensors.get(&name).expect("unknown sensor name");
                    sensor.override_temperature.borrow_mut().replace(override_temperature);
                    responder.send(Ok(())).unwrap();
                }
                fthermal::SensorManagerRequest::ClearTemperatureOverride { name, responder } => {
                    let sensor = sensors.get(&name).expect("unknown sensor name");
                    sensor.override_temperature.borrow_mut().take();
                    responder.send(Ok(())).unwrap();
                }
                fthermal::SensorManagerRequest::Connect { payload, responder } => {
                    let name = payload.name.expect("name is required");
                    let server = payload.server_end.expect("server is required");

                    let sensor = sensors.get(&name).expect("unknown sensor name");
                    spawn_sensor_server(server, sensor.clone());
                    responder.send(Ok(())).unwrap();
                }
                _ => unreachable!(),
            }
        }
    })
    .detach();
}

#[fuchsia::main]
async fn main() {
    info!("Started fake-thermal-sensor-manager");

    let sensors = Rc::new({
        let mut sensors = HashMap::new();
        sensors.insert(
            SENSOR_NAME.to_string(),
            Rc::new(SensorProps {
                info: fthermal::SensorInfo {
                    name: Some(SENSOR_NAME.to_string()),
                    ..Default::default()
                },
                temperature: DEFAULT_SENSOR_TEMPERATURE.into(),
                override_temperature: RefCell::new(None),
            }),
        );
        sensors
    });

    let mut fs = ServiceFs::new_local();
    fs.dir("svc")
        .add_fidl_service(move |stream| handle_sensor_manager_stream(stream, sensors.clone()));

    fs.take_and_serve_directory_handle().unwrap();
    fs.collect::<()>().await;
}
