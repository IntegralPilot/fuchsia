// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::Nla;
use netlink_packet_utils::Emitable;
use std::mem::{size_of, size_of_val};

use crate::nl80211::constants::*;

#[allow(unused)]
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211FrequencyAttr {
    Frequency(u32),
    Disabled,
    DfsState(Nl80211DfsState),
}

impl Nla for Nl80211FrequencyAttr {
    fn value_len(&self) -> usize {
        use Nl80211FrequencyAttr::*;
        match self {
            Frequency(val) => size_of_val(val),
            Disabled => 0,
            DfsState(_) => size_of::<u32>(),
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211FrequencyAttr::*;
        match self {
            Frequency(_) => NL80211_FREQUENCY_ATTR_FREQ,
            Disabled => NL80211_FREQUENCY_ATTR_DISABLED,
            DfsState(_) => NL80211_FREQUENCY_ATTR_DFS_STATE,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211FrequencyAttr::*;
        match self {
            Frequency(val) => NativeEndian::write_u32(buffer, *val),
            Disabled => (),
            DfsState(val) => NativeEndian::write_u32(buffer, val.into()),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211DfsState {
    Usable,
    Unavailable,
    Available,
}

impl From<&Nl80211DfsState> for u32 {
    fn from(state: &Nl80211DfsState) -> u32 {
        use Nl80211DfsState::*;
        match state {
            Usable => NL80211_DFS_USABLE,
            Unavailable => NL80211_DFS_UNAVAILABLE,
            Available => NL80211_DFS_AVAILABLE,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211BandAttr {
    Frequencies(Vec<Vec<Nl80211FrequencyAttr>>),
    // TODO(https://fxbug.dev/335283785): Remove or explain unused code.
    #[allow(dead_code)]
    HtCapable,
    // TODO(https://fxbug.dev/335283785): Remove or explain unused code.
    #[allow(dead_code)]
    VhtCapable,
}

impl Nla for Nl80211BandAttr {
    fn value_len(&self) -> usize {
        use Nl80211BandAttr::*;
        match self {
            Frequencies(freqs) => super::to_nested_nlas(freqs).as_slice().buffer_len(),
            HtCapable => 0,
            VhtCapable => 0,
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211BandAttr::*;
        match self {
            Frequencies(_) => NL80211_BAND_ATTR_FREQS,
            HtCapable => NL80211_BAND_ATTR_HT_CAPA,
            VhtCapable => NL80211_BAND_ATTR_VHT_CAPA,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211BandAttr::*;
        match self {
            Frequencies(freqs) => super::to_nested_nlas(freqs).as_slice().emit(buffer),
            HtCapable => (),
            VhtCapable => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freq_attrs() {
        let attrs = vec![
            Nl80211FrequencyAttr::Frequency(0xaabb),
            Nl80211FrequencyAttr::Disabled,
            Nl80211FrequencyAttr::DfsState(Nl80211DfsState::Available),
        ];

        let mut buffer = vec![0; attrs.as_slice().buffer_len()];
        attrs.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            8, 0, // length
            1, 0, // kind: frequency
            0xbb, 0xaa, 0, 0, // value
            4, 0, // length
            2, 0, // kind: disabled
            8, 0, // length
            7, 0, // kind: dfs state
            2, 0, 0, 0, // value
        ];

        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn test_band_attrs() {
        let attrs = vec![
            Nl80211BandAttr::Frequencies(vec![]),
            Nl80211BandAttr::HtCapable,
            Nl80211BandAttr::VhtCapable,
        ];
        let mut buffer = vec![0; attrs.as_slice().buffer_len()];
        attrs.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            4, 0, // length
            1, 0, // kind: frequencies
            4, 0, // length
            4, 0, // kind: ht capable
            4, 0, // length
            8, 0, // kind: vht capable
        ];

        assert_eq!(buffer, expected_buffer);
    }
}
