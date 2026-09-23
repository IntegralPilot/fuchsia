// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::Algorithm;
use crate::Error;

use hmac::{KeyInit as _, Mac as _};

pub struct HmacSha256;

impl HmacSha256 {
    pub fn new() -> HmacSha256 {
        HmacSha256 {}
    }
}

impl Algorithm for HmacSha256 {
    fn compute(&self, key: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
        let mut hmac =
            hmac::Hmac::<sha2::Sha256>::new_from_slice(key).expect("construct new HmacSha256");
        hmac.update(data);
        let bytes: [u8; 32] = hmac.finalize().into_bytes().into();
        Ok(bytes.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex::FromHex;
    use test_case::test_case;

    // RFC 4231, 4. Test Vectors
    #[test_case(
        "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
        "4869205468657265",
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7";
        "test_case_1"
    )]
    #[test_case(
        "4a656665",
        "7768617420646f2079612077616e7420666f72206e6f7468696e673f",
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
        "test_case_2"
    )]
    #[test_case(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
        "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe";
        "test_case_3"
    )]
    #[test_case(
        "0102030405060708090a0b0c0d0e0f10111213141516171819",
        "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
        "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b";
        "test_case_4"
    )]
    #[test_case(
        "0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c",
        "546573742057697468205472756e636174696f6e",
        "a3b6167473100ee06e0c796c2955552b";
        "test_case_5"
    )]
    #[test_case(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "54657374205573696e67204c6172676572205468616e20426c6f636b2d53697a65204b6579202d2048617368204b6579204669727374",
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54";
        "test_case_6"
    )]
    #[test_case(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "5468697320697320612074657374207573696e672061206c6172676572207468616e20626c6f636b2d73697a65206b657920616e642061206c6172676572207468616e20626c6f636b2d73697a6520646174612e20546865206b6579206e6565647320746f20626520686173686564206265666f7265206265696e6720757365642062792074686520484d414320616c676f726974686d2e",
        "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2";
        "test_case_7"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_compute(key_hex_str: &str, data_hex_str: &str, expected_hex_str: &str) {
        let key = Vec::from_hex(key_hex_str).unwrap();
        let data = Vec::from_hex(data_hex_str).unwrap();
        let expected = Vec::from_hex(expected_hex_str).unwrap();
        // Make sure the input test vector is valid. All the expected outputs should be 32
        // bytes, except for test case 5 which truncates it to 16 bytes.
        assert!(expected.len() == 16 || expected.len() == 32);

        let actual = HmacSha256::new().compute(&key[..], &data[..]).unwrap();
        // The actual output should always be 32 bytes.
        assert_eq!(actual.len(), 32);
        assert_eq!(actual[0..expected.len()], expected);
    }
}
