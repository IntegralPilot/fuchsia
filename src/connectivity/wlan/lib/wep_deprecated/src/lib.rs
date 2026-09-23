// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use hex::FromHex;
use ieee80211::{MacAddr, MacAddrBytes};
use std::ops::Deref;
use thiserror::Error;
use wlan_common::ie::rsn::cipher;
use wlan_common::ie::rsn::suite_selector::OUI;
use {fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211, fidl_fuchsia_wlan_mlme as fidl_mlme};

#[derive(Debug, Error, PartialEq)]
pub enum Error {
    #[error("error deriving WEP key from given hex input: {}", _0)]
    InvalidHexInput(hex::FromHexError),
    #[error("expected ASCII or hex format for WEP-40 or WEP-104 keys")]
    InvalidInputLength,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Bits40([u8; 5]),
    Bits104([u8; 13]),
}

impl Key {
    /// Returns an IEEE 802.11 compatible cipher suite type.
    /// IEEE Std 802.11-2016, 9.4.2.25.2, Table 9-131
    fn cipher_suite_type(&self) -> u8 {
        match self {
            Key::Bits40(_) => cipher::WEP_40,
            Key::Bits104(_) => cipher::WEP_104,
        }
    }
}

impl Deref for Key {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match self {
            Key::Bits40(key) => &key[..],
            Key::Bits104(key) => &key[..],
        }
    }
}

/// Derives a WEP key from the given input.
/// The input can be either in ASCII or hex representation.
/// The key length is derived from the input length.
/// WEP-40 and WEP-104 is supported.
/// Returns an Error if WEP support is disabled.
pub fn derive_key(input: &[u8]) -> Result<Key, Error> {
    match input.len() {
        // WEP-40:
        5 => Ok(Key::Bits40(to_array(input))),
        10 => <[u8; 5]>::from_hex(input).map(Key::Bits40).map_err(Error::InvalidHexInput),
        // WEP-104:
        13 => Ok(Key::Bits104(to_array(input))),
        26 => <[u8; 13]>::from_hex(input).map(Key::Bits104).map_err(Error::InvalidHexInput),
        _ => Err(Error::InvalidInputLength),
    }
}

/// Create an MLME-SETKEYS.request primitive for the given key and BSSID.
pub fn make_mlme_set_keys_request(peer: MacAddr, key: &Key) -> fidl_mlme::SetKeysRequest {
    fidl_mlme::SetKeysRequest {
        keylist: vec![fidl_mlme::SetKeyDescriptor {
            key_type: fidl_mlme::KeyType::Pairwise,
            key: key.to_vec(),
            key_id: 0,
            address: peer.to_array(),
            cipher_suite_oui: OUI.into(),
            cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(
                key.cipher_suite_type().into(),
            ),
            rsc: 0,
        }],
    }
}

fn to_array<A>(slice: &[u8]) -> A
where
    A: Sized + Default + AsMut<[u8]>,
{
    let mut array = Default::default();
    <A as AsMut<[u8]>>::as_mut(&mut array).clone_from_slice(slice);
    array
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_supported() {
        // Invalid WEP key length:
        derive_key(&[1; 3]).expect_err("expected failure for invalid WEP key");

        // WEP-40 ASCII:
        assert_eq!(derive_key(&[1; 5]), Ok(Key::Bits40([1; 5])));

        // WEP-104 ASCII:
        assert_eq!(derive_key(&[1; 13]), Ok(Key::Bits104([1; 13])));

        // WEP-40 HEX:
        assert_eq!(derive_key(&[65; 10]), Ok(Key::Bits40([0xAA; 5])));

        // WEP-104 HEX:
        assert_eq!(derive_key(&[65; 26]), Ok(Key::Bits104([0xAA; 13])));

        // Invalid HEX characters:
        derive_key(&[75; 26]).expect_err("cannot derive WEP key from invalid hex string");
    }

    #[test]
    fn test_make_mlme_set_keys_request() {
        // WEP-40:
        let actual = make_mlme_set_keys_request([4; 6].into(), &Key::Bits40([3; 5]));
        let expected = fidl_mlme::SetKeysRequest {
            keylist: vec![fidl_mlme::SetKeyDescriptor {
                key_type: fidl_mlme::KeyType::Pairwise,
                key: vec![3; 5],
                key_id: 0,
                address: [4; 6],
                cipher_suite_oui: [0x00, 0x0F, 0xAC],
                cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(1),
                rsc: 0,
            }],
        };
        assert_eq!(actual, expected);

        // WEP-104:
        let actual = make_mlme_set_keys_request([4; 6].into(), &Key::Bits104([3; 13]));
        let expected = fidl_mlme::SetKeysRequest {
            keylist: vec![fidl_mlme::SetKeyDescriptor {
                key_type: fidl_mlme::KeyType::Pairwise,
                key: vec![3; 13],
                key_id: 0,
                address: [4; 6],
                cipher_suite_oui: [0x00, 0x0F, 0xAC],
                cipher_suite_type: fidl_ieee80211::CipherSuiteType::from_primitive_allow_unknown(5),
                rsc: 0,
            }],
        };
        assert_eq!(actual, expected);
    }
}
