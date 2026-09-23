// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::stash_store::StashStore;
use crate::storage_store::StorageStore;
use anyhow::{Context, Error, format_err};
use fidl_fuchsia_stash as fidl_stash;
use fuchsia_component::client::connect_to_protocol;
use wlan_metrics_registry::StashMigrationResultsMetricDimensionMigrationResult as MigrationResult;

pub use wlan_storage_constants::{
    self, Credential, NetworkIdentifier, POLICY_STORAGE_ID, PersistentData, PersistentStorageData,
    SecurityType,
};

/// If the store ID is saved_networks, the file will be saved at /data/network-data.saved_networks
const FILE_PATH_FORMAT: &str = "/data/network-data.";

/// Manages access to the persistent storage. This layer on top of storage just migrates the legacy
/// persisted data if it hasn't been migrated yet.
pub struct PolicyStorage {
    /// This is the actual store.
    root: StorageStore,
    /// This is used to get legacy data, and should only ever be used once, when migrating the data
    /// the first time. The result is carried here because we only care about any errors if we end
    /// up migrating the data.
    legacy_stash: Result<StashStore, Error>,
    cobalt_proxy: Option<fidl_fuchsia_metrics::MetricEventLoggerProxy>,
}

impl PolicyStorage {
    /// Initialize new store with the ID provided by the Saved Networks Manager. The ID will
    /// identify stored values as being part of the same persistent storage.
    pub async fn new_with_id(id: &str) -> Self {
        let path = format!("{FILE_PATH_FORMAT}{id}");
        let root = StorageStore::new(&path);

        let proxy = connect_to_protocol::<fidl_stash::SecureStoreMarker>();
        let legacy_stash =
            proxy.and_then(|p| StashStore::from_secure_store_proxy(id, p)).map_err(|e| e);

        let cobalt_proxy = init_telemetry_channel()
            .await
            .inspect_err(|e| {
                log::info!(
                    "Error accessing telemetry. Stash migration metric will not be logged: {}",
                    e
                );
            })
            .ok();

        Self { root, legacy_stash, cobalt_proxy }
    }

    /// Initializer for tests outside of this module
    pub fn new_with_stash_proxy_and_id(
        stash_proxy: fidl_stash::SecureStoreProxy,
        id: &str,
    ) -> Self {
        let root = StorageStore::new(id);
        let legacy_stash = StashStore::from_secure_store_proxy(id, stash_proxy);
        let cobalt_proxy = None;
        Self { root, legacy_stash, cobalt_proxy }
    }

    /// Initialize the storage wrapper and load all saved network configs from persistent storage.
    /// If there is an error loading from local storage, that may mean stash data hasn't been
    /// migrated yet. If so, the function will try to load from legacy stash data.
    pub async fn load(&mut self) -> Result<Vec<PersistentStorageData>, Error> {
        // If there is an error loading from the new version of storage, it means it hasn't been
        // create and should be loaded from stash.
        let load_err = match self.root.load() {
            Ok(networks) => {
                self.log_load_metric(MigrationResult::AlreadyMigrated).await;
                return Ok(networks);
            }
            Err(e) => e,
        };
        let stash_store: &mut StashStore = if let Ok(stash) = self.legacy_stash.as_mut() {
            stash
        } else {
            return Err(format_err!("error accessing stash"));
        };
        // Try and read from Stash since store doesn't exist yet
        if let Ok(config) = stash_store.load().await {
            // Read the stash data and convert it to a flattened list of config data.
            let mut networks_list = Vec::new();
            for (id, legacy_configs) in config.into_iter() {
                let mut new_configs = legacy_configs
                    .into_iter()
                    .map(|c| PersistentStorageData::new_from_legacy_data(id.clone(), c));
                networks_list.extend(&mut new_configs);
            }

            // Write the data to the new storage.
            match self.root.write(networks_list.clone()) {
                Ok(_) => {
                    log::info!("Migrated saved networks from stash");
                    // Delete from stash if writing was successful.
                    let delete_result = stash_store.delete_store().await;
                    match delete_result {
                        Ok(()) => {
                            self.log_load_metric(MigrationResult::Success).await;
                        }
                        Err(e) => {
                            log::info!(
                                "Failed to delete legacy stash data after migration: {:?}",
                                e
                            );
                            self.log_load_metric(MigrationResult::MigratedButFailedToDeleteLegacy)
                                .await;
                        }
                    }
                }
                Err(e) => {
                    log::info!(e:?; "Failed to write migrated saved networks");
                    self.log_load_metric(MigrationResult::FailedToWriteNewStore).await;
                }
            }
            Ok(networks_list)
        } else {
            // The backing file is only actually created when a write happens, but we
            // don't want to intentionally create a file if migrating stash fails since
            // then we will never try to read from stash again.
            log::info!(load_err:?; "Failed to read saved networks from file and legacy stash, new file will be created when a network is saved",);
            self.log_load_metric(MigrationResult::FailedToLoadLegacyData).await;
            Ok(Vec::new())
        }
    }

