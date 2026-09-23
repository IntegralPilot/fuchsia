// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use log::{error, info, warn};
use std::collections::HashMap;
use std::io::Write;
use wlan_storage_constants::{FileContent, PersistentStorageData};

type FileContents = HashMap<String, FileContent>;

const SAVED_NETWORKS_KEY: &str = "saved_networks";
const VERSION_KEY: &str = "version";
const CURRENT_VERSION: u8 = 1;

/// StorageStore is the wrapper for writing saved network data to persistent storage. It
/// "atomically" writes data by writing everything to a new file and then moving that file to the
/// path data is stored. So failures to write or interruptions to file writing shouldn't corrupt
/// the existing persisted data. Data is not actually internally kept within this struct.
pub struct StorageStore {
    path: std::path::PathBuf,
}

impl StorageStore {
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        Self { path: path.as_ref().into() }
    }

    /// Clears the backing file if it exists or creates a new one
    /// {"version:" 1, saved_networks: [] }
    pub fn empty_store(&self) -> Result<(), Error> {
        self.write(Vec::new())
    }

    /// This writes and flushes the network data to a file, and then moves that to the location
    /// saved network data is stored so it is only updated after the data is successfully written.
    pub fn write(&self, network_configs: Vec<PersistentStorageData>) -> Result<(), Error> {
        let mut tmp_path = self.path.clone();
        tmp_path.as_mut_os_string().push(".tmp");
        {
            // Build the map structured file contents to be serialized.
            let mut data = FileContents::new();
            data.insert(VERSION_KEY.to_string(), FileContent::Version(CURRENT_VERSION));
            data.insert(SAVED_NETWORKS_KEY.to_string(), FileContent::Networks(network_configs));
            let serialized_data = serde_json::to_vec(&data)?;

            // Write to the file, using a temporary file to ensure the original data isn't lost if
            // there is an error.
            let mut file = std::fs::File::create(tmp_path.clone())?;
            file.write_all(&serialized_data)?;
            // This fsync is required because the storage stack doesn't guarantee data is flushed
            // before the rename.
            fuchsia_nix::unistd::fsync(&file)?;
        }
        std::fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }

    /// Returns the network data from the backing file. Returns an error if the file doesn't
    /// exist or is corrupt. This function does not initialize a new file, and instead lets the
    /// caller do that so that the caller knows it may need to migrate over old data from before
    /// this file was used.
    pub fn load(&self) -> Result<Vec<PersistentStorageData>, Error> {
        let serialized_data = std::fs::read(&self.path)?;
        let mut contents: FileContents = serde_json::from_slice(&serialized_data)?;

        // Check the file version. There should only be one possible version.
        match contents.get(VERSION_KEY) {
            Some(FileContent::Version(version)) => {
                if version > &CURRENT_VERSION {
                    info!(
                        "The version of the networks data is newer than the current version.
                        Unrecognizable data may not be retained."
                    );
                }
            }
            Some(FileContent::Networks(_)) => {
                error!(
                    "The recorded version was interpreted as saved networks data. The newest version will be used to interpret data."
                );
            }
            None => {
                warn!(
                    "The version is not present in the saved networks file. It will be read
                    as the current version."
                );
            }
        }

        // Take the saved networks list that should have been loaded.
        let networks = match contents.remove(SAVED_NETWORKS_KEY) {
            Some(FileContent::Networks(n)) => n,
            Some(_) => {
                return Err(format_err!("Networks deserialized as the wrong type"));
            }
            None => {
                return Err(format_err!("Networks missing from storage file"));
            }
        };

        Ok(networks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::rand_string;
    use wlan_storage_constants::{Credential, SecurityType};

    #[fuchsia::test]
    fn test_write_and_load() {
        let path = format!("/data/config.{}", rand_string());
        let store = StorageStore::new(&path);
        let network_configs = vec![PersistentStorageData {
            ssid: "foo".into(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"password".to_vec()),
            has_ever_connected: false,
            hidden_probability: None,
        }];
        store.write(network_configs.clone()).expect("write failed");
        let store = StorageStore::new(&path);
        assert_eq!(store.load().expect("load failed"), network_configs);
    }

    #[fuchsia::test]
    fn test_load_if_necessary_parts_present_missing_field() {
        // Test that if a data from a future version with a missing field is loaded, the data can
        // still be loaded. If the necessary parts (SSID, security type, credential) are present,
        // reading the data should work.

        // Test file contents that are missing "has_ever_connected"
        let future_version = 255;
        let file_contents = format!(
            "{{\
            \"saved_networks\":[{{\
                \"ssid\":[100, 100, 100, 100, 100, 100],\
                \"security_type\":\"Wpa2\",\
                \"credential\":{{\"Password\":[100, 100, 100, 100, 100, 100]}}\
            }}],\
            \"{VERSION_KEY}\":{future_version}\
        }}"
        )
        .into_bytes();

        let network_configs = vec![PersistentStorageData {
            ssid: vec![100, 100, 100, 100, 100, 100],
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(vec![100, 100, 100, 100, 100, 100]),
            has_ever_connected: false,
            hidden_probability: None,
        }];

        // Write the data with a missing field to the file.
        let path = format!("/data/config.{}", rand_string());
        let mut file = std::fs::File::create(&path).expect("failed to open file for writing");
        assert_eq!(
            file.write(&file_contents.clone()).expect("Failed to write to file"),
            file_contents.len()
        );
        file.flush().expect("failed to flush contents of file");

        // Load the file data and check that the expected network is there.
        let store = StorageStore::new(&path);
        assert_eq!(store.load().expect("load failed"), network_configs);
    }

    #[fuchsia::test]
    fn test_load_if_necessary_parts_present_extra_field() {
        // Test that if a data from a future version with a new unrecognizable field, the data
        // will still be loaded and the new field is ignored.

        // Test file contents with an extra network field "other_field"
        let future_version = 255;
        let file_contents = format!(
            "{{\
            \"saved_networks\":[{{\
                \"ssid\":[100, 100, 100, 100, 100, 100],\
                \"security_type\":\"Wpa2\",\
                \"credential\":{{\"Password\":[100, 100, 100, 100, 100, 100]}},\
                \"has_ever_connected\": false,
                \"other_field\": true
            }}],\
            \"{VERSION_KEY}\":{future_version}\
        }}"
        )
        .into_bytes();

        let network_configs = vec![PersistentStorageData {
            ssid: vec![100, 100, 100, 100, 100, 100],
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(vec![100, 100, 100, 100, 100, 100]),
            has_ever_connected: false,
            hidden_probability: None,
        }];

        // Write the data that has an extra field to the StorageStore's backing file.
        let path = format!("/data/config.{}", rand_string());
        let mut file = std::fs::File::create(&path).expect("failed to open file for writing");
        assert_eq!(
            file.write(&file_contents.clone()).expect("Failed to write to file"),
            file_contents.len()
        );
        file.flush().expect("failed to flush contents of file");

        // Load the file data and check that the expected network is there.
        let store = StorageStore::new(&path);
        assert_eq!(store.load().expect("load failed"), network_configs);
    }

    #[fuchsia::test]
    fn test_delete_store() {
        let path = format!("/data/config.{}", rand_string());
        let store = StorageStore::new(&path);
        let network_config = vec![PersistentStorageData {
            ssid: "foo".into(),
            security_type: SecurityType::Wpa2,
            credential: Credential::Password(b"password".to_vec()),
            has_ever_connected: false,
            hidden_probability: None,
        }];
        store.write(network_config.clone()).expect("write failed");
        assert_eq!(store.load().expect("load failed"), network_config);

        // Empty the store and check that nothing is loaded.
        store.empty_store().expect("empty_store failed");
        let store = StorageStore::new(&path);
        assert_eq!(store.load().expect("load failed"), Vec::new());
    }
}
