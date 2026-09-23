// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fxfs_crypt_common::CryptBase;
use fxfs_crypto::{KeyPurpose, WrappingKeyId};

pub const DATA_KEY: [u8; 32] = [
    0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf, 0x10, 0x11,
    0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];
pub const METADATA_KEY: [u8; 32] = [
    0xff, 0xfe, 0xfd, 0xfc, 0xfb, 0xfa, 0xf9, 0xf8, 0xf7, 0xf6, 0xf5, 0xf4, 0xf3, 0xf2, 0xf1, 0xf0,
    0xef, 0xee, 0xed, 0xec, 0xeb, 0xea, 0xe9, 0xe8, 0xe7, 0xe6, 0xe5, 0xe4, 0xe3, 0xe2, 0xe1, 0xe0,
];
const DATA_WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(0);
const METADATA_WRAPPING_KEY_ID: WrappingKeyId = u128::to_le_bytes(1);

/// Creates a `CryptBase` instance pre-configured with insecure keys.
///
/// It is intended for use only in test code where actual security is inconsequential.
pub fn new_insecure_crypt() -> CryptBase {
    let crypt = CryptBase::new();
    crypt.add_wrapping_key(DATA_WRAPPING_KEY_ID, DATA_KEY).unwrap();
    crypt.add_wrapping_key(METADATA_WRAPPING_KEY_ID, METADATA_KEY).unwrap();
    crypt.set_active_key(KeyPurpose::Data, DATA_WRAPPING_KEY_ID).unwrap();
    crypt.set_active_key(KeyPurpose::Metadata, METADATA_WRAPPING_KEY_ID).unwrap();
    crypt
}
