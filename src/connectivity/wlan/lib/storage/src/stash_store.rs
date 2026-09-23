// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, bail, format_err};
use fidl::endpoints::create_proxy;
use fidl_fuchsia_stash as fidl_stash;
use std::collections::HashMap;
use wlan_storage_constants::{
    NODE_SEPARATOR, NetworkIdentifier, POLICY_DATA_KEY, POLICY_STASH_PREFIX, PersistentData,
};

struct StashNode {
    // Always terminated with `NODE_SEPARATOR`
    key: String,
    stash: fidl_stash::StoreAccessorProxy,
}

/// A convenience wrapper around a Stash-backed `HashMap`.
struct StashNodeFields(HashMap<String, fidl_stash::Value>);

impl StashNodeFields {
    #[cfg(test)]
    fn names(&self) -> Vec<String> {
        self.0.keys().map(|k| k.into()).collect()
    }

    fn get_str(&self, field: &str) -> Option<&String> {
        match self.0.get(field) {
            Some(fidl_stash::Value::Stringval(x)) => Some(x),
            _ => None,
        }
    }
}

impl StashNode {
    fn delete(&mut self) -> Result<(), Error> {
        self.stash.delete_prefix(&self.key)?;
        Ok(())
    }

    fn child(&self, key: &str) -> Self {
        Self { key: format!("{}{}{}", self.key, key, NODE_SEPARATOR), stash: self.stash.clone() }
    }

    async fn fields(&self) -> Result<StashNodeFields, Error> {
        let (local, remote) = fidl::endpoints::create_proxy::<_>();
        let () = self.stash.get_prefix(&self.key, remote)?;
        let parent_key_len = self.key.len();
        let mut fields = HashMap::new();
        loop {
            let key_value_list = local.get_next().await?;
            if key_value_list.is_empty() {
                break;
            }

            for key_value in key_value_list {
                let key = &key_value.key[parent_key_len..];
                if let None = key.find(NODE_SEPARATOR) {
                    fields.insert(key.to_string(), key_value.val);
                }
            }
        }
        Ok(StashNodeFields(fields))
    }

    async fn children(&self) -> Result<Vec<Self>, Error> {
        let (local, remote) = fidl::endpoints::create_proxy::<_>();
        let () = self.stash.list_prefix(&self.key, remote)?;

        let parent_key_len = self.key.len();
        let mut children = Vec::new();
        loop {
            let key_list = local.get_next().await.map_err(|e| match e {
                fidl::Error::ClientChannelClosed { epitaph, .. } => {
                    format_err!(
                        "Failed to get stash data from closed channel: {epitaph:?}. Any networks \
                         saved this boot will not be persisted in stash"
                    )
                }
                _ => format_err!(e),
            })?;
            if key_list.is_empty() {
                break;
            }

            for list_item in key_list {
                let key = &list_item.key[parent_key_len..];
                if let Some(idx) = key.find(NODE_SEPARATOR) {
                    children.push(self.child(&key[..idx]));
                }
            }
        }
        Ok(children)
    }

    fn key(&self) -> String {
        self.key.clone()
    }

    #[cfg(test)]
    fn write_val(&mut self, field: &str, value: fidl_stash::Value) -> Result<(), Error> {
        self.stash.set_value(&format!("{}{}", self.key, field), value)?;
        Ok(())
    }

    #[cfg(test)]
    fn write_str(&mut self, field: &str, s: String) -> Result<(), Error> {
        self.write_val(field, fidl_stash::Value::Stringval(s))
    }
}

pub struct StashStore(StashNode);

impl StashStore {
    pub fn new(stash: fidl_stash::StoreAccessorProxy, base_key: &str) -> Self {
        Self(StashNode { key: format!("{NODE_SEPARATOR}{base_key}{NODE_SEPARATOR}"), stash })
    }