    /// Update the network configs of a given network identifier to persistent storage, deleting
    /// the key entirely if the new list of configs is empty.
    pub fn write(&self, network_configs: Vec<PersistentStorageData>) -> Result<(), Error> {
        self.root.write(network_configs)
    }

    /// Remove all saved values from the stash. It will delete everything under the root node,
    /// and anything else in the same stash but not under the root node would be ignored.
    pub fn clear(&mut self) -> Result<(), Error> {
        self.root.empty_store()
    }

    async fn log_load_metric(&self, result_event_code: MigrationResult) {
        // No need to log an error if the cobalt proxy is none, errors would have been logged on
        // failed initialization of the channel.
        let cobalt_proxy = match &self.cobalt_proxy {
            Some(proxy) => proxy,
            None => return,
        };

        let events = &[fidl_fuchsia_metrics::MetricEvent {
            metric_id: wlan_metrics_registry::STASH_MIGRATION_RESULTS_METRIC_ID,
            event_codes: vec![result_event_code as u32],
            payload: fidl_fuchsia_metrics::MetricEventPayload::Count(1),
        }];

        // The error type of this inner result is a fidl_fuchsia_metrics defined error.
        match cobalt_proxy.log_metric_events(events).await {
            Err(e) => {
                log::info!(
                    "Error logging metric {:?} for migration result: {:?}",
                    result_event_code,
                    e
                );
            }
            Ok(Err(e)) => {
                log::info!(
                    "Error sending metric {:?} for migration result: {:?}",
                    result_event_code,
                    e
                );
            }
            Ok(_) => (),
        }
    }
}

