// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::client::*;
use crate::playback::*;
use crate::sensor_update_sender::SensorUpdateSender;
use crate::service_watcher::*;
use crate::utils::*;
use anyhow::{Context as _, Error};
use fidl::endpoints::RequestStream;
use fidl_fuchsia_hardware_sensors::{
    PlaybackSourceConfig, {self as driver_fidl},
};
use fidl_fuchsia_sensors::*;
use fidl_fuchsia_sensors_types::*;
use fuchsia_component::client as fclient;
use fuchsia_component::server::ServiceFs;
use futures::lock::Mutex;
use futures_util::{StreamExt, TryStreamExt};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub type SensorId = i32;

#[derive(Debug, Clone)]
pub struct SensorManager {
    pub(crate) sensors: HashMap<SensorId, Sensor>,
    driver_proxies: Vec<driver_fidl::DriverProxy>,
    playback: Option<Playback>,
    clients: HashSet<Client>,
    pub(crate) update_sender: SensorUpdateSender,
}

#[derive(Debug, Clone)]
pub struct Sensor {
    pub(crate) driver: driver_fidl::DriverProxy,
    pub(crate) info: SensorInfo,
    // A subset of SensorManager::clients.
    pub(crate) clients: HashSet<Client>,
}

enum IncomingRequest {
    SensorManager(ManagerRequestStream),
}

async fn handle_sensors_request(
    request: ManagerRequest,
    manager: &Arc<Mutex<SensorManager>>,
    client: &Client,
) -> anyhow::Result<()> {
    let mut manager = manager.lock().await;
    match request {
        ManagerRequest::GetSensorsList { responder } => {
            let _ = responder.send(&manager.get_sensors_list().await);
        }
        ManagerRequest::ConfigureSensorRates { id, sensor_rate_config, responder } => {
            let _ = responder.send(manager.configure_sensor_rates(id, sensor_rate_config).await);
        }
        ManagerRequest::Activate { id, responder } => {
            let _ = responder.send(manager.activate(id, client.clone()).await);
        }
        ManagerRequest::Deactivate { id, responder } => {
            let _ = responder.send(manager.deactivate(id, client).await);
        }
        ManagerRequest::ConfigurePlayback { source_config, responder } => {
            let _ = responder.send(manager.configure_playback(source_config).await);
        }
        ManagerRequest::_UnknownMethod { ordinal, .. } => {
            log::warn!("ManagerRequest::_UnknownMethod with ordinal {}", ordinal);
        }
    }

    manager.update_sender.update_sensor_map(manager.sensors.clone()).await;

    Ok(())
}

async fn handle_sensor_manager_request_stream(
    mut stream: ManagerRequestStream,
    manager: Arc<Mutex<SensorManager>>,
    client: Client,
) -> Result<(), Error> {
    while let Some(request) =
        stream.try_next().await.context("Error handling SensorManager events")?
    {
        handle_sensors_request(request, &manager, &client)
            .await
            .expect("Error handling sensor request");
    }
    Ok(())
}

impl SensorManager {
    pub fn new(update_sender: SensorUpdateSender, playback: Option<Playback>) -> Self {
        let sensors = HashMap::new();
        let clients = HashSet::new();
        let driver_proxies = Vec::new();

        Self { sensors, driver_proxies, playback, clients, update_sender }
    }

    async fn get_sensors_list(&mut self) -> Vec<SensorInfo> {
        self.populate_sensors().await;
        let fidl_sensors: Vec<SensorInfo> =
            self.sensors.values().into_iter().map(|x| x.info.clone()).collect();
        if fidl_sensors.is_empty() {
            log::warn!("Failed to get any sensors from driver. Sending empty list");
        }

        fidl_sensors
    }

    async fn activate(&mut self, id: SensorId, client: Client) -> Result<(), ActivateSensorError> {
        if let Some(sensor) = self.sensors.get_mut(&id) {
            let res = sensor.driver.activate_sensor(id).await;
            if let Err(e) = res {
                log::warn!("Error while activating sensor: {:#?}", e);
                Err(ActivateSensorError::DriverUnavailable)
            } else {
                sensor.clients.insert(client);
                Ok(())
            }
        } else {
            log::warn!("Received request to activate unknown sensor id: {}", id);
            Err(ActivateSensorError::InvalidSensorId)
        }
    }

