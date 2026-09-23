// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{MgmtFrame, MgmtSubtype};
use crate::ie;
use zerocopy::{Ref, SplitByteSlice};

mod fields;
mod reason;
mod status;

pub use fields::*;
pub use reason::*;
pub use status::*;

/// Frame or frame body distinguished by acknowledgement or lack thereof, namely action frames.
#[derive(Clone, Copy, Debug)]
pub struct NoAck<const NO_ACK: bool, T>(pub T);

impl<const NO_ACK: bool, B> NoAck<NO_ACK, ActionBody<B>>
where
    B: SplitByteSlice,
{
    // NOTE: The management frame subtype is not part of the body. This function assumes whether or
    //       not acknowledgement is required.
    pub fn parse(bytes: B) -> Option<Self> {
        ActionBody::parse(bytes).map(NoAck)
    }
}

impl<const NO_ACK: bool, T> NoAck<NO_ACK, T> {
    pub fn into_body(self) -> T {
        self.0
    }
}

impl<const NO_ACK: bool, T> AsRef<T> for NoAck<NO_ACK, T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

/// Action frame that requires acknowledgement per the `ACTION` management frame subtype.
pub type ActionAckFrame<B> = NoAck<false, ActionBody<B>>;

/// Action frame that does **not** require acknowledgement per the `ACTION_NO_ACK` management frame
/// subtype.
pub type ActionNoAckFrame<B> = NoAck<true, ActionBody<B>>;

/// Action frame that may or may not require acknowledgement.
#[derive(Debug)]
pub enum ActionFrame<B>
where
    B: SplitByteSlice,
{
    Ack(ActionAckFrame<B>),
    NoAck(ActionNoAckFrame<B>),
}

impl<B> ActionFrame<B>
where
    B: SplitByteSlice,
{
    pub fn parse(subtype: MgmtSubtype, bytes: B) -> Option<Self> {
        match subtype {
            MgmtSubtype::ACTION => ActionAckFrame::parse(bytes).map(From::from),
            MgmtSubtype::ACTION_NO_ACK => ActionNoAckFrame::parse(bytes).map(From::from),
            _ => None,
        }
    }

    pub fn into_body(self) -> ActionBody<B> {
        match self {
            ActionFrame::Ack(frame) => frame.into_body(),
            ActionFrame::NoAck(frame) => frame.into_body(),
        }
    }

    pub fn ack(self) -> Option<ActionAckFrame<B>> {
        match self {
            ActionFrame::Ack(frame) => Some(frame),
            _ => None,
        }
    }

    pub fn no_ack(self) -> Option<ActionNoAckFrame<B>> {
        match self {
            ActionFrame::NoAck(frame) => Some(frame),
            _ => None,
        }
    }

    pub fn is_ack(&self) -> bool {
        matches!(self, ActionFrame::Ack(_))
    }

    pub fn is_no_ack(&self) -> bool {
        matches!(self, ActionFrame::NoAck(_))
    }
}

impl<B> AsRef<ActionBody<B>> for ActionFrame<B>
where
    B: SplitByteSlice,
{
    fn as_ref(&self) -> &ActionBody<B> {
        match self {
            ActionFrame::Ack(frame) => frame.as_ref(),
            ActionFrame::NoAck(frame) => frame.as_ref(),
        }
    }
}

impl<B> From<ActionAckFrame<B>> for ActionFrame<B>
where
    B: SplitByteSlice,
{
    fn from(frame: ActionAckFrame<B>) -> Self {
        ActionFrame::Ack(frame)
    }
}

impl<B> From<ActionNoAckFrame<B>> for ActionFrame<B>
where
    B: SplitByteSlice,
{
    fn from(frame: ActionNoAckFrame<B>) -> Self {
        ActionFrame::NoAck(frame)
    }
}

/// Action frame body.
///
/// Contains the contents of an action frame: an action header and action elements.
#[derive(Debug)]
pub struct ActionBody<B>
where
    B: SplitByteSlice,
{
    pub action_hdr: Ref<B, ActionHdr>,
    pub elements: B,
}

