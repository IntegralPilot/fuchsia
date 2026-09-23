// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::client::Client;
use crate::sensor_manager::*;
use fidl::endpoints::ControlHandle;
use fidl_fuchsia_hardware_sensors::{DriverEvent, DriverEventStream};
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::select;
use futures::stream::{FuturesUnordered, StreamFuture};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct SensorUpdateSender {
    sender: Arc<Mutex<mpsc::Sender<SensorUpdate>>>,
}

pub enum SensorUpdate {
    SensorMap(HashMap<SensorId, Sensor>),
    EventStream(DriverEventStream),
}

// Sends all clients of a sensor an event if event contains a DriverEvent. If the channel to the
// client has closed, it will remove that client from the list of subscribers for that particular
// sensor.
//
// Returns whether or not to continue polling the event stream that generated this sensor event.
fn handle_sensor_event(
    sensors: &mut HashMap<SensorId, Sensor>,
    sensor_event: Option<Result<DriverEvent, fidl::Error>>,
) -> bool {
    let mut should_send_more_events = true;
    match sensor_event {
        Some(Ok(DriverEvent::OnSensorEvent { event })) => {
            if let Some(sensor) = sensors.get_mut(&event.sensor_id) {
                let mut clients_to_remove: Vec<Client> = Vec::new();
                for client in &sensor.clients {
                    if !client.control_handle.is_closed() {
                        if let Err(e) = client.control_handle.send_on_sensor_event(&event) {
                            log::warn!("Failed to send sensor event: {:#?}", e);
                        }
                    } else {
                        log::error!("Client was PEER_CLOSED! Removing from clients list");
                        clients_to_remove.push(client.clone());
                    }
                }
                for client in clients_to_remove {
                    sensor.clients.remove(&client);
                }
            }
        }
        Some(Ok(DriverEvent::_UnknownEvent { ordinal, .. })) => {
            log::warn!("SensorManager received an UnknownEvent with ordinal: {:#?}", ordinal);
        }
        Some(Err(e)) => {
            log::error!("Received an error from sensor driver: {:#?}", e);
            should_send_more_events = false;
        }
        None => {
            log::error!("Got None from driver");
            should_send_more_events = false;
        }
    }

    should_send_more_events
}

// Handles the stream of sensor events from all drivers. Receives updates about sensor information
// and new sensors from a channel to the SensorManager.
pub async fn handle_sensor_event_streams(mut update_receiver: mpsc::Receiver<SensorUpdate>) {
    let mut event_streams: FuturesUnordered<StreamFuture<DriverEventStream>> =
        FuturesUnordered::new();
    let mut sensors: HashMap<SensorId, Sensor> = HashMap::new();
    loop {
        select! {
            sensor_event = event_streams.next() => {
                if let Some((event, stream)) = sensor_event {
                    if handle_sensor_event(&mut sensors, event) {
                        // Once the future has resolved, the rest of the events need to be
                        // placed back onto the list of futures.
                        event_streams.push(stream.into_future());
                    }
                }
            },
            sensor_update = update_receiver.next() => {
                match sensor_update {
                    Some(SensorUpdate::SensorMap(updated_sensors)) => {
                        sensors = updated_sensors;
                    },
                    Some(SensorUpdate::EventStream(stream)) => {
                        event_streams.push(stream.into_future());
                    }
                    None => {
                        log::error!("Channel has hung up! Will no longer receive sensor updates.");
                    }
                }
            },
        }
    }
}

impl SensorUpdateSender {
    pub fn new(sender: Arc<Mutex<mpsc::Sender<SensorUpdate>>>) -> Self {
        SensorUpdateSender { sender }
    }

    async fn send_update(&self, update: SensorUpdate) {
        if let Err(e) = self.sender.lock().await.try_send(update) {
            log::warn!("Failed to send sensor update! {:#?}", e);
        }
    }

    pub(crate) async fn update_sensor_map(&self, sensors: HashMap<SensorId, Sensor>) {
        self.send_update(SensorUpdate::SensorMap(sensors)).await
    }

    pub(crate) async fn add_event_stream(&self, event_stream: DriverEventStream) {
        self.send_update(SensorUpdate::EventStream(event_stream)).await
    }
}
