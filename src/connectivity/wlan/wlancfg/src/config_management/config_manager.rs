// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::network_config::{
    ConnectFailure, Credential, FailureReason, HIDDEN_PROBABILITY_HIGH, HiddenProbEvent,
    NetworkConfig, NetworkConfigError, NetworkIdentifier, PastConnectionData, PastConnectionList,
    SecurityType,
};
use super::stash_conversion::*;
use crate::client::types::{self, ScanObservation};
use crate::config_management::new_past_connection_list;
use crate::telemetry::{TelemetryEvent, TelemetrySender};
use anyhow::format_err;
use async_trait::async_trait;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async as fasync;
use futures::lock::Mutex;
use log::{error, info};
use std::collections::{HashMap, HashSet};
use wlan_storage::policy::{POLICY_STORAGE_ID, PolicyStorage};

const MAX_SAVED_NETWORKS: usize = 1000;

/// The Saved Network Manager keeps track of saved networks and provides thread-safe access to
/// saved networks. Networks are saved by NetworkConfig and accessed by their NetworkIdentifier
/// (SSID and security protocol). Network configs are saved in-memory, and part of each network
/// data is saved persistently. Futures aware locks are used in order to wait for the storage flush
/// operations to complete when data changes.
pub struct SavedNetworksManager {
    saved_networks: Mutex<NetworkConfigMap>,
    // Persistent storage for networks, which should be updated when there is a change to the data
    // that is saved between reboots.
    store: Mutex<PolicyStorage>,
    telemetry_sender: TelemetrySender,
}

/// Save a single network config per NetworkIdentifier (which combines SSID and security type).
type NetworkConfigMap = HashMap<NetworkIdentifier, NetworkConfig>;

#[async_trait(?Send)]
pub trait SavedNetworksManagerApi {
    /// Attempt to remove the NetworkConfig described by the specified NetworkIdentifier.
    /// Return true if a NetworkConfig is removed and false otherwise.
    async fn remove(&self, network_id: NetworkIdentifier) -> Result<bool, NetworkConfigError>;

    /// Get the count of networks in store, including multiple values with same SSID
    async fn known_network_count(&self) -> usize;

    /// Return the network config that matches the given NetworkIdentifier.
    async fn lookup(&self, id: &NetworkIdentifier) -> Option<NetworkConfig>;

    /// Return a list of network configs that could be used with the security type seen in a scan.
    /// This includes configs that have a lower security type that can be upgraded to match the
    /// provided detailed security type.
    async fn lookup_compatible(
        &self,
        ssid: &types::Ssid,
        scan_security: types::SecurityTypeDetailed,
    ) -> Vec<NetworkConfig>;

    /// Save a network. If a network with the same identifier already exists, it is overwritten
    /// and the old configuration is returned.
    async fn store(
        &self,
        network_id: NetworkIdentifier,
        credential: Credential,
    ) -> Result<Option<NetworkConfig>, NetworkConfigError>;

    /// Update the specified saved network with the result of an attempted connect.  If the
    /// specified network could have been connected to with a different security type and we
    /// do not find the specified config, we will check the other possible security type. For
    /// example if a WPA3 network is specified, we will check WPA2 if it isn't found. If the
    /// specified network is not saved, this function does not save it.
    async fn record_connect_result(
        &self,
        id: NetworkIdentifier,
        credential: &Credential,
        bssid: types::Bssid,
        connect_result: fidl_sme::ConnectResult,
        scan_type: types::ScanObservation,
    );

    /// Record the disconnect from a network, to be used for things such as avoiding connections
    /// that drop soon after starting.
    async fn record_disconnect(
        &self,
        id: &NetworkIdentifier,
        credential: &Credential,
        data: PastConnectionData,
    );

    async fn record_periodic_metrics(&self);

    /// Update hidden networks probabilities based on scan results. Record either results of a
    /// passive scan or a directed active scan.
    async fn record_scan_result(
        &self,
        target_ssids: Vec<types::Ssid>,
        results: &HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>>,
    );

    async fn is_network_single_bss(
        &self,
        id: &NetworkIdentifier,
        credential: &Credential,
    ) -> Result<bool, anyhow::Error>;

    // Return a list of every network config that has been saved.
    async fn get_networks(&self) -> Vec<NetworkConfig>;

    // Get the list of past connections for a specific BSS
    async fn get_past_connections(
        &self,
        id: &NetworkIdentifier,
        credential: &Credential,
        bssid: &types::Bssid,
    ) -> PastConnectionList;
}

impl SavedNetworksManager {
    /// Initializes a new Saved Network Manager by reading saved networks from local storage using
    /// a WLAN helper library. It will attempt to migrate any data from legacy storage.
    pub async fn new(telemetry_sender: TelemetrySender) -> Self {
        let storage = PolicyStorage::new_with_id(POLICY_STORAGE_ID).await;
        Self::new_with_storage(storage, telemetry_sender).await
    }

    /// Load data from persistent storage. The legacy stash data is deleted if it exists.
    pub async fn new_with_storage(
        mut store: PolicyStorage,
        telemetry_sender: TelemetrySender,
    ) -> Self {
        let mut saved_networks: HashMap<NetworkIdentifier, NetworkConfig> = HashMap::new();
        // Load saved networks from persistent storage. An error loading would mean that there was
        // nothing saved in the current version of persistent store and there was an error loading
        // legacy stash data.
        let stored_networks = store.load().await.unwrap_or_else(|e| {
            // If there is an error loading saved networks, we will run with no saved networks.
            error!("No saved networks loaded; error loading saved networks from storage: {}", e);
            Vec::new()
        });
        let mut errors_building_configs = HashSet::new();

        // Collect the list of persisted networks into the map that will be used internally.
        for persisted_data in stored_networks.into_iter() {
            let id = NetworkIdentifier::new(
                types::Ssid::from_bytes_unchecked(persisted_data.ssid),
                persisted_data.security_type.into(),
            );
            let config = NetworkConfig::new(
                id.clone(),
                persisted_data.credential.clone().into(),
                persisted_data.has_ever_connected,
                persisted_data.hidden_probability,
            );
            match config {
                Ok(config) => {
                    _ = saved_networks.insert(id, config);
                }
                Err(e) => {
                    _ = errors_building_configs.insert(e);
                }
            }
        }

        // If there errors creating network configs from persisted data, log unique types.
        if !errors_building_configs.is_empty() {
            error!(
                "At least one error occurred building network config from persisted data: {:?}",
                errors_building_configs
            )
        }

        Self {
            saved_networks: Mutex::new(saved_networks),
            store: Mutex::new(store),
            telemetry_sender,
        }
    }

    /// Creates a new config with a random storage path, ensuring a clean environment for an
    /// individual test
    #[cfg(test)]
    pub async fn new_for_test() -> Self {
        use crate::util::testing::generate_string;
        use futures::channel::mpsc;

        let store_id = generate_string();
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let store = PolicyStorage::new_with_id(&store_id).await;
        Self::new_with_storage(store, telemetry_sender).await
    }

    /// Clear the in memory storage and the persistent storage.
    #[cfg(test)]
    pub async fn clear(&self) -> Result<(), anyhow::Error> {
        self.saved_networks.lock().await.clear();
        self.store.lock().await.clear()
    }
}

#[async_trait(?Send)]
impl SavedNetworksManagerApi for SavedNetworksManager {
    async fn remove(&self, network_id: NetworkIdentifier) -> Result<bool, NetworkConfigError> {
        let mut saved_networks = self.saved_networks.lock().await;
        if saved_networks.remove(&network_id).is_some() {
            // Update persistent storage
            self.store
                .lock()
                .await
                .write(persistent_data_from_config_map(&saved_networks))
                .map_err(|e| {
                    error!("error writing network to persistent storage: {}", e);
                    NetworkConfigError::FileWriteError
                })?;
            return Ok(true);
        } else {
            // Check whether there is another network with the same SSID but different security
            // type to remove.
            let mut found_securities = SecurityType::list_variants();
            found_securities.retain(|security| {
                let id = NetworkIdentifier::new(network_id.ssid.clone(), *security);
                saved_networks.contains_key(&id)
            });
            if found_securities.is_empty() {
                info!("No network was found to remove with the provided SSID.");
            } else {
                info!(
                    "No config to remove with security type {:?}. Help: found different config(s) for this SSID with security {:?}",
                    network_id.security_type, found_securities
                );
            }
        }
        Ok(false)
    }

    /// Get the count of networks in store, including multiple values with same SSID
    async fn known_network_count(&self) -> usize {
        self.saved_networks.lock().await.values().count()
    }

    /// Return the network config that matches the given NetworkIdentifier. Note that this is a copy
    /// of the current data, so if data could have changed it should be looked up again. For
    /// example, data about roam scans change throughout a connection so callers cannot keep using
    /// the same network config for that data throughout the connection.
    async fn lookup(&self, id: &NetworkIdentifier) -> Option<NetworkConfig> {
        self.saved_networks.lock().await.get(id).cloned()
    }

