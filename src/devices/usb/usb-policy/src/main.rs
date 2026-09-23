// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "1024"]
// Turn on additional lints that could lead to unexpected crashes in production code
#![warn(clippy::indexing_slicing)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
#![warn(clippy::unreachable)]
#![warn(clippy::unimplemented)]

use anyhow::{Error, format_err};
use fidl_fuchsia_hardware_usb_peripheral as peripheral;
use fidl_fuchsia_hardware_usb_policy as fpolicy;
use fidl_fuchsia_hwinfo as hwinfo;
use fidl_fuchsia_usb_policy as usb_policy;
use fuchsia_component::client::Service;

use fidl_fuchsia_hardware_usb_policy::DeviceState;
use fuchsia_async::TimeoutExt as _;

use futures::{FutureExt, StreamExt};

use log::warn;
use std::sync::Arc;
mod controller;

use fuchsia_component::server::ServiceFs;

const DEFAULT_SERIAL_NUMBER: &str = "12345678";
const GET_CONFIG_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_millis(500);

const USB_CLASS_COMMUNICATIONS: u8 = 2;
const USB_CLASS_CDC_DATA: u8 = 10;
const USB_CLASS_VENDOR_SPECIFIC: u8 = 255;
const USB_SUBCLASS_ADB: u8 = 0x42;
const USB_PROTOCOL_ADB: u8 = 0x01;
const USB_SUBCLASS_VSOCK: u8 = 0x43;
const USB_PROTOCOL_VSOCK: u8 = 0x00;

/// State shared between the background discovery task and the FIDL server instances.
///
/// It holds the active USB controller state (once discovered) and a list of waiters
/// that need to be notified as soon as the controller becomes available.
struct UsbPolicySharedStateInner {
    /// The active controller state, if discovered.
    controller: Option<Arc<controller::ControllerState>>,

    /// Senders to notify tasks waiting for the controller to become available.
    waiters: Vec<futures::channel::oneshot::Sender<()>>,
}

/// A thread-safe wrapper around `UsbPolicySharedStateInner` that encapsulates locking.
struct UsbPolicySharedState {
    inner: std::sync::Mutex<UsbPolicySharedStateInner>,
    serial_number: String,
}

impl UsbPolicySharedState {
    pub fn new() -> Self {
        Self::new_with_serial(DEFAULT_SERIAL_NUMBER.to_string())
    }

    pub fn new_with_serial(serial_number: String) -> Self {
        Self {
            inner: std::sync::Mutex::new(UsbPolicySharedStateInner {
                controller: None,
                waiters: Vec::new(),
            }),
            serial_number,
        }
    }

    pub fn get_serial_number(&self) -> String {
        self.serial_number.clone()
    }

    pub fn set_controller(&self, state: Arc<controller::ControllerState>) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.controller = Some(state);
        for sender in inner.waiters.drain(..) {
            let _ = sender.send(());
        }
    }

    pub fn get_controller(&self) -> Option<Arc<controller::ControllerState>> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.controller.clone()
    }

    pub async fn wait_for_controller(&self) -> Arc<controller::ControllerState> {
        loop {
            let rx = {
                let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(controller) = &inner.controller {
                    return controller.clone();
                }
                let (sender, receiver) = futures::channel::oneshot::channel();
                inner.waiters.push(sender);
                receiver
            };
            let _ = rx.await;
        }
    }
}

impl Default for UsbPolicySharedState {
    fn default() -> Self {
        Self::new()
    }
}

enum IncomingRequest {
    Health(usb_policy::HealthRequestStream),
    Provider(usb_policy::PolicyProviderRequestStream),
    Configuration(usb_policy::ConfigurationRequestStream),
}

async fn get_peripheral_device() -> Result<peripheral::DeviceProxy, anyhow::Error> {
    let service = Service::open(peripheral::ServiceMarker)?;
    let instance = service.watch_for_any().await?;
    let device = instance.connect_to_device()?;
    Ok(device)
}

