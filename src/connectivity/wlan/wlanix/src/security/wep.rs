// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Error};
use wlan_common::security::wep::WepKey;

const NUM_WEP_KEYS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct WepKeys {
    keys: [Option<WepKey>; NUM_WEP_KEYS],
    key_index: Option<usize>,
}

impl WepKeys {
    pub fn new() -> Self {
        Self { keys: [const { None }; NUM_WEP_KEYS], key_index: None }
    }

    /// This sets which key to use, based on the index of the key.
    pub fn set_index(&mut self, key_index: usize) -> Result<(), Error> {
        match key_index {
            0..NUM_WEP_KEYS => {
                self.key_index = Some(key_index);
                Ok(())
            }
            NUM_WEP_KEYS.. => Err(format_err!(
                "invalid key index greater than {NUM_WEP_KEYS}, index will not be set"
            )),
        }
    }

    /// Set one WEP key. This checks that the index is valid and that the key is a valid WEP key.
    pub fn set_key(&mut self, key: Vec<u8>, index: usize) -> Result<(), Error> {
        let index = match index {
            0..NUM_WEP_KEYS => index,
            NUM_WEP_KEYS.. => {
                return Err(format_err!(
                    "invalid key index greater than {NUM_WEP_KEYS}, key will not be set"
                ))
            }
        };

        let key = WepKey::parse(key)?;
        self.keys[index] = Some(key);

        // Set the key index if the index has not been set and this is the first key saved. This
        // will not override the index set through the API.
        if self.key_index.is_none() {
            self.key_index = Some(index);
        }

        Ok(())
    }

    /// This returns the key to use of the multiple that could be saved. The key index is set if a
    /// new key is added and no index has been set, so this should only be None if no key has been
    /// set. The index must be valid to be set, so there is no error checking here.
    pub fn get_key(&self) -> Option<WepKey> {
        self.key_index.and_then(|index| self.keys[index].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wlan_common::security::wep::{WEP104_KEY_BYTES, WEP40_KEY_BYTES};

    #[test]
    fn test_wep_keys_new() {
        let wep_keys = WepKeys::new();
        assert_eq!(wep_keys.keys, [None, None, None, None]);
        assert_eq!(wep_keys.key_index, None);
    }

    #[test]
    fn test_wep_keys_set_key_valid() {
        let mut wep_keys = WepKeys::new();
        let key = [0x01, 0x02, 0x03, 0x04, 0x05];
        let index = 0;
        assert!(wep_keys.set_key(key.to_vec(), index).is_ok());
        assert_eq!(wep_keys.keys[0], Some(WepKey::Wep40(key)));
    }

    #[test]
    fn test_wep_keys_set_key_invalid_index_fails() {
        let mut wep_keys = WepKeys::new();
        let key = vec![0x01, 0x02, 0x03, 0x04, 0x05];
        let invalid_index = 4;
        assert!(wep_keys.set_key(key, invalid_index).is_err());
    }

    #[test]
    fn test_wep_keys_set_key_invalid_length() {
        let mut wep_keys = WepKeys::new();
        let key_too_short = vec![];
        assert!(wep_keys.set_key(key_too_short, 0).is_err());
        let key_too_long = vec![0; 27];
        assert!(wep_keys.set_key(key_too_long, 0).is_err());
        let key_invalid = vec![0; 6];
        assert!(wep_keys.set_key(key_invalid, 0).is_err());
    }

    #[test]
    fn test_wep_keys_set_index_valid() {
        let mut wep_keys = WepKeys::new();
        assert!(wep_keys.set_index(3).is_ok());
        assert_eq!(wep_keys.key_index, Some(3));
    }

    #[test]
    fn test_wep_keys_set_index_invalid() {
        let mut wep_keys = WepKeys::new();
        assert!(wep_keys.set_index(4).is_err());
    }

    #[test]
    fn test_wep_keys_get_key() {
        let mut wep_keys = WepKeys::new();
        let key1 = [0x01; WEP40_KEY_BYTES];
        let key2 = [0x02; WEP104_KEY_BYTES];
        wep_keys.set_key(key1.to_vec(), 0).unwrap();
        wep_keys.set_key(key2.to_vec(), 2).unwrap();

        // By default, if no key has been set, the first key added will be used.
        assert_eq!(wep_keys.get_key(), Some(WepKey::Wep40(key1)));

        // Set index to a key that exists
        wep_keys.set_index(2).unwrap();
        assert_eq!(wep_keys.get_key(), Some(WepKey::Wep104(key2)));

        // Set index to a key that doesn't exist
        wep_keys.set_index(1).unwrap();
        assert_eq!(wep_keys.get_key(), None);
    }
}
