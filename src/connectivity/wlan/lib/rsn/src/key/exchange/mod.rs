// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod handshake;

use self::handshake::fourway::{self, Fourway};
use self::handshake::group_key::{self, GroupKey};
use crate::Error;
use crate::key::gtk::Gtk;
use crate::key::igtk::Igtk;
use crate::key::pmk::Pmk;
use crate::key::ptk::Ptk;
use crate::rsna::{Dot11VerifiedKeyFrame, NegotiatedProtection, UpdateSink};
use zerocopy::SplitByteSlice;

#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Pmk(Pmk),
    Ptk(Ptk),
    Gtk(Gtk),
    Igtk(Igtk),
    MicRx(Vec<u8>),
    MicTx(Vec<u8>),
    Smk(Vec<u8>),
    Stk(Vec<u8>),
}

impl Key {
    pub fn name(&self) -> &'static str {
        match self {
            Key::Pmk(..) => "PMK",
            Key::Ptk(..) => "PTK",
            Key::Gtk(..) => "GTK",
            Key::Igtk(..) => "IGTK",
            Key::MicRx(..) => "MIC_RX",
            Key::MicTx(..) => "MIC_TX",
            Key::Smk(..) => "SMK",
            Key::Stk(..) => "STK",
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Method {
    FourWayHandshake(Box<Fourway>),
    GroupKeyHandshake(GroupKey),
}

impl Method {
    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_eapol_key_frame<B: SplitByteSlice>(
        &mut self,
        update_sink: &mut UpdateSink,
        frame: Dot11VerifiedKeyFrame<B>,
    ) -> Result<(), Error> {
        match self {
            Method::FourWayHandshake(hs) => hs.on_eapol_key_frame(update_sink, frame),
            Method::GroupKeyHandshake(hs) => hs.on_eapol_key_frame(update_sink, frame),
        }
    }

    #[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
    pub fn on_rsna_response_timeout(&self) -> Result<(), Error> {
        match self {
            Method::FourWayHandshake(hs) => hs.on_rsna_response_timeout(),
            // Only 4-Way Handshake returns critical RSNA timeout errors.
            _ => Ok(()),
        }
    }

    pub fn destroy(self) -> Config {
        match self {
            Method::FourWayHandshake(hs) => (*hs).destroy(),
            Method::GroupKeyHandshake(hs) => hs.destroy(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Config {
    FourWayHandshake(fourway::Config),
    GroupKeyHandshake(group_key::Config),
}

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
/// Computes and returns a serialized key frame's MIC.
/// Fails if the AKM has no associated integrity algorithm or MIC size.
pub fn compute_mic_from_buf(
    kck: &[u8],
    protection: &NegotiatedProtection,
    frame: &[u8],
) -> Result<Vec<u8>, Error> {
    let integrity_alg = protection.integrity_algorithm()?;
    let mic_len = protection.akm.mic_bytes().ok_or(Error::UnsupportedAkmSuite)? as usize;
    let mut mic = integrity_alg.compute(kck, frame)?;
    mic.truncate(mic_len);
    Ok(mic)
}

#[allow(clippy::result_large_err, reason = "mass allow for https://fxbug.dev/381896734")]
/// Computes and returns a key frame's MIC.
/// Fails if the AKM has no associated integrity algorithm or MIC size, the given Key Frame's MIC
/// has a different size than the MIC length derived from the AKM or the Key Frame doesn't have its
/// MIC bit set.
pub fn compute_mic<B: SplitByteSlice>(
    kck: &[u8],
    protection: &NegotiatedProtection,
    frame: &eapol::KeyFrameRx<B>,
) -> Result<Vec<u8>, Error> {
    let integrity_alg = protection.integrity_algorithm()?;
    let mic_len = protection.akm.mic_bytes().ok_or(Error::UnsupportedAkmSuite)? as usize;
    if !frame.key_frame_fields.key_info().key_mic() {
        return Err(Error::ComputingMicForUnprotectedFrame);
    }
    if frame.key_mic.len() != mic_len {
        return Err(Error::MicSizesDiffer(frame.key_mic.len(), mic_len));
    }

    let buf = frame.to_bytes(true);
    let mut mic = integrity_alg.compute(kck, &buf[..])?;
    mic.truncate(mic_len);
    Ok(mic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::{self, Algorithm};
    use crate::rsna::test_util;
    use assert_matches::assert_matches;
    use wlan_common::ie::rsn::akm::Akm;

    fn fake_key_frame(mic_len: usize) -> eapol::KeyFrameTx {
        let key_info = eapol::KeyInformation(0).with_key_mic(true);
        eapol::KeyFrameTx::new(
            eapol::ProtocolVersion::IEEE802DOT1X2010,
            eapol::KeyFrameFields::new(
                eapol::KeyDescriptor::IEEE802DOT11,
                key_info,
                16,
                0,
                [0u8; 32],
                [0u8; 16],
                0,
            ),
            vec![],
            mic_len,
        )
    }

    #[test]
    fn compute_mic_unknown_akm() {
        const KCK: [u8; 16] = [5; 16];
        let frame = fake_key_frame(16)
            .serialize()
            .finalize_with_mic(&[0u8; 16][..])
            .expect("failed to create fake key frame");
        let mut protection = test_util::get_rsne_protection();
        protection.akm = Akm::new_dot11(200);
        let result = compute_mic(&KCK[..], &protection, &frame.keyframe());
        assert_matches!(result, Err(Error::UnknownIntegrityAlgorithm));
    }

    #[test]
    fn compute_mic_bit_not_set() {
        const KCK: [u8; 16] = [5; 16];
        let mut frame = fake_key_frame(16);
        frame.key_frame_fields.set_key_info(eapol::KeyInformation(0));
        let frame =
            frame.serialize().finalize_without_mic().expect("failed to create fake key frame");
        let result = compute_mic(&KCK[..], &test_util::get_rsne_protection(), &frame.keyframe());
        assert_matches!(result, Err(Error::ComputingMicForUnprotectedFrame));
    }

    #[test]
    fn compute_mic_different_mic_sizes() {
        const KCK: [u8; 16] = [5; 16];
        let frame = fake_key_frame(0)
            .serialize()
            .finalize_with_mic(&[][..])
            .expect("failed to create fake key frame");
        let result = compute_mic(&KCK[..], &test_util::get_rsne_protection(), &frame.keyframe());
        assert_matches!(result, Err(Error::MicSizesDiffer(0, 16)));
    }

    #[test]
    fn compute_mic_success() {
        const KCK: [u8; 16] = [5; 16];
        let frame = fake_key_frame(16)
            .serialize()
            .finalize_with_mic(&[0u8; 16][..])
            .expect("failed to create fake key frame");
        let mic = compute_mic(&KCK[..], &test_util::get_rsne_protection(), &frame.keyframe())
            .expect("expected failure with unsupported AKM");
        assert!(integrity::hmac_sha1::HmacSha1::new().verify(&KCK[..], &frame[..], &mic[..]));
    }
}
