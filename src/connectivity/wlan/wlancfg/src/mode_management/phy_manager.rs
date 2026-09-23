// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::types as client_types;
use crate::mode_management::recovery::{
    self, IfaceRecoveryOperation, PhyRecoveryOperation, RecoveryAction,
};
use crate::mode_management::{Defect, EventHistory, IfaceFailure, PhyFailure};
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_device_service as fidl_service;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_inspect::{self as inspect, NumericProperty};
use ieee80211::{MacAddr, MacAddrBytes, NULL_ADDR};
use log::{error, info, warn};
use std::collections::{HashMap, HashSet};
use std::iter::Iterator;
use thiserror::Error;

// Number of seconds that recoverable event histories should be stored.  Store past events for 24
// hours (86400s).
const DEFECT_RETENTION_SECONDS: u32 = 86400;

/// Errors raised while attempting to query information about or configure PHYs and ifaces.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PhyManagerError {
    #[error("the requested operation is not supported")]
    Unsupported,
    #[error("unable to query phy information")]
    PhyQueryFailure,
    #[error("failed to set country for new PHY")]
    PhySetCountryFailure,
    #[error("unable to reset PHY")]
    PhyResetFailure,
    #[error("unable to query iface information")]
    IfaceQueryFailure,
    #[error("unable to create iface")]
    IfaceCreateFailure,
    #[error("unable to destroy iface")]
    IfaceDestroyFailure,
    #[error("internal state has become inconsistent")]
    InternalError,
}

/// There are a variety of reasons why the calling code may want to create client interfaces.  The
/// main logic to do so is identical, but there are different intents for making the call.  This
/// enum allows callers to express their intent when making the call to ensure that internal
/// PhyManager state remains consistent with the current desired mode of operation.
#[derive(PartialEq)]
pub enum CreateClientIfacesReason {
    StartClientConnections,
    RecoverClientIfaces,
}

/// Stores information about a WLAN PHY and any interfaces that belong to it.
pub(crate) struct PhyContainer {
    supported_mac_roles: HashSet<fidl_common::WlanMacRole>,
    client_ifaces: HashSet<u16>,
    ap_ifaces: HashSet<u16>,
    // It is possible for interface destruction and defect reporting to race.  Keeping a set of
    // past interface IDs ensures that defects can be associated with the appropriate PHY.
    destroyed_ifaces: HashSet<u16>,
    defects: EventHistory<Defect>,
    recoveries: EventHistory<recovery::RecoveryAction>,
}

#[async_trait(?Send)]
pub trait PhyManagerApi {
    /// Checks to see if this PHY is already accounted for.  If it is not, queries its PHY
    /// attributes and places it in the hash map.
    async fn add_phy(&mut self, phy_id: u16) -> Result<(), PhyManagerError>;

    /// If the PHY is accounted for, removes the associated `PhyContainer` from the hash map.
    fn remove_phy(&mut self, phy_id: u16);

    /// Queries the interface properties to get the PHY ID.  If the `PhyContainer`
    /// representing the interface's parent PHY is already present and its
    /// interface information is obsolete, updates it.  The PhyManager will track ifaces
    /// as it creates and deletes them, but it is possible that another caller circumvents the
    /// policy layer and creates an interface.  If no `PhyContainer` exists
    /// for the new interface, creates one and adds the newly discovered interface
    /// ID to it.
    async fn on_iface_added(&mut self, iface_id: u16) -> Result<(), PhyManagerError>;

    /// Ensures that the `iface_id` is not present in any of the `PhyContainer` interface lists.
    fn on_iface_removed(&mut self, iface_id: u16);

    /// Creates client interfaces for all PHYs that are capable of acting as clients.  For newly
    /// discovered PHYs, create client interfaces if the PHY can support them.  This method returns
    /// a containing all newly-created client interface IDs along with a representation of
    /// any errors encountered along the way.
    async fn create_all_client_ifaces(
        &mut self,
        reason: CreateClientIfacesReason,
    ) -> HashMap<u16, Result<Vec<u16>, PhyManagerError>>;

    /// The PhyManager is the authoritative source of whether or not the policy layer is allowed to
    /// create client interfaces.  This method allows other parts of the policy layer to determine
    /// whether the API client has allowed client interfaces to be created.
    fn client_connections_enabled(&self) -> bool;

    /// Destroys all client interfaces.  Do not allow the creation of client interfaces for newly
    /// discovered PHYs.
    async fn destroy_all_client_ifaces(&mut self) -> Result<(), PhyManagerError>;

    /// Finds a PHY with a client interface and returns the interface's ID to the caller.
    fn get_client(&mut self) -> Option<u16>;

    /// Finds a PHY that is capable of functioning as an AP.  PHYs that do not yet have AP ifaces
    /// associated with them are searched first.  If one is found, an AP iface is created and its
    /// ID is returned.  If all AP-capable PHYs already have AP ifaces associated with them, one of
    /// the existing AP iface IDs is returned.  If there are no AP-capable PHYs, None is returned.
    async fn create_or_get_ap_iface(&mut self) -> Result<Option<u16>, PhyManagerError>;

    /// Destroys the interface associated with the given interface ID.
    async fn destroy_ap_iface(&mut self, iface_id: u16) -> Result<(), PhyManagerError>;

    /// Destroys all AP interfaces.
    async fn destroy_all_ap_ifaces(&mut self) -> Result<(), PhyManagerError>;

    /// Sets a suggested MAC address to be used by new AP interfaces.
    fn suggest_ap_mac(&mut self, mac: MacAddr);

    /// Returns the IDs for all currently known PHYs.
    fn get_phy_ids(&self) -> Vec<u16>;

    /// Logs phy add failure inspect metrics.
    fn log_phy_add_failure(&mut self);

    /// Sets the country code on all known PHYs and stores the country code to be applied to
    /// newly-discovered PHYs.
    async fn set_country_code(
        &mut self,
        country_code: Option<client_types::CountryCode>,
    ) -> Result<(), PhyManagerError>;

    /// Store a record for the provided defect.
    fn record_defect(&mut self, defect: Defect);

    /// Take the recovery action proposed by the recovery summary.
    async fn perform_recovery(&mut self, summary: recovery::RecoverySummary);
}

/// Maintains a record of all PHYs that are present and their associated interfaces.
pub struct PhyManager {
    phys: HashMap<u16, PhyContainer>,
    recovery_profile: recovery::RecoveryProfile,
    recovery_enabled: bool,
    device_monitor: fidl_service::DeviceMonitorProxy,
    client_connections_enabled: bool,
    suggested_ap_mac: Option<MacAddr>,
    saved_country_code: Option<client_types::CountryCode>,
    _node: inspect::Node,
    telemetry_sender: TelemetrySender,
    recovery_action_sender: recovery::RecoveryActionSender,
    phy_add_fail_count: inspect::UintProperty,
}

impl PhyContainer {
    /// Stores the PhyInfo associated with a newly discovered PHY and creates empty vectors to hold
    /// interface IDs that belong to this PHY.
    pub fn new(supported_mac_roles: Vec<fidl_common::WlanMacRole>) -> Self {
        PhyContainer {
            supported_mac_roles: supported_mac_roles.into_iter().collect(),
            client_ifaces: HashSet::new(),
            ap_ifaces: HashSet::new(),
            destroyed_ifaces: HashSet::new(),
            defects: EventHistory::<Defect>::new(DEFECT_RETENTION_SECONDS),
            recoveries: EventHistory::<RecoveryAction>::new(DEFECT_RETENTION_SECONDS),
        }
    }
}

// TODO(https://fxbug.dev/42126575): PhyManager makes the assumption that WLAN PHYs that support client and AP modes can
// can operate as clients and APs simultaneously.  For PHYs where this is not the case, the
// existing interface should be destroyed before the new interface is created.
impl PhyManager {
    /// Internally stores a DeviceMonitorProxy to query PHY and interface properties and create and
    /// destroy interfaces as requested.
    pub fn new(
        device_monitor: fidl_service::DeviceMonitorProxy,
        recovery_profile: recovery::RecoveryProfile,
        recovery_enabled: bool,
        node: inspect::Node,
        telemetry_sender: TelemetrySender,
        recovery_action_sender: recovery::RecoveryActionSender,
    ) -> Self {
        let phy_add_fail_count = node.create_uint("phy_add_fail_count", 0);
        PhyManager {
            phys: HashMap::new(),
            recovery_profile,
            recovery_enabled,
            device_monitor,
            client_connections_enabled: false,
            suggested_ap_mac: None,
            saved_country_code: None,
            _node: node,
            telemetry_sender,
            recovery_action_sender,
            phy_add_fail_count,
        }
    }
    /// Verifies that a given PHY ID is accounted for and, if not, adds a new entry for it.
    async fn ensure_phy(&mut self, phy_id: u16) -> Result<&mut PhyContainer, PhyManagerError> {
        if !self.phys.contains_key(&phy_id) {
            self.add_phy(phy_id).await?;
        }

        // The phy_id is guaranteed to exist at this point because it was either previously
        // accounted for or was just added above.
        Ok(self.phys.get_mut(&phy_id).ok_or_else(|| {
            error!("Phy ID did not exist in self.phys");
            PhyManagerError::InternalError
        }))?
    }

    /// Queries the information associated with the given iface ID.
    async fn query_iface(
        &self,
        iface_id: u16,
    ) -> Result<Option<fidl_service::QueryIfaceResponse>, PhyManagerError> {
        match self.device_monitor.query_iface(iface_id).await {
            Ok(Ok(response)) => Ok(Some(response)),
            Ok(Err(zx::sys::ZX_ERR_NOT_FOUND)) => Ok(None),
            _ => Err(PhyManagerError::IfaceQueryFailure),
        }
    }

    /// Returns a list of PHY IDs that can have interfaces of the requested MAC role.
    fn phys_for_role(&self, role: fidl_common::WlanMacRole) -> Vec<u16> {
        self.phys
            .iter()
            .filter_map(|(k, v)| {
                if v.supported_mac_roles.contains(&role) {
                    return Some(*k);
                }
                None
            })
            .collect()
    }

    /// Log the provided recovery summary.
    fn log_recovery_action(&mut self, summary: recovery::RecoverySummary) {
        let affected_phy_id = match summary.action {
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id }) => {
                if let Some(container) = self.phys.get_mut(&phy_id) {
                    container.recoveries.add_event(summary.action);
                    Some(phy_id)
                } else {
                    None
                }
            }
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id })
            | RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect { iface_id })
            | RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp { iface_id }) => {
                let mut affected_phy_id = None;
                for (phy_id, phy_info) in self.phys.iter_mut() {
                    if phy_info.ap_ifaces.contains(&iface_id)
                        || phy_info.client_ifaces.contains(&iface_id)
                    {
                        phy_info.recoveries.add_event(summary.action);
                        affected_phy_id = Some(*phy_id);
                    }
                }

                affected_phy_id
            }
        };

        if let Some(phy_id) = affected_phy_id {
            warn!("Recovery has been recommended for PHY {}: {:?}", phy_id, summary.action);
        }

        if let Some(recovery_summary) = summary.as_recovery_reason() {
            self.telemetry_sender.send(TelemetryEvent::RecoveryEvent { reason: recovery_summary });
        }
    }

    /// Creates an interface of the requested role for the requested PHY ID.  Returns either the
    /// ID of the created interface or an error.
    async fn create_iface(
        &mut self,
        phy_id: u16,
        role: fidl_common::WlanMacRole,
        sta_addr: MacAddr,
    ) -> Result<u16, PhyManagerError> {
        let request = fidl_service::DeviceMonitorCreateIfaceRequest {
            phy_id: Some(phy_id),
            role: Some(role),
            sta_address: Some(sta_addr.to_array()),
            ..Default::default()
        };

        let response = self.device_monitor.create_iface(&request).await;
        let result = match response {
            Err(e) => {
                warn!("Failed to create iface: phy {}, FIDL error {:?}", phy_id, e);
                Err(PhyManagerError::IfaceCreateFailure)
            }
            Ok(Err(e)) => {
                warn!("Failed to create iface: phy {}, error {:?}", phy_id, e);
                Err(PhyManagerError::IfaceCreateFailure)
            }
            Ok(Ok(fidl_service::DeviceMonitorCreateIfaceResponse { iface_id: None, .. })) => {
                warn!("Failed to create iface. No iface ID received: phy {}", phy_id);
                Err(PhyManagerError::IfaceCreateFailure)
            }
            Ok(Ok(fidl_service::DeviceMonitorCreateIfaceResponse {
                iface_id: Some(iface_id),
                ..
            })) => Ok(iface_id),
        };

        self.telemetry_sender.send(TelemetryEvent::IfaceCreationResult {
            role,
            result: result.as_ref().map_err(|_| ()).copied(),
        });
        if result.is_err() {
            self.record_defect(Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id }));
        }
        result
    }
}

#[async_trait(?Send)]
impl PhyManagerApi for PhyManager {
    async fn add_phy(&mut self, phy_id: u16) -> Result<(), PhyManagerError> {
        let supported_mac_roles = self
            .device_monitor
            .get_supported_mac_roles(phy_id)
            .await
            .map_err(|e| {
                warn!("Failed to communicate with monitor service: {:?}", e);
                PhyManagerError::PhyQueryFailure
            })?
            .map_err(|e| {
                warn!("Unable to get supported MAC roles: {:?}", e);
                PhyManagerError::PhyQueryFailure
            })?;

        // Create a new container to store the PHY's information.
        info!("adding PHY ID #{}", phy_id);
        let mut phy_container = PhyContainer::new(supported_mac_roles);

        // Attempt to set the country for the newly-discovered PHY.
        let set_country_result = match self.saved_country_code {
            Some(country_code) => {
                set_phy_country_code(&self.device_monitor, phy_id, country_code).await
            }
            None => Ok(()),
        };

        // If setting the country code fails, clear the PHY's country code so that it is in WW
        // and can continue to operate.  If this process fails, return early and do not use this
        // PHY.
        if set_country_result.is_err() {
            clear_phy_country_code(&self.device_monitor, phy_id).await?;
        }

        if self.client_connections_enabled
            && phy_container.supported_mac_roles.contains(&fidl_common::WlanMacRole::Client)
        {
            let iface_id =
                self.create_iface(phy_id, fidl_common::WlanMacRole::Client, NULL_ADDR).await?;
            let _ = phy_container.client_ifaces.insert(iface_id);
        }

        if self.phys.insert(phy_id, phy_container).is_some() {
            warn!("Unexpectedly replaced existing phy information for id {}", phy_id);
        };

        Ok(())
    }

    fn remove_phy(&mut self, phy_id: u16) {
        if self.phys.remove(&phy_id).is_none() {
            warn!("Attempted to remove non-existed phy {}", phy_id);
        };
    }