async fn fetch_serial_number() -> String {
    let Ok(device_info_proxy) =
        fuchsia_component::client::connect_to_protocol::<hwinfo::DeviceMarker>()
    else {
        warn!(
            "Failed to connect to fuchsia.hwinfo.Device; using default serial {DEFAULT_SERIAL_NUMBER}"
        );
        return DEFAULT_SERIAL_NUMBER.to_string();
    };

    match device_info_proxy.get_info().await {
        Ok(info) => {
            if let Some(serial) = info.serial_number {
                if !serial.is_empty() {
                    serial
                } else {
                    warn!(
                        "fuchsia.hwinfo.Device returned empty serial; using default serial {DEFAULT_SERIAL_NUMBER}"
                    );
                    DEFAULT_SERIAL_NUMBER.to_string()
                }
            } else {
                warn!(
                    "fuchsia.hwinfo.Device returned no serial; using default serial {DEFAULT_SERIAL_NUMBER}"
                );
                DEFAULT_SERIAL_NUMBER.to_string()
            }
        }
        Err(e) => {
            warn!(
                "Failed to get info from fuchsia.hwinfo.Device: {:?}; using default serial {DEFAULT_SERIAL_NUMBER}",
                e
            );
            DEFAULT_SERIAL_NUMBER.to_string()
        }
    }
}

async fn handle_get_configuration()
-> Result<(peripheral::DeviceDescriptor, Vec<Vec<peripheral::FunctionDescriptor>>), zx::Status> {
    let get_config_fut = async {
        let device = get_peripheral_device().await.map_err(|_| zx::Status::UNAVAILABLE)?;
        let (device_desc, config_descriptors) = device
            .get_configuration()
            .await
            .map_err(|_| zx::Status::INTERNAL)?
            .map_err(zx::Status::err_from_raw)?;
        Ok((device_desc, config_descriptors))
    };

    get_config_fut
        .on_timeout(zx::MonotonicInstant::after(GET_CONFIG_TIMEOUT), || Err(zx::Status::TIMED_OUT))
        .await
}

async fn handle_set_configuration(
    mut device_desc: peripheral::DeviceDescriptor,
    config_descriptors: Vec<Vec<peripheral::FunctionDescriptor>>,
    serial_number: String,
) -> Result<(), zx::Status> {
    let device = get_peripheral_device().await.map_err(|_| zx::Status::UNAVAILABLE)?;
    if device_desc.serial.is_empty() {
        device_desc.serial = serial_number;
    }
    device.clear_functions().await.map_err(|_| zx::Status::INTERNAL)?;
    device
        .set_configuration(&device_desc, &config_descriptors)
        .await
        .map_err(|_| zx::Status::INTERNAL)?
        .map_err(zx::Status::err_from_raw)?;
    Ok(())
}

async fn run_configuration_server(
    mut stream: usb_policy::ConfigurationRequestStream,
    shared_state: Arc<UsbPolicySharedState>,
) {
    while let Some(request_result) = stream.next().await {
        match request_result {
            Ok(request) => match request {
                usb_policy::ConfigurationRequest::GetConfiguration { responder } => {
                    match handle_get_configuration().await {
                        Ok((device_desc, config_descriptors)) => {
                            if let Err(e) = responder.send(Ok((&device_desc, &config_descriptors)))
                            {
                                warn!("Failed to send GetConfiguration response: {:?}", e);
                            }
                        }
                        Err(status) => {
                            if let Err(e) = responder.send(Err(status.into_raw())) {
                                warn!("Failed to send GetConfiguration error: {:?}", e);
                            }
                        }
                    }
                }
                usb_policy::ConfigurationRequest::SetConfiguration {
                    device_desc,
                    config_descriptors,
                    responder,
                } => match handle_set_configuration(
                    device_desc,
                    config_descriptors,
                    shared_state.get_serial_number(),
                )
                .await
                {
                    Ok(()) => {
                        if let Err(e) = responder.send(Ok(())) {
                            warn!("Failed to send SetConfiguration response: {:?}", e);
                        }
                    }
                    Err(status) => {
                        if let Err(e) = responder.send(Err(status.into_raw())) {
                            warn!("Failed to send SetConfiguration error: {:?}", e);
                        }
                    }
                },
                usb_policy::ConfigurationRequest::_UnknownMethod { .. } => {
                    warn!("Unknown Configuration request");
                }
            },
            Err(e) => {
                warn!("ConfigurationRequestStream error: {:?}", e);
                break;
            }
        }
    }
}

