// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::common_utils::result_debug_panic::ResultDebugPanic;
use crate::message::{Message, MessageReturn};
use crate::node::Node;
use crate::ok_or_default_err;
use crate::types::Celsius;
use anyhow::{Error, Result};
use async_trait::async_trait;
use fidl_fuchsia_hardware_temperature as ftemperature;
use fidl_fuchsia_thermal as fthermal;
use fuchsia_async as fasync;
use fuchsia_component::server::{ServiceFs, ServiceFsDir, ServiceObjLocal};
use futures::{TryFutureExt, TryStreamExt};
use log::*;
use serde_derive::Deserialize;
use serde_json as json;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub struct ThermalSensorManagerBuilder<'a, 'b> {
    outgoing_svc_dir: Option<ServiceFsDir<'a, ServiceObjLocal<'b, ()>>>,
    temperature_handler_nodes: Option<Vec<Rc<dyn Node>>>,
}

impl<'a, 'b> ThermalSensorManagerBuilder<'a, 'b> {
    #[cfg(test)]
    pub fn new() -> Self {
        Self { outgoing_svc_dir: None, temperature_handler_nodes: None }
    }

    #[cfg(test)]
    pub fn with_outgoing_svc_dir(
        mut self,
        outgoing_svc_dir: ServiceFsDir<'a, ServiceObjLocal<'b, ()>>,
    ) -> Self {
        self.outgoing_svc_dir = Some(outgoing_svc_dir);
        self
    }

    #[cfg(test)]
    pub fn with_temperature_handler_nodes(
        mut self,
        temperature_handler_nodes: Vec<Rc<dyn Node>>,
    ) -> Self {
        self.temperature_handler_nodes = Some(temperature_handler_nodes);
        self
    }

    pub fn new_from_json(
        json_data: json::Value,
        nodes: &HashMap<String, Rc<dyn Node>>,
        service_fs: &'a mut ServiceFs<ServiceObjLocal<'b, ()>>,
    ) -> Self {
        #[derive(Deserialize)]
        struct Dependencies {
            temperature_handler_nodes: Vec<String>,
        }

        #[derive(Deserialize)]
        struct JsonData {
            dependencies: Dependencies,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        let temperature_handler_node_deps =
            data.dependencies.temperature_handler_nodes.iter().collect::<HashSet<&String>>();
        let temperature_handler_nodes = nodes
            .iter()
            .filter_map(|(name, node)| temperature_handler_node_deps.get(name).map(|_| node))
            .cloned()
            .collect();

        Self {
            outgoing_svc_dir: Some(service_fs.dir("svc")),
            temperature_handler_nodes: Some(temperature_handler_nodes),
        }
    }

    pub fn build(self) -> Result<Rc<ThermalSensorManager>> {
        let thermal_sensor_manager = Rc::new(ThermalSensorManager {
            temperature_handler_nodes: ok_or_default_err!(self.temperature_handler_nodes)
                .or_debug_panic()?,
            sensor_deps: Rc::new(async_lock::OnceCell::new()),
            scope: Default::default(),
        });

        thermal_sensor_manager
            .publish_service(&mut ok_or_default_err!(self.outgoing_svc_dir).or_debug_panic()?);
        Ok(thermal_sensor_manager)
    }
}

async fn run_sensor_proxy_server(
    node: Rc<dyn Node>,
    sensor_name: String,
    sensor_server: fthermal::SensorServer_,
) {
    match sensor_server {
        fthermal::SensorServer_::Temperature(device_server) => {
            let mut stream = device_server.into_stream();
            info!("Starting proxy server for sensor {}", sensor_name);
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    ftemperature::DeviceRequest::GetTemperatureCelsius { responder } => {
                        let temperature_c = match node
                            .handle_message(&Message::ReadTemperature)
                            .await
                        {
                            Ok(MessageReturn::ReadTemperature(Celsius(temp_c))) => temp_c,
                            e => {
                                warn!(
                                    "Failed to read temperature from sensor {}: {:?}",
                                    sensor_name, e
                                );
                                if let Err(e) = responder.send(zx::Status::IO.into_raw(), 0.0) {
                                    warn!(
                                        "Failed to send temperature to client from sensor {}: {:?}",
                                        sensor_name, e
                                    );
                                }
                                continue;
                            }
                        };

                        if let Err(e) = responder.send(zx::sys::ZX_OK, temperature_c as f32) {
                            warn!(
                                "Failed to send temperature to client from sensor {}: {:?}",
                                sensor_name, e
                            );
                        }
                    }
                    ftemperature::DeviceRequest::GetSensorName { responder } => {
                        if let Err(e) = responder.send(&sensor_name) {
                            warn!(
                                "Failed to send sensor name to client from sensor {}: {:?}",
                                sensor_name, e
                            );
                        }
                    }
                }
            }
            info!("Stopping proxy server for sensor {}", sensor_name);
        }
        _ => {
            unreachable!("Mismatch between server type and sensor proxy");
        }
    }
}

