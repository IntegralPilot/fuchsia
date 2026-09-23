// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::StreamExt;

use async_utils::hanging_get::client::HangingGetStream;

use fidl_fuchsia_hardware_usb_policy as fpolicy;
use fpolicy::{ControllerProxy, DeviceState};

use fuchsia_async::{MonotonicInstant, Timer};
use futures::channel::mpsc as fmpsc;
use zx::MonotonicDuration;

use fuchsia_inspect::Node;
use fuchsia_inspect_contrib::inspect_log;
use fuchsia_inspect_contrib::nodes::BoundedListNode;
use log::{error, info};
use std::sync::Mutex;

fn device_state_to_str(state: DeviceState) -> &'static str {
    match state {
        DeviceState::NotAttached => "NotAttached",
        DeviceState::Attached => "Attached",
        DeviceState::Powered => "Powered",
        DeviceState::Default => "Default",
        DeviceState::Address => "Address",
        DeviceState::Configured => "Configured",
        DeviceState::Suspended => "Suspended",
        _ => "Unknown",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsbState {
    pub device_state: DeviceState,
    pub address: u8,
}

struct InnerControllerState {
    usb_state: UsbState,
    watchers: Vec<fmpsc::UnboundedSender<UsbState>>,
}

pub struct ControllerState {
    proxy: ControllerProxy,
    inner: Mutex<InnerControllerState>,
    inspect_history: Mutex<BoundedListNode>,
}

impl ControllerState {
    const HISTORY_CAPACITY: usize = 50;

    pub fn new(
        proxy: ControllerProxy,
        device_state: DeviceState, // initial state value
        address: u8,
        inspect_node: Node,
    ) -> Self {
        let mut inspect_history = BoundedListNode::new(inspect_node, Self::HISTORY_CAPACITY);
        inspect_log!(inspect_history, state: device_state_to_str(device_state), address: address as u64);
        Self {
            proxy,
            inner: Mutex::new(InnerControllerState {
                usb_state: UsbState { device_state, address },
                watchers: Vec::new(),
            }),
            inspect_history: Mutex::new(inspect_history),
        }
    }

    pub fn subscribe(&self) -> (UsbState, fmpsc::UnboundedReceiver<UsbState>) {
        let (tx, rx) = fmpsc::unbounded();
        let state = {
            let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.watchers.push(tx);
            inner.usb_state
        };
        (state, rx)
    }

    pub async fn monitor_device_state(&self) -> anyhow::Result<()> {
        // Clone the proxy to give the stream its own handle.
        let stream_proxy = self.proxy.clone();

        let mut stream = HangingGetStream::new(stream_proxy, |p| p.watch_device_state());

        while let Some(result) = stream.next().await {
            match result {
                Ok(Ok(update)) => {
                    let new_state = update.state.unwrap_or_else(DeviceState::unknown);
                    let new_address = update.address.unwrap_or(0);
                    info!("Received new state: {:?} address: {}", new_state, new_address);

                    {
                        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        inner.usb_state.device_state = new_state;
                        inner.usb_state.address = new_address;

                        inner.watchers.retain(|tx| {
                            tx.unbounded_send(UsbState {
                                device_state: new_state,
                                address: new_address,
                            })
                            .is_ok()
                        });
                    }

                    {
                        let mut history =
                            self.inspect_history.lock().unwrap_or_else(|e| e.into_inner());
                        inspect_log!(history, state: device_state_to_str(new_state), address: new_address as u64);
                    }

                    info!("State updated, waiting for next...");
                }
                Ok(Err(e)) => {
                    error!("Monitor loop failed! System Error: {:?}", e);
                    Timer::new(MonotonicInstant::after(MonotonicDuration::from_seconds(1))).await;
                }
                Err(e) => {
                    error!("Monitor loop failed! FIDL Error: {:?}", e);

                    Timer::new(MonotonicInstant::after(MonotonicDuration::from_seconds(1))).await;
                }
            }
        }
        error!("monitor_device_state() loop exited because the server closed the channel.");
        Ok(())
    }

    pub fn get_state(&self) -> UsbState {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.usb_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fuchsia_inspect::Inspector;
    use std::sync::Arc;

    #[test]
    fn test_device_state_to_str_mapping() {
        assert_eq!(device_state_to_str(DeviceState::NotAttached), "NotAttached");
        assert_eq!(device_state_to_str(DeviceState::Attached), "Attached");
        assert_eq!(device_state_to_str(DeviceState::Powered), "Powered");
        assert_eq!(device_state_to_str(DeviceState::Default), "Default");
        assert_eq!(device_state_to_str(DeviceState::Address), "Address");
        assert_eq!(device_state_to_str(DeviceState::Configured), "Configured");
        assert_eq!(device_state_to_str(DeviceState::Suspended), "Suspended");
        assert_eq!(device_state_to_str(DeviceState::unknown()), "Unknown");
    }

    #[fuchsia::test]
    async fn test_controller_inspect_initialization() {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("usb_state_history");

        let (controller_proxy, _) =
            fidl::endpoints::create_proxy_and_stream::<fpolicy::ControllerMarker>();

        // Initialize controller
        let _controller =
            ControllerState::new(controller_proxy, DeviceState::Attached, 42, inspect_node);

        // Verify the initial entry was logged
        assert_data_tree!(inspector, root: {
            usb_state_history: {
                "0": {
                    "@time": AnyProperty,
                    state: "Attached",
                    address: 42u64,
                }
            }
        });
    }

    #[fuchsia::test]
    async fn test_controller_inspect_series() -> anyhow::Result<()> {
        let inspector = Inspector::default();
        let inspect_node = inspector.root().create_child("usb_state_history");

        let (controller_proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<fpolicy::ControllerMarker>();

        let controller = Arc::new(ControllerState::new(
            controller_proxy,
            DeviceState::NotAttached,
            0,
            inspect_node,
        ));

        let (initial_state, mut state_rx) = controller.subscribe();
        assert_eq!(initial_state.device_state, DeviceState::NotAttached);
        assert_eq!(initial_state.address, 0);

        let controller_clone = controller.clone();
        let _task = fuchsia_async::Task::local(async move {
            let _ = controller_clone.monitor_device_state().await;
        });

        // Simulate server responses
        for i in 1..=2 {
            if let Some(Ok(request)) = request_stream.next().await {
                match request {
                    fpolicy::ControllerRequest::WatchDeviceState { responder } => {
                        let (state, address) = if i == 1 {
                            (DeviceState::Attached, 42)
                        } else {
                            (DeviceState::Configured, 42)
                        };
                        responder.send(Ok(&fpolicy::DeviceStateUpdate {
                            state: Some(state),
                            address: Some(address),
                            ..Default::default()
                        }))?;
                    }
                    _ => panic!("Unexpected request"),
                }
            }
        }

        // Wait for and verify the expected state transitions in order.
        let state1 = match state_rx.next().await {
            Some(s) => s,
            None => return Err(anyhow::Error::msg("Stream closed before receiving Attached")),
        };
        assert_eq!(state1.device_state, DeviceState::Attached);
        assert_eq!(state1.address, 42);

        let state2 = match state_rx.next().await {
            Some(s) => s,
            None => return Err(anyhow::Error::msg("Stream closed before receiving Configured")),
        };
        assert_eq!(state2.device_state, DeviceState::Configured);
        assert_eq!(state2.address, 42);

        // Verify the entries
        assert_data_tree!(inspector, root: {
            usb_state_history: {
                "0": {
                    "@time": AnyProperty,
                    state: "NotAttached",
                    address: 0u64,
                },
                "1": {
                    "@time": AnyProperty,
                    state: "Attached",
                    address: 42u64,
                },
                "2": {
                    "@time": AnyProperty,
                    state: "Configured",
                    address: 42u64,
                }
            }
        });
        Ok(())
    }
}
