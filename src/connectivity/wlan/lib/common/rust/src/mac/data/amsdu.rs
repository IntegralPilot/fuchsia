// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::buffer_reader::BufferReader;
use crate::mac::{MacAddr, round_up};
use zerocopy::byteorder::big_endian::U16;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, Unaligned};

// IEEE Std 802.11-2016, 9.3.2.2.2
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
#[repr(C, packed)]
pub struct AmsduSubframeHdr {
    // Note this is the same as the IEEE 802.3 frame format.
    pub da: MacAddr,
    pub sa: MacAddr,
    pub msdu_len: U16,
}

pub struct AmsduSubframe<B> {
    pub hdr: Ref<B, AmsduSubframeHdr>,
    pub body: B,
}

/// Parse an A-MSDU subframe from the byte stream and advance the cursor in the `BufferReader` if
/// successful. Parsing is only successful if the byte stream starts with a valid subframe.
/// TODO(https://fxbug.dev/42104386): The received AMSDU should not be greater than `max_amsdu_len`, specified in
/// HtCapabilities IE of Association. Warn or discard if violated.
impl<B: SplitByteSlice> AmsduSubframe<B> {
    pub fn parse(buffer_reader: &mut BufferReader<B>) -> Option<Self> {
        let hdr = buffer_reader.read::<AmsduSubframeHdr>()?;
        let msdu_len = hdr.msdu_len.get() as usize;
        if buffer_reader.bytes_remaining() < msdu_len {
            None
        } else {
            let body = buffer_reader.read_bytes(msdu_len)?;
            let base_len = std::mem::size_of::<AmsduSubframeHdr>() + msdu_len;
            let padded_len = round_up(base_len, 4);
            let padding_len = padded_len - base_len;
            if buffer_reader.bytes_remaining() == 0 {
                Some(Self { hdr, body })
            } else if buffer_reader.bytes_remaining() <= padding_len {
                // The subframe is invalid if EITHER one of the following is true
                // a) there are not enough bytes in the buffer for padding
                // b) the remaining buffer only contains padding bytes
                // IEEE 802.11-2016 9.3.2.2.2 `The last A-MSDU subframe has no padding.`
                None
            } else {
                buffer_reader.read_bytes(padding_len)?;
                Some(Self { hdr, body })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mac::{self, data};
    use crate::test_utils::fake_frames::*;
    use zerocopy::Ref;

    #[test]
    fn msdu_iter_aggregated() {
        let bytes = make_data_frame_amsdu();
        data::harness::assert_msdus_llc_frame_eq(
            mac::DataFrame::parse(bytes.as_slice(), false)
                .expect("failed to parse aggregated data frame"),
            [
                mac::LlcFrame {
                    hdr: Ref::from_bytes(MSDU_1_LLC_HDR).unwrap(),
                    body: MSDU_1_PAYLOAD,
                },
                mac::LlcFrame {
                    hdr: Ref::from_bytes(MSDU_2_LLC_HDR).unwrap(),
                    body: MSDU_2_PAYLOAD,
                },
            ],
        );
    }

    #[test]
    fn msdu_iter_aggregated_with_padding_too_short() {
        let bytes = make_data_frame_amsdu_padding_too_short();
        data::harness::assert_msdus_llc_frame_eq(
            mac::DataFrame::parse(bytes.as_slice(), false)
                .expect("failed to parse aggregated data frame"),
            [mac::LlcFrame { hdr: Ref::from_bytes(MSDU_1_LLC_HDR).unwrap(), body: MSDU_1_PAYLOAD }],
        );
    }
}