    async fn on_iface_added(&mut self, iface_id: u16) -> Result<(), PhyManagerError> {
        if let Some(query_iface_response) = self.query_iface(iface_id).await? {
            let iface_id = query_iface_response.id;
            let phy = self.ensure_phy(query_iface_response.phy_id).await?;

            match query_iface_response.role {
                fidl_common::WlanMacRole::Client => {
                    if phy.client_ifaces.insert(iface_id) {
                        // The iface wasn't in the hashset, so it was created by someone else
                        warn!(
                            "Detected an unexpected client iface id {} created outside of PhyManager",
                            iface_id
                        );
                    }
                }
                fidl_common::WlanMacRole::Ap => {
                    if phy.ap_ifaces.insert(iface_id) {
                        // `.insert()` returns true if the value was not already present
                        warn!("Detected an unexpected AP iface created outside of PhyManager");
                    }
                }
                fidl_common::WlanMacRole::Mesh => {
                    return Err(PhyManagerError::Unsupported);
                }
                fidl_common::WlanMacRoleUnknown!() => {
                    error!("Unknown WlanMacRole type {:?}", query_iface_response.role);
                    return Err(PhyManagerError::Unsupported);
                }
            }
        }
        Ok(())
    }

    fn on_iface_removed(&mut self, iface_id: u16) {
        for phy_info in self.phys.values_mut() {
            // The presence or absence of the interface in the PhyManager internal records is
            // irrelevant.  Simply remove any reference to the removed interface ID to ensure that
            // it is not used for future operations.
            let _ = phy_info.client_ifaces.remove(&iface_id);
            let _ = phy_info.ap_ifaces.remove(&iface_id);
        }
    }

    async fn create_all_client_ifaces(
        &mut self,
        reason: CreateClientIfacesReason,
    ) -> HashMap<u16, Result<Vec<u16>, PhyManagerError>> {
        if reason == CreateClientIfacesReason::StartClientConnections {
            self.client_connections_enabled = true;
        }

        let mut available_iface_ids = HashMap::new();
        if self.client_connections_enabled {
            let client_capable_phy_ids = self.phys_for_role(fidl_common::WlanMacRole::Client);

            for phy_id in client_capable_phy_ids.iter().copied() {
                let phy_container = match self.phys.get_mut(&phy_id) {
                    Some(phy_container) => phy_container,
                    None => {
                        let _ = available_iface_ids
                            .insert(phy_id, Err(PhyManagerError::PhyQueryFailure));
                        continue;
                    }
                };

                // If a PHY should be able to have a client interface and it does not, create a new
                // client interface for the PHY.
                if phy_container.client_ifaces.is_empty() {
                    let iface_id = match self
                        .create_iface(phy_id, fidl_common::WlanMacRole::Client, NULL_ADDR)
                        .await
                    {
                        Ok(iface_id) => iface_id,
                        Err(e) => {
                            warn!("Failed to recover iface for PHY {}: {:?}", phy_id, e);
                            let _ = available_iface_ids.insert(phy_id, Err(e));
                            continue;
                        }
                    };

                    // Safe to unwrap here: this phy_id was just used create an interface. If we
                    // can't find it now, it's reasonable to panic
                    #[expect(clippy::unwrap_used)]
                    let phy_container = self.phys.get_mut(&phy_id).unwrap();
                    let _ = phy_container.client_ifaces.insert(iface_id);

                    // There is only one client iface because this branch only runs when
                    // phy_container.client_ifaces is initially empty.
                    let _ = available_iface_ids.insert(phy_id, Ok(vec![iface_id]));
                } else {
                    let _ = available_iface_ids
                        .insert(phy_id, Ok(phy_container.client_ifaces.iter().copied().collect()));
                }
            }
        }

        available_iface_ids
    }

    fn client_connections_enabled(&self) -> bool {
        self.client_connections_enabled
    }

    async fn destroy_all_client_ifaces(&mut self) -> Result<(), PhyManagerError> {
        self.client_connections_enabled = false;

        let client_capable_phys = self.phys_for_role(fidl_common::WlanMacRole::Client);
        let mut result = Ok(());
        let mut failing_phys = Vec::new();

        for client_phy in client_capable_phys.iter() {
            let phy_container =
                self.phys.get_mut(client_phy).ok_or(PhyManagerError::PhyQueryFailure)?;

            // Continue tracking interface IDs for which deletion fails.
            let mut lingering_ifaces = HashSet::new();

            for iface_id in phy_container.client_ifaces.drain() {
                match destroy_iface(
                    &self.device_monitor,
                    iface_id,
                    fidl_common::WlanMacRole::Client,
                    &self.telemetry_sender,
                )
                .await
                {
                    Ok(()) => {
                        let _ = phy_container.destroyed_ifaces.insert(iface_id);
                    }
                    Err(e) => {
                        result = Err(e);
                        failing_phys.push(*client_phy);
                        if !lingering_ifaces.insert(iface_id) {
                            warn!("Unexpected duplicate lingering iface for id {}", iface_id);
                        };
                    }
                }
            }
            phy_container.client_ifaces = lingering_ifaces;
        }

        if result.is_err() {
            for phy_id in failing_phys {
                self.record_defect(Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id }))
            }
        }

        result
    }

    fn get_client(&mut self) -> Option<u16> {
        if !self.client_connections_enabled {
            return None;
        }

        let client_capable_phys = self.phys_for_role(fidl_common::WlanMacRole::Client);

        // Find the first PHY with any client interfaces and return its first client interface.
        let first_client_capable_phy = client_capable_phys.first()?;
        let phy = self.phys.get_mut(first_client_capable_phy)?;
        phy.client_ifaces.iter().next().copied()
    }

    async fn create_or_get_ap_iface(&mut self) -> Result<Option<u16>, PhyManagerError> {
        let ap_capable_phy_ids = self.phys_for_role(fidl_common::WlanMacRole::Ap);

        // First check for any PHYs that can have AP interfaces but do not yet
        for ap_phy_id in ap_capable_phy_ids.iter() {
            let phy_container =
                self.phys.get_mut(ap_phy_id).ok_or(PhyManagerError::PhyQueryFailure)?;
            if phy_container.ap_ifaces.is_empty() {
                let mac = match self.suggested_ap_mac {
                    Some(mac) => mac,
                    None => NULL_ADDR,
                };
                let iface_id =
                    self.create_iface(*ap_phy_id, fidl_common::WlanMacRole::Ap, mac).await?;

                // Need to reborrow from self here, since self.create_iface also borrows self
                // mutably. It's ok to unwrap here, since we just got this same interface a few
                // lines above, and would be appropriate to panic if we can't get it.
                #[expect(clippy::unwrap_used)]
                let phy_container = self.phys.get_mut(ap_phy_id).unwrap();
                let _ = phy_container.ap_ifaces.insert(iface_id);
                return Ok(Some(iface_id));
            }
        }

        // If all of the AP-capable PHYs have created AP interfaces already, return the
        // first observed existing AP interface
        // TODO(https://fxbug.dev/42126856): Figure out a better method of interface selection.
        let Some(first_ap_capable_phy) = ap_capable_phy_ids.first() else {
            return Ok(None);
        };
        let phy = match self.phys.get_mut(first_ap_capable_phy) {
            Some(phy_container) => phy_container,
            None => return Ok(None),
        };
        match phy.ap_ifaces.iter().next() {
            Some(iface_id) => Ok(Some(*iface_id)),
            None => Ok(None),
        }
    }

    async fn destroy_ap_iface(&mut self, iface_id: u16) -> Result<(), PhyManagerError> {
        let mut result = Ok(());
        let mut failing_phy = None;

        // If the interface has already been destroyed, return Ok.  Only error out in the case that
        // the request to destroy the interface results in a failure.
        for (phy_id, phy_container) in self.phys.iter_mut() {
            if phy_container.ap_ifaces.remove(&iface_id) {
                match destroy_iface(
                    &self.device_monitor,
                    iface_id,
                    fidl_common::WlanMacRole::Ap,
                    &self.telemetry_sender,
                )
                .await
                {
                    Ok(()) => {
                        let _ = phy_container.destroyed_ifaces.insert(iface_id);
                    }
                    Err(e) => {
                        let _ = phy_container.ap_ifaces.insert(iface_id);
                        result = Err(e);
                        failing_phy = Some(*phy_id);
                    }
                }
                break;
            }
        }

        if let (Err(_), Some(phy_id)) = (result.as_ref(), failing_phy) {
            self.record_defect(Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id }))
        }

        result
    }

    async fn destroy_all_ap_ifaces(&mut self) -> Result<(), PhyManagerError> {
        let ap_capable_phys = self.phys_for_role(fidl_common::WlanMacRole::Ap);
        let mut result = Ok(());
        let mut failing_phys = Vec::new();

        for ap_phy in ap_capable_phys.iter() {
            let phy_container =
                self.phys.get_mut(ap_phy).ok_or(PhyManagerError::PhyQueryFailure)?;

            // Continue tracking interface IDs for which deletion fails.
            let mut lingering_ifaces = HashSet::new();
            for iface_id in phy_container.ap_ifaces.drain() {
                match destroy_iface(
                    &self.device_monitor,
                    iface_id,
                    fidl_common::WlanMacRole::Ap,
                    &self.telemetry_sender,
                )
                .await
                {
                    Ok(()) => {
                        let _ = phy_container.destroyed_ifaces.insert(iface_id);
                    }
                    Err(e) => {
                        result = Err(e);
                        failing_phys.push(ap_phy);
                        let _ = lingering_ifaces.insert(iface_id);
                    }
                }
            }
            phy_container.ap_ifaces = lingering_ifaces;
        }

        if result.is_err() {
            for phy_id in failing_phys {
                self.record_defect(Defect::Phy(PhyFailure::IfaceDestructionFailure {
                    phy_id: *phy_id,
                }))
            }
        }

        result
    }

    fn suggest_ap_mac(&mut self, mac: MacAddr) {
        self.suggested_ap_mac = Some(mac);
    }

    fn get_phy_ids(&self) -> Vec<u16> {
        self.phys.keys().cloned().collect()
    }

    fn log_phy_add_failure(&mut self) {
        let _ = self.phy_add_fail_count.add(1);
    }

    async fn set_country_code(
        &mut self,
        country_code: Option<client_types::CountryCode>,
    ) -> Result<(), PhyManagerError> {
        self.saved_country_code = country_code;

        match country_code {
            Some(country_code) => {
                for phy_id in self.phys.keys() {
                    set_phy_country_code(&self.device_monitor, *phy_id, country_code).await?;
                }
            }
            None => {
                for phy_id in self.phys.keys() {
                    clear_phy_country_code(&self.device_monitor, *phy_id).await?;
                }
            }
        }

        Ok(())
    }

    fn record_defect(&mut self, defect: Defect) {
        let mut recovery_action = None;

        match defect {
            Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id }) => {
                if let Some(container) = self.phys.get_mut(&phy_id) {
                    container.defects.add_event(defect);
                    recovery_action = (self.recovery_profile)(
                        phy_id,
                        &mut container.defects,
                        &mut container.recoveries,
                        defect,
                    )
                }
            }
            Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id }) => {
                if let Some(container) = self.phys.get_mut(&phy_id) {
                    container.defects.add_event(defect);
                    recovery_action = (self.recovery_profile)(
                        phy_id,
                        &mut container.defects,
                        &mut container.recoveries,
                        defect,
                    )
                }
            }
            Defect::Iface(IfaceFailure::CanceledScan { iface_id })
            | Defect::Iface(IfaceFailure::FailedScan { iface_id })
            | Defect::Iface(IfaceFailure::EmptyScanResults { iface_id })
            | Defect::Iface(IfaceFailure::ConnectionFailure { iface_id }) => {
                for (phy_id, phy_info) in self.phys.iter_mut() {
                    if phy_info.client_ifaces.contains(&iface_id)
                        || phy_info.destroyed_ifaces.contains(&iface_id)
                    {
                        phy_info.defects.add_event(defect);

                        recovery_action = (self.recovery_profile)(
                            *phy_id,
                            &mut phy_info.defects,
                            &mut phy_info.recoveries,
                            defect,
                        );

                        break;
                    }
                }
            }
            Defect::Iface(IfaceFailure::ApStartFailure { iface_id }) => {
                for (phy_id, phy_info) in self.phys.iter_mut() {
                    if phy_info.ap_ifaces.contains(&iface_id)
                        || phy_info.destroyed_ifaces.contains(&iface_id)
                    {
                        phy_info.defects.add_event(defect);

                        recovery_action = (self.recovery_profile)(
                            *phy_id,
                            &mut phy_info.defects,
                            &mut phy_info.recoveries,
                            defect,
                        );

                        break;
                    }
                }
            }
            Defect::Iface(IfaceFailure::Timeout { iface_id, source }) => {
                self.telemetry_sender.send(TelemetryEvent::SmeTimeout { source });

                for (phy_id, phy_info) in self.phys.iter_mut() {
                    if phy_info.ap_ifaces.contains(&iface_id)
                        || phy_info.client_ifaces.contains(&iface_id)
                        || phy_info.destroyed_ifaces.contains(&iface_id)
                    {
                        phy_info.defects.add_event(defect);

                        recovery_action = (self.recovery_profile)(
                            *phy_id,
                            &mut phy_info.defects,
                            &mut phy_info.recoveries,
                            defect,
                        );

                        break;
                    }
                }
            }
        }

        if let Some(recovery_action) = recovery_action.take()
            && let Err(e) = self
                .recovery_action_sender
                .try_send(recovery::RecoverySummary::new(defect, recovery_action))
        {
            warn!("Unable to suggest recovery action {:?}: {:?}", recovery_action, e);
        }
    }

    async fn perform_recovery(&mut self, summary: recovery::RecoverySummary) {
        self.log_recovery_action(summary);

        if self.recovery_enabled {
            match summary.action {
                RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id }) => {
                    for phy_container in self.phys.values_mut() {
                        if phy_container.ap_ifaces.remove(&iface_id) {
                            #[allow(
                                clippy::redundant_pattern_matching,
                                reason = "mass allow for https://fxbug.dev/381896734"
                            )]
                            if let Err(_) = destroy_iface(
                                &self.device_monitor,
                                iface_id,
                                fidl_common::WlanMacRole::Ap,
                                &self.telemetry_sender,
                            )
                            .await
                            {
                                let _ = phy_container.ap_ifaces.insert(iface_id);
                            } else {
                                let _ = phy_container.destroyed_ifaces.insert(iface_id);
                            }

                            return;
                        }

                        if phy_container.client_ifaces.remove(&iface_id) {
                            if destroy_iface(
                                &self.device_monitor,
                                iface_id,
                                fidl_common::WlanMacRole::Client,
                                &self.telemetry_sender,
                            )
                            .await
                            .is_err()
                            {
                                let _ = phy_container.client_ifaces.insert(iface_id);
                            } else {
                                let _ = phy_container.destroyed_ifaces.insert(iface_id);
                            }

                            return;
                        }
                    }

                    warn!(
                        "Recovery suggested destroying iface {}, but no record was found.",
                        iface_id
                    );
                }
                RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id }) => {
                    for recorded_phy_id in self.phys.keys() {
                        if phy_id == *recorded_phy_id {
                            if let Err(e) = reset_phy(&self.device_monitor, phy_id).await {
                                warn!("Resetting PHY {} failed: {:?}", phy_id, e);
                            }

                            // The phy reset may clear its country code. Re-set it now if we have one.
                            if let Some(country_code) = self.saved_country_code {
                                info!("Setting country code after phy reset");
                                if let Err(e) =
                                    set_phy_country_code(&self.device_monitor, phy_id, country_code)
                                        .await
                                {
                                    warn!(
                                        "Proceeding with default country code because we failed to set the cached one: {}",
                                        e
                                    );
                                }
                            };

                            return;
                        }
                    }
                }
                RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect { iface_id }) => {
                    if let Err(e) = disconnect(&self.device_monitor, iface_id).await {
                        warn!("Disconnecting client {} failed: {:?}", iface_id, e);
                    }
                }
                RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp { iface_id }) => {
                    if let Err(e) = stop_ap(&self.device_monitor, iface_id).await {
                        warn!("Stopping AP {} failed: {:?}", iface_id, e);
                    }
                }
            }
        }
    }
}