impl<B> ActionBody<B>
where
    B: SplitByteSlice,
{
    pub fn parse(bytes: B) -> Option<Self> {
        Ref::from_prefix(bytes)
            .ok()
            .map(|(action_hdr, elements)| ActionBody { action_hdr, elements })
    }
}

#[derive(Debug)]
pub struct AssocReqFrame<B>
where
    B: SplitByteSlice,
{
    pub assoc_req_hdr: Ref<B, AssocReqHdr>,
    pub elements: B,
}

impl<B> AssocReqFrame<B>
where
    B: SplitByteSlice,
{
    pub fn parse(bytes: B) -> Option<Self> {
        Ref::from_prefix(bytes)
            .ok()
            .map(|(assoc_req_hdr, elements)| AssocReqFrame { assoc_req_hdr, elements })
    }

    pub fn ies(&self) -> impl '_ + Iterator<Item = (ie::Id, &'_ B::Target)> {
        ie::Reader::new(self.elements.deref())
    }
}

#[derive(Debug)]
pub struct AssocRespFrame<B>
where
    B: SplitByteSlice,
{
    pub assoc_resp_hdr: Ref<B, AssocRespHdr>,
    pub elements: B,
}

impl<B> AssocRespFrame<B>
where
    B: SplitByteSlice,
{
    pub fn parse(bytes: B) -> Option<Self> {
        Ref::from_prefix(bytes)
            .ok()
            .map(|(assoc_resp_hdr, elements)| AssocRespFrame { assoc_resp_hdr, elements })
    }

    pub fn into_assoc_resp_body(self) -> (Ref<B, AssocRespHdr>, B) {
        let AssocRespFrame { assoc_resp_hdr, elements } = self;
        (assoc_resp_hdr, elements)
    }

    pub fn ies(&self) -> impl '_ + Iterator<Item = (ie::Id, &'_ B::Target)> {
        ie::Reader::new(self.elements.deref())
    }
}

#[derive(Debug)]
pub struct AuthFrame<B>
where
    B: SplitByteSlice,
{
    pub auth_hdr: Ref<B, AuthHdr>,
    pub elements: B,
}

impl<B> AuthFrame<B>
where
    B: SplitByteSlice,
{
    pub fn parse(bytes: B) -> Option<Self> {
        Ref::from_prefix(bytes).ok().map(|(auth_hdr, elements)| AuthFrame { auth_hdr, elements })
    }

    pub fn into_auth_body(self) -> (Ref<B, AuthHdr>, B) {
        let AuthFrame { auth_hdr, elements } = self;
        (auth_hdr, elements)
    }
}

#[derive(Debug)]
pub struct ProbeReqFrame<B>
where
    B: SplitByteSlice,
{
    pub elements: B,
}

impl<B> ProbeReqFrame<B>
where
    B: SplitByteSlice,
{
    pub fn parse(bytes: B) -> Option<Self> {
        Some(ProbeReqFrame { elements: bytes })
    }
}

// TODO(https://fxbug.dev/42079361): Implement dedicated management body types for remaining
//                                   variants with fields and change those variants to tuple
//                                   variants instead.
#[derive(Debug)]
pub enum MgmtBody<B: SplitByteSlice> {
    Beacon { bcn_hdr: Ref<B, BeaconHdr>, elements: B },
    ProbeReq(ProbeReqFrame<B>),
    ProbeResp { probe_resp_hdr: Ref<B, ProbeRespHdr>, elements: B },
    Authentication(AuthFrame<B>),
    AssociationReq(AssocReqFrame<B>),
    AssociationResp(AssocRespFrame<B>),
    Deauthentication { deauth_hdr: Ref<B, DeauthHdr>, elements: B },
    Disassociation { disassoc_hdr: Ref<B, DisassocHdr>, elements: B },
    Action(ActionFrame<B>),
    Unsupported { subtype: MgmtSubtype },
}