    pub fn from_secure_store_proxy(
        id: &str,
        proxy: fidl_stash::SecureStoreProxy,
    ) -> Result<Self, Error> {
        proxy.identify(id).context("failed to identify client to store")?;
        let (store_proxy, accessor_server) = create_proxy();
        proxy.create_accessor(false, accessor_server).context("failed to create accessor")?;

        Ok(Self::new(store_proxy, POLICY_STASH_PREFIX))
    }

    /// Make string value of NetworkIdentifier that will be the key for a config in the stash.
    #[cfg(test)]
    fn serialize_key(id: &NetworkIdentifier) -> Result<String, serde_json::error::Error> {
        serde_json::to_string(id)
    }

    /// Write the persisting values (not including network ID) of a network config to the provided
    /// stash node.
    #[cfg(test)]
    fn write_config(node: &mut StashNode, persistent_data: &PersistentData) -> Result<(), Error> {
        let data_str = serde_json::to_string(&persistent_data).map_err(|e| format_err!("{}", e))?;
        node.write_str(POLICY_DATA_KEY, data_str)
    }

    /// Create the NetworkIdentifier described by the StashNode's key. The key must be in the
    /// format of the root's key followed by a JSON representation of a NetworkIdentifier and then
    /// a node separator. Everything after in the key will be ignored.
    fn id_from_key(&self, node: &StashNode) -> Result<NetworkIdentifier, Error> {
        let key = node.key();
        // Verify that the key begins with the root node's key and remove it.
        if !key.starts_with(&self.0.key()) {
            bail!("key is missing the beginning node separator");
        }
        let prefix_len = self.0.key().len();
        let mut key_after_root = key[prefix_len..].to_string();
        if let Some(index) = key_after_root.find(NODE_SEPARATOR) {
            key_after_root.truncate(index);
        } else {
            bail!("key is missing node separator after network identifier");
        }
        // key_after_root should now just be the serialization of the NetworkIdentifier
        serde_json::from_str(&key_after_root).map_err(|e| format_err!("{}", e))
    }

    /// Read persisting data of a given StashNode and use it to build a network config.
    async fn read_config(node: &StashNode) -> Result<PersistentData, Error> {
        let fields = node.fields().await?;
        let data = fields
            .get_str(POLICY_DATA_KEY)
            .ok_or_else(|| format_err!("failed to read config's data"))?;
        let data: PersistentData = serde_json::from_str(data).map_err(|e| format_err!("{}", e))?;
        Ok(data)
    }

    pub async fn flush(&self) -> Result<(), Error> {
        self.0.stash.flush().await?.map_err(|e| format_err!("failed to flush changes: {:?}", e))
    }

    pub async fn delete_store(&mut self) -> Result<(), Error> {
        self.0.delete()?;
        self.flush().await
    }

    #[cfg(test)]
    pub async fn write(
        &self,
        id: &NetworkIdentifier,
        network_configs: &[PersistentData],
    ) -> Result<(), Error> {
        // write each config to a StashNode under the network identifier. The key of the StashNode
        // will be POLICY_STASH_PREFIX#<net_id>#<index>
        let id_key = Self::serialize_key(id)
            .map_err(|_| format_err!("failed to serialize network identifier"))?;
        let mut id_node = self.0.child(&id_key);

        // If configs to update is empty, we essentially delete the network ID's configs.
        if network_configs.is_empty() {
            id_node.delete()?;
        } else {
            // use a different number to separate each child network config
            let mut config_index = 0;
            for network_config in network_configs {
                let mut config_node = id_node.child(&config_index.to_string());
                Self::write_config(&mut config_node, network_config)?;
                config_index += 1;
            }
        }
        Ok(())
    }