    async fn lookup_compatible(
        &self,
        ssid: &types::Ssid,
        scan_security: types::SecurityTypeDetailed,
    ) -> Vec<NetworkConfig> {
        let saved_networks_guard = self.saved_networks.lock().await;
        let mut matching_configs = Vec::new();
        for security in compatible_policy_securities(&scan_security) {
            let id = NetworkIdentifier::new(ssid.clone(), security);
            if let Some(config) = saved_networks_guard.get(&id)
                && security_is_compatible(&scan_security, &config.credential)
            {
                matching_configs.push(config.clone());
            }
        }
        matching_configs
    }

    async fn store(
        &self,
        network_id: NetworkIdentifier,
        credential: Credential,
    ) -> Result<Option<NetworkConfig>, NetworkConfigError> {
        let mut saved_networks = self.saved_networks.lock().await;
        let num_saved_networks = saved_networks.len();

        if let Some(config) = saved_networks.get(&network_id)
            && config.credential == credential
        {
            info!("Saving a previously saved network with same password.");
            return Ok(None);
        }

        // Check if there are too many saved networks.
        if num_saved_networks >= MAX_SAVED_NETWORKS {
            return Err(NetworkConfigError::MaxSavedNetworksReached);
        }

        let network_config =
            NetworkConfig::new(network_id.clone(), credential.clone(), false, None)?;
        let evicted_config = saved_networks.insert(network_id, network_config);

        self.store.lock().await.write(persistent_data_from_config_map(&saved_networks)).map_err(
            |e| {
                error!("error writing network to persistent storage: {}", e);
                NetworkConfigError::FileWriteError
            },
        )?;

        Ok(evicted_config)
    }

    async fn record_connect_result(
        &self,
        id: NetworkIdentifier,
        credential: &Credential,
        bssid: types::Bssid,
        connect_result: fidl_sme::ConnectResult,
        scan_type: types::ScanObservation,
    ) {
        let mut saved_networks = self.saved_networks.lock().await;
        let network = match saved_networks.get_mut(&id) {
            Some(n) => n,
            None => {
                error!("Failed to find network to record result of connect attempt.");
                return;
            }
        };
        if &network.credential == credential {
            match (connect_result.code, connect_result.is_credential_rejected) {
                (fidl_ieee80211::StatusCode::Success, _) => {
                    let mut has_change = false;
                    let old_hidden_prob = network.hidden_probability;
                    if !network.has_ever_connected {
                        network.has_ever_connected = true;
                        has_change = true;
                    }
                    // Update hidden network probabiltiy
                    match scan_type {
                        types::ScanObservation::Passive => {
                            network.update_hidden_prob(HiddenProbEvent::ConnectPassive);
                        }
                        types::ScanObservation::Active => {
                            network.update_hidden_prob(HiddenProbEvent::ConnectActive);
                        }
                        types::ScanObservation::Unknown => {}
                    };

                    if network.hidden_probability != old_hidden_prob {
                        has_change = true;
                    }

                    if has_change {
                        // Update persistent storage since a config has changed.
                        let data = persistent_data_from_config_map(&saved_networks);
                        if let Err(e) = self.store.lock().await.write(data) {
                            info!("Failed to record successful connect in store: {}", e);
                        }
                    }
                }
                (fidl_ieee80211::StatusCode::Canceled, _) => {}
                (_, true) => {
                    network.perf_stats.connect_failures.entry(bssid).or_default().add(
                        ConnectFailure {
                            time: fasync::MonotonicInstant::now(),
                            reason: FailureReason::CredentialRejected,
                            bssid,
                        },
                    );
                }
                (_, _) => {
                    network.perf_stats.connect_failures.entry(bssid).or_default().add(
                        ConnectFailure {
                            time: fasync::MonotonicInstant::now(),
                            reason: FailureReason::GeneralFailure,
                            bssid,
                        },
                    );
                }
            }
            return;
        }
        // Will not reach here if we find the saved network with matching SSID and credential.
        error!("Failed to find matching network to record result of connect attempt.");
    }

    async fn record_disconnect(
        &self,
        id: &NetworkIdentifier,
        credential: &Credential,
        data: PastConnectionData,
    ) {
        let bssid = data.bssid;
        let mut saved_networks = self.saved_networks.lock().await;
        let network = match saved_networks.get_mut(id) {
            Some(n) => n,
            None => {
                info!("Failed to find network to record disconnect stats");
                return;
            }
        };
        if &network.credential == credential {
            network.perf_stats.past_connections.entry(bssid).or_default().add(data);
        }
    }

    async fn record_periodic_metrics(&self) {
        let saved_networks = self.saved_networks.lock().await;
        // Count the number of configs for each saved network
        let config_counts = saved_networks.iter().map(|_| 1).collect();
        self.telemetry_sender.send(TelemetryEvent::SavedNetworkCount {
            saved_network_count: saved_networks.len(),
            config_count_per_saved_network: config_counts,
        });
    }

    async fn record_scan_result(
        &self,
        target_ssids: Vec<types::Ssid>,
        results: &HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>>,
    ) {
        let mut saved_networks = self.saved_networks.lock().await;
        let mut has_change = false;

        for (network, bss_list) in results {
            // If there are BSSs seen with the same SSID but different security, it will be
            // recorded as multi BSS. But this is fine since the network will just not get the
            //  improvement to scan less.
            let has_multiple_bss = bss_list.len() > 1;
            // Determine if any BSSs seen for this network were observed passively.
            if bss_list.iter().any(|bss| bss.observation == ScanObservation::Passive) {
                // Look for compatible configs and record them as "SeenPassive" and with single
                // or multi BSS data.
                for security in compatible_policy_securities(&network.security_type) {
                    let config = match saved_networks
                        .get_mut(&NetworkIdentifier::new(network.ssid.clone(), security))
                    {
                        Some(config) => config,
                        None => continue,
                    };
                    // Check that the credential is compatible with the actual security type of
                    // the scan result.
                    if security_is_compatible(&network.security_type, &config.credential) {
                        let old_hidden_prob = config.hidden_probability;
                        config.update_hidden_prob(HiddenProbEvent::SeenPassive);
                        config.update_seen_multiple_bss(has_multiple_bss);
                        if config.hidden_probability != old_hidden_prob {
                            has_change = true;
                        }
                    }
                }
            }
        }

        // Update saved networks that match one of the targeted SSIDs but were *not* in scan results.
        for (id, config) in saved_networks.iter_mut() {
            if !target_ssids.contains(&id.ssid) {
                continue;
            }
            // For each config, check whether there is a scan result that
            // could be used to connect. If not, update the hidden probability.
            let potential_scan_results =
                results.iter().filter(|(scan_id, _)| scan_id.ssid == id.ssid).collect::<Vec<_>>();
            if !potential_scan_results.iter().any(|(scan_id, _)| {
                compatible_policy_securities(&scan_id.security_type).contains(&config.security_type)
                    && security_is_compatible(&scan_id.security_type, &config.credential)
            }) {
                let old_hidden_prob = config.hidden_probability;
                config.update_hidden_prob(HiddenProbEvent::NotSeenActive);
                if config.hidden_probability != old_hidden_prob {
                    has_change = true;
                }
            }
        }
        if has_change {
            let data = persistent_data_from_config_map(&saved_networks);
            if let Err(e) = self.store.lock().await.write(data) {
                info!("Failed to record scan result updates in store: {}", e);
            }
        }
    }

    /// Returns whether or not the network likely has only one BSS based on previous scans. This
    /// should be used instead of the network config if the network config may have been updated.
    /// For example, when making roam scan decisions this should be used instead of a network
    /// config obtained at the time of connecting.
    async fn is_network_single_bss(
        &self,
        id: &NetworkIdentifier,
        credential: &Credential,
    ) -> Result<bool, anyhow::Error> {
        let saved_networks_guard = self.saved_networks.lock().await;
        let config = saved_networks_guard.get(id).ok_or_else(|| {
            format_err!(
                "error checking if network is single BSS; no config with matching identifier"
            )
        })?;
        if &config.credential != credential {
            return Err(format_err!(
                "error checking if network is single BSS; saved credential does not match"
            ));
        }
        return Ok(config.is_likely_single_bss());
    }

    async fn get_networks(&self) -> Vec<NetworkConfig> {
        self.saved_networks.lock().await.values().cloned().collect()
    }

    async fn get_past_connections(
        &self,
        id: &NetworkIdentifier,
        credential: &Credential,
        bssid: &types::Bssid,
    ) -> PastConnectionList {
        self.saved_networks
            .lock()
            .await
            .get(id)
            .filter(|config| &config.credential == credential)
            .map(|config| {
                config.perf_stats.past_connections.get(bssid).cloned().unwrap_or_default()
            })
            .unwrap_or_else(|| new_past_connection_list())
    }
}

