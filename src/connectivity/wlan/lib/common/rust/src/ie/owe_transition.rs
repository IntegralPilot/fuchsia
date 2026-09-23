// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer_reader::BufferReader;
use anyhow::{bail, format_err};
use ieee80211::{Bssid, Ssid};

pub const VENDOR_SPECIFIC_TYPE: u8 = 0x1C;

/// WFA Opportunistic Wireless Encryption Specification v1.0, Section 2.3.1
#[derive(Eq, PartialEq, Hash, Clone, Debug)]
pub struct OweTransition {
    pub bssid: Bssid,
    pub ssid: Ssid,
    pub band_and_channel: Option<[u8; 2]>,
}

pub fn parse_owe_transition(raw_body: &[u8]) -> Result<OweTransition, anyhow::Error> {
    let mut reader = BufferReader::new(raw_body);
    const TOO_SHORT_ERR: &'static str = "OWE Transition Element too short";
    let bssid = *reader
        .read::<Bssid>()
        .ok_or_else(|| format_err!("Failed parsing BSSID: {}", TOO_SHORT_ERR))?;
    let ssid_length = reader
        .read_byte()
        .ok_or_else(|| format_err!("Failed parsing SSID length: {}", TOO_SHORT_ERR))?;
    let ssid = reader
        .read_array(ssid_length as usize)
        .ok_or_else(|| format_err!("Failed parsing SSID: {}", TOO_SHORT_ERR))?;
    let ssid =
        Ssid::try_from(&*ssid).map_err(|e| format_err!("Unexpected error reading SSID {:?}", e))?;

    let mut band_and_channel = None;
    if reader.bytes_remaining() > 0 {
        band_and_channel = Some(*reader.read::<[u8; 2]>().ok_or_else(|| {
            format_err!("Failed parsing band and channel info: {}", TOO_SHORT_ERR)
        })?);
    }

    if reader.bytes_remaining() > 0 {
        bail!("OWE Transition Element too long");
    }

    Ok(OweTransition { bssid, ssid, band_and_channel })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_owe_transition() {
        let raw: Vec<u8> = vec![
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // BSSID
            0x03, // SSID length
            0x66, 0x6f, 0x6f, // SSID "foo"
            0x01, 0x06, // Band and channel
        ];
        let owe_transition = parse_owe_transition(&raw).unwrap();
        assert_eq!(owe_transition.bssid, Bssid::from([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]));
        assert_eq!(owe_transition.ssid, b"foo"[..]);
        assert_eq!(owe_transition.band_and_channel, Some([0x01, 0x06]));

        let raw_no_band_channel: Vec<u8> = vec![
            0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // BSSID
            0x03, // SSID length
            0x66, 0x6f, 0x6f, // SSID "foo"
        ];
        let owe_transition = parse_owe_transition(&raw_no_band_channel).unwrap();
        assert_eq!(owe_transition.bssid, Bssid::from([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]));
        assert_eq!(owe_transition.ssid, b"foo"[..]);
        assert_eq!(owe_transition.band_and_channel, None);
    }
}