async fn run_provider_server(
    mut stream: usb_policy::PolicyProviderRequestStream,
    shared_state: Arc<UsbPolicySharedState>,
) {
    let state = shared_state.wait_for_controller().await;
    let (initial_state, mut rx) = state.subscribe();
    let mut current_state = initial_state;
    // We haven't sent anything to the client yet, so the first WatchDeviceState
    // should get `current_state`.
    let mut state_changed = true;

    while let Some(request_result) = stream.next().await {
        match request_result {
            Ok(request) => match request {
                usb_policy::PolicyProviderRequest::WatchDeviceState { responder } => {
                    if !state_changed {
                        // Wait until we get an update from rx
                        if let Some(new_state) = rx.next().await {
                            current_state = new_state;
                            state_changed = true;

                            // Drain any additional buffered states
                            while let Some(Some(latest_state)) = rx.next().now_or_never() {
                                current_state = latest_state;
                            }
                        } else {
                            // The sender was dropped, meaning the controller is gone.
                            break;
                        }
                    }

                    if state_changed {
                        let update = fpolicy::DeviceStateUpdate {
                            state: Some(current_state.device_state),
                            address: Some(current_state.address),
                            ..Default::default()
                        };
                        if let Err(e) = responder.send(Ok(&update)) {
                            warn!("Failed to send PolicyProvider response: {:?}", e);
                        }
                        state_changed = false;
                    }
                }
                usb_policy::PolicyProviderRequest::_UnknownMethod { .. } => {
                    warn!("Unknown PolicyProvider request");
                }
            },
            Err(e) => {
                warn!("PolicyProviderRequestStream error: {:?}", e);
                break;
            }
        }
    }
}

fn collate_function_health(
    config_descriptors: &[Vec<peripheral::FunctionDescriptor>],
    is_attached: bool,
    is_configured: bool,
) -> (usb_policy::AdbHealth, usb_policy::CdcEthernetHealth, usb_policy::VsockHealth) {
    let mut adb_configured = false;
    let mut cdc_configured = false;
    let mut vsock_configured = false;

    // TODO(https://fxbug.dev/547909518): Currently inspects all configurations; when multi-config
    // peripheral support is added, restrict this check to the currently active configuration.
    for config in config_descriptors {
        for func in config {
            match func.interface_class {
                USB_CLASS_COMMUNICATIONS | USB_CLASS_CDC_DATA => cdc_configured = true,
                USB_CLASS_VENDOR_SPECIFIC => {
                    if func.interface_subclass == USB_SUBCLASS_ADB
                        && func.interface_protocol == USB_PROTOCOL_ADB
                    {
                        adb_configured = true;
                    } else if func.interface_subclass == USB_SUBCLASS_VSOCK
                        && func.interface_protocol == USB_PROTOCOL_VSOCK
                    {
                        vsock_configured = true;
                    }
                }
                _ => {}
            }
        }
    }

    let compute_status = |configured: bool| -> (usb_policy::FunctionStatus, String) {
        if !configured {
            (usb_policy::FunctionStatus::Disabled, "Not configured".to_string())
        } else if !is_attached {
            (usb_policy::FunctionStatus::Disconnected, "Cable disconnected".to_string())
        } else if is_configured {
            (usb_policy::FunctionStatus::Connected, "Active and connected".to_string())
        } else {
            (
                usb_policy::FunctionStatus::Disconnected,
                "Enumerating / Waiting for host configuration".to_string(),
            )
        }
    };

    let (adb_status, adb_details) = compute_status(adb_configured);
    let (cdc_status, cdc_details) = compute_status(cdc_configured);
    let (vsock_status, vsock_details) = compute_status(vsock_configured);

    let adb = usb_policy::AdbHealth {
        status: Some(adb_status),
        details: Some(adb_details),
        ..Default::default()
    };

    let cdc_ethernet = usb_policy::CdcEthernetHealth {
        status: Some(cdc_status),
        details: Some(cdc_details),
        ..Default::default()
    };

    let vsock = usb_policy::VsockHealth {
        status: Some(vsock_status),
        // TODO(https://fxbug.dev/547909518): Query actual open socket channel count from the vsock-service
        // driver once an Inspect or FIDL interface is exposed.
        active_channels: None,
        details: Some(vsock_details),
        ..Default::default()
    };

    (adb, cdc_ethernet, vsock)
}

