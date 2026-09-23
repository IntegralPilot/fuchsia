// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::key::Tk;
use crate::{Error, rsn_ensure};
use mundane::bytes;
use std::hash::{Hash, Hasher};
use wlan_common::ie::rsn::cipher::Cipher;

/// This GTK provider does not support key rotations yet.
#[derive(Debug)]
pub struct GtkProvider(Gtk);

impl GtkProvider {
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn new(cipher: Cipher, key_id: u8, key_rsc: u64) -> Result<GtkProvider, Error> {
        Ok(GtkProvider(Gtk::generate_random(cipher, key_id, key_rsc)?))
    }

    pub fn get_gtk(&self) -> &Gtk {
        &self.0
    }
}

#[derive(Debug, Clone, Eq)]
pub struct Gtk {
    pub bytes: Box<[u8]>,
    cipher: Cipher,
    tk_len: usize,
    key_id: u8,
    key_rsc: u64,
}

/// PartialEq implementation explicitly excludes the RSC.
/// Both PartialEq and Hash ignore the RSC to prevent key re-installation (KRACK) on retransmissions.
impl PartialEq for Gtk {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.tk_len == other.tk_len && self.key_id == other.key_id
    }
}

/// Custom Hash implementation which doesn't take the RSC or cipher suite into consideration.
/// Make sure to check that this property is upheld: `v1 == v2 => hash(v1) == hash(v2)`
impl Hash for Gtk {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key_id.hash(state);
        self.tk().hash(state);
    }
}

impl Gtk {
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn generate_random(cipher: Cipher, key_id: u8, key_rsc: u64) -> Result<Gtk, Error> {
        // IEEE 802.11-2016 12.7.4 EAPOL-Key frame notation
        rsn_ensure!(
            0 < key_id && key_id < 4,
            "GTK key ID must not be zero and must fit in a two bit field"
        );

        let tk_len: usize =
            cipher.tk_bytes().ok_or(Error::GtkHierarchyUnsupportedCipherError)?.into();
        let mut gtk_bytes: Box<[u8]> = vec![0; tk_len].into();
        bytes::rand(&mut gtk_bytes[..]);

        Ok(Gtk { bytes: gtk_bytes, cipher, tk_len, key_id, key_rsc })
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn from_bytes(
        gtk_bytes: Box<[u8]>,
        cipher: Cipher,
        key_id: u8,
        key_rsc: u64,
    ) -> Result<Gtk, Error> {
        // IEEE 802.11-2016 12.7.4 EAPOL-Key frame notation
        rsn_ensure!(
            0 < key_id && key_id < 4,
            "GTK key ID must not be zero and must fit in a two bit field"
        );

        let tk_len: usize =
            cipher.tk_bytes().ok_or(Error::GtkHierarchyUnsupportedCipherError)?.into();
        // TODO(https://fxbug.dev/523310267): Handle the case where `gtk_bytes.len() > tk_len`
        rsn_ensure!(gtk_bytes.len() >= tk_len, "GTK must be larger than the resulting TK");

        Ok(Gtk { bytes: gtk_bytes, cipher, tk_len, key_id, key_rsc })
    }

    pub fn cipher(&self) -> &Cipher {
        &self.cipher
    }

    pub fn key_id(&self) -> u8 {
        self.key_id
    }

    pub fn key_rsc(&self) -> u64 {
        self.key_rsc
    }
}

impl Tk for Gtk {
    fn tk(&self) -> &[u8] {
        &self.bytes[0..self.tk_len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use wlan_common::ie::rsn::cipher;
    use wlan_common::ie::rsn::suite_selector::OUI;

    #[test]
    fn generated_gtks_are_not_zero_and_not_constant_with_high_probability() {
        let mut gtks = HashSet::new();
        for i in 0..10 {
            let provider =
                GtkProvider::new(Cipher { oui: OUI, suite_type: cipher::CCMP_128 }, 2, 5)
                    .expect("failed creating GTK Provider");
            let gtk_bytes: Box<[u8]> = provider.get_gtk().tk().into();
            assert!(gtk_bytes.iter().any(|&x| x != 0));
            if i > 0 && !gtks.contains(&gtk_bytes) {
                return;
            }
            gtks.insert(gtk_bytes);
        }
        panic!("GtkProvider::generate_gtk() generated the same GTK 10 times in a row.");
    }

    #[test]
    fn generated_gtk_captures_key_id() {
        let provider = GtkProvider::new(Cipher { oui: OUI, suite_type: cipher::CCMP_128 }, 1, 3)
            .expect("failed creating GTK Provider");
        let gtk = provider.get_gtk();
        assert_eq!(gtk.key_id(), 1);
    }

    #[test]
    fn generated_gtk_captures_key_rsc() {
        let provider = GtkProvider::new(Cipher { oui: OUI, suite_type: cipher::CCMP_128 }, 1, 3)
            .expect("failed creating GTK Provider");
        let gtk = provider.get_gtk();
        assert_eq!(gtk.key_rsc(), 3);
    }

    #[test]
    fn gtk_generation_fails_with_key_id_zero() {
        GtkProvider::new(Cipher { oui: OUI, suite_type: cipher::CCMP_128 }, 0, 4)
            .expect_err("GTK provider incorrectly accepts key ID 0");
    }
}