struct SensorDeps {
    sensors: Vec<fthermal::SensorInfo>,
    sensor_map: HashMap<String, Rc<dyn Node>>,
}

pub struct ThermalSensorManager {
    temperature_handler_nodes: Vec<Rc<dyn Node>>,
    sensor_deps: Rc<async_lock::OnceCell<SensorDeps>>,
    scope: fasync::Scope,
}

impl ThermalSensorManager {
    /// Publishes a service to expose sensor manager control of the Power Manager.
    fn publish_service<'a, 'b>(
        &self,
        outgoing_svc_dir: &mut ServiceFsDir<'a, ServiceObjLocal<'b, ()>>,
    ) {
        info!("Starting sensor_manager service");

        let scope = self.scope.to_handle();
        let inner = self.sensor_deps.clone();

        outgoing_svc_dir.add_fidl_service(
            move |mut stream: fthermal::SensorManagerRequestStream| {
                let inner = inner.clone();
                let inner_scope = scope.clone();
                scope.spawn_local(async move {
                    let SensorDeps { sensors, sensor_map } = inner.wait().await;
                    info!("sensor_manager server is running");
                    while let Some(req) = stream.try_next().await? {
                        match req {
                            fthermal::SensorManagerRequest::ListSensors { responder } => {
                                responder.send(&sensors)?;
                            }
                            fthermal::SensorManagerRequest::SetTemperatureOverride {
                                name,
                                override_temperature,
                                responder,
                            } => {
                                let Some(node) = sensor_map.get(&name) else {
                                    warn!("Failed to find sensor with name '{}'", name);
                                    responder.send(Err(
                                        fthermal::SetTemperatureOverrideError::SensorNotFound,
                                    ))?;
                                    continue;
                                };

                                if let Err(e) = node
                                    .handle_message(&Message::SetTemperatureOverride(Celsius(
                                        override_temperature.into(),
                                    )))
                                    .await
                                {
                                    warn!("Failed to set temperature for sensor '{}': {:?}", name, e);
                                    responder
                                        .send(Err(fthermal::SetTemperatureOverrideError::Internal))?;
                                    continue;
                                }

                                responder.send(Ok(()))?;
                            }
                            fthermal::SensorManagerRequest::ClearTemperatureOverride {
                                name,
                                responder,
                            } => {
                                let Some(node) = sensor_map.get(&name) else {
                                    warn!("Failed to find sensor with name '{}'", name);
                                    responder.send(Err(
                                        fthermal::ClearTemperatureOverrideError::SensorNotFound,
                                    ))?;
                                    continue;
                                };

                                if let Err(e) =
                                    node.handle_message(&Message::ClearTemperatureOverride).await
                                {
                                    warn!(
                                        "Failed to clear temperature override for sensor '{}': {:?}",
                                        name, e
                                    );
                                    responder
                                        .send(Err(fthermal::ClearTemperatureOverrideError::Internal))?;
                                    continue;
                                }

                                responder.send(Ok(()))?;
                            }
                            fthermal::SensorManagerRequest::Connect { payload, responder } => {
                                let Some(name) = payload.name else {
                                    warn!("'name' is required when calling Connect");
                                    responder.send(Err(fthermal::ConnectError::InvalidArguments))?;
                                    continue;
                                };

                                let Some(server_end) = payload.server_end else {
                                    warn!("'server' is required when calling Connect");
                                    responder.send(Err(fthermal::ConnectError::InvalidArguments))?;
                                    continue;
                                };

                                let Some(node) = sensor_map.get(&name) else {
                                    warn!("Failed to find sensor with name '{}'", name);
                                    responder.send(Err(fthermal::ConnectError::SensorNotFound))?;
                                    continue;
                                };

                                inner_scope.spawn_local(run_sensor_proxy_server(node.clone(), name, server_end));
                                responder.send(Ok(()))?;
                            }
                            fthermal::SensorManagerRequest::_UnknownMethod {
                                method_type,
                                ordinal,
                                ..
                            } => {
                                warn!(method_type:?, ordinal; "Unknown SensorManagerRequest");
                            }
                        }
                    }
                    Ok(())
                }.unwrap_or_else(|e: anyhow::Error| error!("{:?}", e)));
            },
        );
    }
}

