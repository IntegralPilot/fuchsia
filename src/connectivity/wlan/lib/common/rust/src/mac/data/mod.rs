// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod amsdu;
mod fields;
mod msdu;

pub use amsdu::*;
pub use fields::*;
pub use msdu::*;

/// Test harnesses and fixtures.
#[cfg(test)]
pub mod harness {
    use super::*;
    use itertools::{EitherOrBoth, Itertools};
    use std::fmt::Debug;
    use zerocopy::SplitByteSlice;

    pub fn assert_msdus_exactly_one_eq<M, E>(
        msdus: impl IntoIterator<Item = Msdu<M>>,
        expected: Msdu<E>,
    ) where
        M: SplitByteSlice + Debug,
        E: SplitByteSlice + Debug,
    {
        let mut msdus = msdus.into_iter().peekable();
        let Msdu { dst_addr, src_addr, llc_frame } =
            msdus.next().expect("expected exactly one MSDU, but got zero");
        assert_eq!(dst_addr, expected.dst_addr);
        assert_eq!(src_addr, expected.src_addr);
        assert_eq!(llc_frame.hdr.protocol_id, expected.llc_frame.hdr.protocol_id);
        assert_eq!(&*llc_frame.body, &*expected.llc_frame.body);
        if msdus.peek().is_some() {
            panic!(
                "expected exactly one MSDU, but got more\n{}",
                format!("unexpected MSDU: {:x?}", msdus.map(Msdu::into_body).format("\n"))
            );
        }
    }

    pub fn assert_msdus_llc_frame_eq<M, E>(
        msdus: impl IntoIterator<Item = Msdu<M>>,
        expected: impl IntoIterator<Item = LlcFrame<E>>,
    ) where
        M: SplitByteSlice + Debug,
        E: SplitByteSlice + Debug,
    {
        let mut msdus = msdus.into_iter().zip_longest(expected).peekable();
        while let Some(EitherOrBoth::Both(Msdu { llc_frame, .. }, expected)) = msdus.peek() {
            // Only LLC frame data is asserted here. Other `Msdu` fixture fields are not exposed to
            // tests, but these fields are validated by other assertions.
            assert_eq!(llc_frame.hdr.protocol_id, expected.hdr.protocol_id);
            assert_eq!(&*llc_frame.body, &*expected.body);
            msdus.next();
        }
        if let Some(zipped) = msdus.peek() {
            let difference = if zipped.is_left() { "more" } else { "fewer" };
            panic!(
                "expected some number of MSDUs, but got {}\n{}",
                difference,
                msdus
                    .map(|zipped| match zipped {
                        EitherOrBoth::Left(msdu) =>
                            format!("unexpected MSDU: {:x?}", msdu.into_body()),
                        EitherOrBoth::Right(expected) =>
                            format!("expected MSDU (not found): {:x?}", expected.into_body()),
                        _ => unreachable!(),
                    })
                    .join("\n"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mac;
    use crate::test_utils::fake_frames::*;
    use assert_matches::assert_matches;

    #[test]
    fn parse_data_frame() {
        let bytes = make_data_frame_single_llc(None, None);
        assert_matches!(
            mac::DataFrame::parse(bytes.as_slice(), false),
            Some(mac::DataFrame { fixed_fields, addr4, qos_ctrl, ht_ctrl, body }) => {
                assert_eq!(0b00000000_10001000, { fixed_fields.frame_ctrl.0 });
                assert_eq!(0x0202, { fixed_fields.duration });
                assert_eq!(mac::MacAddr::from([3, 3, 3, 3, 3, 3]), fixed_fields.addr1);
                assert_eq!(mac::MacAddr::from([4, 4, 4, 4, 4, 4]), fixed_fields.addr2);
                assert_eq!(mac::MacAddr::from([5, 5, 5, 5, 5, 5]), fixed_fields.addr3);
                assert_eq!(0x0606, { fixed_fields.seq_ctrl.0 });
                assert!(addr4.is_none());
                assert_eq!(qos_ctrl.expect("qos_ctrl not present").get().0, 0x0101);
                assert!(ht_ctrl.is_none());
                assert_eq!(&body[..], &[7, 7, 7, 8, 8, 8, 9, 10, 11, 11, 11]);
            },
            "failed to parse data frame");
    }

    #[test]
    fn parse_data_frame_with_padding() {
        let bytes = make_data_frame_with_padding();
        assert_matches!(
            mac::DataFrame::parse(bytes.as_slice(), true),
            Some(mac::DataFrame { qos_ctrl, body, .. }) => {
                assert_eq!(qos_ctrl.expect("qos_ctrl not present").get().0, 0x0101);
                assert_eq!(&body[..], &[7, 7, 7, 8, 8, 8, 9, 10, 11, 11, 11, 11, 11]);
            },
            "failed to parse padded data frame");
    }
}