async fn build_health_report(shared_state: &Arc<UsbPolicySharedState>) -> usb_policy::HealthReport {
    let controller = shared_state.get_controller();
    let (dev_state, address) = if let Some(state) = &controller {
        let current_state = state.get_state();
        (Some(current_state.device_state), Some(current_state.address))
    } else {
        (None, None)
    };

    let is_attached = dev_state.is_some_and(|s| s != DeviceState::NotAttached);
    let is_configured = dev_state == Some(DeviceState::Configured);

    let cable_status = dev_state.map(|s| {
        if s != DeviceState::NotAttached {
            usb_policy::CableStatus::Connected
        } else {
            usb_policy::CableStatus::Disconnected
        }
    });

    let (adb, cdc_ethernet, vsock) = match handle_get_configuration().await {
        Ok((_device_desc, config_descriptors)) => {
            collate_function_health(&config_descriptors, is_attached, is_configured)
        }
        Err(status) => {
            let err_msg = format!("Failed to query peripheral configuration: {status}");
            let adb = usb_policy::AdbHealth {
                status: Some(usb_policy::FunctionStatus::Error),
                details: Some(err_msg.clone()),
                ..Default::default()
            };
            let cdc_ethernet = usb_policy::CdcEthernetHealth {
                status: Some(usb_policy::FunctionStatus::Error),
                details: Some(err_msg.clone()),
                ..Default::default()
            };
            let vsock = usb_policy::VsockHealth {
                status: Some(usb_policy::FunctionStatus::Error),
                active_channels: None,
                details: Some(err_msg),
                ..Default::default()
            };
            (adb, cdc_ethernet, vsock)
        }
    };

    usb_policy::HealthReport {
        state: dev_state,
        address,
        cable_status,
        adb: Some(adb),
        cdc_ethernet: Some(cdc_ethernet),
        vsock: Some(vsock),
        ..Default::default()
    }
}

async fn run_health_server(
    mut stream: usb_policy::HealthRequestStream,
    shared_state: Arc<UsbPolicySharedState>,
) {
    while let Some(request_result) = stream.next().await {
        match request_result {
            Ok(request) => match request {
                usb_policy::HealthRequest::GetReport { responder } => {
                    let report = build_health_report(&shared_state).await;
                    if let Err(e) = responder.send(Ok(&report)) {
                        warn!("Failed to send Health report: {:?}", e);
                    }
                }
                usb_policy::HealthRequest::_UnknownMethod { .. } => {
                    warn!("Unknown Health request");
                }
            },
            Err(e) => {
                warn!("HealthRequestStream error: {:?}", e);
                break;
            }
        }
    }
}