#[async_trait(?Send)]
impl Node for ThermalSensorManager {
    fn name(&self) -> String {
        "ThermalSensorManager".to_string()
    }

    async fn init(&self) -> Result<(), Error> {
        let mut sensor_map = HashMap::new();
        let mut sensors = Vec::new();
        for node in &self.temperature_handler_nodes {
            let sensor_name = match node.handle_message(&Message::GetSensorName).await {
                Ok(MessageReturn::GetSensorName(name)) => name,
                e => {
                    warn!("Failed to get sensor name for node: {:?}", e);
                    continue;
                }
            };

            sensor_map.insert(sensor_name.clone(), node.clone());
            sensors.push(fthermal::SensorInfo { name: Some(sensor_name), ..Default::default() });
        }

        match self.sensor_deps.set(SensorDeps { sensors, sensor_map }).await {
            Ok(_) => Ok(()),
            Err(_) => Err(anyhow::anyhow!("Failed to set ThermalSensorManager inner state")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::mock_node::{MessageMatcher, MockNodeMaker, create_dummy_node};
    use futures::StreamExt;

    /// Tests that well-formed configuration JSON does not panic the `new_from_json` function.
    #[fuchsia::test]
    fn test_new_from_json() {
        let json_data = json::json!({
            "type": "ThermalSensorManager",
            "name": "thermal_sensor_manager",
            "dependencies": {
                "temperature_handler_nodes": [
                    "soc_pll_thermal",
                    "gpu_thermal",
                ]
            }
        });

        let mut nodes: HashMap<String, Rc<dyn Node>> = HashMap::new();
        nodes.insert("soc_pll_thermal".to_string(), create_dummy_node());
        nodes.insert("gpu_thermal".to_string(), create_dummy_node());

        let _ = ThermalSensorManagerBuilder::new_from_json(
            json_data,
            &nodes,
            &mut ServiceFs::new_local(),
        );
    }

    /// Tests that the fuchsia.thermal.SensorManager server waits for node initialization before
    /// replying to clients.
    #[fuchsia::test]
    fn test_protocol_waits_on_init() {
        let mut mock_node_maker = MockNodeMaker::new();
        let fake_sensor_name = "temp_c_sensor";
        let mut executor = fasync::TestExecutor::new();

        let mock_node = mock_node_maker.make(
            fake_sensor_name,
            vec![(
                MessageMatcher::Eq(Message::GetSensorName),
                Ok(MessageReturn::GetSensorName(fake_sensor_name.to_string())),
            )],
        );

        let mut service_fs = ServiceFs::new_local();
        let thermal_sensor_manager = ThermalSensorManagerBuilder::new()
            .with_temperature_handler_nodes(vec![mock_node])
            .with_outgoing_svc_dir(service_fs.root_dir())
            .build()
            .unwrap();

        let connector = service_fs.create_protocol_connector().unwrap();
        fasync::Task::local(service_fs.collect()).detach();

        let sensor_manager_proxy =
            connector.connect_to_protocol::<fthermal::SensorManagerMarker>().unwrap();

        let mut list_sensor_fut = sensor_manager_proxy.list_sensors();
        assert!(executor.run_until_stalled(&mut list_sensor_fut).is_pending());

        executor.run_singlethreaded(thermal_sensor_manager.init()).unwrap();
        let sensors = executor.run_singlethreaded(list_sensor_fut).unwrap();

        assert_eq!(1, sensors.len());
        assert_eq!(Some(fake_sensor_name.to_string()), sensors[0].name);
    }

    #[fuchsia::test]
    #[allow(unused_variables)]
    #[allow(unreachable_code)]
    async fn test_calls_are_proxied_to_temperature_handler() {
        let mut mock_node_maker = MockNodeMaker::new();
        let fake_sensor_name = "temp_c_sensor";
        let fake_sensor_temp: f32 = 10.0;
        let override_temp: f32 = 65.0;

        let mock_node = mock_node_maker.make(
            fake_sensor_name,
            vec![
                (
                    MessageMatcher::Eq(Message::GetSensorName),
                    Ok(MessageReturn::GetSensorName(fake_sensor_name.to_string())),
                ),
                (
                    MessageMatcher::Eq(Message::ReadTemperature),
                    Ok(MessageReturn::ReadTemperature(Celsius(fake_sensor_temp.into()))),
                ),
                (
                    MessageMatcher::Eq(Message::SetTemperatureOverride(Celsius(
                        override_temp.into(),
                    ))),
                    Ok(MessageReturn::SetTemperatureOverride),
                ),
                (
                    MessageMatcher::Eq(Message::ReadTemperature),
                    Ok(MessageReturn::ReadTemperature(Celsius(override_temp.into()))),
                ),
            ],
        );

        let mut service_fs = ServiceFs::new_local();
        let thermal_sensor_manager = ThermalSensorManagerBuilder::new()
            .with_temperature_handler_nodes(vec![mock_node])
            .with_outgoing_svc_dir(service_fs.root_dir())
            .build()
            .unwrap();
        thermal_sensor_manager.init().await.unwrap();

        let connector = service_fs.create_protocol_connector().unwrap();
        let task = fasync::Task::local(service_fs.collect::<()>());

        // Scope connections to the fuchsia.thermal.SensorManager server so that spawned Tasks
        // complete before awaiting on no tasks.
        {
            let sensor_manager_proxy =
                connector.connect_to_protocol::<fthermal::SensorManagerMarker>().unwrap();

            let (sensor, sensor_server_end) = fidl::endpoints::create_proxy();
            sensor_manager_proxy
                .connect(fthermal::SensorManagerConnectRequest {
                    name: Some(fake_sensor_name.to_string()),
                    server_end: Some(fthermal::SensorServer_::Temperature(sensor_server_end)),
                    ..Default::default()
                })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(fake_sensor_temp, sensor.get_temperature_celsius().await.unwrap().1);

            sensor_manager_proxy
                .set_temperature_override(fake_sensor_name, override_temp.into())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(override_temp, sensor.get_temperature_celsius().await.unwrap().1);
        }

        // Tasks and connections must be explicitly aborted and dropped before MockNodeMaker goes
        // out of scope to prevent a panic from temp_c_sensor still being held by Tasks within
        // ThermalSensorManager's scope.
        thermal_sensor_manager.scope.on_no_tasks().await;
        task.abort().await;
    }
}
