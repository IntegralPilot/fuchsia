// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use assert_matches::assert_matches;
use futures::channel::mpsc;
use fuzz::fuzz;
use rand::distr::{Alphanumeric, SampleString};
use wlancfg_lib::config_management::network_config::{Credential, NetworkIdentifier};
use wlancfg_lib::config_management::{SavedNetworksManager, SavedNetworksManagerApi};
use wlancfg_lib::telemetry::{TelemetryEvent, TelemetrySender};

#[fuzz]
async fn fuzz_saved_networks_manager_store(id: NetworkIdentifier, credential: Credential) {
    // Test with fuzzed inputs that if a network is stored, we can look it up again and if it is
    // loaded from stash when SavedNetworksManager is initialized from stash the values of the
    // saved network are correct.
    let store_id = generate_string();

    // Expect the store to be constructed successfully even if the file doesn't
    // exist yet
    let saved_networks = create_saved_networks(&store_id).await;

    assert!(saved_networks.lookup(&id).await.is_none());
    assert_eq!(0, saved_networks.known_network_count().await);

    // Store a fuzzed network identifier and credential.
    assert!(
        saved_networks
            .store(id.clone(), credential.clone())
            .await
            .expect("storing network failed")
            .is_none()
    );
    assert_matches!(saved_networks.lookup(&id).await,
        Some(network_config) => {
            assert_eq!(network_config.ssid, id.ssid);
            assert_eq!(network_config.security_type, id.security_type);
            assert_eq!(network_config.credential, credential);
        }
    );
    assert_eq!(1, saved_networks.known_network_count().await);

    // Saved networks should persist when we create a saved networks manager with the same ID.
    let saved_networks = create_saved_networks(&store_id).await;
    assert_matches!(saved_networks.lookup(&id).await,
        Some(network_config) => {
            assert_eq!(network_config.ssid, id.ssid);
            assert_eq!(network_config.security_type, id.security_type);
            assert_eq!(network_config.credential, credential);
        }
    );
    assert_eq!(1, saved_networks.known_network_count().await);
}

/// Create a saved networks manager with the specified storage path so that saved networks are
/// can be loaded from the same place in each test run. If tests may run in parallel, the paths
/// should be different so the tests don't conflict.
async fn create_saved_networks(store_id: &String) -> SavedNetworksManager {
    let (telemetry_sender, _telemetry_receiver) = mpsc::channel::<TelemetryEvent>(100);
    let telemetry_sender = TelemetrySender::new(telemetry_sender);
    let store = wlan_storage::policy::PolicyStorage::new_with_id(store_id).await;

    let saved_networks = SavedNetworksManager::new_with_storage(store, telemetry_sender).await;
    saved_networks
}

/// Generate a random string of length 16
pub fn generate_string() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 16)
}
