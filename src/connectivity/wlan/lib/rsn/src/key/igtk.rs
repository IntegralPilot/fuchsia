// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::key::Tk;
use crate::key_data::kde;
use crate::{Error, rsn_ensure};
use mundane::bytes;
use wlan_common::ie::rsn::cipher::Cipher;

/// This IGTK provider does not support key rotations yet.
#[derive(Debug)]
pub struct IgtkProvider {
    key: Box<[u8]>,
    tk_bytes: usize,
    cipher: Cipher,
}

// IEEE 802.11-2016 12.7.1.5 - The Authenticator shall select the IGTK
// as a random value each time it is generated.
fn generate_random_igtk(len: usize) -> Box<[u8]> {
    let mut key = vec![0; len];
    bytes::rand(&mut key[..]);
    key.into_boxed_slice()
}

impl IgtkProvider {
    pub fn new(cipher: Cipher) -> Result<IgtkProvider, anyhow::Error> {
        let tk_bytes: usize =
            cipher.tk_bytes().ok_or(Error::IgtkHierarchyUnsupportedCipherError)?.into();
        Ok(IgtkProvider { key: generate_random_igtk(tk_bytes), cipher, tk_bytes })
    }

    pub fn cipher(&self) -> Cipher {
        self.cipher
    }

    pub fn rotate_key(&mut self) {
        self.key = generate_random_igtk(self.tk_bytes);
    }

    pub fn get_igtk(&self) -> Igtk {
        Igtk { igtk: self.key.to_vec(), key_id: 0, ipn: [0u8; 6], cipher: self.cipher.clone() }
    }
}

#[derive(Debug, Clone, Eq)]
pub struct Igtk {
    pub igtk: Vec<u8>,
    pub key_id: u16,
    pub ipn: [u8; 6],
    pub cipher: Cipher,
}

/// PartialEq implementation explicitly excludes the IPN (Integrity Packet Number).
/// Both PartialEq and Hash ignore the IPN to prevent key re-installation (KRACK) on retransmissions.
impl PartialEq for Igtk {
    fn eq(&self, other: &Self) -> bool {
        self.igtk == other.igtk && self.key_id == other.key_id && self.cipher == other.cipher
    }
}

impl std::hash::Hash for Igtk {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.igtk.hash(state);
        self.key_id.hash(state);
        self.cipher.hash(state);
    }
}

impl Igtk {
    #[allow(clippy::result_large_err, reason = "large Error enum")]
    pub fn from_kde(element: kde::Igtk, cipher: Cipher) -> Result<Self, Error> {
        let tk_len: usize =
            cipher.tk_bytes().ok_or(Error::IgtkHierarchyUnsupportedCipherError)?.into();
        // TODO(https://fxbug.dev/523310267): Handle the case where `element.igtk.len() > tk_len`
        rsn_ensure!(
            element.igtk.len() >= tk_len,
            Error::InvalidKeyLength(element.igtk.len(), tk_len)
        );
        Ok(Self { igtk: element.igtk, key_id: element.id, ipn: element.ipn, cipher })
    }
}

impl Tk for Igtk {
    fn tk(&self) -> &[u8] {
        &self.igtk[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wlan_common::ie::rsn::suite_filter::DEFAULT_GROUP_MGMT_CIPHER;

    #[test]
    fn test_igtk_generation() {
        let mut igtk_provider =
            IgtkProvider::new(DEFAULT_GROUP_MGMT_CIPHER).expect("failed creating IgtkProvider");

        let first_igtk = igtk_provider.get_igtk().tk().to_vec();
        for _ in 0..3 {
            igtk_provider.rotate_key();
            if first_igtk != igtk_provider.get_igtk().tk().to_vec() {
                return;
            }
        }
        panic!("IGTK key rotation always generates the same key!");
    }

    #[test]
    fn test_igtk_from_kde_validation() {
        // DEFAULT_GROUP_MGMT_CIPHER is BIP-CMAC-128, which expects a 16-byte key.
        let ipn = [0u8; 6];

        // 1. Correct key size (16 bytes)
        let exact_key = vec![0xaa; 16];
        let element = kde::Igtk::new(1, &ipn[..], &exact_key[..]);
        let igtk = Igtk::from_kde(element, DEFAULT_GROUP_MGMT_CIPHER);
        assert!(igtk.is_ok());
        let igtk = igtk.unwrap();
        assert_eq!(igtk.igtk, exact_key);

        // 2. Shorter key size (e.g. 15 bytes) -> should fail.
        let short_key = vec![0xcc; 15];
        let element = kde::Igtk::new(1, &ipn[..], &short_key[..]);
        let igtk = Igtk::from_kde(element, DEFAULT_GROUP_MGMT_CIPHER);
        assert!(igtk.is_err());
    }
}