/// Returns a subset of potentially hidden saved networks, filtering probabilistically based
/// on how certain they are to be hidden.
pub fn select_subset_potentially_hidden_networks(
    saved_networks: Vec<NetworkConfig>,
) -> Vec<types::NetworkIdentifier> {
    saved_networks
        .into_iter()
        .filter(|saved_network| {
            // Roll a dice to see if we should scan for it. The function gen_range(low..high)
            // has an inclusive lower bound and exclusive upper bound, so using it as
            // `hidden_probability > gen_range(0..1)` means that:
            // - hidden_probability of 1 will _always_ be selected
            // - hidden_probability of 0 will _never_ be selected
            saved_network.hidden_probability > rand::random_range(0.0..1.0)
        })
        .map(|network| types::NetworkIdentifier {
            ssid: network.ssid,
            security_type: network.security_type,
        })
        .collect()
}

/// Returns all saved networks which we think have a high probability of being hidden.
pub fn select_high_probability_hidden_networks(
    saved_networks: Vec<NetworkConfig>,
) -> Vec<types::NetworkIdentifier> {
    saved_networks
        .into_iter()
        .filter(|saved_network| saved_network.hidden_probability >= HIDDEN_PROBABILITY_HIGH)
        .map(|network| types::NetworkIdentifier {
            ssid: network.ssid,
            security_type: network.security_type,
        })
        .collect()
}

/// Gets compatible `SecurityType`s for network candidates.
///
/// This function returns a sequence of `SecurityType`s that may be used to connect to a network
/// configured as described by the given `SecurityTypeDetailed`. If there is no compatible
/// `SecurityType`, then the sequence will be empty.
pub fn compatible_policy_securities(
    detailed_security: &types::SecurityTypeDetailed,
) -> Vec<SecurityType> {
    use fidl_sme::Protection::*;
    match detailed_security {
        Wpa3Enterprise | Wpa3Personal | Wpa2Wpa3Personal => {
            vec![SecurityType::Wpa2, SecurityType::Wpa3]
        }
        Wpa2Enterprise
        | Wpa2Personal
        | Wpa1Wpa2Personal
        | Wpa2PersonalTkipOnly
        | Wpa1Wpa2PersonalTkipOnly => vec![SecurityType::Wpa, SecurityType::Wpa2],
        Wpa1 => vec![SecurityType::Wpa],
        Wep => vec![SecurityType::Wep],
        // TODO(https://fxbug.dev/462514157): Map Owe and OpenOweTransition to correct security types
        Owe => vec![SecurityType::None],
        OpenOweTransition => vec![SecurityType::None],
        Open => vec![SecurityType::None],
        Unknown => vec![],
    }
}