/// Destroys the specified interface.
async fn destroy_iface(
    proxy: &fidl_service::DeviceMonitorProxy,
    iface_id: u16,
    role: fidl_common::WlanMacRole,
    telemetry_sender: &TelemetrySender,
) -> Result<(), PhyManagerError> {
    let request = fidl_service::DestroyIfaceRequest { iface_id };
    let (destroy_iface_response, metric) = match proxy.destroy_iface(&request).await {
        Ok(status) => match status {
            zx::sys::ZX_OK => (Ok(()), Some(Ok(iface_id))),
            zx::sys::ZX_ERR_NOT_FOUND => {
                info!("Interface not found, assuming it is already destroyed");
                // Don't return a metric here, we neither succeeded nor failed to destroy
                (Ok(()), None)
            }
            e => {
                warn!("failed to destroy iface {}: {}", iface_id, e);
                (Err(PhyManagerError::IfaceDestroyFailure), Some(Err(())))
            }
        },
        Err(e) => {
            warn!("failed to send destroy iface {}: {}", iface_id, e);
            (Err(PhyManagerError::IfaceDestroyFailure), Some(Err(())))
        }
    };

    if let Some(result) = metric {
        telemetry_sender.send(TelemetryEvent::IfaceDestructionResult { role, result });
    }

    destroy_iface_response
}

async fn reset_phy(
    proxy: &fidl_service::DeviceMonitorProxy,
    phy_id: u16,
) -> Result<(), PhyManagerError> {
    let result = proxy.reset(phy_id).await.map_err(|e| {
        warn!("Request to reset PHY {} failed: {:?}", phy_id, e);
        PhyManagerError::InternalError
    })?;

    result.map_err(|e| {
        warn!("Failed to reset PHY {}: {:?}", phy_id, e);
        PhyManagerError::PhyResetFailure
    })
}

async fn set_phy_country_code(
    proxy: &fidl_service::DeviceMonitorProxy,
    phy_id: u16,
    country_code: client_types::CountryCode,
) -> Result<(), PhyManagerError> {
    let status = proxy
        .set_country(&fidl_service::SetCountryRequest { phy_id, alpha2: country_code.into() })
        .await
        .map_err(|e| {
            error!("Failed to set country code for PHY {}: {:?}", phy_id, e);
            PhyManagerError::PhySetCountryFailure
        })?;

    zx::ok(status).map_err(|e| {
        error!("Received bad status when setting country code for PHY {}: {}", phy_id, e);
        PhyManagerError::PhySetCountryFailure
    })
}

async fn clear_phy_country_code(
    proxy: &fidl_service::DeviceMonitorProxy,
    phy_id: u16,
) -> Result<(), PhyManagerError> {
    let status =
        proxy.clear_country(&fidl_service::ClearCountryRequest { phy_id }).await.map_err(|e| {
            error!("Failed to clear country code for PHY {}: {:?}", phy_id, e);
            PhyManagerError::PhySetCountryFailure
        })?;

    zx::ok(status).map_err(|e| {
        error!("Received bad status when clearing country code for PHY {}: {}", phy_id, e);
        PhyManagerError::PhySetCountryFailure
    })
}

async fn disconnect(
    dev_monitor_proxy: &fidl_service::DeviceMonitorProxy,
    iface_id: u16,
) -> Result<(), Error> {
    let (sme_proxy, remote) = create_proxy();
    dev_monitor_proxy.get_client_sme(iface_id, remote).await?.map_err(zx::Status::err_from_raw)?;

    sme_proxy
        .disconnect(fidl_sme::UserDisconnectReason::Recovery)
        .await
        .map_err(|e| format_err!("Disconnect failed: {:?}", e))
}