async fn init_telemetry_channel() -> Result<fidl_fuchsia_metrics::MetricEventLoggerProxy, Error> {
    // Get channel for logging cobalt 1.1 metrics.
    let factory_proxy = fuchsia_component::client::connect_to_protocol::<
        fidl_fuchsia_metrics::MetricEventLoggerFactoryMarker,
    >()?;

    let (cobalt_proxy, cobalt_1dot1_server) =
        fidl::endpoints::create_proxy::<fidl_fuchsia_metrics::MetricEventLoggerMarker>();

    let project_spec = fidl_fuchsia_metrics::ProjectSpec {
        customer_id: None, // defaults to fuchsia
        project_id: Some(wlan_metrics_registry::PROJECT_ID),
        ..Default::default()
    };

    factory_proxy
        .create_metric_event_logger(&project_spec, cobalt_1dot1_server)
        .await
        .context("failed to create metrics event logger")?
        .map_err(|e| format_err!("failed to create metrics event logger: {:?}", e))?;

    Ok(cobalt_proxy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{network_id, rand_string};
    use assert_matches::assert_matches;
    use fidl::endpoints::create_request_stream;
    use fidl_fuchsia_stash::{SecureStoreRequest, StoreAccessorRequest};
    use fuchsia_async as fasync;
    use futures::task::Poll;
    use futures::{StreamExt, TryStreamExt};
    use ieee80211::Ssid;
    use std::convert::TryFrom;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use wlan_storage_constants::PersistentData;

    /// The PSK provided must be the bytes form of the 64 hexadecimal character hash. This is a
    /// duplicate of a definition in wlan/wlancfg/src, since I don't think there's a good way to
    /// import just that constant.
    pub const PSK_BYTE_LEN: usize = 32;

    #[fuchsia::test]
    async fn write_and_read() {
        let mut store = new_storage(&rand_string()).await;
        let cfg = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"password".to_vec()),
            has_ever_connected: true,
            hidden_probability: None,
        };

        // Save a network config to storage
        store.write(vec![cfg.clone()]).expect("Failed writing to storage");

        // Expect to read the same value back with the same key
        let cfgs_from_store = store.load().await.expect("Failed reading from storage");
        assert_eq!(1, cfgs_from_store.len());
        assert_eq!(vec![cfg.clone()], cfgs_from_store);

        // Overwrite the list of configs saved in storage
        let cfg_2 = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"other-password".to_vec()),
            has_ever_connected: false,
            hidden_probability: None,
        };
        store.write(vec![cfg.clone(), cfg_2.clone()]).expect("Failed writing to storage");

        // Expect to read the saved value back
        let cfgs_from_store = store.load().await.expect("Failed reading from stash");
        assert_eq!(2, cfgs_from_store.len());
        assert!(cfgs_from_store.contains(&cfg));
        assert!(cfgs_from_store.contains(&cfg_2));
    }

    #[fuchsia::test]
    async fn write_read_security_types() {
        let mut store = new_storage(&rand_string()).await;
        let password = Credential::Password(b"config-password".to_vec());

        // create and write configs with each security type
        let cfg_open = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::None,
            credential: Credential::None,
            has_ever_connected: false,
            hidden_probability: None,
        };
        let cfg_wep = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wep,
            credential: password.clone(),
            has_ever_connected: false,
            hidden_probability: None,
        };
        let cfg_wpa = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa,
            credential: password.clone(),
            has_ever_connected: false,
            hidden_probability: None,
        };
        let cfg_wpa2 = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: password.clone(),
            has_ever_connected: false,
            hidden_probability: None,
        };
        let cfg_wpa3 = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa3,
            credential: password.clone(),
            has_ever_connected: false,
            hidden_probability: None,
        };

        // Write the saved network list as it would be each time a new network was added.
        let mut saved_networks = vec![cfg_open.clone()];
        store.write(saved_networks.clone()).expect("failed to write config");
        saved_networks.push(cfg_wep.clone());
        store.write(saved_networks.clone()).expect("failed to write config");
        saved_networks.push(cfg_wpa.clone());
        store.write(saved_networks.clone()).expect("failed to write config");
        saved_networks.push(cfg_wpa2.clone());
        store.write(saved_networks.clone()).expect("failed to write config");
        saved_networks.push(cfg_wpa3.clone());
        store.write(saved_networks.clone()).expect("failed to write config");

        // Load storage and expect each config that we wrote.
        let configs = store.load().await.expect("failed loading from storage");
        assert_eq!(configs.len(), 5);
        assert!(configs.contains(&cfg_open));
        assert!(configs.contains(&cfg_wep));
        assert!(configs.contains(&cfg_wpa));
        assert!(configs.contains(&cfg_wpa2));
        assert!(configs.contains(&cfg_wpa3));
    }

    #[fuchsia::test]
    async fn write_read_credentials() {
        let mut store = new_storage(&rand_string()).await;

        // Create and write configs with each type credential.
        let password = Credential::Password(b"config-password".to_vec());
        let psk = Credential::Psk([65; PSK_BYTE_LEN].to_vec());

        let cfg_none = PersistentStorageData {
            ssid: b"bar-none".to_vec(),
            security_type: SecurityType::None,
            credential: Credential::None,
            has_ever_connected: false,
            hidden_probability: None,
        };
        let cfg_password = PersistentStorageData {
            ssid: b"bar-password".to_vec(),
            security_type: SecurityType::Wpa2,
            credential: password,
            has_ever_connected: false,
            hidden_probability: None,
        };
        let cfg_psk = PersistentStorageData {
            ssid: b"bar-psk".to_vec(),
            security_type: SecurityType::Wpa2,
            credential: psk,
            has_ever_connected: false,
            hidden_probability: None,
        };

        // Write configs to storage as they would be when saved.
        let mut saved_networks = vec![cfg_none.clone()];
        store.write(saved_networks.clone()).expect("failed to write");
        saved_networks.push(cfg_password.clone());
        store.write(saved_networks.clone()).expect("failed to write");
        saved_networks.push(cfg_psk.clone());
        store.write(saved_networks.clone()).expect("failed to write");

        // Check that the configs are loaded correctly.
        let configs = store.load().await.expect("failed loading from storage");
        assert_eq!(3, configs.len());
        assert!(configs.contains(&cfg_none));
        assert!(configs.contains(&cfg_password));
        assert!(configs.contains(&cfg_psk));
    }

    #[fuchsia::test]
    async fn write_read_hidden_probability() {
        let mut store = new_storage(&rand_string()).await;

        // create and write configs with each hidden probability
        let cfg_zero = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::None,
            credential: Credential::None,
            has_ever_connected: false,
            hidden_probability: Some(0.0),
        };
        let cfg_low = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::None,
            credential: Credential::None,
            has_ever_connected: false,
            hidden_probability: Some(0.05),
        };
        let cfg_high = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::None,
            credential: Credential::None,
            has_ever_connected: false,
            hidden_probability: Some(0.9),
        };

        // Write the saved network list as it would be each time a new network was added.
        let mut saved_networks = vec![cfg_zero.clone()];
        store.write(saved_networks.clone()).expect("failed to write config");
        saved_networks.push(cfg_low.clone());
        store.write(saved_networks.clone()).expect("failed to write config");
        saved_networks.push(cfg_high.clone());
        store.write(saved_networks.clone()).expect("failed to write config");

        // Load storage and expect each config that we wrote.
        let configs = store.load().await.expect("failed loading from storage");
        assert_eq!(configs.len(), 3);
        assert!(configs.contains(&cfg_zero));
        assert!(configs.contains(&cfg_low));
        assert!(configs.contains(&cfg_high));
    }

    #[fuchsia::test]
    async fn write_persists() {
        let path = rand_string();
        let store = new_storage(&path).await;
        let cfg = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"password".to_vec()),
            has_ever_connected: true,
            hidden_probability: None,
        };

        // Save a network config to the stash
        store.write(vec![cfg.clone()]).expect("Failed writing to storage");

        // Create the storage again with same id
        let mut store = PolicyStorage::new_with_id(&path).await;

        // Expect to read the same value back with the same key, should exist in new stash
        let cfgs_from_store = store.load().await.expect("Failed reading from storage");
        assert_eq!(1, cfgs_from_store.len());
        assert!(cfgs_from_store.contains(&cfg));
    }

    #[fuchsia::test]
    async fn load_storage() {
        let mut store = new_storage(&rand_string()).await;
        let cfg_foo = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"12345678".to_vec()),
            has_ever_connected: true,
            hidden_probability: None,
        };
        let cfg_bar = PersistentStorageData {
            ssid: Ssid::try_from("bar").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"qwertyuiop".to_vec()),
            has_ever_connected: true,
            hidden_probability: None,
        };

        // Store two networks in our stash.
        store
            .write(vec![cfg_foo.clone(), cfg_bar.clone()])
            .expect("Failed to save configs to stash");

        // load should give us the two networks we saved
        let expected_cfgs = vec![cfg_foo, cfg_bar];
        assert_eq!(expected_cfgs, store.load().await.expect("Failed to load configs from stash"));
    }

    #[fuchsia::test]
    async fn load_empty_storage_does_loads_empty_list() {
        let store_id = &rand_string();
        let mut store = new_storage(&store_id).await;

        // write to storage an empty saved networks list
        store.write(vec![]).expect("failed to write value");

        // recreate the storage to load it
        let loaded_configs = store.load().await.expect("failed to load store");
        assert!(loaded_configs.is_empty());
    }

    #[fuchsia::test]
    pub async fn load_no_file_creates_file() {
        // Test what would happen if policy persistent storage is loaded twice - the first attempt
        // should initialize the backing file. The second attempt should not attempt to load from
        // the legacy stash.
        let store_id = &rand_string();
        let backing_file_path = format!("{}{}", FILE_PATH_FORMAT, store_id).to_string();
        let mut store = PolicyStorage::new_with_id(store_id).await;

        // The file should not exist yet, so reading it would give an error.
        std::fs::read(&backing_file_path).expect_err("The file for the store should not exist yet");

        // Load the store.
        let loaded_configs = store.load().await.expect("failed to load store");
        assert_eq!(loaded_configs, vec![]);

        // Check that the file is created. It should have some JSON structure even though there
        // are no saved networks.
        let file_contents = std::fs::read(&backing_file_path).expect(
            "Failed to read file that should have been created when loading non-existant file",
        );
        assert!(!file_contents.is_empty());

        // Load the store again, but with some values in the legacy stash which should be ignored.
        let cfg_id = NetworkIdentifier {
            ssid: Ssid::try_from(rand_string().as_str()).unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
        };
        let cfg = PersistentData {
            credential: Credential::Password(rand_string().as_bytes().to_vec()),
            has_ever_connected: true,
        };

        match store.legacy_stash.as_mut() {
            Ok(stash) => {
                stash.write(&cfg_id, &[cfg]).await.expect("Failed writing to legacy stash");
                stash.flush().await.expect("Failed to flush legacy stash");
            }
            Err(e) => {
                panic!("error initializing legacy stash: {}", e);
            }
        }
        let loaded_configs = store.load().await.expect("failed to load store");
        assert!(loaded_configs.is_empty());

        // The file should still exist.
        let file_contents = std::fs::read(&backing_file_path).expect(
            "Failed to read file that should have been created when loading non-existant file",
        );
        assert!(!file_contents.is_empty());
    }

    #[fuchsia::test]
    async fn clear_storage() {
        let storage_id = &rand_string();
        let mut storage = new_storage(&storage_id).await;

        // add some configs to the storage
        let cfg_foo = PersistentStorageData {
            ssid: Ssid::try_from("foo").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"qwertyuio".to_vec()),
            has_ever_connected: true,
            hidden_probability: None,
        };
        let cfg_bar = PersistentStorageData {
            ssid: Ssid::try_from("bar").unwrap().to_vec(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"12345678".to_vec()),
            has_ever_connected: false,
            hidden_probability: None,
        };
        storage.write(vec![cfg_foo.clone(), cfg_bar.clone()]).expect("Failed to write to storage");

        // verify that the configs are found in storage
        let configs_from_storage = storage.load().await.expect("Failed to read");
        assert_eq!(2, configs_from_storage.len());
        assert!(configs_from_storage.contains(&cfg_foo));
        assert!(configs_from_storage.contains(&cfg_bar));

        // clear the storage
        storage.clear().expect("Failed to clear storage");
        // verify that the configs are no longer in the storage
        let configs_from_storage = storage.load().await.expect("Failed to read");
        assert_eq!(0, configs_from_storage.len());

        // recreate storage and verify that clearing the storage persists
        let mut storage = PolicyStorage::new_with_id(storage_id).await;
        let configs_from_storage = storage.load().await.expect("Failed to read");
        assert_eq!(0, configs_from_storage.len());
    }

    #[fuchsia::test]
    async fn test_migration() {
        let storage_id = rand_string();
        let stash_client = connect_to_protocol::<fidl_stash::SecureStoreMarker>()
            .expect("failed to connect to store");
        let ssid = "foo";
        let security_type = SecurityType::Wpa2;
        let credential = Credential::Password(b"password".to_vec());
        let has_ever_connected = false;
        // This is the version used by the previous storage mechanism.
        let network_id = network_id(ssid, security_type);
        let previous_data = PersistentData { credential: credential.clone(), has_ever_connected };

        // This is the version used by the new storage mechanism.
        let network_config = vec![PersistentStorageData {
            ssid: ssid.into(),
            security_type: security_type,
            credential: credential.clone(),
            has_ever_connected,
            hidden_probability: None,
        }];

        // Write the config to stash that storage will migrate from.
        let stash = StashStore::from_secure_store_proxy(&storage_id, stash_client.clone())
            .expect("failed to get stash proxy");
        stash.write(&network_id, &vec![previous_data]).await.expect("write failed");

        // Initialize storage, and give it the stash with the saved network data.
        let mut storage = PolicyStorage::new_with_id(&storage_id).await;
        storage.legacy_stash = Ok(stash);
        assert_eq!(storage.load().await.expect("load failed"), network_config);

        // The config should have been deleted from stash.
        // The stash connection can't be reused, or the stash store will fail to access stash.
        let stash_client = connect_to_protocol::<fidl_stash::SecureStoreMarker>()
            .expect("failed to connect to store");
        let stash = StashStore::from_secure_store_proxy(&storage_id, stash_client)
            .expect("failed to get stash proxy");
        assert!(stash.load().await.expect("load failed").is_empty());

        // And once more, but this time there should be no migration.
        let mut storage = PolicyStorage::new_with_id(&storage_id).await;
        assert_eq!(storage.load().await.expect("load failed"), network_config);
    }

    #[fuchsia::test]
    async fn test_migration_with_bad_stash() {
        let store_id = rand_string();

        let (client, mut request_stream) = create_request_stream::<fidl_stash::SecureStoreMarker>();

        // This will be set to true if stash is accessed, so that the test can check whether stash
        // was read by the migration code.
        let read_from_stash = Arc::new(AtomicBool::new(false));

        // This responds in the background to any stash requests for initializing the connection to
        // stash (identify or create accessor), and responds to requests for reading data with an
        // an error by dropping the responder.
        let _task = {
            let read_from_stash = read_from_stash.clone();
            fasync::Task::spawn(async move {
                while let Some(request) = request_stream.next().await {
                    match request.unwrap() {
                        SecureStoreRequest::Identify { .. } => {}
                        SecureStoreRequest::CreateAccessor { accessor_request, .. } => {
                            let read_from_stash = read_from_stash.clone();
                            fuchsia_async::Task::spawn(async move {
                                let mut request_stream = accessor_request.into_stream();
                                while let Some(request) = request_stream.next().await {
                                    match request.unwrap() {
                                        StoreAccessorRequest::ListPrefix { .. } => {
                                            read_from_stash.store(true, Ordering::Relaxed);
                                            // If we just drop the iterator, it should trigger a
                                            // read error.
                                        }
                                        _ => unreachable!(),
                                    }
                                }
                            })
                            .detach();
                        }
                    }
                }
            })
        };

        // Initialize the store but switch out with the stash we made to act corrupted.
        let mut store = PolicyStorage::new_with_id(&store_id).await;
        let proxy_fn = client.into_proxy();
        store.legacy_stash = StashStore::from_secure_store_proxy(&store_id, proxy_fn);

        // Try and load the config. It should provide empty config.
        assert!(&store.load().await.expect("load failed").is_empty());

        // Make sure there was an attempt to actually read from stash.
        assert!(read_from_stash.load(Ordering::Relaxed));
    }

    /// Creates a new persistent storage with a file bath based on the given ID and clears any
    /// values saved in the store.
    pub async fn new_storage(id: &str) -> PolicyStorage {
        let mut store = PolicyStorage::new_with_id(id).await;
        store.clear().expect("failed to clear stash");
        store
    }

    /// Metrics tests need to be able to control stash behavior and check what is sent to cobalt.
    struct MetricsTestValues {
        store: PolicyStorage,
        cobalt_stream: fidl_fuchsia_metrics::MetricEventLoggerRequestStream,
        stash_stream: fidl_stash::SecureStoreRequestStream,
    }

    /// This initializes PolicyStorage with a cobalt channel and stash channel that is
    /// controlled by the test. It is for tests that want to control stash responses and read
    /// messages send to cobalt.
    fn migration_metrics_test_values() -> MetricsTestValues {
        let (cobalt_proxy, cobalt_stream) = fidl::endpoints::create_proxy_and_stream::<
            fidl_fuchsia_metrics::MetricEventLoggerMarker,
        >();

        let (legacy_stash, stash_stream) = stash_for_test();
        let root = StorageStore::new(format!("{FILE_PATH_FORMAT}{}", rand_string()));
        let store = PolicyStorage { root, legacy_stash, cobalt_proxy: Some(cobalt_proxy) };

        MetricsTestValues { store, cobalt_stream, stash_stream }
    }

    fn stash_for_test() -> (Result<StashStore, Error>, fidl_stash::SecureStoreRequestStream) {
        let (client, stash_stream) = create_request_stream::<fidl_stash::SecureStoreMarker>();
        let proxy_fn = client.into_proxy();
        let id = rand_string();
        let legacy_stash = StashStore::from_secure_store_proxy(&id, proxy_fn);

        (legacy_stash, stash_stream)
    }

    // Checks that the metric event is correct and acks the metric event so that the load fut
    // can continue.
    fn check_load_metric(
        logged_metric: fidl_fuchsia_metrics::MetricEventLoggerRequest,
        expected_event: MigrationResult,
    ) {
        assert_matches!(logged_metric, fidl_fuchsia_metrics::MetricEventLoggerRequest::LogMetricEvents {
            mut events, responder, ..
        } => {
            assert_eq!(events.len(), 1);
            let event = events.pop().unwrap();
            assert_matches!(event, fidl_fuchsia_metrics::MetricEvent { metric_id, event_codes, payload: _payload } => {
                assert_eq!(metric_id, wlan_metrics_registry::STASH_MIGRATION_RESULTS_METRIC_ID);
                assert_eq!(event_codes, [expected_event as u32]);
            });

            assert!(responder.send(Ok(())).is_ok());
        });
    }

    fn process_init_stash(
        exec: &mut fasync::TestExecutor,
        mut stash_stream: fidl_stash::SecureStoreRequestStream,
    ) -> fidl_stash::StoreAccessorRequestStream {
        assert_matches!(
            exec.run_until_stalled(&mut stash_stream.next()),
            Poll::Ready(Some(Ok(SecureStoreRequest::Identify { .. })))
        );

        let accessor_req_stream = assert_matches!(
            exec.run_until_stalled(&mut stash_stream.next()),
            Poll::Ready(Some(Ok(SecureStoreRequest::CreateAccessor { accessor_request, .. }))) =>
        {
            accessor_request.into_stream()
        });

        accessor_req_stream
    }

    /// Respond to the ListPrefix request with empty data, which matches the scenario where nothing
    /// is saved in stash.
    fn respond_to_stash_list_prefix(
        exec: &mut fasync::TestExecutor,
        stash_server: &mut fidl_stash::StoreAccessorRequestStream,
    ) {
        let request = assert_matches!(exec.run_until_stalled(&mut stash_server.next()), Poll::Ready(req) => {
            req.expect("ListPrefix stash request not recieved.")
        });
        match request.unwrap() {
            StoreAccessorRequest::ListPrefix { it, .. } => {
                let mut iter = it.into_stream();
                assert_matches!(
                    exec.run_until_stalled(&mut iter.try_next()),
                    Poll::Ready(Ok(Some(fidl_stash::ListIteratorRequest::GetNext { responder }))) => {
                        responder.send(&[]).expect("error sending stash response");
                });
            }
            _ => unreachable!(),
        }
    }

    fn process_stash_delete(
        exec: &mut fasync::TestExecutor,
        stash_server: &mut fidl_stash::StoreAccessorRequestStream,
    ) {
        // Respond to stash delete.
        let request = assert_matches!(exec.run_until_stalled(&mut stash_server.next()), Poll::Ready(req) => {
            req.expect("DeletePrefix stash request not recieved.")
        });
        match request.unwrap() {
            StoreAccessorRequest::DeletePrefix { .. } => {}
            _ => unreachable!(),
        }

        // Respond to stash flush.
        assert_matches!(
            exec.run_until_stalled(&mut stash_server.try_next()),
            Poll::Ready(Ok(Some(fidl_stash::StoreAccessorRequest::Flush{responder}))) => {
                responder.send(Ok(())).expect("failed to send stash response");
            }
        );
    }

    #[fuchsia::test]
    pub fn test_load_logs_result_success_metric() {
        let mut exec = fasync::TestExecutor::new();
        // Use a PolicyStorage with the default stash proxy for this test, but switch out the
        // cobalt proxy to intercept metric events.
        let mut test_values = migration_metrics_test_values();

        // Load for the first time successfully.
        {
            let load_fut = test_values.store.load();
            futures::pin_mut!(load_fut);
            assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

            // Respond to stash initialization requests.
            let mut accessor_req_stream = process_init_stash(&mut exec, test_values.stash_stream);
            assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

            // Respond to stash read requests with empty data.
            respond_to_stash_list_prefix(&mut exec, &mut accessor_req_stream);
            assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

            // Process stash delete and flush.
            process_stash_delete(&mut exec, &mut accessor_req_stream);
            assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

            let mut metric_fut = test_values.cobalt_stream.next();
            assert_matches!(exec.run_until_stalled(&mut metric_fut), Poll::Ready(Some(Ok(logged_metric))) => {
                check_load_metric(logged_metric, MigrationResult::Success);
            });
            assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Ready(Ok(_)));
        }

        // Load again, an AlreadyMigrated metric event code should be logged. Stash do not need
        // handling because the stash wrapper internally stores the data.
        let load_fut = test_values.store.load();
        futures::pin_mut!(load_fut);
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        let mut metric_fut = test_values.cobalt_stream.next();
        assert_matches!(exec.run_until_stalled(&mut metric_fut), Poll::Ready(Some(Ok(logged_metric))) => {
            check_load_metric(logged_metric, MigrationResult::AlreadyMigrated);
        });

        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Ready(Ok(_)));

        // Check that nothing else was logged
        assert_matches!(exec.run_until_stalled(&mut metric_fut), Poll::Pending);
    }

    #[fuchsia::test]
    pub fn test_load_failure_logs_result_metric() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = migration_metrics_test_values();
        let load_fut = test_values.store.load();
        futures::pin_mut!(load_fut);

        // Start running the future to load and trigger migration. It should halt waiting on a
        // stash request.
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Respond to stash initialization requests.
        let mut accessor_req_stream = process_init_stash(&mut exec, test_values.stash_stream);
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Drop the request to read stash so that loading stash fails.
        let request = assert_matches!(exec.run_until_stalled(&mut accessor_req_stream.next()), Poll::Ready(req) => {
            req.expect("ListPrefix stash request not recieved.")
        });
        match request.unwrap() {
            StoreAccessorRequest::ListPrefix { it: _, .. } => {
                // If we just drop the iterator without responding, it should trigger a read error.
            }
            _ => unreachable!(),
        }

        // Continue the load fut, it should wait on a response to sending a metric.
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Check for the correct metric and ack.
        assert_matches!(exec.run_until_stalled(&mut test_values.cobalt_stream.next()), Poll::Ready(Some(Ok(metric))) => {
            check_load_metric(metric, MigrationResult::FailedToLoadLegacyData);
        });

        // The load should finish this time.
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Ready(Ok(data)) => {
            assert!(data.is_empty());
        });

        // Verify that nothing else was send through the cobalt channel.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.cobalt_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    pub fn test_load_delete_stash_failure_logs_result_metric() {
        let mut exec = fuchsia_async::TestExecutor::new();
        let mut test_values = migration_metrics_test_values();
        let load_fut = test_values.store.load();
        futures::pin_mut!(load_fut);

        // Start running the future to load and trigger migration. It should halt waiting on a
        // stash request.
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Respond to stash initialization requests.
        let mut accessor_req_stream = process_init_stash(&mut exec, test_values.stash_stream);
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Respond to stash requests as if loading empty stash data.
        respond_to_stash_list_prefix(&mut exec, &mut accessor_req_stream);

        // Drop the accessor request stream to trigger an error when deleting stash.
        drop(accessor_req_stream);

        // Continue the load fut, it should wait on a response to sending a metric.
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Check for the correct metric and ack.
        assert_matches!(exec.run_until_stalled(&mut test_values.cobalt_stream.next()), Poll::Ready(Some(Ok(metric))) => {
            check_load_metric(metric, MigrationResult::MigratedButFailedToDeleteLegacy);
        });

        // The load should finish this time.
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Ready(Ok(data)) => {
            assert!(data.is_empty());
        });

        // Verify that nothing else was sent through the cobalt channel.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.cobalt_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    pub fn test_load_logs_result_failed_to_write_metric() {
        let mut exec = fasync::TestExecutor::new();
        // Use a path that is invalid so that writing to it fails.
        let store_id = "//";
        let mut test_values = migration_metrics_test_values();

        // Switch out StorageStore to one using the invalid path.
        test_values.store.root = StorageStore::new(std::path::Path::new(store_id));

        // Start loading to migrate stash data.
        let load_fut = test_values.store.load();
        futures::pin_mut!(load_fut);
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Respond to stash initialization requests.
        let mut accessor_req_stream = process_init_stash(&mut exec, test_values.stash_stream);
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Respond to stash requests as if loading empty stash data.
        respond_to_stash_list_prefix(&mut exec, &mut accessor_req_stream);
        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Pending);

        // Check that the metric is logged for the failure to write.
        let mut metric_fut = test_values.cobalt_stream.next();
        assert_matches!(exec.run_until_stalled(&mut metric_fut), Poll::Ready(Some(Ok(logged_metric))) => {
            check_load_metric(logged_metric, MigrationResult::FailedToWriteNewStore);
        });

        assert_matches!(exec.run_until_stalled(&mut load_fut), Poll::Ready(Ok(_)));

        // Verify that nothing else was sent through the cobalt channel.
        assert_matches!(
            exec.run_until_stalled(&mut test_values.cobalt_stream.next()),
            Poll::Pending
        );
    }

    #[fuchsia::test]
    pub fn load_wpa2_and_open_network_golden_file_test() {
        // This tests that this example persisted file with one WPA2 network and one open network
        // can still be read by the code, even if the corresponding write logic changes.
        // This test should NOT change even if the way data is saved changes, unless there has been
        // a stepping stone version that migrates this format, since the purpose is to test that
        // this format can be read after an update.

        let file_contents =
            "{\"saved_networks\":[\
                {\
                    \"ssid\":[115,111,109,101,45,110,101,116,119,111,114,107],\
                    \"security_type\":\"Wpa2\",\
                    \"credential\":{\"Password\":[115,111,109,101,45,112,97,115,115,119,111,114,100]},\
                    \"has_ever_connected\":false
                },\
                {
                    \"ssid\":[111,112,101,110,45,110,101,116,119,111,114,107],\
                    \"security_type\":\"None\",\
                    \"credential\":\"None\",\
                    \"has_ever_connected\":true\
                }],\
                \"version\":1\
            }"
        .as_bytes();
        let store_id = &rand_string();

        let network_configs = vec![
            PersistentStorageData {
                ssid: vec![115, 111, 109, 101, 45, 110, 101, 116, 119, 111, 114, 107],
                security_type: SecurityType::Wpa2,
                credential: Credential::Password(vec![100, 100, 100, 100, 100, 100]),
                has_ever_connected: false,
                hidden_probability: None,
            },
            PersistentStorageData {
                ssid: vec![111, 112, 101, 110, 45, 110, 101, 116, 119, 111, 114, 107],
                security_type: SecurityType::None,
                credential: Credential::None,
                has_ever_connected: true,
                hidden_probability: None,
            },
        ];

        // Write the data to the StorageStore's backing file.
        let path = format!("/data/config.{}", store_id);
        let mut file = std::fs::File::create(&path).expect("failed to open file for writing");
        assert_eq!(
            file.write(&file_contents).expect("Failed to write to file"),
            file_contents.len()
        );
        file.flush().expect("failed to flush contents of file");

        // Load the file data and check that the expected networks are there.
        let store = StorageStore::new(&path);
        let loaded_configs = store.load().expect("load failed");
        assert_eq!(loaded_configs.len(), network_configs.len());
        for config in network_configs.iter() {
            assert!(network_configs.contains(config));
        }
    }
}
