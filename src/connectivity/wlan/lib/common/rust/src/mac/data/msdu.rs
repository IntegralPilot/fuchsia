// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer_reader::BufferReader;
use crate::mac::data::*;
use crate::mac::{DataFrame, MacAddr};
use zerocopy::byteorder::big_endian::U16;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, Unaligned};

// RFC 1042
pub const LLC_SNAP_EXTENSION: u8 = 0xAA;
pub const LLC_SNAP_UNNUMBERED_INFO: u8 = 0x03;
pub const LLC_SNAP_OUI: [u8; 3] = [0, 0, 0];

// IEEE Std 802.2-1998, 3.2
// IETF RFC 1042
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C, packed)]
pub struct LlcHdr {
    pub dsap: u8,
    pub ssap: u8,
    pub control: u8,
    pub oui: [u8; 3],
    pub protocol_id: U16,
}

pub struct LlcFrame<B> {
    pub hdr: Ref<B, LlcHdr>,
    pub body: B,
}

impl<B> LlcFrame<B> {
    pub fn parse(bytes: B) -> Option<Self>
    where
        B: SplitByteSlice,
    {
        // An LLC frame is only valid if it contains enough bytes for the header and one or more
        // bytes for the body.
        let (hdr, body) = Ref::from_prefix(bytes).ok()?;
        if body.is_empty() { None } else { Some(Self { hdr, body }) }
    }

    pub fn into_body(self) -> B {
        self.body
    }
}

/// A single MSDU.
pub struct Msdu<B> {
    pub dst_addr: MacAddr,
    pub src_addr: MacAddr,
    pub llc_frame: LlcFrame<B>,
}

impl<B> Msdu<B> {
    pub fn into_body(self) -> B {
        self.llc_frame.into_body()
    }
}

/// Iterator over the MSDUs of a data frame.
///
/// This iterator supports NULL, non-aggregated, and aggregated data fields, which yield zero, one,
/// and zero or more MSDUs, respectively.
pub struct IntoMsduIter<B> {
    subtype: MsduIterSubtype<B>,
}

impl<B> From<DataFrame<B>> for IntoMsduIter<B>
where
    B: SplitByteSlice,
{
    fn from(frame: DataFrame<B>) -> Self {
        IntoMsduIter { subtype: frame.into() }
    }
}

impl<B> Iterator for IntoMsduIter<B>
where
    B: SplitByteSlice,
{
    // TODO(https://fxbug.dev/330761628): The item should probably be `Result<Msdu<B>, _>` so that
    //                                    client code can distinguish between exhausting the
    //                                    iterator and a parse failure (which is unexpected and
    //                                    likely indicates a meaningful error).
    type Item = Msdu<B>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.subtype {
            MsduIterSubtype::Null => None,
            MsduIterSubtype::Llc { dst_addr, src_addr, ref mut body } => body
                .take()
                .and_then(LlcFrame::parse)
                .map(|llc_frame| Msdu { dst_addr, src_addr, llc_frame }),
            MsduIterSubtype::Amsdu(ref mut reader) => AmsduSubframe::parse(reader)
                .and_then(|AmsduSubframe { hdr, body }| {
                    LlcFrame::parse(body).map(|llc_frame| (hdr, llc_frame))
                })
                .map(|(hdr, llc_frame)| Msdu { dst_addr: hdr.da, src_addr: hdr.sa, llc_frame }),
        }
    }
}

enum MsduIterSubtype<B> {
    /// Non-aggregated data frame (exactly one MSDU).
    Llc { dst_addr: MacAddr, src_addr: MacAddr, body: Option<B> },
    /// Aggregated data frame (zero or more MSDUs).
    Amsdu(BufferReader<B>),
    /// NULL data frame (zero MSDUs).
    Null,
}

impl<B> From<DataFrame<B>> for MsduIterSubtype<B>
where
    B: SplitByteSlice,
{
    fn from(frame: DataFrame<B>) -> Self {
        let fc = frame.fixed_fields.frame_ctrl;
        if fc.data_subtype().null() {
            MsduIterSubtype::Null
        } else if frame.qos_ctrl.is_some_and(|qos_ctrl| qos_ctrl.get().amsdu_present()) {
            MsduIterSubtype::Amsdu(BufferReader::new(frame.body))
        } else {
            MsduIterSubtype::Llc {
                dst_addr: data_dst_addr(&frame.fixed_fields),
                src_addr: data_src_addr(&frame.fixed_fields, frame.addr4.map(|addr4| *addr4))
                    .expect("failed to reparse data frame source address"),
                body: Some(frame.body),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac::{self, data};
    use crate::test_utils::fake_frames::*;
    use assert_matches::assert_matches;

    #[test]
    fn msdu_iter_non_aggregated() {
        let bytes = make_data_frame_single_llc(None, None);
        data::harness::assert_msdus_exactly_one_eq(
            mac::DataFrame::parse(bytes.as_slice(), false)
                .expect("failed to parse non-aggregated data frame"),
            mac::Msdu {
                dst_addr: mac::MacAddr::from([3; 6]),
                src_addr: mac::MacAddr::from([4; 6]),
                llc_frame: mac::LlcFrame {
                    hdr: Ref::from_bytes(&[7, 7, 7, 8, 8, 8, 9, 10][..]).unwrap(),
                    body: &[11; 3][..],
                },
            },
        );
    }

    #[test]
    fn msdu_iter_non_aggregated_with_padding() {
        let bytes = make_data_frame_with_padding();
        data::harness::assert_msdus_exactly_one_eq(
            mac::DataFrame::parse(bytes.as_slice(), true)
                .expect("failed to parse non-aggregated data frame with padding"),
            mac::Msdu {
                dst_addr: mac::MacAddr::from([3; 6]),
                src_addr: mac::MacAddr::from([4; 6]),
                llc_frame: mac::LlcFrame {
                    hdr: Ref::from_bytes(&[7, 7, 7, 8, 8, 8, 9, 10][..]).unwrap(),
                    body: &[11; 5][..],
                },
            },
        );
    }

    #[test]
    fn msdu_iter_null() {
        let bytes = make_null_data_frame();
        let data_frame = mac::DataFrame::parse(bytes.as_slice(), false)
            .expect("failed to parse NULL data frame");
        let n = data_frame.into_iter().count();
        assert_eq!(0, n, "expected no MSDUs, but read {}", n);
    }

    #[test]
    fn parse_llc_frame_with_addr4_ht_ctrl() {
        let bytes =
            make_data_frame_single_llc(Some(MacAddr::from([1, 2, 3, 4, 5, 6])), Some([4, 3, 2, 1]));
        assert_matches!(
            mac::DataFrame::parse(bytes.as_slice(), false),
            Some(mac::DataFrame { body, .. }) => {
                let llc_frame = LlcFrame::parse(body).expect("failed to parse LLC frame");
                assert_eq!(7, llc_frame.hdr.dsap);
                assert_eq!(7, llc_frame.hdr.ssap);
                assert_eq!(7, llc_frame.hdr.control);
                assert_eq!([8, 8, 8], llc_frame.hdr.oui);
                assert_eq!([9, 10], llc_frame.hdr.protocol_id.to_bytes());
                assert_eq!(0x090A, llc_frame.hdr.protocol_id.get());
                assert_eq!(&[11, 11, 11], llc_frame.body);
            },
            "failed to parse data frame");
    }
}