async fn run_usb_policy_service() -> Result<(), Error> {
    let serial_number = fetch_serial_number().await;
    let shared_state = Arc::new(UsbPolicySharedState::new_with_serial(serial_number));

    let shared_state_clone = shared_state.clone();
    let scope = fuchsia_async::Scope::new();
    let _task = scope.spawn(async move {
        let result = async {
            let client = Service::open(fpolicy::ServiceMarker)?;
            let instance = client.watch_for_any().await?;
            let controller = instance.connect_to_controller()?;
            let inspector = fuchsia_inspect::component::inspector();
            let inspect_node = inspector.root().create_child("usb_state_history");
            let controller_state = Arc::new(controller::ControllerState::new(
                controller,
                DeviceState::NotAttached,
                0,
                inspect_node,
            ));
            shared_state_clone.set_controller(controller_state.clone());
            let _ = controller_state.monitor_device_state().await;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(e) = result {
            warn!("Background discovery failed: {:?}", e);
        }
    });

    let mut fs = ServiceFs::new_local();
    fs.dir("svc")
        .add_fidl_service(IncomingRequest::Health)
        .add_service_at(
            "fuchsia.usb.policy.PolicyProvider",
            fuchsia_component::server::FidlService::from(IncomingRequest::Provider),
        )
        .add_service_at(
            "fuchsia.usb.policy.Configuration",
            fuchsia_component::server::FidlService::from(IncomingRequest::Configuration),
        );
    fs.take_and_serve_directory_handle()?;

    let health_server_fut = fs.for_each_concurrent(None, |req| {
        let state = shared_state.clone();
        async move {
            match req {
                IncomingRequest::Health(stream) => run_health_server(stream, state).await,
                IncomingRequest::Provider(stream) => run_provider_server(stream, state).await,
                IncomingRequest::Configuration(stream) => {
                    run_configuration_server(stream, state).await
                }
            }
        }
    });

    let _ = health_server_fut.await;
    Ok(())
}

#[fuchsia::main(logging_tags = ["usb-policy"])]
async fn main() -> Result<(), Error> {
    let _inspect_server_task = inspect_runtime::publish(
        fuchsia_inspect::component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );
    Box::pin(run_usb_policy_service()).await.and(Err(format_err!("USB policy layer stopped")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;

    #[fuchsia::test]
    async fn test_health_report() -> Result<(), anyhow::Error> {
        let (controller_proxy, _) = create_proxy_and_stream::<fpolicy::ControllerMarker>();
        let shared_state = Arc::new(UsbPolicySharedState::new());
        let controller_state = Arc::new(controller::ControllerState::new(
            controller_proxy,
            DeviceState::Attached,
            42,
            fuchsia_inspect::Inspector::default().root().create_child("test"),
        ));
        shared_state.set_controller(controller_state);

        let (health_proxy, stream) = create_proxy_and_stream::<usb_policy::HealthMarker>();

        fasync::Task::local(run_health_server(stream, shared_state)).detach();

        let report_res = health_proxy.get_report().await?;
        let report = match report_res {
            Ok(r) => r,
            Err(e) => return Err(format_err!("Health report error: {:?}", e)),
        };
        assert_eq!(report.state, Some(DeviceState::Attached));
        assert_eq!(report.address, Some(42));
        assert_eq!(report.cable_status, Some(usb_policy::CableStatus::Connected));
        assert!(report.adb.is_some());
        assert!(report.cdc_ethernet.is_some());
        assert!(report.vsock.is_some());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_health_report_configured() -> Result<(), anyhow::Error> {
        let (controller_proxy, _) = create_proxy_and_stream::<fpolicy::ControllerMarker>();
        let shared_state = Arc::new(UsbPolicySharedState::new());
        let controller_state = Arc::new(controller::ControllerState::new(
            controller_proxy,
            DeviceState::Configured,
            22,
            fuchsia_inspect::Inspector::default().root().create_child("test"),
        ));
        shared_state.set_controller(controller_state);

        let (health_proxy, stream) = create_proxy_and_stream::<usb_policy::HealthMarker>();
        fasync::Task::local(run_health_server(stream, shared_state)).detach();

        let report_res = health_proxy.get_report().await?;
        let report = match report_res {
            Ok(r) => r,
            Err(e) => return Err(format_err!("Health report error: {:?}", e)),
        };
        assert_eq!(report.state, Some(DeviceState::Configured));
        assert_eq!(report.address, Some(22));
        assert_eq!(report.cable_status, Some(usb_policy::CableStatus::Connected));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_health_report_not_attached() -> Result<(), anyhow::Error> {
        let (controller_proxy, _) = create_proxy_and_stream::<fpolicy::ControllerMarker>();
        let shared_state = Arc::new(UsbPolicySharedState::new());
        let controller_state = Arc::new(controller::ControllerState::new(
            controller_proxy,
            DeviceState::NotAttached,
            0,
            fuchsia_inspect::Inspector::default().root().create_child("test"),
        ));
        shared_state.set_controller(controller_state);

        let (health_proxy, stream) = create_proxy_and_stream::<usb_policy::HealthMarker>();
        fasync::Task::local(run_health_server(stream, shared_state)).detach();

        let report_res = health_proxy.get_report().await?;
        let report = match report_res {
            Ok(r) => r,
            Err(e) => return Err(format_err!("Health report error: {:?}", e)),
        };
        assert_eq!(report.state, Some(DeviceState::NotAttached));
        assert_eq!(report.address, Some(0));
        assert_eq!(report.cable_status, Some(usb_policy::CableStatus::Disconnected));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_provider_server_state() -> Result<(), anyhow::Error> {
        let (controller_proxy, _) = create_proxy_and_stream::<fpolicy::ControllerMarker>();
        let shared_state = Arc::new(UsbPolicySharedState::new());
        let controller_state = Arc::new(controller::ControllerState::new(
            controller_proxy,
            DeviceState::Configured,
            10,
            fuchsia_inspect::Inspector::default().root().create_child("test"),
        ));
        shared_state.set_controller(controller_state);

        let (provider_proxy, stream) =
            create_proxy_and_stream::<usb_policy::PolicyProviderMarker>();

        fasync::Task::local(run_provider_server(stream, shared_state)).detach();

        let update_res = provider_proxy.watch_device_state().await?;
        let update = match update_res {
            Ok(u) => u,
            Err(e) => return Err(format_err!("Watch state error: {:?}", e)),
        };
        assert_eq!(update.state, Some(DeviceState::Configured));
        assert_eq!(update.address, Some(10));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_health_report_not_ready() -> Result<(), anyhow::Error> {
        let shared_state = Arc::new(UsbPolicySharedState::new());
        let (health_proxy, stream) = create_proxy_and_stream::<usb_policy::HealthMarker>();

        fasync::Task::local(run_health_server(stream, shared_state)).detach();

        let report_res = health_proxy.get_report().await?;
        let report = match report_res {
            Ok(r) => r,
            Err(e) => return Err(format_err!("Health report error: {:?}", e)),
        };
        assert_eq!(report.state, None);
        assert_eq!(report.address, None);
        assert_eq!(report.cable_status, None);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_provider_server_wait() -> Result<(), anyhow::Error> {
        let shared_state = Arc::new(UsbPolicySharedState::new());
        let (provider_proxy, stream) =
            create_proxy_and_stream::<usb_policy::PolicyProviderMarker>();

        fasync::Task::local(run_provider_server(stream, shared_state.clone())).detach();

        let (controller_proxy, _) = create_proxy_and_stream::<fpolicy::ControllerMarker>();
        let controller_state = Arc::new(controller::ControllerState::new(
            controller_proxy,
            DeviceState::Configured,
            10,
            fuchsia_inspect::Inspector::default().root().create_child("test"),
        ));

        let shared_state_clone = shared_state.clone();
        fasync::Task::local(async move {
            shared_state_clone.set_controller(controller_state);
        })
        .detach();

        let update_res = provider_proxy.watch_device_state().await?;
        let update = match update_res {
            Ok(u) => u,
            Err(e) => return Err(format_err!("Watch state error: {:?}", e)),
        };
        assert_eq!(update.state, Some(DeviceState::Configured));
        assert_eq!(update.address, Some(10));
        Ok(())
    }
    #[fuchsia::test]
    async fn test_fetch_serial_number_fallback() {
        let serial = fetch_serial_number().await;
        assert!(!serial.is_empty());
        assert_eq!(serial, "12345678");
    }

    #[fuchsia::test]
    async fn test_configuration_server_unavailable() -> Result<(), anyhow::Error> {
        let (config_proxy, stream) = create_proxy_and_stream::<usb_policy::ConfigurationMarker>();

        let shared_state = Arc::new(UsbPolicySharedState::new());
        fasync::Task::local(run_configuration_server(stream, shared_state)).detach();

        let get_res = config_proxy.get_configuration().await?;
        assert_eq!(get_res.err(), Some(zx::Status::UNAVAILABLE.into_raw()));

        let dev_desc = peripheral::DeviceDescriptor {
            bcd_usb: 0x0200,
            b_device_class: 0,
            b_device_sub_class: 0,
            b_device_protocol: 0,
            b_max_packet_size0: 64,
            id_vendor: 0x18d1,
            id_product: 0xa022,
            bcd_device: 0x0100,
            manufacturer: "Test".to_string(),
            product: "Test".to_string(),
            serial: "123".to_string(),
            b_num_configurations: 1,
        };
        let set_res = config_proxy.set_configuration(&dev_desc, &[]).await?;
        assert_eq!(set_res.err(), Some(zx::Status::UNAVAILABLE.into_raw()));
        Ok(())
    }

    #[test]
    fn test_collate_function_health_all_configured() {
        let configs = vec![vec![
            peripheral::FunctionDescriptor {
                interface_class: 255,
                interface_subclass: 0x42,
                interface_protocol: 0x01,
            },
            peripheral::FunctionDescriptor {
                interface_class: 2,
                interface_subclass: 0,
                interface_protocol: 0,
            },
            peripheral::FunctionDescriptor {
                interface_class: 255,
                interface_subclass: 0x43,
                interface_protocol: 0x00,
            },
        ]];

        let (adb, cdc, vsock) = collate_function_health(&configs, true, true);
        assert_eq!(adb.status, Some(usb_policy::FunctionStatus::Connected));
        assert_eq!(adb.details, Some("Active and connected".to_string()));
        assert_eq!(adb.online, None);
        assert_eq!(adb.driver_state, None);
        assert_eq!(cdc.status, Some(usb_policy::FunctionStatus::Connected));
        assert_eq!(cdc.details, Some("Active and connected".to_string()));
        assert_eq!(cdc.online, None);
        assert_eq!(vsock.status, Some(usb_policy::FunctionStatus::Connected));
        assert_eq!(vsock.details, Some("Active and connected".to_string()));
        assert_eq!(vsock.online, None);
        assert_eq!(vsock.driver_state, None);
        assert_eq!(vsock.active_channels, None);
    }

    #[test]
    fn test_collate_function_health_fastboot_not_vsock() {
        // Fastboot is 255 / 0x42 / 0x03
        let configs = vec![vec![peripheral::FunctionDescriptor {
            interface_class: 255,
            interface_subclass: 0x42,
            interface_protocol: 0x03,
        }]];

        let (adb, cdc, vsock) = collate_function_health(&configs, true, true);
        assert_eq!(adb.status, Some(usb_policy::FunctionStatus::Disabled));
        assert_eq!(cdc.status, Some(usb_policy::FunctionStatus::Disabled));
        assert_eq!(vsock.status, Some(usb_policy::FunctionStatus::Disabled));
    }

    #[test]
    fn test_collate_function_health_disconnected_and_enumerating() {
        let configs = vec![vec![peripheral::FunctionDescriptor {
            interface_class: 255,
            interface_subclass: 0x42,
            interface_protocol: 0x01,
        }]];

        // Cable disconnected
        let (adb_disconn, _, _) = collate_function_health(&configs, false, false);
        assert_eq!(adb_disconn.status, Some(usb_policy::FunctionStatus::Disconnected));
        assert_eq!(adb_disconn.details, Some("Cable disconnected".to_string()));

        // Cable connected, but enumerating (not yet configured)
        let (adb_enum, _, _) = collate_function_health(&configs, true, false);
        assert_eq!(adb_enum.status, Some(usb_policy::FunctionStatus::Disconnected));
        assert_eq!(
            adb_enum.details,
            Some("Enumerating / Waiting for host configuration".to_string())
        );
    }
}
