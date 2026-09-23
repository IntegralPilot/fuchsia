// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::access_point::{state_machine as ap_fsm, types as ap_types};
use crate::client::types as client_types;
use crate::mode_management::iface_manager_api as api;
use anyhow::Error;
use futures::channel::oneshot;

pub use ieee80211::Ssid;

#[cfg_attr(test, derive(Debug))]
pub struct DisconnectRequest {
    pub network_id: ap_types::NetworkIdentifier,
    pub reason: client_types::DisconnectReason,
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[cfg_attr(test, derive(Debug))]
pub struct ConnectRequest {
    pub request: api::ConnectAttemptRequest,
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[derive(Debug)]
pub struct RecordIdleIfaceRequest {
    pub iface_id: u16,
    pub responder: oneshot::Sender<()>,
}

#[derive(Debug)]
pub struct HasIdleIfaceRequest {
    pub responder: oneshot::Sender<bool>,
}

#[derive(Debug)]
pub struct AddIfaceRequest {
    pub iface_id: u16,
    pub responder: oneshot::Sender<()>,
}

#[derive(Debug)]
pub struct RemoveIfaceRequest {
    pub iface_id: u16,
    pub responder: oneshot::Sender<()>,
}

#[derive(Debug)]
pub struct ScanProxyRequest {
    pub responder: oneshot::Sender<Result<api::SmeForScan, Error>>,
}

#[derive(Debug)]
pub struct StopClientConnectionsRequest {
    pub reason: client_types::DisconnectReason,
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[derive(Debug)]
pub struct StartClientConnectionsRequest {
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[cfg_attr(test, derive(Debug))]
pub struct StartApRequest {
    pub config: ap_fsm::ApConfig,
    pub responder: oneshot::Sender<Result<oneshot::Receiver<()>, Error>>,
}

#[cfg_attr(test, derive(Debug))]
pub struct StopApRequest {
    pub ssid: Ssid,
    pub password: Vec<u8>,
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[derive(Debug)]
pub struct StopAllApsRequest {
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[derive(Debug)]
pub struct SetCountryRequest {
    pub country_code: Option<client_types::CountryCode>,
    pub responder: oneshot::Sender<Result<(), Error>>,
}

#[cfg_attr(test, derive(Debug))]
pub enum IfaceManagerRequest {
    Connect(ConnectRequest),
    RecordIdleIface(RecordIdleIfaceRequest),
    HasIdleIface(HasIdleIfaceRequest),
    AddIface(AddIfaceRequest),
    RemoveIface(RemoveIfaceRequest),
    GetScanProxy(ScanProxyRequest),
    StartClientConnections(StartClientConnectionsRequest),
    StartAp(StartApRequest),
    Disconnect(DisconnectRequest),
    StopClientConnections(StopClientConnectionsRequest),
    StopAp(StopApRequest),
    StopAllAps(StopAllApsRequest),
    SetCountry(SetCountryRequest),
}

#[cfg_attr(test, derive(Debug))]
pub(crate) struct SetCountryOperationState {
    pub client_connections_initially_enabled: bool,
    pub initial_ap_configs: Vec<ap_fsm::ApConfig>,
    pub set_country_result: Result<(), Error>,
    pub responder: oneshot::Sender<Result<(), Error>>,
}
