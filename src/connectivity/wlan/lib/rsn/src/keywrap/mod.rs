// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod aes;
mod rc4;

use crate::Error;

use aes::NistAes;
use rc4::Rc4;
use wlan_common::ie::rsn::akm;

/// An arbitrary algorithm used to encrypt the key data field of an EAPoL keyframe.
/// Usage is specified in IEEE 802.11-2016 8.5.2 j
pub trait Algorithm {
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    /// Uses the given KEK and IV as a key to wrap the given data for secure transmission.
    fn wrap_key(&self, kek: &[u8], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, Error>;
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    /// Uses the given KEK and IV as a key to unwrap the given data after secure transmission.
    fn unwrap_key(&self, kek: &[u8], iv: &[u8; 16], data: &[u8]) -> Result<Vec<u8>, Error>;
}

/// IEEE Std 802.11-2024, 12.7.2 b.1)
/// IEEE Std 802.11-2024, 12.7.3 Table 12-11
pub fn keywrap_algorithm(
    key_descriptor_version: u16,
    akm: &akm::Akm,
) -> Option<Box<dyn Algorithm>> {
    match key_descriptor_version {
        1 => Some(Box::new(Rc4)),
        2 => Some(Box::new(NistAes)),
        0 if akm.suite_type == akm::SAE || akm.suite_type == akm::OWE => Some(Box::new(NistAes)),
        _ => None,
    }
}