async fn stop_ap(
    dev_monitor_proxy: &fidl_service::DeviceMonitorProxy,
    iface_id: u16,
) -> Result<(), Error> {
    let (sme_proxy, remote) = create_proxy();
    dev_monitor_proxy.get_ap_sme(iface_id, remote).await?.map_err(zx::Status::err_from_raw)?;

    match sme_proxy.stop().await {
        Ok(result) => match result {
            fidl_sme::StopApResultCode::Success => Ok(()),
            err => Err(format_err!("Stop AP failed: {:?}", err)),
        },
        Err(e) => Err(format_err!("Stop AP request failed: {:?}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry;
    use crate::util::testing::{poll_ap_sme_req, poll_sme_req};
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints;
    use fidl_fuchsia_wlan_device_service as fidl_service;
    use fidl_fuchsia_wlan_sme as fidl_sme;
    use fuchsia_async::TestExecutor;
    use fuchsia_inspect as inspect;
    use futures::channel::mpsc;
    use futures::stream::StreamExt;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;
    use zx::sys::{ZX_ERR_NOT_FOUND, ZX_OK};

    /// Hold the client and service ends for DeviceMonitor to allow mocking DeviceMonitor responses
    /// for unit tests.
    struct TestValues {
        monitor_proxy: fidl_service::DeviceMonitorProxy,
        monitor_stream: fidl_service::DeviceMonitorRequestStream,
        inspector: inspect::Inspector,
        node: inspect::Node,
        telemetry_sender: TelemetrySender,
        telemetry_receiver: mpsc::Receiver<TelemetryEvent>,
        recovery_sender: recovery::RecoveryActionSender,
        recovery_receiver: recovery::RecoveryActionReceiver,
    }

    /// Create a TestValues for a unit test.
    fn test_setup() -> TestValues {
        let (monitor_proxy, monitor_requests) =
            endpoints::create_proxy::<fidl_service::DeviceMonitorMarker>();
        let monitor_stream = monitor_requests.into_stream();

        let inspector = inspect::Inspector::default();
        let node = inspector.root().create_child("phy_manager");
        let (sender, telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(sender);
        let (recovery_sender, recovery_receiver) =
            mpsc::channel::<recovery::RecoverySummary>(recovery::RECOVERY_SUMMARY_CHANNEL_CAPACITY);

        TestValues {
            monitor_proxy,
            monitor_stream,
            inspector,
            node,
            telemetry_sender,
            telemetry_receiver,
            recovery_sender,
            recovery_receiver,
        }
    }

    /// Take in the service side of a DeviceMonitor::GetSupportedMacRoles request and respond with
    /// the given WlanMacRoles responst.
    fn send_get_supported_mac_roles_response(
        exec: &mut TestExecutor,
        server: &mut fidl_service::DeviceMonitorRequestStream,
        supported_mac_roles: Result<&[fidl_common::WlanMacRole], zx::sys::zx_status_t>,
    ) {
        let _ = assert_matches!(
            exec.run_until_stalled(&mut server.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::GetSupportedMacRoles {
                    responder, ..
                }
            ))) => {
                responder.send(supported_mac_roles)
            }
        );
    }

    /// Create a PhyInfo object for unit testing.
    #[track_caller]
    fn send_query_iface_response(
        exec: &mut TestExecutor,
        server: &mut fidl_service::DeviceMonitorRequestStream,
        iface_info: Option<fidl_service::QueryIfaceResponse>,
    ) {
        let response = iface_info.as_ref().ok_or(ZX_ERR_NOT_FOUND);
        assert_matches!(
            exec.run_until_stalled(&mut server.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::QueryIface {
                    iface_id: _,
                    responder,
                }
            ))) => {
                responder.send(response).expect("sending fake iface info");
            }
        );
    }

    /// Handles the service side of a DeviceMonitor::CreateIface request by replying with the
    /// provided optional iface ID.
    #[track_caller]
    fn send_create_iface_response(
        exec: &mut TestExecutor,
        server: &mut fidl_service::DeviceMonitorRequestStream,
        iface_id: Option<u16>,
    ) {
        assert_matches!(
            exec.run_until_stalled(&mut server.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::CreateIface {
                    responder,
                    ..
                }
            ))) => {
                match iface_id {
                    Some(iface_id) => responder.send(
                        Ok(&fidl_service::DeviceMonitorCreateIfaceResponse {
                            iface_id: Some(iface_id),
                            ..Default::default()
                        })
                    )
                    .expect("sending fake iface id"),
                    None => responder.send(Err(fidl_service::DeviceMonitorError::unknown())).expect("sending fake response with none")
                }
            }
        );
    }

    /// Handles the service side of a DeviceMonitor::DestroyIface request by replying with the
    /// provided zx_status_t.
    fn send_destroy_iface_response(
        exec: &mut TestExecutor,
        server: &mut fidl_service::DeviceMonitorRequestStream,
        return_status: zx::sys::zx_status_t,
    ) {
        assert_matches!(
            exec.run_until_stalled(&mut server.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::DestroyIface {
                    req: _,
                    responder,
                }
            ))) => {
                responder
                    .send(return_status)
                    .unwrap_or_else(|e| panic!("sending fake response: {return_status}: {e:?}"));
            }
        );
    }

    /// Creates a QueryIfaceResponse from the arguments provided by the caller.
    fn create_iface_response(
        role: fidl_common::WlanMacRole,
        id: u16,
        phy_id: u16,
        phy_assigned_id: u16,
        sta_addr: [u8; 6],
        factory_addr: [u8; 6],
    ) -> fidl_service::QueryIfaceResponse {
        fidl_service::QueryIfaceResponse {
            role,
            id,
            phy_id,
            phy_assigned_id,
            sta_addr,
            factory_addr,
        }
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnPhyCreated event and
    /// calling add_phy on PhyManager for a PHY that exists.  The expectation is that the
    /// PhyManager initially does not have any PHYs available.  After the call to add_phy, the
    /// PhyManager should have a new PhyContainer.
    #[fuchsia::test]
    fn add_valid_phy() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        let fake_phy_id = 0;
        let fake_mac_roles = vec![];

        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        {
            let add_phy_fut = phy_manager.add_phy(0);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));
        assert_eq!(
            phy_manager.phys.get(&fake_phy_id).unwrap().supported_mac_roles,
            fake_mac_roles.into_iter().collect()
        );
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnPhyCreated event and
    /// calling add_phy on PhyManager for a PHY that does not exist.  The PhyManager in this case
    /// should not create and store a new PhyContainer.
    #[fuchsia::test]
    fn add_invalid_phy() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        {
            let add_phy_fut = phy_manager.add_phy(1);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Err(zx::sys::ZX_ERR_NOT_FOUND),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }
        assert!(phy_manager.phys.is_empty());
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnPhyCreated event and
    /// calling add_phy on PhyManager for a PHY that has already been accounted for, but whose
    /// properties have changed.  The PhyManager in this case should update the associated PhyInfo.
    #[fuchsia::test]
    fn add_duplicate_phy() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let fake_phy_id = 0;
        let fake_mac_roles = vec![];

        {
            let add_phy_fut = phy_manager.add_phy(fake_phy_id);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        {
            assert!(phy_manager.phys.contains_key(&fake_phy_id));
            assert_eq!(
                phy_manager.phys.get(&fake_phy_id).unwrap().supported_mac_roles,
                fake_mac_roles.clone().into_iter().collect()
            );
        }

        // Send an update for the same PHY ID and ensure that the PHY info is updated.
        {
            let add_phy_fut = phy_manager.add_phy(fake_phy_id);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));
        assert_eq!(
            phy_manager.phys.get(&fake_phy_id).unwrap().supported_mac_roles,
            fake_mac_roles.into_iter().collect()
        );
    }

    #[fuchsia::test]
    fn create_all_client_ifaces_after_phys_added() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        for phy_id in 0..2 {
            {
                let add_phy_fut = phy_manager.add_phy(phy_id);
                let mut add_phy_fut = pin!(add_phy_fut);

                assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

                send_get_supported_mac_roles_response(
                    &mut exec,
                    &mut test_values.monitor_stream,
                    Ok(&[fidl_common::WlanMacRole::Client]),
                );

                assert_matches!(exec.run_until_stalled(&mut add_phy_fut), Poll::Ready(Ok(())));
            }
            assert!(phy_manager.phys.contains_key(&phy_id));
        }

        {
            let start_connections_fut = phy_manager
                .create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
            let mut start_connections_fut = pin!(start_connections_fut);

            // This is a little fragile since it may not be guaranteed that the create iface calls
            // come in on the phys in the same order they're added.
            for iface_id in [10, 20] {
                assert!(exec.run_until_stalled(&mut start_connections_fut).is_pending());

                send_create_iface_response(
                    &mut exec,
                    &mut test_values.monitor_stream,
                    Some(iface_id),
                );
            }

            assert_matches!(exec.run_until_stalled(&mut start_connections_fut),
                Poll::Ready(iface_ids) => {
                    assert!(iface_ids.values().all(Result::is_ok));
                    assert!(iface_ids.contains_key(&0));
                    assert!(iface_ids.contains_key(&1));
                    let iface_ids: HashSet<_> = iface_ids.into_values().flat_map(Result::unwrap).collect();
                    assert_eq!(iface_ids, HashSet::from([10, 20]));
                }
            );
        }

        let mut iface_ids = HashSet::new();
        for phy_id in 0..2 {
            let phy_container = phy_manager.phys.get(&phy_id).unwrap();
            // Because of how this test is mocked, the iface ids could be assigned in
            // either order.
            assert_eq!(phy_container.client_ifaces.len(), 1);
            phy_container.client_ifaces.iter().for_each(|iface_id| {
                assert!(iface_ids.insert(*iface_id));
            });
            assert!(phy_container.defects.events.is_empty());
        }
        assert_eq!(iface_ids, HashSet::from([10, 20]));
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnPhyRemoved event and
    /// calling remove_phy on PhyManager for a PHY that not longer exists.  The PhyManager in this
    /// case should remove the PhyContainer associated with the removed PHY ID.
    #[fuchsia::test]
    fn add_phy_after_create_all_client_ifaces() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];

        {
            let start_connections_fut = phy_manager
                .create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
            let mut start_connections_fut = pin!(start_connections_fut);
            assert!(exec.run_until_stalled(&mut start_connections_fut).is_ready());
        }

        // Add a new phy.  Since client connections have been started, it should also create a
        // client iface.
        {
            let add_phy_fut = phy_manager.add_phy(fake_phy_id);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_create_iface_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Some(fake_iface_id),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));
        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.client_ifaces.contains(&fake_iface_id));
        assert!(phy_container.defects.events.is_empty());
    }

    /// Tests the case where a PHY is added after client connections have been enabled but creating
    /// an interface for the new PHY fails.  In this case, the PHY is not added.
    ///
    /// If this behavior changes, defect accounting needs to be updated and tested here.
    #[fuchsia::test]
    fn add_phy_with_iface_creation_failure() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];

        {
            let start_connections_fut = phy_manager
                .create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
            let mut start_connections_fut = pin!(start_connections_fut);
            assert!(exec.run_until_stalled(&mut start_connections_fut).is_ready());
        }

        // Add a new phy.  Since client connections have been started, it should also create a
        // client iface.
        {
            let add_phy_fut = phy_manager.add_phy(fake_phy_id);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            // Send back an error to mimic a failure to create an interface.
            send_create_iface_response(&mut exec, &mut test_values.monitor_stream, None);

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        assert!(!phy_manager.phys.contains_key(&fake_phy_id));
    }

    /// Tests the case where a new PHY is discovered after the country code has been set.
    #[fuchsia::test]
    fn test_add_phy_after_setting_country_code() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        {
            let set_country_fut = phy_manager.set_country_code(Some("US".parse().unwrap()));
            let mut set_country_fut = pin!(set_country_fut);
            assert_matches!(exec.run_until_stalled(&mut set_country_fut), Poll::Ready(Ok(())));
        }

        {
            let add_phy_fut = phy_manager.add_phy(fake_phy_id);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());

            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                Poll::Ready(Some(Ok(
                    fidl_service::DeviceMonitorRequest::SetCountry {
                        req: fidl_service::SetCountryRequest {
                            phy_id: 1,
                            alpha2: [b'U', b'S'],
                        },
                        responder,
                    }
                ))) => {
                    responder.send(ZX_OK).expect("sending fake set country response");
                }
            );

            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));
        assert_eq!(
            phy_manager.phys.get(&fake_phy_id).unwrap().supported_mac_roles,
            fake_mac_roles.into_iter().collect()
        );
    }

    #[fuchsia::test]
    async fn remove_valid_phy() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        let phy_container = PhyContainer::new(fake_mac_roles);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);
        phy_manager.remove_phy(fake_phy_id);
        assert!(phy_manager.phys.is_empty());
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnPhyRemoved event and
    /// calling remove_phy on PhyManager for a PHY ID that is not accounted for by the PhyManager.
    /// The PhyManager should realize that it is unaware of this PHY ID and leave its PhyContainers
    /// unchanged.
    #[fuchsia::test]
    async fn remove_nonexistent_phy() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        let phy_container = PhyContainer::new(fake_mac_roles);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);
        phy_manager.remove_phy(2);
        assert!(phy_manager.phys.contains_key(&fake_phy_id));
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnIfaceAdded event for
    /// an iface that belongs to a PHY that has been accounted for.  The PhyManager should add the
    /// newly discovered iface to the existing PHY's list of client ifaces.
    #[fuchsia::test]
    fn on_iface_added() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        let phy_container = PhyContainer::new(fake_mac_roles);

        // Create an IfaceResponse to be sent to the PhyManager when the iface ID is queried
        let fake_role = fidl_common::WlanMacRole::Client;
        let fake_iface_id = 1;
        let fake_phy_assigned_id = 1;
        let fake_sta_addr = [0, 1, 2, 3, 4, 5];
        let fake_factory_addr = [0, 1, 2, 3, 4, 5];
        let iface_response = create_iface_response(
            fake_role,
            fake_iface_id,
            fake_phy_id,
            fake_phy_assigned_id,
            fake_sta_addr,
            fake_factory_addr,
        );

        {
            // Inject the fake PHY information
            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Add the fake iface
            let on_iface_added_fut = phy_manager.on_iface_added(fake_iface_id);
            let mut on_iface_added_fut = pin!(on_iface_added_fut);
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_pending());

            send_query_iface_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Some(iface_response),
            );

            // Wait for the PhyManager to finish processing the received iface information
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_ready());
        }

        // Expect that the PhyContainer associated with the fake PHY has been updated with the
        // fake client
        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.client_ifaces.contains(&fake_iface_id));
    }

    #[fuchsia::test]
    fn on_iface_added_unknown_role_is_unsupported() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        let phy_container = PhyContainer::new(fake_mac_roles);

        // Create an IfaceResponse to be sent to the PhyManager when the iface ID is queried
        let fake_role = fidl_common::WlanMacRole::unknown();
        let fake_iface_id = 1;
        let fake_phy_assigned_id = 1;
        let fake_sta_addr = [0, 1, 2, 3, 4, 5];
        let fake_factory_addr = [0, 1, 2, 3, 4, 5];
        let iface_response = create_iface_response(
            fake_role,
            fake_iface_id,
            fake_phy_id,
            fake_phy_assigned_id,
            fake_sta_addr,
            fake_factory_addr,
        );

        {
            // Inject the fake PHY information
            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Add the fake iface
            let on_iface_added_fut = phy_manager.on_iface_added(fake_iface_id);
            let mut on_iface_added_fut = pin!(on_iface_added_fut);
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_pending());

            send_query_iface_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Some(iface_response),
            );

            // Show that on_iface_added results in an error since unknown WlanMacRole is unsupported
            assert_matches!(
                exec.run_until_stalled(&mut on_iface_added_fut),
                Poll::Ready(Err(PhyManagerError::Unsupported))
            );
        }

        // Expect that the PhyContainer associated with the fake PHY has been updated with the
        // fake client
        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(!phy_container.client_ifaces.contains(&fake_iface_id));
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnIfaceAdded event for
    /// an iface that belongs to a PHY that has not been accounted for.  The PhyManager should
    /// query the PHY's information, create a new PhyContainer, and insert the new iface ID into
    /// the PHY's list of client ifaces.
    #[fuchsia::test]
    fn on_iface_added_missing_phy() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        // Create an IfaceResponse to be sent to the PhyManager when the iface ID is queried
        let fake_role = fidl_common::WlanMacRole::Client;
        let fake_iface_id = 1;
        let fake_phy_assigned_id = 1;
        let fake_sta_addr = [0, 1, 2, 3, 4, 5];
        let fake_factory_addr = [0, 1, 2, 3, 4, 5];
        let iface_response = create_iface_response(
            fake_role,
            fake_iface_id,
            fake_phy_id,
            fake_phy_assigned_id,
            fake_sta_addr,
            fake_factory_addr,
        );

        {
            // Add the fake iface
            let on_iface_added_fut = phy_manager.on_iface_added(fake_iface_id);
            let mut on_iface_added_fut = pin!(on_iface_added_fut);

            // Since the PhyManager has not accounted for any PHYs, it will get the iface
            // information first and then query for the iface's PHY's information.

            // The iface query goes out first
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_pending());

            send_query_iface_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Some(iface_response),
            );

            // And then the PHY information is queried.
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_pending());

            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&fake_mac_roles),
            );

            // Wait for the PhyManager to finish processing the received iface information
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_ready());
        }

        // Expect that the PhyContainer associated with the fake PHY has been updated with the
        // fake client
        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.client_ifaces.contains(&fake_iface_id));
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnIfaceAdded event for
    /// an iface that was created by PhyManager and has already been accounted for.  The PhyManager
    /// should simply ignore the duplicate iface ID and not append it to its list of clients.
    #[fuchsia::test]
    fn add_duplicate_iface() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        // Inject the fake PHY information
        let phy_container = PhyContainer::new(fake_mac_roles);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Create an IfaceResponse to be sent to the PhyManager when the iface ID is queried
        let fake_role = fidl_common::WlanMacRole::Client;
        let fake_iface_id = 1;
        let fake_phy_assigned_id = 1;
        let fake_sta_addr = [0, 1, 2, 3, 4, 5];
        let fake_factory_addr = [0, 1, 2, 3, 4, 5];
        let iface_response = create_iface_response(
            fake_role,
            fake_iface_id,
            fake_phy_id,
            fake_phy_assigned_id,
            fake_sta_addr,
            fake_factory_addr,
        );

        // Add the same iface ID twice
        for _ in 0..2 {
            // Add the fake iface
            let on_iface_added_fut = phy_manager.on_iface_added(fake_iface_id);
            let mut on_iface_added_fut = pin!(on_iface_added_fut);
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_pending());

            send_query_iface_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Some(iface_response),
            );

            // Wait for the PhyManager to finish processing the received iface information
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_ready());
        }

        // Expect that the PhyContainer associated with the fake PHY has been updated with only one
        // reference to the fake client
        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert_eq!(phy_container.client_ifaces.len(), 1);
        assert!(phy_container.client_ifaces.contains(&fake_iface_id));
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnIfaceAdded event for
    /// an iface that has already been removed.  The PhyManager should fail to query the iface info
    /// and not account for the iface ID.
    #[fuchsia::test]
    fn add_nonexistent_iface() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        {
            // Add the non-existent iface
            let on_iface_added_fut = phy_manager.on_iface_added(1);
            let mut on_iface_added_fut = pin!(on_iface_added_fut);
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_pending());

            send_query_iface_response(&mut exec, &mut test_values.monitor_stream, None);

            // Wait for the PhyManager to finish processing the received iface information
            assert!(exec.run_until_stalled(&mut on_iface_added_fut).is_ready());
        }

        // Expect that the PhyContainer associated with the fake PHY has been updated with the
        // fake client
        assert!(phy_manager.phys.is_empty());
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnIfaceRemoved event
    /// for an iface that has been accounted for by the PhyManager.  The PhyManager should remove
    /// the iface ID from the PHY's list of client ifaces.
    #[fuchsia::test]
    async fn test_on_iface_removed() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        // Inject the fake PHY information
        let mut phy_container = PhyContainer::new(fake_mac_roles);
        let fake_iface_id = 1;
        let _ = phy_container.client_ifaces.insert(fake_iface_id);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        phy_manager.on_iface_removed(fake_iface_id);

        // Expect that the iface ID has been removed from the PhyContainer
        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.client_ifaces.is_empty());
    }

    /// This test mimics a client of the DeviceWatcher watcher receiving an OnIfaceRemoved event
    /// for an iface that has not been accounted for.  The PhyManager should simply ignore the
    /// request and leave its list of client iface IDs unchanged.
    #[fuchsia::test]
    async fn remove_missing_iface() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![];

        let present_iface_id = 1;
        let removed_iface_id = 2;

        // Inject the fake PHY information
        let mut phy_container = PhyContainer::new(fake_mac_roles);
        let _ = phy_container.client_ifaces.insert(present_iface_id);
        let _ = phy_container.client_ifaces.insert(removed_iface_id);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);
        phy_manager.on_iface_removed(removed_iface_id);

        // Expect that the iface ID has been removed from the PhyContainer
        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert_eq!(phy_container.client_ifaces.len(), 1);
        assert!(phy_container.client_ifaces.contains(&present_iface_id));
    }

    /// Tests the response of the PhyManager when a client iface is requested, but no PHYs are
    /// present.  The expectation is that the PhyManager returns None.
    #[fuchsia::test]
    async fn get_client_no_phys() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let client = phy_manager.get_client();
        assert!(client.is_none());
    }

    /// Tests the response of the PhyManager when a client iface is requested, a client-capable PHY
    /// has been discovered, but client connections have not been started.  The expectation is that
    /// the PhyManager returns None.
    #[fuchsia::test]
    async fn get_unconfigured_client() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let phy_container = PhyContainer::new(fake_mac_roles);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Retrieve the client ID
        let client = phy_manager.get_client();
        assert!(client.is_none());
    }

    /// Tests the response of the PhyManager when a client iface is requested and a client iface is
    /// present.  The expectation is that the PhyManager should reply with the iface ID of the
    /// client iface.
    #[fuchsia::test]
    async fn get_configured_client() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        phy_manager.client_connections_enabled = true;

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let phy_container = PhyContainer::new(fake_mac_roles);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Insert the fake iface
        let fake_iface_id = 1;
        let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
        let _ = phy_container.client_ifaces.insert(fake_iface_id);

        // Retrieve the client ID
        let client = phy_manager.get_client();
        assert_eq!(client.unwrap(), fake_iface_id)
    }

    /// Tests the response of the PhyManager when a client iface is requested and the only PHY
    /// that is present does not support client ifaces and has an AP iface present.  The
    /// expectation is that the PhyManager returns None.
    #[fuchsia::test]
    async fn get_client_no_compatible_phys() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];
        let mut phy_container = PhyContainer::new(fake_mac_roles);
        let _ = phy_container.ap_ifaces.insert(fake_iface_id);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Retrieve the client ID
        let client = phy_manager.get_client();
        assert!(client.is_none());
    }

    /// Tests that PhyManager will not return a client interface when client connections are not
    /// enabled.
    #[fuchsia::test]
    fn get_client_while_stopped() {
        let _exec = TestExecutor::new();
        let test_values = test_setup();

        // Create a new PhyManager.  On construction, client connections are disabled.
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        assert!(!phy_manager.client_connections_enabled);

        // Add a PHY with a lingering client interface.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let mut phy_container = PhyContainer::new(fake_mac_roles);
        let _ = phy_container.client_ifaces.insert(1);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Try to get a client interface.  No interface should be returned since client connections
        // are disabled.
        assert_eq!(phy_manager.get_client(), None);
    }

    /// Tests the PhyManager's response to stop_client_connection when there is an existing client
    /// iface.  The expectation is that the client iface is destroyed and there is no remaining
    /// record of the iface ID in the PhyManager.
    #[fuchsia::test]
    fn destroy_all_client_ifaces() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let phy_container = PhyContainer::new(fake_mac_roles);

        {
            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.client_ifaces.insert(fake_iface_id);

            // Stop client connections
            let stop_clients_future = phy_manager.destroy_all_client_ifaces();
            let mut stop_clients_future = pin!(stop_clients_future);

            assert!(exec.run_until_stalled(&mut stop_clients_future).is_pending());

            send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

            assert!(exec.run_until_stalled(&mut stop_clients_future).is_ready());
        }

        // Ensure that the client interface that was added has been removed.
        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(!phy_container.client_ifaces.contains(&fake_iface_id));

        // Verify that the client_connections_enabled has been set to false.
        assert!(!phy_manager.client_connections_enabled);

        // Verify that the destroyed interface ID was recorded.
        assert!(phy_container.destroyed_ifaces.contains(&fake_iface_id));
    }

    /// Tests the PhyManager's response to destroy_all_client_ifaces when no client ifaces are
    /// present but an AP iface is present.  The expectation is that the AP iface is left intact.
    #[fuchsia::test]
    fn destroy_all_client_ifaces_no_clients() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];
        let phy_container = PhyContainer::new(fake_mac_roles);

        // Insert the fake AP iface and then stop clients
        {
            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake AP iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.ap_ifaces.insert(fake_iface_id);

            // Stop client connections
            let stop_clients_future = phy_manager.destroy_all_client_ifaces();
            let mut stop_clients_future = pin!(stop_clients_future);

            assert!(exec.run_until_stalled(&mut stop_clients_future).is_ready());
        }

        // Ensure that the fake PHY and AP interface are still present.
        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.ap_ifaces.contains(&fake_iface_id));
    }

    /// This test validates the behavior when stopping client connections fails.
    #[fuchsia::test]
    fn destroy_all_client_ifaces_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the monitor stream so that the request to destroy the interface fails.
        drop(test_values.monitor_stream);

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let mut phy_container = PhyContainer::new(fake_mac_roles);

        // For the sake of this test, force the retention period to be indefinite to make sure
        // that an event is logged.
        phy_container.defects = EventHistory::<Defect>::new(u32::MAX);

        {
            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.client_ifaces.insert(fake_iface_id);

            // Stop client connections and expect the future to fail immediately.
            let stop_clients_future = phy_manager.destroy_all_client_ifaces();
            let mut stop_clients_future = pin!(stop_clients_future);
            assert!(exec.run_until_stalled(&mut stop_clients_future).is_ready());
        }

        // Ensure that the client interface is still present
        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.client_ifaces.contains(&fake_iface_id));
        assert_eq!(phy_container.defects.events.len(), 1);
        assert_eq!(
            phy_container.defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 1 })
        );
    }

    /// Tests the PhyManager's response to a request for an AP when no PHYs are present.  The
    /// expectation is that the PhyManager will return None in this case.
    #[fuchsia::test]
    fn get_ap_no_phys() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        let get_ap_future = phy_manager.create_or_get_ap_iface();

        let mut get_ap_future = pin!(get_ap_future);
        assert_matches!(exec.run_until_stalled(&mut get_ap_future), Poll::Ready(Ok(None)));
    }

    /// Tests the PhyManager's response when the PhyManager holds a PHY that can have an AP iface
    /// but the AP iface has not been created yet.  The expectation is that the PhyManager creates
    /// a new AP iface and returns its ID to the caller.
    #[fuchsia::test]
    fn get_unconfigured_ap() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];
        let phy_container = PhyContainer::new(fake_mac_roles.clone());

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Retrieve the AP interface ID
        let fake_iface_id = 1;
        {
            let get_ap_future = phy_manager.create_or_get_ap_iface();

            let mut get_ap_future = pin!(get_ap_future);
            assert!(exec.run_until_stalled(&mut get_ap_future).is_pending());

            send_create_iface_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Some(fake_iface_id),
            );
            assert_matches!(
                exec.run_until_stalled(&mut get_ap_future),
                Poll::Ready(Ok(Some(iface_id))) => assert_eq!(iface_id, fake_iface_id)
            );
        }

        assert!(phy_manager.phys[&fake_phy_id].ap_ifaces.contains(&fake_iface_id));
    }

    /// Tests the case where an AP interface is requested but interface creation fails.
    #[fuchsia::test]
    fn get_ap_iface_creation_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the monitor stream so that the request to destroy the interface fails.
        drop(test_values.monitor_stream);

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];
        let mut phy_container = PhyContainer::new(fake_mac_roles.clone());

        // For the sake of this test, force the retention period to be indefinite to make sure
        // that an event is logged.
        phy_container.defects = EventHistory::<Defect>::new(u32::MAX);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        {
            let get_ap_future = phy_manager.create_or_get_ap_iface();

            let mut get_ap_future = pin!(get_ap_future);
            assert!(exec.run_until_stalled(&mut get_ap_future).is_ready());
        }

        assert!(phy_manager.phys[&fake_phy_id].ap_ifaces.is_empty());
        assert_eq!(phy_manager.phys[&fake_phy_id].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&fake_phy_id].defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 1 })
        );
    }

    /// Tests the PhyManager's response to a create_or_get_ap_iface call when there is a PHY with an AP iface
    /// that has already been created.  The expectation is that the PhyManager should return the
    /// iface ID of the existing AP iface.
    #[fuchsia::test]
    fn get_configured_ap() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];
        let phy_container = PhyContainer::new(fake_mac_roles);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Insert the fake iface
        let fake_iface_id = 1;
        let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
        let _ = phy_container.ap_ifaces.insert(fake_iface_id);

        // Retrieve the AP iface ID
        let get_ap_future = phy_manager.create_or_get_ap_iface();
        let mut get_ap_future = pin!(get_ap_future);
        assert_matches!(
            exec.run_until_stalled(&mut get_ap_future),
            Poll::Ready(Ok(Some(iface_id))) => assert_eq!(iface_id, fake_iface_id)
        );
    }

    /// This test attempts to get an AP iface from a PhyManager that has a PHY that can only have
    /// a client interface.  The PhyManager should return None.
    #[fuchsia::test]
    fn get_ap_no_compatible_phys() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let phy_container = PhyContainer::new(fake_mac_roles);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Retrieve the client ID
        let get_ap_future = phy_manager.create_or_get_ap_iface();
        let mut get_ap_future = pin!(get_ap_future);
        assert_matches!(exec.run_until_stalled(&mut get_ap_future), Poll::Ready(Ok(None)));
    }

    /// This test stops a valid AP iface on a PhyManager.  The expectation is that the PhyManager
    /// should retain the record of the PHY, but the AP iface ID should be removed.
    #[fuchsia::test]
    fn stop_valid_ap_iface() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];

        {
            let phy_container = PhyContainer::new(fake_mac_roles.clone());

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.ap_ifaces.insert(fake_iface_id);

            // Remove the AP iface ID
            let destroy_ap_iface_future = phy_manager.destroy_ap_iface(fake_iface_id);
            let mut destroy_ap_iface_future = pin!(destroy_ap_iface_future);
            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_pending());
            send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(!phy_container.ap_ifaces.contains(&fake_iface_id));
        assert!(phy_container.defects.events.is_empty());
        assert!(phy_container.destroyed_ifaces.contains(&fake_iface_id));
    }

    /// This test attempts to stop an invalid AP iface ID.  The expectation is that a valid iface
    /// ID is unaffected.
    #[fuchsia::test]
    fn stop_invalid_ap_iface() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];

        {
            let phy_container = PhyContainer::new(fake_mac_roles);

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.ap_ifaces.insert(fake_iface_id);

            // Remove a non-existent AP iface ID
            let destroy_ap_iface_future = phy_manager.destroy_ap_iface(2);
            let mut destroy_ap_iface_future = pin!(destroy_ap_iface_future);
            assert_matches!(
                exec.run_until_stalled(&mut destroy_ap_iface_future),
                Poll::Ready(Ok(()))
            );
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.ap_ifaces.contains(&fake_iface_id));
        assert!(phy_container.defects.events.is_empty());
    }

    /// This test fails to stop a valid AP iface on a PhyManager.  The expectation is that the
    /// PhyManager should retain the AP interface and log a defect.
    #[fuchsia::test]
    fn stop_ap_iface_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the monitor stream so that the request to destroy the interface fails.
        drop(test_values.monitor_stream);

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];

        {
            let mut phy_container = PhyContainer::new(fake_mac_roles.clone());

            // For the sake of this test, force the retention period to be indefinite to make sure
            // that an event is logged.
            phy_container.defects = EventHistory::<Defect>::new(u32::MAX);

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.ap_ifaces.insert(fake_iface_id);

            // Remove the AP iface ID
            let destroy_ap_iface_future = phy_manager.destroy_ap_iface(fake_iface_id);
            let mut destroy_ap_iface_future = pin!(destroy_ap_iface_future);
            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.ap_ifaces.contains(&fake_iface_id));
        assert_eq!(phy_container.defects.events.len(), 1);
        assert_eq!(
            phy_container.defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 1 })
        );
    }

    /// This test attempts to stop an invalid AP iface ID.  The expectation is that a valid iface
    /// This test creates two AP ifaces for a PHY that supports AP ifaces.  destroy_all_ap_ifaces is then
    /// called on the PhyManager.  The expectation is that both AP ifaces should be destroyed and
    /// the records of the iface IDs should be removed from the PhyContainer.
    #[fuchsia::test]
    fn stop_all_ap_ifaces() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // ifaces are added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];

        {
            let phy_container = PhyContainer::new(fake_mac_roles.clone());

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.ap_ifaces.insert(0);
            let _ = phy_container.ap_ifaces.insert(1);

            // Expect two interface destruction requests
            let destroy_ap_iface_future = phy_manager.destroy_all_ap_ifaces();
            let mut destroy_ap_iface_future = pin!(destroy_ap_iface_future);

            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_pending());
            send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_pending());
            send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.ap_ifaces.is_empty());
        assert!(phy_container.destroyed_ifaces.contains(&0));
        assert!(phy_container.destroyed_ifaces.contains(&1));
        assert!(phy_container.defects.events.is_empty());
    }

    /// This test calls destroy_all_ap_ifaces on a PhyManager that only has a client iface.  The expectation
    /// is that no interfaces should be destroyed and the client iface ID should remain in the
    /// PhyManager
    #[fuchsia::test]
    fn stop_all_ap_ifaces_with_client() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];

        {
            let phy_container = PhyContainer::new(fake_mac_roles);

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.client_ifaces.insert(fake_iface_id);

            // Stop all AP ifaces
            let destroy_ap_iface_future = phy_manager.destroy_all_ap_ifaces();
            let mut destroy_ap_iface_future = pin!(destroy_ap_iface_future);
            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert!(phy_container.client_ifaces.contains(&fake_iface_id));
        assert!(phy_container.defects.events.is_empty());
    }

    /// This test validates the behavior when destroying all AP interfaces fails.
    #[fuchsia::test]
    fn stop_all_ap_ifaces_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the monitor stream so that the request to destroy the interface fails.
        drop(test_values.monitor_stream);

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // ifaces are added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];

        {
            let mut phy_container = PhyContainer::new(fake_mac_roles.clone());

            // For the sake of this test, force the retention period to be indefinite to make sure
            // that an event is logged.
            phy_container.defects = EventHistory::<Defect>::new(u32::MAX);

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

            // Insert the fake iface
            let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
            let _ = phy_container.ap_ifaces.insert(0);
            let _ = phy_container.ap_ifaces.insert(1);

            // Expect interface destruction to finish immediately.
            let destroy_ap_iface_future = phy_manager.destroy_all_ap_ifaces();
            let mut destroy_ap_iface_future = pin!(destroy_ap_iface_future);
            assert!(exec.run_until_stalled(&mut destroy_ap_iface_future).is_ready());
        }

        assert!(phy_manager.phys.contains_key(&fake_phy_id));

        let phy_container = phy_manager.phys.get(&fake_phy_id).unwrap();
        assert_eq!(phy_container.ap_ifaces.len(), 2);
        assert_eq!(phy_container.defects.events.len(), 2);
        assert_eq!(
            phy_container.defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 1 })
        );
        assert_eq!(
            phy_container.defects.events[1].value,
            Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 1 })
        );
    }

    /// Verifies that setting a suggested AP MAC address results in that MAC address being used as
    /// a part of the request to create an AP interface.  Ensures that this does not affect client
    /// interface requests.
    #[fuchsia::test]
    fn test_suggest_ap_mac() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Ap];
        let phy_container = PhyContainer::new(fake_mac_roles.clone());

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Insert the fake iface
        let phy_container = phy_manager.phys.get_mut(&fake_phy_id).unwrap();
        let _ = phy_container.client_ifaces.insert(fake_iface_id);

        // Suggest an AP MAC
        let mac: MacAddr = [1, 2, 3, 4, 5, 6].into();
        phy_manager.suggest_ap_mac(mac);

        let get_ap_future = phy_manager.create_or_get_ap_iface();
        let mut get_ap_future = pin!(get_ap_future);
        assert_matches!(exec.run_until_stalled(&mut get_ap_future), Poll::Pending);

        // Verify that the suggested MAC is included in the request
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::CreateIface {
                    payload,
                    responder,
                }
            ))) => {
                let requested_mac: MacAddr = payload.sta_address.unwrap().into();
                assert_eq!(requested_mac, mac);
                let response = fidl_service::DeviceMonitorCreateIfaceResponse {
                    iface_id: Some(fake_iface_id),
                    ..Default::default()
                };
                responder.send(Ok(&response)).expect("sending fake iface id");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut get_ap_future), Poll::Ready(_));
    }

    #[fuchsia::test]
    fn test_suggested_mac_does_not_apply_to_client() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_iface_id = 1;
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let phy_container = PhyContainer::new(fake_mac_roles.clone());

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Suggest an AP MAC
        let mac: MacAddr = [1, 2, 3, 4, 5, 6].into();
        phy_manager.suggest_ap_mac(mac);

        // Start client connections so that an IfaceRequest is issued for the client.
        let start_client_future =
            phy_manager.create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
        let mut start_client_future = pin!(start_client_future);
        assert_matches!(exec.run_until_stalled(&mut start_client_future), Poll::Pending);

        // Verify that the suggested MAC is NOT included in the request
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::CreateIface {
                    payload,
                    responder,
                }
            ))) => {
                assert_eq!(payload.sta_address, Some(ieee80211::NULL_ADDR.to_array()));
                let response = fidl_service::DeviceMonitorCreateIfaceResponse {
                    iface_id: Some(fake_iface_id),
                    ..Default::default()
                };
                responder.send(Ok(&response)).expect("sending fake iface id");
            }
        );
        assert_matches!(exec.run_until_stalled(&mut start_client_future), Poll::Ready(_));
    }

    /// Tests the case where creating a client interface fails while starting client connections.
    #[fuchsia::test]
    fn test_iface_creation_fails_during_start_client_connections() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the monitor stream so that the request to create the interface fails.
        drop(test_values.monitor_stream);

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let mut phy_container = PhyContainer::new(fake_mac_roles.clone());

        // For the sake of this test, force the retention period to be indefinite to make sure
        // that an event is logged.
        phy_container.defects = EventHistory::<Defect>::new(u32::MAX);

        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        {
            // Start client connections so that an IfaceRequest is issued for the client.
            let start_client_future = phy_manager
                .create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
            let mut start_client_future = pin!(start_client_future);
            assert!(exec.run_until_stalled(&mut start_client_future).is_ready());
        }

        // Verify that a defect has been logged.
        assert_eq!(phy_manager.phys[&fake_phy_id].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&fake_phy_id].defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 1 })
        );
    }

    #[fuchsia::test]
    fn test_all_iface_creation_failures_retained_across_multiple_phys() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the monitor stream so that the request to create the interface fails.
        drop(test_values.monitor_stream);

        // Create an initial PhyContainer to be inserted into the test PhyManager before the fake
        // iface is added.
        for fake_phy_id in 0..2 {
            let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
            let mut phy_container = PhyContainer::new(fake_mac_roles.clone());

            // For the sake of this test, force the retention period to be indefinite to make sure
            // that an event is logged.
            phy_container.defects = EventHistory::<Defect>::new(u32::MAX);

            let _ = phy_manager.phys.insert(fake_phy_id, phy_container);
        }

        let start_client_future =
            phy_manager.create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
        let mut start_client_future = pin!(start_client_future);
        assert_matches!(exec.run_until_stalled(&mut start_client_future),
            Poll::Ready(iface_ids) => {
                assert_eq!(iface_ids.len(), 2);
                assert_eq!(iface_ids[&0], Err(PhyManagerError::IfaceCreateFailure));
                assert_eq!(iface_ids[&1], Err(PhyManagerError::IfaceCreateFailure));
            }
        );
    }

    /// Tests get_phy_ids() when no PHYs are present. The expectation is that the PhyManager will
    /// Tests get_phy_ids() when no PHYs are present. The expectation is that the PhyManager will
    /// return an empty `Vec` in this case.
    #[fuchsia::test]
    async fn get_phy_ids_no_phys() {
        let test_values = test_setup();
        let phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        assert_eq!(phy_manager.get_phy_ids(), Vec::<u16>::new());
    }

    /// Tests get_phy_ids() when a single PHY is present. The expectation is that the PhyManager will
    /// return a single element `Vec`, with the appropriate ID.
    #[fuchsia::test]
    fn get_phy_ids_single_phy() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        {
            let add_phy_fut = phy_manager.add_phy(1);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());
            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&[]),
            );
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        assert_eq!(phy_manager.get_phy_ids(), vec![1]);
    }

    /// Tests get_phy_ids() when two PHYs are present. The expectation is that the PhyManager will
    /// return a two-element `Vec`, containing the appropriate IDs. Ordering is not guaranteed.
    #[fuchsia::test]
    fn get_phy_ids_two_phys() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        {
            let add_phy_fut = phy_manager.add_phy(1);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());
            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&[]),
            );
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        {
            let add_phy_fut = phy_manager.add_phy(2);
            let mut add_phy_fut = pin!(add_phy_fut);
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_pending());
            send_get_supported_mac_roles_response(
                &mut exec,
                &mut test_values.monitor_stream,
                Ok(&[]),
            );
            assert!(exec.run_until_stalled(&mut add_phy_fut).is_ready());
        }

        let phy_ids = phy_manager.get_phy_ids();
        assert!(phy_ids.contains(&1), "expected phy_ids to contain `1`, but phy_ids={phy_ids:?}");
        assert!(phy_ids.contains(&2), "expected phy_ids to contain `2`, but phy_ids={phy_ids:?}");
    }

    /// Tests log_phy_add_failure() to ensure the appropriate inspect count is incremented by 1.
    #[fuchsia::test]
    async fn log_phy_add_failure() {
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        assert_data_tree!(test_values.inspector, root: {
            phy_manager: {
                phy_add_fail_count: 0u64,
            },
        });

        phy_manager.log_phy_add_failure();
        assert_data_tree!(test_values.inspector, root: {
            phy_manager: {
                phy_add_fail_count: 1u64,
            },
        });
    }

    /// Tests the initialization of the country code and the ability of the PhyManager to cache a
    /// country code update.
    #[fuchsia::test]
    fn test_set_country_code() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Insert a couple fake PHYs.
        let _ = phy_manager.phys.insert(
            0,
            PhyContainer {
                supported_mac_roles: HashSet::new(),
                client_ifaces: HashSet::new(),
                ap_ifaces: HashSet::new(),
                destroyed_ifaces: HashSet::new(),
                defects: EventHistory::new(DEFECT_RETENTION_SECONDS),
                recoveries: EventHistory::new(DEFECT_RETENTION_SECONDS),
            },
        );
        let _ = phy_manager.phys.insert(
            1,
            PhyContainer {
                supported_mac_roles: HashSet::new(),
                client_ifaces: HashSet::new(),
                ap_ifaces: HashSet::new(),
                destroyed_ifaces: HashSet::new(),
                defects: EventHistory::new(DEFECT_RETENTION_SECONDS),
                recoveries: EventHistory::new(DEFECT_RETENTION_SECONDS),
            },
        );

        // Initially the country code should be unset.
        assert!(phy_manager.saved_country_code.is_none());

        // Apply a country code and ensure that it is propagated to the device service.
        {
            let set_country_fut = phy_manager.set_country_code(Some("US".parse().unwrap()));
            let mut set_country_fut = pin!(set_country_fut);

            // Ensure that both PHYs have their country codes set.
            for _ in 0..2 {
                assert_matches!(exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
                assert_matches!(
                    exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                    Poll::Ready(Some(Ok(
                        fidl_service::DeviceMonitorRequest::SetCountry {
                            req: fidl_service::SetCountryRequest {
                                phy_id: _,
                                alpha2: [b'U', b'S'],
                            },
                            responder,
                        }
                    ))) => {
                        responder.send(ZX_OK).expect("sending fake set country response");
                    }
                );
            }

            assert_matches!(exec.run_until_stalled(&mut set_country_fut), Poll::Ready(Ok(())));
        }
        assert_eq!(phy_manager.saved_country_code, Some("US".parse().unwrap()));

        // Unset the country code and ensure that the clear country code message is sent to the
        // device service.
        {
            let set_country_fut = phy_manager.set_country_code(None);
            let mut set_country_fut = pin!(set_country_fut);

            // Ensure that both PHYs have their country codes cleared.
            for _ in 0..2 {
                assert_matches!(exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
                assert_matches!(
                    exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                    Poll::Ready(Some(Ok(
                        fidl_service::DeviceMonitorRequest::ClearCountry {
                            req: fidl_service::ClearCountryRequest {
                                phy_id: _,
                            },
                            responder,
                        }
                    ))) => {
                        responder.send(ZX_OK).expect("sending fake clear country response");
                    }
                );
            }

            assert_matches!(exec.run_until_stalled(&mut set_country_fut), Poll::Ready(Ok(())));
        }
        assert_eq!(phy_manager.saved_country_code, None);
    }

    // Tests the case where setting the country code is unsuccessful.
    #[fuchsia::test]
    fn test_setting_country_code_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Insert a fake PHY.
        let _ = phy_manager.phys.insert(
            0,
            PhyContainer {
                supported_mac_roles: HashSet::new(),
                client_ifaces: HashSet::new(),
                ap_ifaces: HashSet::new(),
                destroyed_ifaces: HashSet::new(),
                defects: EventHistory::new(DEFECT_RETENTION_SECONDS),
                recoveries: EventHistory::new(DEFECT_RETENTION_SECONDS),
            },
        );

        // Initially the country code should be unset.
        assert!(phy_manager.saved_country_code.is_none());

        // Apply a country code and ensure that it is propagated to the device service.
        {
            let set_country_fut = phy_manager.set_country_code(Some("US".parse().unwrap()));
            let mut set_country_fut = pin!(set_country_fut);

            assert_matches!(exec.run_until_stalled(&mut set_country_fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                Poll::Ready(Some(Ok(
                    fidl_service::DeviceMonitorRequest::SetCountry {
                        req: fidl_service::SetCountryRequest {
                            phy_id: 0,
                            alpha2: [b'U', b'S'],
                        },
                        responder,
                    }
                ))) => {
                    // Send back a failure.
                    responder
                        .send(zx::sys::ZX_ERR_NOT_SUPPORTED)
                        .expect("sending fake set country response");
                }
            );

            assert_matches!(
                exec.run_until_stalled(&mut set_country_fut),
                Poll::Ready(Err(PhyManagerError::PhySetCountryFailure))
            );
        }
        assert_eq!(phy_manager.saved_country_code, Some("US".parse().unwrap()));
    }

    /// Tests the case where multiple client interfaces need to be recovered.
    #[fuchsia::test]
    fn test_recover_client_interfaces_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];

        // Make it look like client connections have been enabled.
        phy_manager.client_connections_enabled = true;

        // Create four fake PHY entries.  For the sake of this test, each PHY will eventually
        // receive and interface ID equal to its PHY ID.
        for phy_id in 0..4 {
            let fake_mac_roles = fake_mac_roles.clone();
            let _ = phy_manager.phys.insert(phy_id, PhyContainer::new(fake_mac_roles.clone()));

            // Give the 0th and 2nd PHYs have client interfaces.
            if phy_id.is_multiple_of(2) {
                let phy_container = phy_manager.phys.get_mut(&phy_id).expect("missing PHY");
                let _ = phy_container.client_ifaces.insert(phy_id);
            }
        }

        // There are now two PHYs with client interfaces and two without.  This looks like two
        // interfaces have undergone recovery.  Run recover_client_ifaces and ensure that the two
        // PHYs that are missing client interfaces have interfaces created for them.
        {
            let recovery_fut =
                phy_manager.create_all_client_ifaces(CreateClientIfacesReason::RecoverClientIfaces);
            let mut recovery_fut = pin!(recovery_fut);
            assert_matches!(exec.run_until_stalled(&mut recovery_fut), Poll::Pending);

            loop {
                // The recovery future will only stall out when either
                // 1. It needs to create a client interface for a PHY that does not have one.
                // 2. The futures completes and has recovered all possible interfaces.
                match exec.run_until_stalled(&mut recovery_fut) {
                    Poll::Pending => {}
                    Poll::Ready(iface_ids) => {
                        if iface_ids.values().any(Result::is_err) {
                            panic!("recovery failed unexpectedly");
                        }
                        let iface_ids: Vec<_> =
                            iface_ids.into_values().flat_map(Result::unwrap).collect();
                        assert!(iface_ids.contains(&1));
                        assert!(iface_ids.contains(&3));
                        break;
                    }
                }

                // Make sure that the stalled future has made a FIDL request to create a client
                // interface.  Send back a response assigning an interface ID equal to the PHY ID.
                assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                Poll::Ready(Some(Ok(
                    fidl_service::DeviceMonitorRequest::CreateIface {
                        payload,
                        responder,
                    }
                ))) => {
                    let response = fidl_service::DeviceMonitorCreateIfaceResponse {
                        iface_id: Some(payload.phy_id.unwrap()),
                        ..Default::default()
                    };
                    responder.send(Ok(&response)).expect("sending fake iface id");
                });
            }
        }

        // Make sure all of the PHYs have interface IDs and that the IDs match the PHY IDs,
        // indicating that they were assigned correctly.
        for phy_id in phy_manager.phys.keys() {
            assert_eq!(phy_manager.phys[phy_id].client_ifaces.len(), 1);
            assert!(phy_manager.phys[phy_id].client_ifaces.contains(phy_id));
        }
    }

    /// Tests the case where a client interface needs to be recovered and recovery fails.
    #[fuchsia::test]
    fn test_recover_client_interfaces_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];

        // Make it look like client connections have been enabled.
        phy_manager.client_connections_enabled = true;

        // For this test, use three PHYs (0, 1, and 2).  Let recovery fail for PHYs 0 and 2 and
        // succeed for PHY 1.  Verify that a create interface request is sent for each PHY and at
        // the end, verify that only one recovered interface is listed and that PHY 1 has been
        // assigned that interface.
        for phy_id in 0..3 {
            let _ = phy_manager.phys.insert(phy_id, PhyContainer::new(fake_mac_roles.clone()));
        }

        // Run recovery.
        {
            let recovery_fut =
                phy_manager.create_all_client_ifaces(CreateClientIfacesReason::RecoverClientIfaces);
            let mut recovery_fut = pin!(recovery_fut);
            assert_matches!(exec.run_until_stalled(&mut recovery_fut), Poll::Pending);

            loop {
                match exec.run_until_stalled(&mut recovery_fut) {
                    Poll::Pending => {}
                    Poll::Ready(iface_ids) => {
                        assert!(iface_ids.values().any(Result::is_err));
                        let iface_ids: Vec<_> =
                            iface_ids.into_values().filter_map(Result::ok).flatten().collect();
                        assert_eq!(iface_ids, vec![1]);
                        break;
                    }
                }

                // Make sure that the stalled future has made a FIDL request to create a client
                // interface.  Send back a response assigning an interface ID equal to the PHY ID.
                assert_matches!(
                    exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                    Poll::Ready(Some(Ok(
                        fidl_service::DeviceMonitorRequest::CreateIface {
                            payload,
                            responder,
                        }
                    ))) => {
                        let iface_id = payload.phy_id.unwrap();
                        let response = fidl_service::DeviceMonitorCreateIfaceResponse {
                            iface_id: Some(iface_id),
                            ..Default::default()
                        };

                        // As noted above, let the requests for 0 and 2 "fail" and let the request
                        // for PHY 1 succeed.
                        match payload.phy_id.unwrap() {
                            1 => {
                                responder.send(Ok(&response)).expect("sending fake iface id")
                            },
                            _ => responder.send(Err(fidl_service::DeviceMonitorError::unknown())).expect("sending fake iface id"),
                        };
                    }
                );
            }
        }

        // Make sure PHYs 0 and 2 do not have interfaces and that PHY 1 does.
        for phy_id in phy_manager.phys.keys() {
            match phy_id {
                1 => {
                    assert_eq!(phy_manager.phys[phy_id].client_ifaces.len(), 1);
                    assert!(phy_manager.phys[phy_id].client_ifaces.contains(phy_id));
                }
                _ => assert!(phy_manager.phys[phy_id].client_ifaces.is_empty()),
            }
        }
    }

    /// Tests the case where a PHY is client-capable, but client connections are disabled and a
    /// caller requests attempts to recover client interfaces.
    #[fuchsia::test]
    fn test_recover_client_interfaces_while_disabled() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Create a fake PHY entry without client interfaces.  Note that client connections have
        // not been set to enabled.
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let _ = phy_manager.phys.insert(0, PhyContainer::new(fake_mac_roles));

        // Run recovery and ensure that it completes immediately and does not recover any
        // interfaces.
        {
            let recovery_fut =
                phy_manager.create_all_client_ifaces(CreateClientIfacesReason::RecoverClientIfaces);
            let mut recovery_fut = pin!(recovery_fut);
            assert_matches!(
                exec.run_until_stalled(&mut recovery_fut),
                Poll::Ready(recovered_ifaces) => {
                    assert!(recovered_ifaces.is_empty());
                }
            );
        }

        // Verify that there are no client interfaces.
        for (_, phy_container) in phy_manager.phys {
            assert!(phy_container.client_ifaces.is_empty());
        }
    }

    /// Tests the case where client connections are re-started following an unsuccessful stop
    /// client connections request.
    #[fuchsia::test]
    fn test_start_after_unsuccessful_stop() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Verify that client connections are initially stopped.
        assert!(!phy_manager.client_connections_enabled);

        // Create a PHY with a lingering client interface.
        let fake_phy_id = 1;
        let fake_mac_roles = vec![fidl_common::WlanMacRole::Client];
        let mut phy_container = PhyContainer::new(fake_mac_roles);
        // Insert the fake iface
        let fake_iface_id = 1;
        let _ = phy_container.client_ifaces.insert(fake_iface_id);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Try creating all client interfaces due to recovery and ensure that no interfaces are
        // returned.
        {
            let start_client_future =
                phy_manager.create_all_client_ifaces(CreateClientIfacesReason::RecoverClientIfaces);
            let mut start_client_future = pin!(start_client_future);
            assert_matches!(
                exec.run_until_stalled(&mut start_client_future),
                Poll::Ready(v) => {
                assert!(v.is_empty())
            });
        }

        // Create all client interfaces with the reason set to StartClientConnections and verify
        // that the existing interface is returned.
        {
            let start_client_future = phy_manager
                .create_all_client_ifaces(CreateClientIfacesReason::StartClientConnections);
            let mut start_client_future = pin!(start_client_future);
            assert_matches!(
                exec.run_until_stalled(&mut start_client_future),
                Poll::Ready(iface_ids) => {
                    assert_eq!(iface_ids.into_values().collect::<Vec<_>>(), vec![Ok(vec![1])]);
                }
            );
        }
    }

    /// Tests reporting of client connections status when client connections are enabled.
    #[fuchsia::test]
    fn test_client_connections_enabled_when_enabled() {
        let _exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        phy_manager.client_connections_enabled = true;
        assert!(phy_manager.client_connections_enabled());
    }

    /// Tests reporting of client connections status when client connections are disabled.
    #[fuchsia::test]
    fn test_client_connections_enabled_when_disabled() {
        let _exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        phy_manager.client_connections_enabled = false;
        assert!(!phy_manager.client_connections_enabled());
    }

    #[fuchsia::test]
    fn test_create_iface_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Issue a create iface request
        let fut = phy_manager.create_iface(0, fidl_common::WlanMacRole::Client, NULL_ADDR);
        let mut fut = pin!(fut);

        // Wait for the request to stall out waiting for DeviceMonitor.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send back a positive response from DeviceMonitor.
        send_create_iface_response(&mut exec, &mut test_values.monitor_stream, Some(0));

        // The future should complete.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(0)));

        // Verify that there is nothing waiting on the telemetry receiver.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceCreationResult {
                role: fidl_common::WlanMacRole::Client,
                result: Ok(0),
            }))
        )
    }

    #[fuchsia::test]
    fn test_create_iface_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        let mut phy_container = PhyContainer::new(vec![]);
        let _ = phy_container.client_ifaces.insert(0);
        let _ = phy_manager.phys.insert(0, phy_container);

        {
            // Issue a create iface request
            let fut = phy_manager.create_iface(0, fidl_common::WlanMacRole::Client, NULL_ADDR);
            let mut fut = pin!(fut);

            // Wait for the request to stall out waiting for DeviceMonitor.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Send back a failure from DeviceMonitor.
            send_create_iface_response(&mut exec, &mut test_values.monitor_stream, None);

            // The future should complete.
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Err(PhyManagerError::IfaceCreateFailure))
            );

            // Verify that a metric has been logged.
            assert_matches!(
                test_values.telemetry_receiver.try_next(),
                Ok(Some(TelemetryEvent::IfaceCreationResult {
                    role: fidl_common::WlanMacRole::Client,
                    result: Err(()),
                }))
            );
        }

        // Verify the defect was recorded.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&0].defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 0 })
        );
    }

    #[fuchsia::test]
    fn test_create_iface_request_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        let mut phy_container = PhyContainer::new(vec![]);
        let _ = phy_container.client_ifaces.insert(0);
        let _ = phy_manager.phys.insert(0, phy_container);

        drop(test_values.monitor_stream);

        {
            // Issue a create iface request
            let fut = phy_manager.create_iface(0, fidl_common::WlanMacRole::Client, NULL_ADDR);
            let mut fut = pin!(fut);

            // The request should immediately fail.
            assert_matches!(
                exec.run_until_stalled(&mut fut),
                Poll::Ready(Err(PhyManagerError::IfaceCreateFailure))
            );

            // Verify that a metric has been logged.
            assert_matches!(
                test_values.telemetry_receiver.try_next(),
                Ok(Some(TelemetryEvent::IfaceCreationResult {
                    role: fidl_common::WlanMacRole::Client,
                    result: Err(()),
                }))
            );
        }

        // Verify the defect was recorded.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&0].defects.events[0].value,
            Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 0 })
        );
    }

    #[fuchsia::test]
    fn test_destroy_iface_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Issue a destroy iface request
        let fut = destroy_iface(
            &test_values.monitor_proxy,
            0,
            fidl_common::WlanMacRole::Client,
            &test_values.telemetry_sender,
        );
        let mut fut = pin!(fut);

        // Wait for the request to stall out waiting for DeviceMonitor.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send back a positive response from DeviceMonitor.
        send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

        // The future should complete.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify that there is nothing waiting on the telemetry receiver.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceDestructionResult {
                role: fidl_common::WlanMacRole::Client,
                result: Ok(0),
            }))
        )
    }

    #[fuchsia::test]
    fn test_destroy_iface_not_found() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Issue a destroy iface request
        let fut = destroy_iface(
            &test_values.monitor_proxy,
            0,
            fidl_common::WlanMacRole::Client,
            &test_values.telemetry_sender,
        );
        let mut fut = pin!(fut);

        // Wait for the request to stall out waiting for DeviceMonitor.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send back NOT_FOUND from DeviceMonitor.
        send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_ERR_NOT_FOUND);

        // The future should complete.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));

        // Verify that no metric has been logged.
        assert_matches!(test_values.telemetry_receiver.try_next(), Err(_))
    }

    #[fuchsia::test]
    fn test_destroy_iface_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Issue a destroy iface request
        let fut = destroy_iface(
            &test_values.monitor_proxy,
            0,
            fidl_common::WlanMacRole::Client,
            &test_values.telemetry_sender,
        );
        let mut fut = pin!(fut);

        // Wait for the request to stall out waiting for DeviceMonitor.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send back a non-NOT_FOUND failure from DeviceMonitor.
        send_destroy_iface_response(
            &mut exec,
            &mut test_values.monitor_stream,
            zx::sys::ZX_ERR_NO_RESOURCES,
        );

        // The future should complete.
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Err(PhyManagerError::IfaceDestroyFailure))
        );

        // Verify that a metric has been logged.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceDestructionResult {
                role: fidl_common::WlanMacRole::Client,
                result: Err(()),
            }))
        )
    }

    #[fuchsia::test]
    fn test_destroy_iface_request_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        drop(test_values.monitor_stream);

        // Issue a destroy iface request
        let fut = destroy_iface(
            &test_values.monitor_proxy,
            0,
            fidl_common::WlanMacRole::Client,
            &test_values.telemetry_sender,
        );
        let mut fut = pin!(fut);

        // The request should immediately fail.
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Err(PhyManagerError::IfaceDestroyFailure))
        );

        // Verify that a metric has been logged.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::IfaceDestructionResult {
                role: fidl_common::WlanMacRole::Client,
                result: Err(()),
            }))
        )
    }

    /// Verify that client iface failures are added properly.
    #[fuchsia::test]
    fn test_record_iface_event() {
        let _exec = TestExecutor::new();
        let test_values = test_setup();

        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Add some PHYs with interfaces.
        let _ = phy_manager.phys.insert(0, PhyContainer::new(vec![]));
        let _ = phy_manager.phys.insert(1, PhyContainer::new(vec![]));
        let _ = phy_manager.phys.insert(2, PhyContainer::new(vec![]));
        let _ = phy_manager.phys.insert(3, PhyContainer::new(vec![]));

        // Add some PHYs with interfaces.
        let _ = phy_manager.phys.get_mut(&0).expect("missing PHY").client_ifaces.insert(123);
        let _ = phy_manager.phys.get_mut(&1).expect("missing PHY").client_ifaces.insert(456);
        let _ = phy_manager.phys.get_mut(&2).expect("missing PHY").client_ifaces.insert(789);
        let _ = phy_manager.phys.get_mut(&3).expect("missing PHY").ap_ifaces.insert(246);

        // Allow defects to be retained indefinitely.
        phy_manager.phys.get_mut(&0).expect("missing PHY").defects = EventHistory::new(u32::MAX);
        phy_manager.phys.get_mut(&1).expect("missing PHY").defects = EventHistory::new(u32::MAX);
        phy_manager.phys.get_mut(&2).expect("missing PHY").defects = EventHistory::new(u32::MAX);
        phy_manager.phys.get_mut(&3).expect("missing PHY").defects = EventHistory::new(u32::MAX);

        // Log some client interface failures.
        phy_manager.record_defect(Defect::Iface(IfaceFailure::CanceledScan { iface_id: 123 }));
        phy_manager.record_defect(Defect::Iface(IfaceFailure::FailedScan { iface_id: 456 }));
        phy_manager.record_defect(Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 789 }));
        phy_manager.record_defect(Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 123 }));

        // Log an AP interface failure.
        phy_manager.record_defect(Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 246 }));

        // Verify that the defects have been logged.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 2);
        assert_eq!(
            phy_manager.phys[&0].defects.events[0].value,
            Defect::Iface(IfaceFailure::CanceledScan { iface_id: 123 })
        );
        assert_eq!(
            phy_manager.phys[&0].defects.events[1].value,
            Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 123 })
        );
        assert_eq!(phy_manager.phys[&1].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&1].defects.events[0].value,
            Defect::Iface(IfaceFailure::FailedScan { iface_id: 456 })
        );
        assert_eq!(phy_manager.phys[&2].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&2].defects.events[0].value,
            Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 789 })
        );
        assert_eq!(phy_manager.phys[&3].defects.events.len(), 1);
        assert_eq!(
            phy_manager.phys[&3].defects.events[0].value,
            Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 246 })
        );
    }

    /// Verify that AP ifaces do not receive client failures..
    #[fuchsia::test]
    fn test_aps_do_not_record_client_defects() {
        let _exec = TestExecutor::new();
        let test_values = test_setup();

        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Add some PHYs with interfaces.
        let _ = phy_manager.phys.insert(0, PhyContainer::new(vec![]));

        // Add some PHYs with interfaces.
        let _ = phy_manager.phys.get_mut(&0).expect("missing PHY").ap_ifaces.insert(123);

        // Allow defects to be retained indefinitely.
        phy_manager.phys.get_mut(&0).expect("missing PHY").defects = EventHistory::new(u32::MAX);

        // Log some client interface failures.
        phy_manager.record_defect(Defect::Iface(IfaceFailure::CanceledScan { iface_id: 123 }));
        phy_manager.record_defect(Defect::Iface(IfaceFailure::FailedScan { iface_id: 123 }));
        phy_manager.record_defect(Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 123 }));
        phy_manager.record_defect(Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 123 }));

        // Verify that the defects have been logged.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 0);
    }

    /// Verify that client ifaces do not receive AP defects.
    #[fuchsia::test]
    fn test_clients_do_not_record_ap_defects() {
        let _exec = TestExecutor::new();
        let test_values = test_setup();

        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Add some PHYs with interfaces.
        let _ = phy_manager.phys.insert(0, PhyContainer::new(vec![]));

        // Add a PHY with a client interface.
        let _ = phy_manager.phys.get_mut(&0).expect("missing PHY").client_ifaces.insert(123);

        // Allow defects to be retained indefinitely.
        phy_manager.phys.get_mut(&0).expect("missing PHY").defects = EventHistory::new(u32::MAX);

        // Log an AP interface failure.
        phy_manager.record_defect(Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 123 }));

        // Verify that the defects have been not logged.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 0);
    }

    fn aggressive_test_recovery_profile(
        _phy_id: u16,
        _defect_history: &mut EventHistory<Defect>,
        _recovery_history: &mut EventHistory<RecoveryAction>,
        _latest_defect: Defect,
    ) -> Option<RecoveryAction> {
        Some(RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 }))
    }

    #[test_case(
        Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 123 }) ;
        "recommend AP start recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 456 }) ;
        "recommend connection failure recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 456 }) ;
        "recommend empty scan recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::FailedScan { iface_id: 456 }) ;
        "recommend failed scan recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::CanceledScan { iface_id: 456 }) ;
        "recommend canceled scan recovery"
    )]
    #[test_case(
        Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 0 }) ;
        "recommend iface destruction recovery"
    )]
    #[test_case(
        Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 0 }) ;
        "recommend iface creation recovery"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_recovery_action_sent_from_record_defect(defect: Defect) {
        let _exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Insert a fake PHY, client interface, and AP interface.
        let mut phy_container = PhyContainer::new(vec![]);
        let _ = phy_container.ap_ifaces.insert(123);
        let _ = phy_container.client_ifaces.insert(456);
        let _ = phy_manager.phys.insert(0, phy_container);

        // Swap the recovery profile with one that always suggests recovery.
        phy_manager.recovery_profile = aggressive_test_recovery_profile;

        // Record the defect.
        phy_manager.record_defect(defect);

        // Verify that a recovery event was sent.
        let recovery_action = test_values.recovery_receiver.try_next().unwrap().unwrap();
        assert_eq!(recovery_action.defect, defect);
        assert_eq!(
            recovery_action.action,
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        );
    }

    #[test_case(
        Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 123 }) ;
        "do not recommend AP start recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 456 }) ;
        "do not recommend connection failure recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 456 }) ;
        "do not recommend empty scan recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::FailedScan { iface_id: 456 }) ;
        "do not recommend failed scan recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::CanceledScan { iface_id: 456 }) ;
        "do not recommend canceled scan recovery"
    )]
    #[test_case(
        Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 0 }) ;
        "do not recommend iface destruction recovery"
    )]
    #[test_case(
        Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 0 }) ;
        "do not recommend iface creation recovery"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_no_recovery_when_defect_contains_bad_ids(defect: Defect) {
        let _exec = TestExecutor::new();
        let mut test_values = test_setup();

        // This PhyManager doesn't have any PHYs or interfaces.
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Swap the recovery profile with one that always suggests recovery.
        phy_manager.recovery_profile = aggressive_test_recovery_profile;

        // Record the defect.
        phy_manager.record_defect(defect);

        // Verify that a recovery event was sent.
        assert!(test_values.recovery_receiver.try_next().is_err());
    }

    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::ScanResultsEmpty(
            telemetry::ClientRecoveryMechanism::PhyReset
        ) ;
        "PHY reset for empty scan results"
    )]
    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::CanceledScan { iface_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::ScanCancellation(
            telemetry::ClientRecoveryMechanism::PhyReset
        ) ;
        "PHY reset for scan cancellation"
    )]
    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::FailedScan { iface_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::ScanFailure(
            telemetry::ClientRecoveryMechanism::PhyReset
        ) ;
        "PHY reset for scan failure"
    )]
    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::StartApFailure(
            telemetry::ApRecoveryMechanism::ResetPhy
        ) ;
        "PHY reset for start AP failure"
    )]
    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::ConnectFailure(
            telemetry::ClientRecoveryMechanism::PhyReset
        ) ;
        "PHY reset for connection failure"
    )]
    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::DestroyIfaceFailure(
            telemetry::PhyRecoveryMechanism::PhyReset
        ) ;
        "PHY reset for iface destruction failure"
    )]
    #[test_case(
        recovery::RecoverySummary {
            defect: Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: 0 }),
            action: RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: 0 })
        },
        telemetry::RecoveryReason::CreateIfaceFailure(
            telemetry::PhyRecoveryMechanism::PhyReset
        ) ;
        "PHY reset for iface creation failure"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_log_recovery_action_sends_metrics(
        summary: recovery::RecoverySummary,
        expected_reason: telemetry::RecoveryReason,
    ) {
        let _exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Send the provided recovery summary and expect the associated telemetry event.
        phy_manager.log_recovery_action(summary);
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::RecoveryEvent { reason } )) => {
        assert_eq!(reason, expected_reason);
            })
    }

    #[test_case(
        Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 456 }) ;
        "recommend AP start recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: 456 }) ;
        "recommend connection failure recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: 456 }) ;
        "recommend empty scan recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::FailedScan { iface_id: 456 }) ;
        "recommend failed scan recovery"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::CanceledScan { iface_id: 456 }) ;
        "recommend canceled scan recovery"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn log_defect_for_destroyed_iface(defect: Defect) {
        let _exec = TestExecutor::new();
        let test_values = test_setup();

        let fake_phy_id = 123;
        let fake_iface_id = 456;

        // Create a PhyManager and give it a PHY that doesn't have any interfaces but does have a
        // record of a past interface that was destroyed.
        let mut phy_manager = PhyManager::new(
            test_values.monitor_proxy,
            recovery::lookup_recovery_profile(""),
            false,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );
        let mut phy_container = PhyContainer::new(vec![]);
        let _ = phy_container.destroyed_ifaces.insert(fake_iface_id);
        let _ = phy_manager.phys.insert(fake_phy_id, phy_container);

        // Record the defect.
        phy_manager.record_defect(defect);
        assert_eq!(phy_manager.phys[&fake_phy_id].defects.events.len(), 1);
    }

    #[fuchsia::test]
    fn test_reset_request_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();

        // Drop the DeviceMonitor request stream so that the request fails.
        drop(test_values.monitor_stream);

        // Make the reset request and observe that it fails.
        let fut = reset_phy(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Err(PhyManagerError::InternalError))
        );
    }

    #[fuchsia::test]
    fn test_reset_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the reset request.
        let fut = reset_phy(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send back a failure.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::Reset { phy_id: 0, responder }
            ))) => {
                responder.send(Err(ZX_ERR_NOT_FOUND)).expect("sending fake reset response");
            }
        );

        // Ensure that the failure is returned to the caller.
        assert_matches!(
            exec.run_until_stalled(&mut fut),
            Poll::Ready(Err(PhyManagerError::PhyResetFailure))
        );
    }

    #[fuchsia::test]
    fn test_reset_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the reset request.
        let fut = reset_phy(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Send back a success.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::Reset { phy_id: 0, responder }
            ))) => {
                responder.send(Ok(())).expect("sending fake reset response");
            }
        );

        // Ensure that the success is returned to the caller.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_disconnect_request_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the disconnect request.
        let fut = disconnect(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // First, the client SME will be requested.
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                iface_id: 0, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Drop the SME server so that the request will fail.
        drop(sme_server);

        // The future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_disconnect_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the disconnect request.
        let fut = disconnect(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // First, the client SME will be requested.
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                iface_id: 0, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Next, the disconnect will be requested.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let mut sme_stream = sme_server.into_stream().into_future();
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ClientSmeRequest::Disconnect{
                responder,
                reason: fidl_fuchsia_wlan_sme::UserDisconnectReason::Recovery
            }) => {
                responder.send().expect("Failed to send disconnect response")
            }
        );

        // Verify the future completes successfully.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_disconnect_cannot_get_sme() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();

        // Drop the DeviceMonitor stream so that the client SME cannot be obtained.
        drop(test_values.monitor_stream);

        // Make the disconnect request.
        let fut = disconnect(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_stop_ap_request_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the stop AP request.
        let fut = stop_ap(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // First, the AP SME will be requested.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                iface_id: 0, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Drop the SME server so that the request will fail.
        drop(sme_server);

        // The future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_stop_ap_fails() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the stop AP request.
        let fut = stop_ap(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // First, the AP SME will be requested.
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                iface_id: 0, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Expect the stop AP request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let mut sme_stream = sme_server.into_stream().into_future();
        assert_matches!(
            poll_ap_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ApSmeRequest::Stop{
                responder,
            }) => {
                responder.send(fidl_sme::StopApResultCode::InternalError).expect("Failed to send stop AP response")
            }
        );

        // The future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    #[fuchsia::test]
    fn test_stop_ap_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();

        // Make the stop AP request.
        let fut = stop_ap(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // First, the AP SME will be requested.
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                iface_id: 0, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Expect the stop AP request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let mut sme_stream = sme_server.into_stream().into_future();
        assert_matches!(
            poll_ap_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ApSmeRequest::Stop{
                responder,
            }) => {
                responder.send(fidl_sme::StopApResultCode::Success).expect("Failed to send stop AP response")
            }
        );

        // The future should complete with an error.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Ok(())));
    }

    #[fuchsia::test]
    fn test_stop_ap_cannot_get_sme() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();

        // Drop the DeviceMonitor stream so that the client SME cannot be obtained.
        drop(test_values.monitor_stream);

        // Make the disconnect request.
        let fut = stop_ap(&test_values.monitor_proxy, 0);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(Err(_)));
    }

    fn phy_manager_for_recovery_test(
        device_monitor: fidl_service::DeviceMonitorProxy,
        node: inspect::Node,
        telemetry_sender: TelemetrySender,
        recovery_action_sender: recovery::RecoveryActionSender,
    ) -> PhyManager {
        let mut phy_manager = PhyManager::new(
            device_monitor,
            recovery::lookup_recovery_profile("thresholded_recovery"),
            true,
            node,
            telemetry_sender,
            recovery_action_sender,
        );

        // Give the PhyManager client and AP interfaces.
        let mut phy_container =
            PhyContainer::new(vec![fidl_common::WlanMacRole::Client, fidl_common::WlanMacRole::Ap]);
        assert!(phy_container.client_ifaces.insert(1,));
        assert!(phy_container.ap_ifaces.insert(2));
        assert!(phy_manager.phys.insert(0, phy_container).is_none());

        phy_manager
    }

    #[fuchsia::test]
    fn test_perform_recovery_destroy_nonexistent_iface() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Suggest a recovery action to destroy nonexistent interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::FailedScan { iface_id: 123 }),
            action: recovery::RecoveryAction::PhyRecovery(
                recovery::PhyRecoveryOperation::DestroyIface { iface_id: 123 },
            ),
        };

        // The future should complete immediately.
        {
            let fut = phy_manager.perform_recovery(summary);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // No request should have been made of DeviceMonitor.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    fn test_perform_recovery_destroy_client_iface_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the DeviceMonitor serving end so that destroying the interface will fail.
        drop(test_values.monitor_stream);

        // Suggest a recovery action to destroy the client interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::FailedScan { iface_id: 1 }),
            action: recovery::RecoveryAction::PhyRecovery(
                recovery::PhyRecoveryOperation::DestroyIface { iface_id: 1 },
            ),
        };

        {
            let fut = phy_manager.perform_recovery(summary);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that the client interface is still present.
        assert!(phy_manager.phys[&0].client_ifaces.contains(&1));
    }

    #[fuchsia::test]
    fn test_perform_recovery_destroy_client_iface_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Suggest a recovery action to destroy the client interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::FailedScan { iface_id: 1 }),
            action: recovery::RecoveryAction::PhyRecovery(
                recovery::PhyRecoveryOperation::DestroyIface { iface_id: 1 },
            ),
        };

        {
            let fut = phy_manager.perform_recovery(summary);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Verify that the DestroyIface request was made and respond with a success.
            send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

            // The future should complete now.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that the client interface has been removed.
        assert!(!phy_manager.phys[&0].client_ifaces.contains(&1));

        // Verify that the destroyed interface ID has been recorded.
        assert!(phy_manager.phys[&0].destroyed_ifaces.contains(&1));
    }

    #[fuchsia::test]
    fn test_perform_recovery_destroy_ap_iface_fails() {
        let mut exec = TestExecutor::new();
        let test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Drop the DeviceMonitor serving end so that destroying the interface will fail.
        drop(test_values.monitor_stream);

        // Suggest a recovery action to destroy the AP interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 2 }),
            action: recovery::RecoveryAction::PhyRecovery(
                recovery::PhyRecoveryOperation::DestroyIface { iface_id: 2 },
            ),
        };

        {
            let fut = phy_manager.perform_recovery(summary);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that the AP interface is still present.
        assert!(phy_manager.phys[&0].ap_ifaces.contains(&2));
    }

    #[fuchsia::test]
    fn test_perform_recovery_destroy_ap_iface_succeeds() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Suggest a recovery action to destroy the AP interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 2 }),
            action: recovery::RecoveryAction::PhyRecovery(
                recovery::PhyRecoveryOperation::DestroyIface { iface_id: 2 },
            ),
        };

        {
            let fut = phy_manager.perform_recovery(summary);
            let mut fut = pin!(fut);
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

            // Verify that the DestroyIface request was made and respond with a success.
            send_destroy_iface_response(&mut exec, &mut test_values.monitor_stream, ZX_OK);

            // The future should complete now.
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
        }

        // Verify that the AP interface has been removed.
        assert!(!phy_manager.phys[&0].ap_ifaces.contains(&2));

        // Verify the destroyed iface ID was recorded.
        assert!(phy_manager.phys[&0].destroyed_ifaces.contains(&2));
    }

    // TODO(https://fxbug.dev/424173437) - Re-enable once IfaceManager deadlock issue has been resolved.
    #[ignore]
    #[test_case(Some("US".parse().unwrap()); "Cached country code")]
    #[test_case(None; "No cached country code")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_perform_recovery_reset_requests_phy_reset(
        cached_country_code: Option<client_types::CountryCode>,
    ) {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Set a country code in the phy manager
        phy_manager.saved_country_code = cached_country_code;

        // Suggest a recovery action to reset the PHY.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 2 }),
            action: recovery::RecoveryAction::PhyRecovery(
                recovery::PhyRecoveryOperation::ResetPhy { phy_id: 0 },
            ),
        };

        let fut = phy_manager.perform_recovery(summary);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the Reset request was made and respond with a success.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(
                fidl_service::DeviceMonitorRequest::Reset {
                    phy_id: 0,
                    responder,
                }
            ))) => {
                responder
                    .send(Ok(()))
                    .expect("failed to send reset response.");
            }
        );

        // Check that we set the country code if we had a cached country code
        if let Some(cached_cc) = cached_country_code {
            assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
            assert_matches!(
                exec.run_until_stalled(&mut test_values.monitor_stream.next()),
                Poll::Ready(Some(Ok(
                    fidl_service::DeviceMonitorRequest::SetCountry {
                        req: fidl_service::SetCountryRequest {
                            phy_id: 0,
                            alpha2: cc_in_req,
                        },
                        responder,
                    }
                ))) => {
                    assert_eq!(cc_in_req, <[u8; 2]>::from(cached_cc));
                    responder
                        .send(zx::sys::ZX_OK)
                        .expect("failed to send setCountry response.");
                }
            );
        }

        // The future should complete now.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn test_perform_recovery_disconnect_issues_request() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Suggest a recovery action to disconnect the client interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::FailedScan { iface_id: 1 }),
            action: recovery::RecoveryAction::IfaceRecovery(
                recovery::IfaceRecoveryOperation::Disconnect { iface_id: 1 },
            ),
        };

        let fut = phy_manager.perform_recovery(summary);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the disconnect request was made and respond with a success.
        // First, the client SME will be requested.
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetClientSme {
                iface_id: 1, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Next, the disconnect will be requested.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let mut sme_stream = sme_server.into_stream().into_future();
        assert_matches!(
            poll_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ClientSmeRequest::Disconnect{
                responder,
                reason: fidl_fuchsia_wlan_sme::UserDisconnectReason::Recovery
            }) => {
                responder.send().expect("Failed to send disconnect response")
            }
        );

        // The future should complete now.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn test_perform_recovery_stop_ap_issues_request() {
        let mut exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Suggest a recovery action to destroy the AP interface.
        let summary = recovery::RecoverySummary {
            defect: Defect::Iface(IfaceFailure::ApStartFailure { iface_id: 2 }),
            action: recovery::RecoveryAction::IfaceRecovery(
                recovery::IfaceRecoveryOperation::StopAp { iface_id: 2 },
            ),
        };

        let fut = phy_manager.perform_recovery(summary);
        let mut fut = pin!(fut);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);

        // Verify that the StopAp request was made and respond with a success.
        // First, the AP SME will be requested.
        let sme_server = assert_matches!(
            exec.run_until_stalled(&mut test_values.monitor_stream.next()),
            Poll::Ready(Some(Ok(fidl_fuchsia_wlan_device_service::DeviceMonitorRequest::GetApSme {
                iface_id: 2, sme_server, responder
            }))) => {
                // Send back a positive acknowledgement.
                assert!(responder.send(Ok(())).is_ok());
                sme_server
            }
        );

        // Expect the stop AP request.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        let mut sme_stream = sme_server.into_stream().into_future();
        assert_matches!(
            poll_ap_sme_req(&mut exec, &mut sme_stream),
            Poll::Ready(fidl_fuchsia_wlan_sme::ApSmeRequest::Stop{
                responder,
            }) => {
                responder.send(fidl_sme::StopApResultCode::Success).expect("Failed to send stop AP response")
            }
        );

        // The future should complete now.
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test]
    fn test_log_timeout_defect() {
        let _exec = TestExecutor::new();
        let mut test_values = test_setup();
        let mut phy_manager = phy_manager_for_recovery_test(
            test_values.monitor_proxy,
            test_values.node,
            test_values.telemetry_sender,
            test_values.recovery_sender,
        );

        // Verify that there are no defects to begin with.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 0);

        // Log a timeout.
        phy_manager.record_defect(Defect::Iface(IfaceFailure::Timeout {
            iface_id: 1,
            source: wlan_telemetry::TimeoutSource::Scan,
        }));

        // Verify that the defect was recorded.
        assert_eq!(phy_manager.phys[&0].defects.events.len(), 1);

        // Verify that the defect was reported to telemetry.
        assert_matches!(
            test_values.telemetry_receiver.try_next(),
            Ok(Some(TelemetryEvent::SmeTimeout { source: wlan_telemetry::TimeoutSource::Scan }))
        )
    }
}
