// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdf_component::{Driver, DriverContext, DriverError, ServiceInstance};
use fidl_next::Responder;
use fidl_next_fuchsia_hardware_pci as pci;
use fidl_next_fuchsia_wlan_common as wlan_common;
use fidl_next_fuchsia_wlan_phy as phy;
use fuchsia_component::server::ServiceFs;
use fuchsia_trace;
use futures::StreamExt;
use wlan_trace as wtrace;
use zx::Status;

/// Logs a message prefaced by a connection identifier.
macro_rules! conn_log {
    ($level:ident, $conn_id:expr, $msg:literal $(, $args:expr)* $(,)?) => {{
        log::$level!(concat!("[conn {}] ", $msg), $conn_id $(, $args)*);
    }}
}

/// Logs a WARNING-level message indicating the response to a request failed.
macro_rules! conn_log_respond_error {
    ($conn_id:expr, $method_name:expr, $error_msg:expr) => {{
        conn_log!(warn, $conn_id, "Failed to respond to {} call: {}", $method_name, $error_msg);
    }};
}

/// Records a trace duration and logs a TRACE-level message indicating that the method was called.
macro_rules! conn_log_method_call {
    ($conn_id:expr, $method_name:expr) => {{
        wtrace::duration!($method_name);
        conn_log!(trace, $conn_id, "{} called", $method_name);
    }};
}

struct WlanVirtioHwsim {
    _node: fdf_component::Node,
    _scope: fuchsia_async::Scope,
}

fdf_component::driver_register!(WlanVirtioHwsim);

impl Driver for WlanVirtioHwsim {
    const NAME: &'static str = "wlan-virtio-hwsim";

    async fn start(mut context: DriverContext) -> Result<Self, DriverError> {
        fuchsia_trace::duration!(c"wlan-virtio-hwsim", c"driver_start");
        log::info!("Starting {} driver skeleton", Self::NAME);

        let pci_service: ServiceInstance<pci::Service> =
            context.incoming.service().connect_next().inspect_err(|status| {
                log::error!(status:?; "Failed to connect to PCI service");
            })?;

        let (client_end, server_end) = zx::Channel::create();
        let _pci_device =
            fidl_next::ClientEnd::<pci::Device, zx::Channel>::from_untyped(client_end);
        pci_service.device(fidl_next::ServerEnd::from_untyped(server_end)).map_err(|e| {
            log::error!("Failed to get PCI device proxy: {:?}", e);
            Status::INTERNAL
        })?;

        let node = context.take_node()?;
        let scope = fuchsia_async::Scope::new_with_name(Self::NAME);
        let mut outgoing = ServiceFs::new();

        let offer = fdf_component::ServiceOffer::<phy::Service>::new_next()
            .add_default_named_next(
                &mut outgoing,
                "default",
                PhyService {
                    scope: scope.to_handle(),
                    next_conn_id: std::sync::atomic::AtomicU32::new(0),
                },
            )
            .build_zircon_offer_next();

        let child_builder = fdf_component::NodeBuilder::new("wlanphy").add_offer(offer);
        let child_args = child_builder.build();

        let _child_controller = node.add_child(child_args).await?;

        context.serve_outgoing(&mut outgoing)?;
        scope.spawn(outgoing.collect());

        Ok(WlanVirtioHwsim { _node: node, _scope: scope })
    }

    async fn stop(&self) {
        log::info!("Stopping {} driver", Self::NAME);
    }
}

struct PhyService {
    scope: fuchsia_async::ScopeHandle,
    next_conn_id: std::sync::atomic::AtomicU32,
}

impl phy::ServiceHandler for PhyService {
    fn device(&self, server_end: fidl_next::ServerEnd<phy::WlanPhy>) {
        let conn_id = self.next_conn_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        log::trace!("[conn {}] connector called with server_end: {:?}", conn_id, server_end);
        self.scope.spawn_local(async move {
            log::trace!("[conn {}] task started", conn_id);
            let dispatcher = fidl_next::ServerDispatcher::new(server_end);
            match dispatcher.run_local(WlanPhyServer { conn_id }).await {
                Ok(_) => log::trace!("[conn {}] task finished successfully", conn_id),
                Err(e) => log::error!("[conn {}] task finished with error: {:?}", conn_id, e),
            }
        });
    }
}

struct WlanPhyServer {
    conn_id: u32,
}

impl phy::WlanPhyLocalServerHandler for WlanPhyServer {
    async fn init(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::Init>,
        responder: Responder<phy::wlan_phy::Init>,
    ) {
        conn_log_method_call!(self.conn_id, "init");
        let response = phy::WlanPhyInitResponse::default();
        if let Err(e) = responder.respond(response).await {
            conn_log_respond_error!(self.conn_id, "init", e);
        }
    }

    async fn get_supported_mac_roles(
        &mut self,
        responder: Responder<phy::wlan_phy::GetSupportedMacRoles>,
    ) {
        conn_log_method_call!(self.conn_id, "get_supported_mac_roles");
        let roles = vec![wlan_common::WlanMacRole::Client];
        let response = phy::WlanPhyGetSupportedMacRolesResponse {
            supported_mac_roles: Some(roles),
            ..Default::default()
        };
        if let Err(e) = responder.respond(response).await {
            conn_log_respond_error!(self.conn_id, "get_supported_mac_roles", e);
        }
    }

