// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{CtrlFrame, CtrlSubtype};
use zerocopy::{Ref, SplitByteSlice};

mod fields;

pub use fields::*;

#[derive(Debug)]
pub enum CtrlBody<B: SplitByteSlice> {
    PsPoll { ps_poll: Ref<B, PsPoll> },
    Unsupported { subtype: CtrlSubtype },
}

impl<B: SplitByteSlice> CtrlBody<B> {
    pub fn parse(subtype: CtrlSubtype, bytes: B) -> Option<Self> {
        match subtype {
            CtrlSubtype::PS_POLL => {
                let (ps_poll, _) = Ref::from_prefix(bytes).ok()?;
                Some(CtrlBody::PsPoll { ps_poll })
            }
            subtype => Some(CtrlBody::Unsupported { subtype }),
        }
    }
}

impl<B> TryFrom<CtrlFrame<B>> for CtrlBody<B>
where
    B: SplitByteSlice,
{
    type Error = ();

    fn try_from(ctrl_frame: CtrlFrame<B>) -> Result<Self, Self::Error> {
        CtrlBody::parse(ctrl_frame.ctrl_subtype(), ctrl_frame.body).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use ieee80211::{Bssid, MacAddr};

    #[test]
    fn parse_ps_poll_frame() {
        let bytes = vec![
            0b00000001, 0b11000000, // Masked AID
            2, 2, 2, 2, 2, 2, // addr1
            4, 4, 4, 4, 4, 4, // addr2
        ];
        assert_matches!(
            CtrlBody::parse(CtrlSubtype::PS_POLL, &bytes[..]),
            Some(CtrlBody::PsPoll { ps_poll }) => {
                assert_eq!(0b1100000000000001, { ps_poll.masked_aid });
                assert_eq!(Bssid::from([2; 6]), ps_poll.bssid);
                assert_eq!(MacAddr::from([4; 6]), ps_poll.ta);
            },
            "expected PS-Poll frame"
        );
    }
}