pub fn security_is_compatible(
    scan_security: &types::SecurityTypeDetailed,
    credential: &Credential,
) -> bool {
    if (scan_security == &types::SecurityTypeDetailed::Wpa3Personal
        || scan_security == &types::SecurityTypeDetailed::Wpa3Enterprise)
        && let Credential::Psk(_) = credential
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_management::{
        PROB_HIDDEN_DEFAULT, PROB_HIDDEN_IF_CONNECT_ACTIVE, PROB_HIDDEN_IF_CONNECT_PASSIVE,
        PROB_HIDDEN_IF_SEEN_PASSIVE,
    };
    use crate::util::testing::{generate_random_bss, generate_string, random_connection_data};
    use assert_matches::assert_matches;
    use futures::channel::mpsc;
    use futures::task::Poll;
    use std::pin::pin;
    use test_case::test_case;

    #[fuchsia::test]
    async fn store_and_lookup() {
        let store_id = generate_string();
        let saved_networks = create_saved_networks(&store_id).await;
        let network_id_foo = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();

        assert!(saved_networks.lookup(&network_id_foo).await.is_none());
        assert_eq!(0, saved_networks.saved_networks.lock().await.len());
        assert_eq!(0, saved_networks.known_network_count().await);

        // Store a network and verify it was stored.
        assert!(
            saved_networks
                .store(network_id_foo.clone(), Credential::Password(b"qwertyuio".to_vec()))
                .await
                .expect("storing 'foo' failed")
                .is_none()
        );
        assert_eq!(
            Some(network_config("foo", "qwertyuio")),
            saved_networks.lookup(&network_id_foo).await
        );
        assert_eq!(1, saved_networks.known_network_count().await);

        // Store another network with the same SSID.
        let popped_network = saved_networks
            .store(network_id_foo.clone(), Credential::Password(b"12345678".to_vec()))
            .await
            .expect("storing 'foo' a second time failed");
        assert_eq!(popped_network, Some(network_config("foo", "qwertyuio")));

        // There should only be one saved "foo" network because we only allow one network per SSID
        // and security type. The second store should have replaced the first.
        assert_eq!(
            Some(network_config("foo", "12345678")),
            saved_networks.lookup(&network_id_foo).await
        );
        assert_eq!(1, saved_networks.known_network_count().await);

        // Store another network and verify.
        let network_id_baz = NetworkIdentifier::try_from("baz", SecurityType::Wpa2).unwrap();
        let psk = Credential::Psk(vec![1; 32]);
        let config_baz = NetworkConfig::new(network_id_baz.clone(), psk.clone(), false, None)
            .expect("failed to create network config");
        assert!(
            saved_networks
                .store(network_id_baz.clone(), psk)
                .await
                .expect("storing 'baz' with PSK failed")
                .is_none()
        );
        assert_eq!(Some(config_baz.clone()), saved_networks.lookup(&network_id_baz).await);
        assert_eq!(2, saved_networks.known_network_count().await);

        // Saved networks should persist when we create a saved networks manager with the same ID.
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let store = PolicyStorage::new_with_id(&store_id).await;

        let saved_networks =
            SavedNetworksManager::new_with_storage(store, TelemetrySender::new(telemetry_sender))
                .await;
        assert_eq!(
            Some(network_config("foo", "12345678")),
            saved_networks.lookup(&network_id_foo).await
        );
        assert_eq!(Some(config_baz), saved_networks.lookup(&network_id_baz).await);
        assert_eq!(2, saved_networks.known_network_count().await);
    }

    #[fuchsia::test]
    async fn store_twice() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();

        assert!(
            saved_networks
                .store(network_id.clone(), Credential::Password(b"qwertyuio".to_vec()))
                .await
                .expect("storing 'foo' failed")
                .is_none()
        );
        let popped_network = saved_networks
            .store(network_id.clone(), Credential::Password(b"qwertyuio".to_vec()))
            .await
            .expect("storing 'foo' a second time failed");
        // Because the same network was stored twice, nothing was evicted, so popped_network == None
        assert_eq!(popped_network, None);
        let expected_cfg = Some(network_config("foo", "qwertyuio"));
        assert_eq!(expected_cfg, saved_networks.lookup(&network_id).await);
        assert_eq!(1, saved_networks.known_network_count().await);
    }

    #[fuchsia::test]
    async fn store_many_same_ssid() {
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let saved_networks = SavedNetworksManager::new_for_test().await;

        // save multiple networks with same SSID and different credentials
        for i in 0..3 {
            let mut password = b"password".to_vec();
            password.push(i as u8);
            let popped_network = saved_networks
                .store(network_id.clone(), Credential::Password(password))
                .await
                .expect("Failed to saved network");
            if i >= 1 {
                assert!(popped_network.is_some());
            } else {
                assert!(popped_network.is_none());
            }
        }

        // since none have been connected to yet, we don't care which config was removed
        assert!(saved_networks.lookup(&network_id).await.is_some());
    }

    #[fuchsia::test]
    async fn store_fails_when_max_saved_networks_reached() {
        let saved_networks = SavedNetworksManager::new_for_test().await;

        // Pre-populate the hashmap with MAX_SAVED_NETWORKS (1000) unique saved networks
        // directly in memory to avoid writing to persistent storage 1000 times.
        let mut map = HashMap::with_capacity(MAX_SAVED_NETWORKS);
        for i in 0..MAX_SAVED_NETWORKS {
            let ssid = format!("ssid_{i}");
            let network_id =
                NetworkIdentifier::try_from(ssid.as_str(), SecurityType::Wpa2).unwrap();
            let config = NetworkConfig::new(
                network_id.clone(),
                Credential::Password(b"password123".to_vec()),
                false,
                None,
            )
            .unwrap();
            assert!(map.insert(network_id, config).is_none());
        }

        {
            let mut saved_map = saved_networks.saved_networks.lock().await;
            *saved_map = map;
        }

        assert_eq!(saved_networks.known_network_count().await, MAX_SAVED_NETWORKS);

        // Try to save a 1001st network with a new unique SSID
        let new_network_id =
            NetworkIdentifier::try_from("overflow_ssid", SecurityType::Wpa2).unwrap();
        let result = saved_networks
            .store(new_network_id, Credential::Password(b"password123".to_vec()))
            .await;

        assert_eq!(result, Err(NetworkConfigError::MaxSavedNetworksReached));
    }

    #[fuchsia::test]
    async fn store_and_remove() {
        let store_id = generate_string();
        let saved_networks = create_saved_networks(&store_id).await;

        let network_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let credential = Credential::Password(b"qwertyuio".to_vec());
        assert!(saved_networks.lookup(&network_id).await.is_none());
        assert_eq!(0, saved_networks.known_network_count().await);

        // Store a network and verify it was stored.
        assert!(
            saved_networks
                .store(network_id.clone(), credential.clone())
                .await
                .expect("storing 'foo' failed")
                .is_none()
        );
        assert_eq!(
            Some(network_config("foo", "qwertyuio")),
            saved_networks.lookup(&network_id).await
        );
        assert_eq!(1, saved_networks.known_network_count().await);

        // Remove the network and check it is gone
        assert!(saved_networks.remove(network_id.clone()).await.expect("removing 'foo' failed"));
        assert_eq!(0, saved_networks.known_network_count().await);
        // Check that the key in the saved networks manager's internal hashmap was removed.
        assert!(saved_networks.saved_networks.lock().await.get(&network_id).is_none());

        // If we try to remove the network again, we won't get an error and nothing happens
        assert!(!saved_networks.remove(network_id.clone()).await.expect("removing 'foo' failed"));

        // Check that removal persists.
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let store = PolicyStorage::new_with_id(&store_id).await;
        let saved_networks =
            SavedNetworksManager::new_with_storage(store, TelemetrySender::new(telemetry_sender))
                .await;
        assert_eq!(0, saved_networks.known_network_count().await);
        assert!(saved_networks.lookup(&network_id).await.is_none());
    }

    #[fuchsia::test]
    fn sme_protection_converts_to_lower_compatible() {
        use fidl_sme::Protection::*;
        let lower_compatible_pairs = vec![
            (Wpa3Enterprise, vec![SecurityType::Wpa2, SecurityType::Wpa3]),
            (Wpa3Personal, vec![SecurityType::Wpa2, SecurityType::Wpa3]),
            (Wpa2Wpa3Personal, vec![SecurityType::Wpa2, SecurityType::Wpa3]),
            (Wpa2Enterprise, vec![SecurityType::Wpa, SecurityType::Wpa2]),
            (Wpa2Personal, vec![SecurityType::Wpa, SecurityType::Wpa2]),
            (Wpa1Wpa2Personal, vec![SecurityType::Wpa, SecurityType::Wpa2]),
            (Wpa2PersonalTkipOnly, vec![SecurityType::Wpa, SecurityType::Wpa2]),
            (Wpa1Wpa2PersonalTkipOnly, vec![SecurityType::Wpa, SecurityType::Wpa2]),
            (Wpa1, vec![SecurityType::Wpa]),
            (Wep, vec![SecurityType::Wep]),
            (Open, vec![SecurityType::None]),
            (Unknown, vec![]),
        ];
        for (detailed_security, security) in lower_compatible_pairs {
            assert_eq!(compatible_policy_securities(&detailed_security), security);
        }
    }

    #[fuchsia::test]
    async fn lookup_compatible_returns_both_compatible_configs() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let ssid = types::Ssid::try_from("foo").unwrap();
        let network_id_wpa2 = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa2);
        let network_id_wpa3 = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa3);
        let credential_wpa2 = Credential::Password(b"password".to_vec());
        let credential_wpa3 = Credential::Password(b"wpa3-password".to_vec());

        // Check that lookup_compatible does not modify the SavedNetworksManager and returns an
        // empty vector if there is no matching config.
        let results = saved_networks
            .lookup_compatible(&ssid, types::SecurityTypeDetailed::Wpa2Wpa3Personal)
            .await;
        assert!(results.is_empty());
        assert_eq!(saved_networks.known_network_count().await, 0);

        // Store a couple of network configs that could both be use to connect to a WPA2/WPA3
        // network.
        assert!(
            saved_networks
                .store(network_id_wpa2.clone(), credential_wpa2.clone())
                .await
                .expect("Failed to store network")
                .is_none()
        );
        assert!(
            saved_networks
                .store(network_id_wpa3.clone(), credential_wpa3.clone())
                .await
                .expect("Failed to store network")
                .is_none()
        );
        // Store a network with the same SSID but a not-compatible security type.
        let network_id_wep = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa);
        assert!(
            saved_networks
                .store(network_id_wep.clone(), Credential::Password(b"abcdefgh".to_vec()))
                .await
                .expect("Failed to store network")
                .is_none()
        );

        let results = saved_networks
            .lookup_compatible(&ssid, types::SecurityTypeDetailed::Wpa2Wpa3Personal)
            .await;
        let expected_config_wpa2 =
            NetworkConfig::new(network_id_wpa2, credential_wpa2, false, None)
                .expect("Failed to create config");
        let expected_config_wpa3 =
            NetworkConfig::new(network_id_wpa3, credential_wpa3, false, None)
                .expect("Failed to create config");
        assert_eq!(results.len(), 2);
        assert!(results.contains(&expected_config_wpa2));
        assert!(results.contains(&expected_config_wpa3));
    }

    #[test_case(types::SecurityTypeDetailed::Wpa3Personal)]
    #[test_case(types::SecurityTypeDetailed::Wpa3Enterprise)]
    #[fuchsia::test(add_test_attr = false)]
    fn lookup_compatible_does_not_return_wpa3_psk(
        wpa3_detailed_security: types::SecurityTypeDetailed,
    ) {
        let mut exec = fasync::TestExecutor::new();
        let saved_networks = exec.run_singlethreaded(SavedNetworksManager::new_for_test());

        // Store a WPA3 config with a password that will match and a PSK config that won't match
        // to a WPA3 network.
        let ssid = types::Ssid::try_from("foo").unwrap();
        let network_id_psk = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa2);
        let network_id_password = NetworkIdentifier::new(ssid.clone(), SecurityType::Wpa3);
        let credential_psk = Credential::Psk(vec![5; 32]);
        let credential_password = Credential::Password(b"mypassword".to_vec());
        assert!(
            exec.run_singlethreaded(
                saved_networks.store(network_id_psk.clone(), credential_psk.clone()),
            )
            .expect("Failed to store network")
            .is_none()
        );
        assert!(
            exec.run_singlethreaded(
                saved_networks.store(network_id_password.clone(), credential_password.clone()),
            )
            .expect("Failed to store network")
            .is_none()
        );

        // Only the WPA3 config with a credential should be returned.
        let expected_config_wpa3 =
            NetworkConfig::new(network_id_password, credential_password, false, None)
                .expect("Failed to create configc");
        let results = exec
            .run_singlethreaded(saved_networks.lookup_compatible(&ssid, wpa3_detailed_security));
        assert_eq!(results, vec![expected_config_wpa3]);
    }

    #[fuchsia::test]
    async fn connect_network() {
        let store_id = generate_string();

        let saved_networks = create_saved_networks(&store_id).await;

        let network_id = NetworkIdentifier::try_from("bar", SecurityType::Wpa2).unwrap();
        let credential = Credential::Password(b"password".to_vec());
        let bssid = types::Bssid::from([4; 6]);

        // If connect and network hasn't been saved, we should not save the network.
        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fake_successful_connect_result(),
                types::ScanObservation::Unknown,
            )
            .await;
        assert!(saved_networks.lookup(&network_id).await.is_none());
        assert_eq!(saved_networks.saved_networks.lock().await.len(), 0);
        assert_eq!(0, saved_networks.known_network_count().await);

        // Save the network and record a successful connection.
        assert!(
            saved_networks
                .store(network_id.clone(), credential.clone())
                .await
                .expect("Failed save network")
                .is_none()
        );

        let config = network_config("bar", "password");
        assert_eq!(Some(config), saved_networks.lookup(&network_id).await);

        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fake_successful_connect_result(),
                types::ScanObservation::Unknown,
            )
            .await;

        // The network should be saved with the connection recorded. We should not have recorded
        // that the network was connected to passively or actively.
        assert_matches!(saved_networks.lookup(&network_id).await, Some(config) => {
            assert!(config.has_ever_connected);
            assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
        });

        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fake_successful_connect_result(),
                types::ScanObservation::Active,
            )
            .await;
        // We should now see that we connected to the network after an active scan.
        assert_matches!(saved_networks.lookup(&network_id).await, Some(config) => {
            assert!(config.has_ever_connected);
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_CONNECT_ACTIVE);
        });

        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fake_successful_connect_result(),
                types::ScanObservation::Passive,
            )
            .await;
        // The config should have a lower hidden probability after connecting after a passive scan.
        assert_matches!(saved_networks.lookup(&network_id).await, Some(config) => {
            assert!(config.has_ever_connected);
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_CONNECT_PASSIVE);
        });

        // Check that recording the connect event updates the persisted data by loading the data.
        let store_reloaded = PolicyStorage::new_with_id(&store_id).await;
        let (telemetry_sender_reloaded, _) = mpsc::channel::<TelemetryEvent>(100);
        let saved_networks_reloaded = SavedNetworksManager::new_with_storage(
            store_reloaded,
            TelemetrySender::new(telemetry_sender_reloaded),
        )
        .await;
        assert_matches!(saved_networks_reloaded.lookup(&network_id).await, Some(config) => {
            assert!(config.has_ever_connected);
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_CONNECT_PASSIVE);
        });
    }

    #[fuchsia::test]
    async fn test_record_connect_updates_one() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let net_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let net_id_also_valid = NetworkIdentifier::try_from("foo", SecurityType::Wpa).unwrap();
        let credential = Credential::Password(b"some_password".to_vec());
        let bssid = types::Bssid::from([2; 6]);

        // Save the networks and record a successful connection.
        assert!(
            saved_networks
                .store(net_id.clone(), credential.clone())
                .await
                .expect("Failed save network")
                .is_none()
        );
        assert!(
            saved_networks
                .store(net_id_also_valid.clone(), credential.clone())
                .await
                .expect("Failed save network")
                .is_none()
        );
        saved_networks
            .record_connect_result(
                net_id.clone(),
                &credential,
                bssid,
                fake_successful_connect_result(),
                types::ScanObservation::Unknown,
            )
            .await;

        assert_matches!(saved_networks.lookup(&net_id).await, Some(config) => {
            assert!(config.has_ever_connected);
        });
        // If the specified network identifier is found, record_conenct_result should not mark
        // another config even if it could also have been used for the connect attempt.
        assert_matches!(saved_networks.lookup(&net_id_also_valid).await, Some(config) => {
            assert!(!config.has_ever_connected);
        });
    }

    #[fuchsia::test]
    async fn test_record_connect_failure() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::None).unwrap();
        let credential = Credential::None;
        let bssid = types::Bssid::from([1; 6]);
        let before_recording = fasync::MonotonicInstant::now();

        // Verify that recording connect result does not save the network.
        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    ..fake_successful_connect_result()
                },
                types::ScanObservation::Unknown,
            )
            .await;
        assert!(saved_networks.lookup(&network_id).await.is_none());
        assert_eq!(0, saved_networks.saved_networks.lock().await.len());
        assert_eq!(0, saved_networks.known_network_count().await);

        // Record that the connect failed.
        assert!(
            saved_networks
                .store(network_id.clone(), credential.clone())
                .await
                .expect("Failed save network")
                .is_none()
        );
        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    ..fake_successful_connect_result()
                },
                types::ScanObservation::Unknown,
            )
            .await;
        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::RefusedReasonUnspecified,
                    is_credential_rejected: true,
                    ..fake_successful_connect_result()
                },
                types::ScanObservation::Unknown,
            )
            .await;

        // Check that the failures were recorded correctly.
        assert_eq!(1, saved_networks.known_network_count().await);
        let saved_config =
            saved_networks.lookup(&network_id).await.expect("Failed to get saved network config");
        let connect_failures = saved_config.get_recent_connection_failures(before_recording);
        assert_matches!(connect_failures, failures => {
            // There are 2 failures. One is a general failure and one rejected credentials failure.
            assert_eq!(failures.len(), 2);
            assert!(failures.iter().any(|failure| failure.reason == FailureReason::GeneralFailure));
            assert!(failures.iter().any(|failure| failure.reason == FailureReason::CredentialRejected));
            // Both failures have the correct BSSID
            for failure in failures.iter() {
                assert_eq!(failure.bssid, bssid);
                assert_eq!(failure.bssid, bssid);
            }
        });
    }

    #[fuchsia::test]
    async fn test_record_connect_cancelled_ignored() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::None).unwrap();
        let credential = Credential::None;
        let bssid = types::Bssid::from([0; 6]);
        let before_recording = fasync::MonotonicInstant::now();

        // Verify that recording connect result does not save the network.
        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::Canceled,
                    ..fake_successful_connect_result()
                },
                types::ScanObservation::Unknown,
            )
            .await;
        assert!(saved_networks.lookup(&network_id).await.is_none());
        assert_eq!(saved_networks.saved_networks.lock().await.len(), 0);
        assert_eq!(0, saved_networks.known_network_count().await);

        // Record that the connect was canceled.
        assert!(
            saved_networks
                .store(network_id.clone(), credential.clone())
                .await
                .expect("Failed save network")
                .is_none()
        );
        saved_networks
            .record_connect_result(
                network_id.clone(),
                &credential,
                bssid,
                fidl_sme::ConnectResult {
                    code: fidl_ieee80211::StatusCode::Canceled,
                    ..fake_successful_connect_result()
                },
                types::ScanObservation::Unknown,
            )
            .await;

        // Check that there are no failures recorded for this saved network.
        assert_eq!(1, saved_networks.known_network_count().await);
        let saved_config =
            saved_networks.lookup(&network_id).await.expect("Failed to get saved network config");
        let connect_failures = saved_config.get_recent_connection_failures(before_recording);
        assert_eq!(0, connect_failures.len());
    }

    #[fuchsia::test]
    async fn test_record_disconnect() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let credential = Credential::Psk(vec![1; 32]);
        let data = random_connection_data();

        saved_networks.record_disconnect(&id, &credential, data).await;
        // Verify that nothing happens if the network was not already saved.
        assert_eq!(saved_networks.saved_networks.lock().await.len(), 0);
        assert_eq!(saved_networks.known_network_count().await, 0);

        // Save the network and record a disconnect.
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        saved_networks.record_disconnect(&id, &credential, data).await;

        // Check that a data was recorded about the connection that just ended.
        let saved_config = saved_networks.lookup(&id).await.expect("Failed to get saved network");
        let recent_connections =
            saved_config.get_recent_connections(fasync::MonotonicInstant::INFINITE_PAST);
        assert_matches!(recent_connections.as_slice(), [connection_data] => {
            assert_eq!(connection_data, &data);
        })
    }

    #[fuchsia::test]
    async fn test_record_undirected_scan() {
        let store_id = generate_string();
        let saved_networks = create_saved_networks(&store_id).await;
        let saved_seen_id = NetworkIdentifier::try_from("foo", SecurityType::None).unwrap();
        let saved_seen_network = types::NetworkIdentifierDetailed {
            ssid: saved_seen_id.ssid.clone(),
            security_type: types::SecurityTypeDetailed::Open,
        };
        let unsaved_id = NetworkIdentifier::try_from("bar", SecurityType::Wpa2).unwrap();
        let unsaved_network = types::NetworkIdentifierDetailed {
            ssid: unsaved_id.ssid.clone(),
            security_type: types::SecurityTypeDetailed::Wpa2Personal,
        };
        let saved_unseen_id = NetworkIdentifier::try_from("baz", SecurityType::Wpa2).unwrap();
        let seen_credential = Credential::None;
        let unseen_credential = Credential::Password(b"password".to_vec());

        // Save the networks
        assert!(
            saved_networks
                .store(saved_seen_id.clone(), seen_credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        assert!(
            saved_networks
                .store(saved_unseen_id.clone(), unseen_credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );

        // Record passive scan results, including the saved network and another network.
        let results: HashMap<types::NetworkIdentifierDetailed, Vec<types::Bss>> = HashMap::from([
            (
                saved_seen_network,
                vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
            ),
            (unsaved_network, vec![generate_random_bss()]),
        ]);

        saved_networks
            .record_scan_result(vec!["some_other_ssid".try_into().unwrap()], &results)
            .await;

        assert_matches!(saved_networks.lookup(&saved_seen_id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_SEEN_PASSIVE);
        });
        assert_matches!(saved_networks.lookup(&saved_unseen_id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
        });

        // Re-open storage from store_id to verify the new hidden_probability persisted across reload.
        let store_reloaded = PolicyStorage::new_with_id(&store_id).await;
        let (telemetry_sender_reloaded, _) = mpsc::channel::<TelemetryEvent>(100);
        let saved_networks_reloaded = SavedNetworksManager::new_with_storage(
            store_reloaded,
            TelemetrySender::new(telemetry_sender_reloaded),
        )
        .await;
        assert_matches!(saved_networks_reloaded.lookup(&saved_seen_id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_SEEN_PASSIVE);
        });
        assert_matches!(saved_networks_reloaded.lookup(&saved_unseen_id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
        });
    }

    #[fuchsia::test]
    async fn test_record_undirected_scan_with_upgraded_security() {
        // Test that if we see a different compatible (higher) scan result for a saved network that
        // could be used to connect, recording the scan results will change the hidden probability.
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foobar", SecurityType::Wpa2).unwrap();
        let credential = Credential::Password(b"credential".to_vec());

        // Save the networks
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );

        // Record passive scan results
        let results = HashMap::from([(
            types::NetworkIdentifierDetailed {
                ssid: id.ssid.clone(),
                security_type: types::SecurityTypeDetailed::Wpa3Personal,
            },
            vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
        )]);
        saved_networks.record_scan_result(vec![], &results).await;
        // The network was seen in a passive scan, so hidden probability should be updated.
        assert_matches!(saved_networks.lookup(&id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_SEEN_PASSIVE);
        });
    }

    #[fuchsia::test]
    async fn test_record_undirected_scan_incompatible_credential() {
        // Test that if we see a different compatible (higher) scan result for a saved network that
        // could be used to connect, recording the scan results will change the hidden probability.
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foobar", SecurityType::Wpa2).unwrap();
        let credential = Credential::Psk(vec![8; 32]);

        // Save the networks
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );

        // Record passive scan results, including the saved network and another network.
        let results = HashMap::from([(
            types::NetworkIdentifierDetailed {
                ssid: id.ssid.clone(),
                security_type: types::SecurityTypeDetailed::Wpa3Personal,
            },
            vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
        )]);
        saved_networks.record_scan_result(vec![], &results).await;
        // The network in the passive scan results was not compatible, so hidden probability should
        // not have been updated.
        assert_matches!(saved_networks.lookup(&id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
        });
    }

    #[fuchsia::test]
    async fn test_record_directed_scan_for_upgraded_security() {
        // Test that if we see a different compatible (higher) scan result for a saved network that
        // could be used to connect in a directed scan, the hidden probability will not be lowered.
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foobar", SecurityType::Wpa).unwrap();
        let credential = Credential::Password(b"credential".to_vec());

        // Save the networks
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);

        // Record directed scan results. The config's probability hidden should not be lowered
        // since we did not fail to see it in a directed scan.
        let results = HashMap::from([(
            types::NetworkIdentifierDetailed {
                ssid: id.ssid.clone(),
                security_type: types::SecurityTypeDetailed::Wpa2Personal,
            },
            vec![types::Bss { observation: ScanObservation::Active, ..generate_random_bss() }],
        )]);
        let target = vec![id.ssid.clone()];
        saved_networks.record_scan_result(target, &results).await;

        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
    }

    #[fuchsia::test]
    async fn test_record_directed_scan_for_incompatible_credential() {
        // Test that if we see a network that is not compatible because of the saved credential
        // (but is otherwise compatible), the directed scan is not considered successful and the
        // hidden probability of the config is lowered.
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let credential = Credential::Psk(vec![11; 32]);

        // Save the networks
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);

        // Record directed scan results. The seen network does not match the saved network even
        // though security is compatible, since the security type is not compatible with the PSK.
        let target = vec![id.ssid.clone()];
        let results = HashMap::from([(
            types::NetworkIdentifierDetailed {
                ssid: id.ssid.clone(),
                security_type: types::SecurityTypeDetailed::Wpa3Personal,
            },
            vec![types::Bss { observation: ScanObservation::Active, ..generate_random_bss() }],
        )]);
        saved_networks.record_scan_result(target, &results).await;
        // The hidden probability should have been lowered because a directed scan failed to find
        // the network.
        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert!(config.hidden_probability < PROB_HIDDEN_DEFAULT);
    }

    #[fuchsia::test]
    async fn test_record_directed_scan_no_ssid_match() {
        // Test that recording directed active scan results does not mistakenly match a config with
        // a network with a different SSID.

        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let credential = Credential::Psk(vec![11; 32]);
        let diff_ssid = types::Ssid::try_from("other-ssid").unwrap();

        // Save the networks
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);

        // Record directed scan results. We target the saved network but see a different one.
        let target = vec![id.ssid.clone()];
        let results = HashMap::from([(
            types::NetworkIdentifierDetailed {
                ssid: diff_ssid,
                security_type: types::SecurityTypeDetailed::Wpa2Personal,
            },
            vec![types::Bss { observation: ScanObservation::Active, ..generate_random_bss() }],
        )]);
        saved_networks.record_scan_result(target, &results).await;

        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert!(config.hidden_probability < PROB_HIDDEN_DEFAULT);
    }

    #[fuchsia::test]
    async fn test_record_directed_one_not_compatible_one_compatible() {
        // Test that if we see two networks with the same SSID but only one is compatible, the scan
        // is recorded as successful for the config. In other words it isn't mistakenly recorded as
        // a failure because of the config that isn't compatible.
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let credential = Credential::Password(b"foo-pass".to_vec());

        // Save the networks
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);

        // Record directed scan results. We see one network with the same SSID that doesn't match,
        // and one that does match.
        let target = vec![id.ssid.clone()];
        let results = HashMap::from([
            (
                types::NetworkIdentifierDetailed {
                    ssid: id.ssid.clone(),
                    security_type: types::SecurityTypeDetailed::Wpa1,
                },
                vec![types::Bss { observation: ScanObservation::Active, ..generate_random_bss() }],
            ),
            (
                types::NetworkIdentifierDetailed {
                    ssid: id.ssid.clone(),
                    security_type: types::SecurityTypeDetailed::Wpa2Personal,
                },
                vec![types::Bss { observation: ScanObservation::Active, ..generate_random_bss() }],
            ),
        ]);
        saved_networks.record_scan_result(target, &results).await;
        // Since the directed scan found a matching network, the hidden probability should not
        // have been lowered.
        let config = saved_networks.lookup(&id).await.expect("failed to lookup config");
        assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
    }

    #[fuchsia::test]
    async fn test_record_both_directed_and_undirected() {
        let saved_networks = SavedNetworksManager::new_for_test().await;
        let saved_undirected_id = NetworkIdentifier::try_from("foo", SecurityType::None).unwrap();
        let saved_undirected_network = types::NetworkIdentifierDetailed {
            ssid: saved_undirected_id.ssid.clone(),
            security_type: types::SecurityTypeDetailed::Open,
        };
        let saved_directed_id = NetworkIdentifier::try_from("bar", SecurityType::None).unwrap();
        let credential = Credential::None;

        // Save the networks
        assert!(
            saved_networks
                .store(saved_undirected_id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );
        assert!(
            saved_networks
                .store(saved_directed_id.clone(), credential.clone())
                .await
                .expect("Failed to save network")
                .is_none()
        );

        // Verify assumption
        assert_matches!(saved_networks.lookup(&saved_directed_id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_DEFAULT);
        });

        // Record scan results
        let results = HashMap::from([(
            saved_undirected_network,
            vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
        )]);
        saved_networks.record_scan_result(vec![saved_directed_id.ssid.clone()], &results).await;

        // The undirected (but seen) network is modified
        assert_matches!(saved_networks.lookup(&saved_undirected_id).await, Some(config) => {
            assert_eq!(config.hidden_probability, PROB_HIDDEN_IF_SEEN_PASSIVE);
        });
        // The directed (but *not* seen) network is modified
        assert_matches!(saved_networks.lookup(&saved_directed_id).await, Some(config) => {
            assert!(config.hidden_probability < PROB_HIDDEN_DEFAULT);
        });
    }

    #[fuchsia::test]
    async fn clear() {
        let store_id = "clear";
        let network_id = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let saved_networks = create_saved_networks(store_id).await;

        assert!(
            saved_networks
                .store(network_id.clone(), Credential::Password(b"qwertyuio".to_vec()))
                .await
                .expect("storing 'foo' failed")
                .is_none()
        );
        assert_eq!(
            Some(network_config("foo", "qwertyuio")),
            saved_networks.lookup(&network_id).await
        );
        assert_eq!(1, saved_networks.known_network_count().await);

        saved_networks.clear().await.expect("failed to clear saved networks");
        assert_eq!(0, saved_networks.saved_networks.lock().await.len());
        assert_eq!(0, saved_networks.known_network_count().await);

        // Load store from storage to verify it is also gone from persistent storage
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let store = PolicyStorage::new_with_id(store_id).await;
        let saved_networks =
            SavedNetworksManager::new_with_storage(store, TelemetrySender::new(telemetry_sender))
                .await;

        assert_eq!(0, saved_networks.known_network_count().await);
    }

    impl std::fmt::Debug for SavedNetworksManager {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("SavedNetworksManager")
                .field("saved_networks", &self.saved_networks)
                .finish()
        }
    }

    #[fuchsia::test]
    fn test_store_errors_cause_write_errors() {
        use fidl::endpoints::create_request_stream;
        use fidl_fuchsia_stash as fidl_stash;
        use futures::StreamExt;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // Use a path for the persistent store that will cause write errors.
        let store_path_str = "/////";
        let mut exec = fasync::TestExecutor::new();

        // Initialize stash proxies such that SavedNetworksManager initialize doesn't wait on
        // and doesn't load anything from the legacy stash.
        let (stash_client, mut request_stream) =
            create_request_stream::<fidl_stash::SecureStoreMarker>();

        let read_from_stash = Arc::new(AtomicBool::new(false));

        let _task = {
            let read_from_stash = read_from_stash.clone();
            fasync::Task::local(async move {
                while let Some(request) = request_stream.next().await {
                    match request.unwrap() {
                        fidl_stash::SecureStoreRequest::Identify { .. } => {}
                        fidl_stash::SecureStoreRequest::CreateAccessor {
                            accessor_request, ..
                        } => {
                            let read_from_stash = read_from_stash.clone();
                            fuchsia_async::EHandle::local().spawn_detached(async move {
                                let mut request_stream = accessor_request.into_stream();
                                while let Some(request) = request_stream.next().await {
                                    match request.unwrap() {
                                        fidl_stash::StoreAccessorRequest::ListPrefix { .. } => {
                                            read_from_stash.store(true, Ordering::Relaxed);
                                            // If we just drop the iterator, it should trigger a
                                            // read error.
                                        }
                                        _ => unreachable!(),
                                    }
                                }
                            });
                        }
                    }
                }
            })
        };

        // Use a persistent store with the invalid file name and legacy stash which returns errors.
        let store =
            PolicyStorage::new_with_stash_proxy_and_id(stash_client.into_proxy(), store_path_str);

        // Initialize the saved networks manager with the file that should cause write errors, the
        // legacy stash that will load nothing, and empty legacy known ess store file.
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let init_fut = SavedNetworksManager::new_with_storage(store, telemetry_sender);
        let mut init_fut = pin!(init_fut);
        let saved_networks = assert_matches!(exec.run_until_stalled(&mut init_fut), Poll::Ready(snm) => {
            snm
        });

        // Save and remove networks and check that we get storage write errors
        let ssid = "foo";
        let credential = Credential::None;
        let network_id = NetworkIdentifier::try_from(ssid, SecurityType::None).unwrap();
        let save_fut = saved_networks.store(network_id.clone(), credential);
        let mut save_fut = pin!(save_fut);

        assert_matches!(
            exec.run_until_stalled(&mut save_fut),
            Poll::Ready(Err(NetworkConfigError::FileWriteError))
        );

        // The network should have been saved temporarily even if saving the network gives an error.
        assert_matches!(exec.run_until_stalled(&mut saved_networks.lookup(&network_id)), Poll::Ready(config) => {
            assert_eq!(config, Some(network_config(ssid, "")));
        });
        assert_matches!(exec.run_until_stalled(&mut saved_networks.known_network_count()), Poll::Ready(count) => {
            assert_eq!(count, 1);
        });
    }

    /// Create a saved networks manager and clear the contents. Storage ID should be different for
    /// each test so that they don't interfere.
    async fn create_saved_networks(store_id: &str) -> SavedNetworksManager {
        let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let store = PolicyStorage::new_with_id(store_id).await;
        let saved_networks =
            SavedNetworksManager::new_with_storage(store, TelemetrySender::new(telemetry_sender))
                .await;
        saved_networks.clear().await.expect("failed to clear saved networks");
        saved_networks
    }

    /// Convience function for creating network configs with default values as they would be
    /// initialized when read from KnownEssStore. Credential is password or none, and security
    /// type is WPA2 or none.
    fn network_config(ssid: &str, password: impl Into<Vec<u8>>) -> NetworkConfig {
        let credential = Credential::from_bytes(password.into());
        let id = NetworkIdentifier::try_from(ssid, credential.derived_security_type()).unwrap();
        let has_ever_connected = false;
        NetworkConfig::new(id, credential, has_ever_connected, None).unwrap()
    }

    #[fuchsia::test]
    async fn record_metrics_when_called_on_class() {
        let store_id = generate_string();
        let (telemetry_sender, mut telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
        let telemetry_sender = TelemetrySender::new(telemetry_sender);
        let store = PolicyStorage::new_with_id(&store_id).await;

        let saved_networks = SavedNetworksManager::new_with_storage(store, telemetry_sender).await;
        let network_id_foo = NetworkIdentifier::try_from("foo", SecurityType::Wpa2).unwrap();
        let network_id_baz = NetworkIdentifier::try_from("baz", SecurityType::Wpa2).unwrap();

        assert!(saved_networks.lookup(&network_id_foo).await.is_none());
        assert_eq!(0, saved_networks.saved_networks.lock().await.len());
        assert_eq!(0, saved_networks.known_network_count().await);

        // Store a network and verify it was stored.
        assert!(
            saved_networks
                .store(network_id_foo.clone(), Credential::Password(b"qwertyuio".to_vec()))
                .await
                .expect("storing 'foo' failed")
                .is_none()
        );
        assert_eq!(1, saved_networks.known_network_count().await);

        // Store another network and verify.
        assert!(
            saved_networks
                .store(network_id_baz.clone(), Credential::Psk(vec![1; 32]))
                .await
                .expect("storing 'baz' with PSK failed")
                .is_none()
        );
        assert_eq!(2, saved_networks.known_network_count().await);

        // Record metrics
        saved_networks.record_periodic_metrics().await;

        // Verify metric is logged with two saved networks, which each have one config
        assert_matches!(telemetry_receiver.try_next(), Ok(Some(TelemetryEvent::SavedNetworkCount { saved_network_count, config_count_per_saved_network })) => {
            assert_eq!(saved_network_count, 2);
            assert_eq!(config_count_per_saved_network, [1, 1]);
        });
    }

    #[fuchsia::test]
    async fn probabilistic_choosing_of_hidden_networks() {
        // Create three networks with 1, 0, 0.5 hidden probability
        let id_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("hidden").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_hidden = NetworkConfig::new(
            id_hidden.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(1.0),
        )
        .expect("failed to create network config");

        let id_not_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("not_hidden").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_not_hidden = NetworkConfig::new(
            id_not_hidden.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(0.0),
        )
        .expect("failed to create network config");

        let id_maybe_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("maybe_hidden").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_maybe_hidden = NetworkConfig::new(
            id_maybe_hidden.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(0.5),
        )
        .expect("failed to create network config");

        let mut maybe_hidden_selection_count = 0;
        let mut hidden_selection_count = 0;

        // Run selection many times, to ensure the probability is working as expected.
        for _ in 1..100 {
            let selected_networks = select_subset_potentially_hidden_networks(vec![
                net_config_hidden.clone(),
                net_config_not_hidden.clone(),
                net_config_maybe_hidden.clone(),
            ]);
            // The 1.0 probability should always be picked
            assert!(selected_networks.contains(&id_hidden));
            // The 0 probability should never be picked
            assert!(!selected_networks.contains(&id_not_hidden));

            // Keep track of how often the networks were selected
            if selected_networks.contains(&id_maybe_hidden) {
                maybe_hidden_selection_count += 1;
            }
            if selected_networks.contains(&id_hidden) {
                hidden_selection_count += 1;
            }
        }

        // The 0.5 probability network should be picked at least once, but not every time. With 100
        // runs, the chances of either of these assertions flaking is 1 / (0.5^100), i.e. 1 in 1e30.
        // Even with a hypothetical 1,000,000 test runs per day, there would be an average of 1e24
        // days between flakes due to this test.
        assert!(maybe_hidden_selection_count > 0);
        assert!(maybe_hidden_selection_count < hidden_selection_count);
    }

    #[fuchsia::test]
    async fn test_select_high_probability_hidden_networks() {
        // Create three networks with 1, 0, 0.5 hidden probability
        let id_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("hidden").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_hidden = NetworkConfig::new(
            id_hidden.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(1.0),
        )
        .expect("failed to create network config");

        let id_maybe_hidden_high = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("maybe_hidden_high").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_maybe_hidden_high = NetworkConfig::new(
            id_maybe_hidden_high.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(0.8),
        )
        .expect("failed to create network config");

        let id_maybe_hidden_low = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("maybe_hidden_low").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_maybe_hidden_low = NetworkConfig::new(
            id_maybe_hidden_low.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(0.7),
        )
        .expect("failed to create network config");

        let id_not_hidden = types::NetworkIdentifier {
            ssid: types::Ssid::try_from("not_hidden").unwrap(),
            security_type: types::SecurityType::Wpa2,
        };
        let net_config_not_hidden = NetworkConfig::new(
            id_not_hidden.clone(),
            Credential::Password(b"password".to_vec()),
            false,
            Some(0.0),
        )
        .expect("failed to create network config");

        let selected_networks = select_high_probability_hidden_networks(vec![
            net_config_hidden.clone(),
            net_config_maybe_hidden_high.clone(),
            net_config_maybe_hidden_low.clone(),
            net_config_not_hidden.clone(),
        ]);

        // The 1.0 probability should always be picked
        assert!(selected_networks.contains(&id_hidden));
        // The high probability should always be picked
        assert!(selected_networks.contains(&id_maybe_hidden_high));
        // The low probability should never be picked
        assert!(!selected_networks.contains(&id_maybe_hidden_low));
        // The 0 probability should never be picked
        assert!(!selected_networks.contains(&id_not_hidden));
    }

    #[fuchsia::test]
    async fn test_record_not_seen_active_scan() {
        // Test that if we update that we haven't seen a couple of networks in active scans, their
        // hidden probability is updated.
        let saved_networks = SavedNetworksManager::new_for_test().await;

        // Seen in active scans
        let id_1 = NetworkIdentifier::try_from("foo", SecurityType::Wpa).unwrap();
        let credential_1 = Credential::Password(b"some_password".to_vec());
        let id_2 = NetworkIdentifier::try_from("bar", SecurityType::Wpa3).unwrap();
        let credential_2 = Credential::Password(b"another_password".to_vec());
        // Seen in active scan but not saved
        let id_3 = NetworkIdentifier::try_from("baz", SecurityType::None).unwrap();
        // Saved and targeted in active scan but not seen
        let id_4 = NetworkIdentifier::try_from("foobar", SecurityType::None).unwrap();
        let credential_4 = Credential::None;

        // Save 3 of the 4 networks
        assert!(
            saved_networks
                .store(id_1.clone(), credential_1)
                .await
                .expect("failed to store network")
                .is_none()
        );
        assert!(
            saved_networks
                .store(id_2.clone(), credential_2)
                .await
                .expect("failed to store network")
                .is_none()
        );
        assert!(
            saved_networks
                .store(id_4.clone(), credential_4)
                .await
                .expect("failed to store network")
                .is_none()
        );
        // Check that the saved networks have the default hidden probability so later we can just
        // check that the probability has changed.
        let config_1 = saved_networks.lookup(&id_1).await.expect("failed to lookup");
        assert_eq!(config_1.hidden_probability, PROB_HIDDEN_DEFAULT);
        let config_2 = saved_networks.lookup(&id_2).await.expect("failed to lookup");
        assert_eq!(config_2.hidden_probability, PROB_HIDDEN_DEFAULT);
        let config_4 = saved_networks.lookup(&id_4).await.expect("failed to lookup");
        assert_eq!(config_4.hidden_probability, PROB_HIDDEN_DEFAULT);

        let not_seen_ids = vec![id_1.ssid.clone(), id_2.ssid.clone(), id_3.ssid.clone()];
        saved_networks.record_scan_result(not_seen_ids, &HashMap::new()).await;

        // Check that the configs' hidden probability has decreased
        let config_1 = saved_networks.lookup(&id_1).await.expect("failed to lookup");
        assert!(config_1.hidden_probability < PROB_HIDDEN_DEFAULT);
        let config_2 = saved_networks.lookup(&id_2).await.expect("failed to lookup");
        assert!(config_2.hidden_probability < PROB_HIDDEN_DEFAULT);

        // Check that for the network that was target but not seen in the active scan, its hidden
        // probability isn't lowered.
        let config_4 = saved_networks.lookup(&id_4).await.expect("failed to lookup");
        assert_eq!(config_4.hidden_probability, PROB_HIDDEN_DEFAULT);

        // Check that a config was not saved for the identifier that was not saved before.
        assert!(saved_networks.lookup(&id_3).await.is_none());
    }

    #[fuchsia::test]
    async fn test_update_scan_stats_for_single_bss() {
        // Record multiple scans for a network where there is only 1 BSS for the network. The
        // config should be considered likely single BSS.
        let saved_networks = SavedNetworksManager::new_for_test().await;

        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa).unwrap();
        let credential = Credential::Password(b"some_password".to_vec());
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("failed to store network")
                .is_none()
        );

        let id_detailed = types::NetworkIdentifierDetailed {
            ssid: id.ssid.clone(),
            security_type: types::SecurityTypeDetailed::Wpa2Personal,
        };
        let scan_results = HashMap::from([(
            id_detailed.clone(),
            vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
        )]);

        // likely has one BSS
        for _ in 0..5 {
            saved_networks.record_scan_result(vec![id.ssid.clone()], &scan_results).await;
        }

        let is_single_bss = saved_networks
            .is_network_single_bss(&id, &credential)
            .await
            .expect("failed to lookup if network is single BSS");
        assert!(is_single_bss);
    }

    #[fuchsia::test]
    async fn test_update_scan_stats_for_multiple_bss_at_least_once() {
        // Record multiple scans for a network where there are multiple BSS. The network config
        // should say that the network is not single BSS.
        let saved_networks = SavedNetworksManager::new_for_test().await;

        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa).unwrap();
        let credential = Credential::Password(b"some_password".to_vec());
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("failed to store network")
                .is_none()
        );

        let id_detailed = types::NetworkIdentifierDetailed {
            ssid: id.ssid.clone(),
            security_type: types::SecurityTypeDetailed::Wpa2Personal,
        };
        let scan_results_single = HashMap::from([(
            id_detailed.clone(),
            vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
        )]);

        let scan_results_multi = HashMap::from([(
            id_detailed.clone(),
            vec![
                types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() },
                types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() },
            ],
        )]);

        // Record some scan results with one BSS, and record once with multiple BSS.
        for _ in 0..2 {
            saved_networks.record_scan_result(vec![id.ssid.clone()], &scan_results_single).await;
        }

        saved_networks.record_scan_result(vec![id.ssid.clone()], &scan_results_multi).await;
        saved_networks.record_scan_result(vec![id.ssid.clone()], &scan_results_single).await;

        // The one scan with multiple BSS results should make the network determined to be
        // multi BSS.
        let is_single_bss = saved_networks
            .is_network_single_bss(&id, &credential)
            .await
            .expect("failed to lookup if network is single BSS");
        assert!(!is_single_bss);
    }

    #[fuchsia::test]
    async fn test_record_scan_more_than_once_to_decide_single_bss() {
        // Test that a network is not decided to be single BSS after only one scan.
        let saved_networks = SavedNetworksManager::new_for_test().await;

        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa).unwrap();
        let credential = Credential::Password(b"some_password".to_vec());
        assert!(
            saved_networks
                .store(id.clone(), credential.clone())
                .await
                .expect("failed to store network")
                .is_none()
        );

        let id_detailed = types::NetworkIdentifierDetailed {
            ssid: id.ssid.clone(),
            security_type: types::SecurityTypeDetailed::Wpa2Personal,
        };
        let scan_results = HashMap::from([(
            id_detailed,
            vec![types::Bss { observation: ScanObservation::Passive, ..generate_random_bss() }],
        )]);

        // Record the scan multiple times, since multiple scans are needed to decide the network
        // likely has one BSS
        saved_networks.record_scan_result(vec![id.ssid.clone()], &scan_results).await;

        let is_single_bss = saved_networks
            .is_network_single_bss(&id, &credential)
            .await
            .expect("failed to lookup if network is single BSS");
        assert!(!is_single_bss);
    }

    #[fuchsia::test]
    async fn test_get_past_connections() {
        let saved_networks_manager = SavedNetworksManager::new_for_test().await;

        let id = NetworkIdentifier::try_from("foo", SecurityType::Wpa).unwrap();
        let credential = Credential::Password(b"some_password".to_vec());
        let mut config = NetworkConfig::new(id.clone(), credential.clone(), true, None)
            .expect("failed to create config");
        let mut past_connections = HashMap::<_, PastConnectionList>::new();

        // Add two past connections with the same bssid
        let data_1 = random_connection_data();
        let bssid_1 = data_1.bssid;
        let mut data_2 = random_connection_data();
        data_2.bssid = bssid_1;
        past_connections.entry(bssid_1).or_default().add(data_1);
        past_connections.entry(bssid_1).or_default().add(data_2);

        // Add a past connection with different bssid
        let data_3 = random_connection_data();
        let bssid_2 = data_3.bssid;
        past_connections.entry(bssid_2).or_default().add(data_3);
        config.perf_stats.past_connections = past_connections;

        // Create SavedNetworksManager with configs that have past connections
        assert!(
            saved_networks_manager.saved_networks.lock().await.insert(id.clone(), config).is_none()
        );

        // Check that get_past_connections gets the two PastConnectionLists for the BSSIDs.
        let mut expected_past_connections = new_past_connection_list();
        expected_past_connections.add(data_1);
        expected_past_connections.add(data_2);
        let actual_past_connections =
            saved_networks_manager.get_past_connections(&id, &credential, &bssid_1).await;
        assert_eq!(actual_past_connections, expected_past_connections);

        let mut expected_past_connections = new_past_connection_list();
        expected_past_connections.add(data_3);
        let actual_past_connections =
            saved_networks_manager.get_past_connections(&id, &credential, &bssid_2).await;
        assert_eq!(actual_past_connections, expected_past_connections);

        // Check that get_past_connections will not get the PastConnectionLists if the specified
        // Credential is different.
        let actual_past_connections = saved_networks_manager
            .get_past_connections(&id, &Credential::Password(b"other-password".to_vec()), &bssid_1)
            .await;
        assert_eq!(actual_past_connections, new_past_connection_list());
    }

    fn fake_successful_connect_result() -> fidl_sme::ConnectResult {
        fidl_sme::ConnectResult {
            code: fidl_ieee80211::StatusCode::Success,
            is_credential_rejected: false,
            is_reconnect: false,
        }
    }
}
