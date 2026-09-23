// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mac::{FrameControl, MgmtHdr, SequenceControl};
use ieee80211::{Bssid, MacAddr};

pub fn mgmt_hdr_to_ap(
    frame_ctrl: FrameControl,
    bssid: Bssid,
    client_addr: MacAddr,
    seq_ctrl: SequenceControl,
) -> MgmtHdr {
    MgmtHdr {
        frame_ctrl,
        duration: 0,
        addr1: bssid.into(),
        addr2: client_addr,
        addr3: bssid.into(),
        seq_ctrl,
    }
}

pub fn mgmt_hdr_from_ap(
    frame_ctrl: FrameControl,
    client_addr: MacAddr,
    bssid: Bssid,
    seq_ctrl: SequenceControl,
) -> MgmtHdr {
    MgmtHdr {
        frame_ctrl,
        duration: 0,
        addr1: client_addr,
        addr2: bssid.into(),
        addr3: bssid.into(),
        seq_ctrl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_to_ap() {
        let got = mgmt_hdr_to_ap(
            FrameControl(1234),
            Bssid::from([1; 6]),
            MacAddr::from([2; 6]),
            SequenceControl(4321),
        );
        let expected = MgmtHdr {
            frame_ctrl: FrameControl(1234),
            duration: 0,
            addr1: MacAddr::from([1; 6]),
            addr2: MacAddr::from([2; 6]),
            addr3: MacAddr::from([1; 6]),
            seq_ctrl: SequenceControl(4321),
        };
        assert_eq!(got, expected);
    }
}