impl<B: SplitByteSlice> MgmtBody<B> {
    pub fn parse(subtype: MgmtSubtype, bytes: B) -> Option<Self> {
        match subtype {
            MgmtSubtype::BEACON => {
                let (bcn_hdr, elements) = Ref::from_prefix(bytes).ok()?;
                Some(MgmtBody::Beacon { bcn_hdr, elements })
            }
            MgmtSubtype::PROBE_REQ => ProbeReqFrame::parse(bytes).map(From::from),
            MgmtSubtype::PROBE_RESP => {
                let (probe_resp_hdr, elements) = Ref::from_prefix(bytes).ok()?;
                Some(MgmtBody::ProbeResp { probe_resp_hdr, elements })
            }
            MgmtSubtype::AUTH => AuthFrame::parse(bytes).map(From::from),
            MgmtSubtype::ASSOC_REQ => AssocReqFrame::parse(bytes).map(From::from),
            MgmtSubtype::ASSOC_RESP => AssocRespFrame::parse(bytes).map(From::from),
            MgmtSubtype::DEAUTH => {
                let (deauth_hdr, elements) = Ref::from_prefix(bytes).ok()?;
                Some(MgmtBody::Deauthentication { deauth_hdr, elements })
            }
            MgmtSubtype::DISASSOC => {
                let (disassoc_hdr, elements) = Ref::from_prefix(bytes).ok()?;
                Some(MgmtBody::Disassociation { disassoc_hdr, elements })
            }
            MgmtSubtype::ACTION => {
                ActionAckFrame::parse(bytes).map(ActionFrame::from).map(From::from)
            }
            MgmtSubtype::ACTION_NO_ACK => {
                ActionNoAckFrame::parse(bytes).map(ActionFrame::from).map(From::from)
            }
            subtype => Some(MgmtBody::Unsupported { subtype }),
        }
    }
}

impl<B> From<ProbeReqFrame<B>> for MgmtBody<B>
where
    B: SplitByteSlice,
{
    fn from(frame: ProbeReqFrame<B>) -> Self {
        MgmtBody::ProbeReq(frame)
    }
}

impl<B> From<AuthFrame<B>> for MgmtBody<B>
where
    B: SplitByteSlice,
{
    fn from(frame: AuthFrame<B>) -> Self {
        MgmtBody::Authentication(frame)
    }
}

impl<B> From<AssocReqFrame<B>> for MgmtBody<B>
where
    B: SplitByteSlice,
{
    fn from(frame: AssocReqFrame<B>) -> Self {
        MgmtBody::AssociationReq(frame)
    }
}

impl<B> From<AssocRespFrame<B>> for MgmtBody<B>
where
    B: SplitByteSlice,
{
    fn from(frame: AssocRespFrame<B>) -> Self {
        MgmtBody::AssociationResp(frame)
    }
}

impl<B> From<ActionFrame<B>> for MgmtBody<B>
where
    B: SplitByteSlice,
{
    fn from(frame: ActionFrame<B>) -> Self {
        MgmtBody::Action(frame)
    }
}

impl<B> TryFrom<MgmtFrame<B>> for MgmtBody<B>
where
    B: SplitByteSlice,
{
    type Error = ();

    fn try_from(mgmt_frame: MgmtFrame<B>) -> Result<Self, Self::Error> {
        mgmt_frame.try_into_mgmt_body().1.ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::*;
    use crate::TimeUnit;
    use assert_matches::assert_matches;

    #[test]
    fn mgmt_hdr_len() {
        assert_eq!(MgmtHdr::len(HtControl::ABSENT), 24);
        assert_eq!(MgmtHdr::len(HtControl::PRESENT), 28);
    }

    #[test]
    fn parse_beacon_frame() {
        #[rustfmt::skip]
            let bytes = vec![
            1,1,1,1,1,1,1,1, // timestamp
            2,2, // beacon_interval
            3,3, // capabilities
            0,5,1,2,3,4,5 // SSID IE: "12345"
        ];
        assert_matches!(
            MgmtBody::parse(MgmtSubtype::BEACON, &bytes[..]),
            Some(MgmtBody::Beacon { bcn_hdr, elements }) => {
                assert_eq!(TimeUnit(0x0202), { bcn_hdr.beacon_interval });
                assert_eq!(0x0303, { bcn_hdr.capabilities.0 });
                assert_eq!(&[0, 5, 1, 2, 3, 4, 5], &elements[..]);
            },
            "expected beacon frame"
        );
    }
}