    async fn create_iface(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::CreateIface>,
        responder: Responder<phy::wlan_phy::CreateIface>,
    ) {
        conn_log_method_call!(self.conn_id, "create_iface");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "create_iface", e);
        }
    }

    async fn destroy_iface(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::DestroyIface>,
        responder: Responder<phy::wlan_phy::DestroyIface>,
    ) {
        conn_log_method_call!(self.conn_id, "destroy_iface");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "destroy_iface", e);
        }
    }

    async fn set_country(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::SetCountry>,
        responder: Responder<phy::wlan_phy::SetCountry>,
    ) {
        conn_log_method_call!(self.conn_id, "set_country");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "set_country", e);
        }
    }

    async fn clear_country(&mut self, responder: Responder<phy::wlan_phy::ClearCountry>) {
        conn_log_method_call!(self.conn_id, "clear_country");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "clear_country", e);
        }
    }

    async fn get_country(&mut self, responder: Responder<phy::wlan_phy::GetCountry>) {
        conn_log_method_call!(self.conn_id, "get_country");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "get_country", e);
        }
    }

    async fn set_power_save_mode(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::SetPowerSaveMode>,
        responder: Responder<phy::wlan_phy::SetPowerSaveMode>,
    ) {
        conn_log_method_call!(self.conn_id, "set_power_save_mode");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "set_power_save_mode", e);
        }
    }

    async fn get_power_save_mode(&mut self, responder: Responder<phy::wlan_phy::GetPowerSaveMode>) {
        conn_log_method_call!(self.conn_id, "get_power_save_mode");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "get_power_save_mode", e);
        }
    }

    async fn power_down(&mut self, responder: Responder<phy::wlan_phy::PowerDown>) {
        conn_log_method_call!(self.conn_id, "power_down");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "power_down", e);
        }
    }

    async fn power_up(&mut self, responder: Responder<phy::wlan_phy::PowerUp>) {
        conn_log_method_call!(self.conn_id, "power_up");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "power_up", e);
        }
    }

    async fn reset(&mut self, responder: Responder<phy::wlan_phy::Reset>) {
        conn_log_method_call!(self.conn_id, "reset");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "reset", e);
        }
    }

    async fn get_power_state(&mut self, responder: Responder<phy::wlan_phy::GetPowerState>) {
        conn_log_method_call!(self.conn_id, "get_power_state");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "get_power_state", e);
        }
    }

    async fn set_bt_coexistence_mode(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::SetBtCoexistenceMode>,
        responder: Responder<phy::wlan_phy::SetBtCoexistenceMode>,
    ) {
        conn_log_method_call!(self.conn_id, "set_bt_coexistence_mode");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "set_bt_coexistence_mode", e);
        }
    }

    async fn set_tx_power_scenario(
        &mut self,
        _request: fidl_next::Request<phy::wlan_phy::SetTxPowerScenario>,
        responder: Responder<phy::wlan_phy::SetTxPowerScenario>,
    ) {
        conn_log_method_call!(self.conn_id, "set_tx_power_scenario");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "set_tx_power_scenario", e);
        }
    }

    async fn reset_tx_power_scenario(
        &mut self,
        responder: Responder<phy::wlan_phy::ResetTxPowerScenario>,
    ) {
        conn_log_method_call!(self.conn_id, "reset_tx_power_scenario");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "reset_tx_power_scenario", e);
        }
    }

    async fn get_tx_power_scenario(
        &mut self,
        responder: Responder<phy::wlan_phy::GetTxPowerScenario>,
    ) {
        conn_log_method_call!(self.conn_id, "get_tx_power_scenario");
        if let Err(e) = responder.respond_err(Status::NOT_SUPPORTED).await {
            conn_log_respond_error!(self.conn_id, "get_tx_power_scenario", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fdf_component::ServiceOffer;
    use fdf_component::testing::harness::TestHarness;
    use fuchsia_component::server::ServiceFs;

    #[fuchsia::test]
    async fn test_driver_start() {
        let mut service_fs = ServiceFs::new();
        let tasks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tasks_clone = tasks.clone();

        struct FakePciService {
            tasks: std::sync::Arc<std::sync::Mutex<Vec<fuchsia_async::Task<()>>>>,
        }

        impl pci::ServiceHandler for FakePciService {
            fn device(&self, server_end: fidl_next::ServerEnd<pci::Device>) {
                let task = fuchsia_async::Task::spawn(async move {
                    let _server_end = server_end;
                    futures::future::pending::<()>().await;
                });
                self.tasks.lock().unwrap().push(task);
            }
        }

        let offer = ServiceOffer::<pci::Service>::new_next()
            .add_default_named_next(
                &mut service_fs,
                "default",
                FakePciService { tasks: tasks_clone },
            )
            .build_zircon_offer_next();

        let mut harness =
            TestHarness::<WlanVirtioHwsim>::new().set_driver_incoming(service_fs).add_offer(offer);

        let started_driver = harness.start_driver().await.expect("failed to start driver");

        let service_proxy: fdf_component::ServiceInstance<phy::Service> =
            started_driver.driver_outgoing().service().connect_next().unwrap();

        let (client_end, server_end) = fidl_next::fuchsia::create_channel();
        service_proxy.device(server_end).unwrap();

        let client = client_end.spawn();

        let response = client.get_supported_mac_roles().await;
        assert!(response.is_ok());
        let get_roles_res = response.unwrap();
        assert!(get_roles_res.is_ok());
        let roles_table = get_roles_res.unwrap();
        let supported_roles = roles_table.supported_mac_roles.as_ref().unwrap();
        assert_eq!(supported_roles.len(), 1);
        assert_eq!(supported_roles[0], wlan_common::WlanMacRole::Client);

        started_driver.stop_driver().await;
    }
}
