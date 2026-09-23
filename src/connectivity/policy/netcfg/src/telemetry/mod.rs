// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod processors;

use crate::telemetry::processors::network_properties::NetworkPropertiesProcessor;
use anyhow::Error;
use fidl_fuchsia_net_policy_socketproxy as fnp_socketproxy;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::{Future, StreamExt};
use log::{info, warn};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub struct NetworkEventMetadata {
    pub id: u64,
    pub name: Option<String>,
    pub transport: fnp_socketproxy::NetworkType,
    pub is_fuchsia_provisioned: bool,
    pub connectivity_state: Option<fnp_socketproxy::ConnectivityState>,
}

#[derive(Debug)]
pub enum TelemetryEvent {
    DefaultNetworkChanged(NetworkEventMetadata),
    DefaultNetworkLost,
    NetworkChanged(NetworkEventMetadata),
}

#[derive(Clone, Debug)]
pub struct TelemetrySender {
    sender: Arc<Mutex<mpsc::Sender<TelemetryEvent>>>,
    sender_is_blocked: Arc<AtomicBool>,
}

impl TelemetrySender {
    pub fn new(sender: mpsc::Sender<TelemetryEvent>) -> Self {
        Self {
            sender: Arc::new(Mutex::new(sender)),
            sender_is_blocked: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn send(&self, event: TelemetryEvent) {
        match self.sender.lock().try_send(event) {
            Ok(_) => {
                if self
                    .sender_is_blocked
                    .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    info!("TelemetrySender recovered and resumed sending");
                }
            }
            Err(_) => {
                if self
                    .sender_is_blocked
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    warn!(
                        "TelemetrySender dropped a msg: either buffer is full or no receiver is waiting"
                    );
                }
            }
        }
    }
}

const TELEMETRY_EVENT_BUFFER_SIZE: usize = 100;

pub fn serve_telemetry(
    telemetry_node: fuchsia_inspect::Node,
    telemetry_path: &str,
) -> (TelemetrySender, impl Future<Output = Result<(), Error>> + use<>) {
    let time_series_node = telemetry_node.create_child("time_series");
    let client =
        windowed_stats::experimental::inspect::TimeMatrixClient::new(time_series_node.clone_weak());

    let processor = NetworkPropertiesProcessor::new(&telemetry_node, telemetry_path, &client);
    telemetry_node.record(time_series_node);

    let (sender, mut receiver) = mpsc::channel::<TelemetryEvent>(TELEMETRY_EVENT_BUFFER_SIZE);
    let sender = TelemetrySender::new(sender);

    let fut = async move {
        let mut processor = processor;
        while let Some(event) = receiver.next().await {
            match event {
                TelemetryEvent::DefaultNetworkChanged(metadata) => {
                    processor.log_default_network_changed(metadata);
                }
                TelemetryEvent::DefaultNetworkLost => {
                    processor.log_default_network_lost();
                }
                TelemetryEvent::NetworkChanged(metadata) => {
                    processor.log_network_changed(metadata, &client);
                }
            }
        }
        Ok(())
    };
    (sender, fut)
}