    pub async fn load(&self) -> Result<HashMap<NetworkIdentifier, Vec<PersistentData>>, Error> {
        // get all the children nodes of root, which represent the unique identifiers,
        let id_nodes = self.0.children().await?;

        let mut network_configs = HashMap::new();
        // for each child representing a network config, read in values
        for id_node in id_nodes {
            let mut config_list = vec![];
            match self.id_from_key(&id_node) {
                Ok(net_id) => {
                    for config_node in id_node.children().await? {
                        match Self::read_config(&config_node).await {
                            // If there is an error reading a saved network from stash, make a note
                            // but don't prevent wlancfg starting up.
                            Ok(network_config) => {
                                config_list.push(network_config);
                            }
                            Err(e) => {
                                log::error!("Error loading from stash: {:?}", e);
                            }
                        }
                    }
                    // If we encountered an error reading the configs, don't add.
                    if config_list.is_empty() {
                        continue;
                    }
                    network_configs.insert(net_id, config_list);
                }
                Err(e) => {
                    log::error!("Error reading network identifier from stash: {:?}", e);
                    continue;
                }
            }
        }

        Ok(network_configs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{network_id, rand_string};
    use fuchsia_component::client::connect_to_protocol;
    use wlan_storage_constants::{Credential, SecurityType};

    fn new_stash_store(id: &str) -> fidl_stash::StoreAccessorProxy {
        let store_client = connect_to_protocol::<fidl_stash::StoreMarker>()
            .expect("failed connecting to Stash service");
        store_client.identify(id).expect("failed identifying client to store");
        let (proxy, remote) = create_proxy();
        store_client.create_accessor(false, remote).expect("failed creating Stash accessor");
        proxy
    }

    #[fuchsia::test]
    async fn test_root() {
        let proxy = new_stash_store("test_root");

        let mut store = StashStore::new(proxy, "prefix");

        // Nothing written yet.
        let fields = store.0.fields().await.expect("error reading fields");
        assert!(fields.names().is_empty());

        // Write a field.
        store.0.write_str("test", "Foobar".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        // Read a single fields and all fields.
        let fields = store.0.fields().await.expect("error reading fields");
        assert!(!fields.names().is_empty());
        assert_eq!(Some(&"Foobar".to_string()), fields.get_str("test"));
    }

    #[fuchsia::test]
    async fn test_overwrite() {
        let proxy = new_stash_store("test_overwrite");

        let mut store = StashStore::new(proxy, "prefix");

        // Write a field.
        store.0.write_str("test", "FoobarOne".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        // Overwrite the same field.
        store.0.write_str("test", "FoobarTwo".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");
        let fields = store.0.fields().await.expect("error reading fields");
        assert_eq!("FoobarTwo", fields.get_str("test").unwrap().as_str());
    }

    #[fuchsia::test]
    async fn test_child() {
        let proxy = new_stash_store("test_child");

        let store = StashStore::new(proxy, "prefix");

        let mut child = store.0.child("child");
        child.write_str("test", "Foobar".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        // Root should have no fields.
        let fields = store.0.fields().await.expect("error reading fields");
        assert!(fields.names().is_empty());

        // Child should hold entry.
        let fields = child.fields().await.expect("error reading fields");
        assert_eq!("Foobar", fields.get_str("test").unwrap());
    }

    #[fuchsia::test]
    async fn test_multi_level_value() {
        let proxy = new_stash_store("test_multi_level_value");

        let mut store = StashStore::new(proxy, "prefix");

        store.0.write_str("same_key", "Value Root".to_string()).expect("error writing field");
        store.0.write_str("root", "Foobar Root".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        let mut child = store.0.child("child");
        child.write_str("same_key", "Value Child".to_string()).expect("error writing field");
        child.write_str("child", "Foobar Child".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        let fields = store.0.fields().await.expect("error reading fields");
        assert_eq!("Value Root", fields.get_str("same_key").unwrap());
        assert_eq!("Foobar Root", fields.get_str("root").unwrap());
        let fields = child.fields().await.expect("error reading fields");
        assert_eq!("Value Child", fields.get_str("same_key").unwrap());
        assert_eq!("Foobar Child", fields.get_str("child").unwrap());
    }

    #[fuchsia::test]
    async fn test_delete() {
        let proxy = new_stash_store("test_delete");

        let mut store = StashStore::new(proxy, "prefix");

        store.0.write_str("same_key", "Value Root".to_string()).expect("error writing field");
        store.0.write_str("root", "Foobar Root".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        let mut child = store.0.child("child");
        child.write_str("same_key", "Value Child".to_string()).expect("error writing field");
        child.write_str("child", "Foobar Child".to_string()).expect("error writing field");
        store.flush().await.expect("error flushing");

        child.delete().expect("failed to delete");
        store.flush().await.expect("error flushing");

        let fields = store.0.fields().await.expect("error reading fields");
        assert_eq!("Value Root", fields.get_str("same_key").unwrap());
        assert_eq!("Foobar Root", fields.get_str("root").unwrap());

        let fields = child.fields().await.expect("error reading fields");
        assert!(fields.names().is_empty());
        assert!(store.0.children().await.expect("error fetching children").is_empty());
    }

    #[fuchsia::test]
    async fn test_children() {
        let proxy = new_stash_store("test_children");

        let store = StashStore::new(proxy, "prefix");

        let mut child_a = store.0.child("child a");
        let expected_key = child_a.key().clone();

        // Accessing the child will not yet create it.
        let children = store.0.children().await.expect("error fetching children");
        assert!(children.is_empty());

        child_a.write_str("a str", "42".to_string()).expect("failed to write value");
        store.flush().await.expect("error flushing");

        // Child should now be available to query.
        let children = store.0.children().await.expect("error fetching children");
        assert_eq!(children.len(), 1);
        let added_child = children.iter().next().unwrap();

        // Read from child.
        let fields = added_child.fields().await.expect("error reading fields");
        assert_eq!("42", fields.get_str("a str").unwrap());
        assert_eq!(expected_key, added_child.key());
    }

    #[fuchsia::test]
    async fn load_stash_ignore_bad_values() {
        let stash = new_stash(&rand_string()).await;

        // write bad value directly to StashNode
        let some_net_id = network_id("foo", SecurityType::Wpa2);
        let net_id_str = StashStore::serialize_key(&some_net_id)
            .expect("failed to serialize network identifier");
        let mut config_node = stash.0.child(&net_id_str).child(&format!("{}", 0));
        let bad_value = "some bad value".to_string();
        config_node.write_str(POLICY_DATA_KEY, bad_value).expect("failed to write to stashnode");

        // check that load doesn't fail because of bad string
        let loaded_configs = stash.load().await.expect("failed to load stash");
        assert!(loaded_configs.is_empty());
    }

    #[fuchsia::test]
    async fn write_to_correct_stash_node() {
        let stash = new_stash(&rand_string()).await;

        let net_id = network_id("foo", SecurityType::Wpa2);
        let credential = Credential::Password(b"password".to_vec());
        let network_config =
            PersistentData { credential: credential.clone(), has_ever_connected: false };

        // write to stash and check that the right thing is written under the right StashNode
        stash.write(&net_id, &vec![network_config]).await.expect("failed to write to stash");
        let net_id_str =
            StashStore::serialize_key(&net_id).expect("failed to serialize network identifier");
        let expected_node = stash.0.child(&net_id_str).child(&format!("{}", 0));
        let fields = expected_node.fields().await.expect("failed to get fields");
        let data_actual = fields.get_str(&format!("{}", POLICY_DATA_KEY));
        let data_expected =
            serde_json::to_string(&PersistentData { credential, has_ever_connected: false })
                .expect("failed to serialize data");
        assert_eq!(data_actual, Some(&data_expected));
    }

    async fn new_stash(stash_id: &str) -> StashStore {
        let store_client = connect_to_protocol::<fidl_stash::SecureStoreMarker>()
            .expect("failed to connect to store");
        store_client.identify(stash_id).expect("failed to identify client to store");
        let (store_proxy, accessor_server) = create_proxy();
        store_client.create_accessor(false, accessor_server).expect("failed to create accessor");
        let mut stash = StashStore::new(store_proxy, POLICY_STASH_PREFIX);
        stash.delete_store().await.expect("failed to clear stash");
        stash
    }
}