    async fn deactivate(
        &mut self,
        id: SensorId,
        client: &Client,
    ) -> Result<(), DeactivateSensorError> {
        let mut response: Result<(), DeactivateSensorError> = Ok(());
        if let Some(sensor) = self.sensors.get_mut(&id) {
            // If this is the last subscriber for this sensor, deactivate it.
            if sensor.clients.len() == 1 {
                if let Err(e) = sensor.driver.deactivate_sensor(id).await {
                    log::warn!("Error while deactivating sensor: {:#?}", e);
                    response = Err(DeactivateSensorError::DriverUnavailable);
                }
            } else {
                if !sensor.clients.is_empty() {
                    log::info!(
                        "Unsubscribing client from sensor {:#?}, but there are other subscribers.",
                        id,
                    );
                }
            }
            sensor.clients.remove(&client);
        } else {
            log::warn!("Received request to deactivate unknown sensor id: {}", id);
            response = Err(DeactivateSensorError::InvalidSensorId);
        }

        response
    }

    async fn configure_sensor_rates(
        &mut self,
        id: SensorId,
        sensor_rate_config: SensorRateConfig,
    ) -> Result<(), ConfigureSensorRateError> {
        if let Some(sensor) = self.sensors.get(&id) {
            match sensor.driver.configure_sensor_rate(id, &sensor_rate_config).await {
                Ok(Ok(())) => Ok(()),
                Ok(Err(driver_fidl::ConfigureSensorRateError::InvalidSensorId)) => {
                    log::warn!(
                        "Received ConfigureSensorRates request for unknown sensor id: {}",
                        id
                    );
                    Err(ConfigureSensorRateError::InvalidSensorId)
                }
                Ok(Err(driver_fidl::ConfigureSensorRateError::InvalidConfig)) => {
                    log::warn!(
                        "Received ConfigureSensorRates request for invalid config: {:#?}",
                        sensor_rate_config
                    );
                    Err(ConfigureSensorRateError::InvalidConfig)
                }
                Err(e) => {
                    log::warn!("Error while configuring sensor rates: {:#?}", e);
                    Err(ConfigureSensorRateError::DriverUnavailable)
                }
                Ok(Err(_)) => unreachable!(),
            }
        } else {
            log::warn!("Received ConfigureSensorRates request for unknown sensor id: {}", id);
            Err(ConfigureSensorRateError::InvalidSensorId)
        }
    }

    async fn configure_playback(
        &mut self,
        source_config: PlaybackSourceConfig,
    ) -> Result<(), ConfigurePlaybackError> {
        let mut response: Result<(), ConfigurePlaybackError> = Ok(());

        if let Some(mut playback) = self.playback.clone() {
            let res = playback.playback_proxy.configure_playback(&source_config).await;

            match res {
                Ok(Ok(())) => {
                    self.sensors.extend(playback.get_sensors_from_config(source_config).await);

                    // Don't add the playback driver proxy if playback was previously
                    // configured.
                    if !self.driver_proxies.iter().any(|x| playback.is_playback_driver_proxy(x)) {
                        self.driver_proxies.push(playback.driver_proxy.clone());
                    }
                    response = Ok(());
                }
                Err(e) => {
                    log::warn!("Error while configuring sensor playback: {:#?}", e);
                    response = Err(ConfigurePlaybackError::PlaybackUnavailable);
                }
                Ok(Err(e)) => {
                    response = Err(from_driver_playback_error(e));
                }
            }

            if !response.is_ok() {
                // Remove the playback driver proxy and its sensors.
                self.driver_proxies.retain(|x| !playback.is_playback_driver_proxy(&x));
                self.sensors.retain(|id, _| !playback.playback_sensor_ids.contains(id));
                playback.playback_sensor_ids.clear();
            } else {
                playback.configured = response.is_ok();
                self.playback = Some(playback);
            }
        }

        response
    }

    pub(crate) async fn add_instance(&mut self, driver_proxy: driver_fidl::DriverProxy) {
        let event_stream = driver_proxy.take_event_stream();
        self.driver_proxies.push(driver_proxy);
        self.populate_sensors().await;
        self.update_sender.update_sensor_map(self.sensors.clone()).await;
        self.update_sender.add_event_stream(event_stream).await;
    }

    async fn populate_sensors(&mut self) {
        let mut sensors = HashMap::new();
        for proxy in self.driver_proxies.clone() {
            if let Ok(driver_sensors) = proxy.get_sensors_list().await {
                for sensor in driver_sensors {
                    if is_sensor_valid(&sensor) {
                        let id = sensor.sensor_id.expect("sensor_id");
                        let mut clients: HashSet<Client> = HashSet::new();
                        if let Some(sensor) = self.sensors.get(&id) {
                            clients = sensor.clients.clone();
                        }

                        sensors.insert(id, Sensor { driver: proxy.clone(), info: sensor, clients });
                    }
                }
            }
        }

        self.sensors = sensors;
    }

    async fn start_service_watcher(&self, manager: Arc<Mutex<SensorManager>>) -> Result<(), Error> {
        let svc = fclient::Service::open(driver_fidl::ServiceMarker)?;
        // Attempt to watch the service directory. If this fails, the manager will check if
        // playback is configured and exit early if it is not.
        svc.watch().await?;

        let svc = fclient::Service::open(driver_fidl::ServiceMarker)?;
        fuchsia_async::Task::local(async move {
            if let Err(e) = watch_service_directory(svc, manager).await {
                log::error!("Failed to open sensor service! Error: {:#?}", e);
            }
        })
        .detach();

        Ok(())
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        // Collect all the driver event streams into a set of futures that will be polled when the
        // futures contain a sensor event.
        if let Some(playback) = &self.playback {
            self.add_instance(playback.driver_proxy.clone()).await;
        }

        let manager: Arc<Mutex<SensorManager>> = Arc::new(Mutex::new(self.clone()));

        if let Err(_) = self.start_service_watcher(manager.clone()).await {
            if self.playback.is_some() {
                log::warn!("Failed to open sensor driver service directory. Starting with playback sensors only.");
            } else {
                return Err(anyhow::anyhow!(
                    "Failed to open sensors service and sensor playback is not enabled on the system"
                ));
            }
        }

        let mut fs = ServiceFs::new_local();
        fs.dir("svc").add_fidl_service(IncomingRequest::SensorManager);
        fs.take_and_serve_directory_handle()?;
        fs.for_each_concurrent(None, move |request: IncomingRequest| {
            let manager = manager.clone();
            async move {
                match request {
                    IncomingRequest::SensorManager(stream) => {
                        let client = Client::new(stream.control_handle());
                        manager.lock().await.clients.insert(client.clone());
                        handle_sensor_manager_request_stream(stream, manager, client)
                            .await
                            .expect("Failed to serve sensor requests");
                    }
                }
            }
        })
        .await;

        Err(anyhow::anyhow!("SensorManager completed unexpectedly."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::*;
    use fidl_fuchsia_hardware_sensors::*;
    use futures::channel::mpsc;

    #[fuchsia::test]
    async fn test_invalid_configure_playback() {
        // Creates an invalid playback_proxy so that ConfigurePlayback gets PEER_CLOSED when trying
        // to make a request.
        let (playback_proxy, _) = create_proxy::<PlaybackMarker>();

        let (driver_proxy, _) = create_proxy::<DriverMarker>();
        let (sender, _receiver) = mpsc::channel(100);
        let sender = SensorUpdateSender::new(Arc::new(Mutex::new(sender)));
        let sm = SensorManager::new(sender, Some(Playback::new(driver_proxy, playback_proxy)));

        let manager = Arc::new(Mutex::new(sm));
        let (proxy, stream) = create_proxy_and_stream::<ManagerMarker>();
        let client = Client::new(stream.control_handle().clone());
        fuchsia_async::Task::spawn(async move {
            manager.lock().await.clients.insert(client.clone());
            handle_sensor_manager_request_stream(stream, manager, client)
                .await
                .expect("Failed to process request stream");
        })
        .detach();

        let res = proxy
            .configure_playback(&PlaybackSourceConfig::FixedValuesConfig(
                FixedValuesPlaybackConfig {
                    sensor_list: None,
                    sensor_events: None,
                    ..Default::default()
                },
            ))
            .await;
        assert_eq!(
            res.unwrap(),
            Err(fidl_fuchsia_sensors::ConfigurePlaybackError::PlaybackUnavailable)
        );
    }
}
