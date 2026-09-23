// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mac::MacAddr;
use zerocopy::byteorder::big_endian::U16;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, Unaligned};

// RFC 704, Appendix B.2
// https://www.iana.org/assignments/ieee-802-numbers/ieee-802-numbers.xhtml
pub const ETHER_TYPE_EAPOL: u16 = 0x888E;
pub const ETHER_TYPE_IPV4: u16 = 0x0800;
pub const ETHER_TYPE_IPV6: u16 = 0x86DD;

pub const MAX_ETH_FRAME_LEN: usize = 2048;

// IEEE Std 802.3-2015, 3.1.1
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned, Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct EthernetIIHdr {
    pub da: MacAddr,
    pub sa: MacAddr,
    pub ether_type: U16,
}

pub struct EthernetFrame<B: SplitByteSlice> {
    pub hdr: Ref<B, EthernetIIHdr>,
    pub body: B,
}

impl<B: SplitByteSlice> EthernetFrame<B> {
    pub fn parse(bytes: B) -> Option<Self> {
        let (hdr, body) = Ref::from_prefix(bytes).ok()?;
        Some(Self { hdr, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eth_hdr_big_endian() {
        let mut bytes: Vec<u8> = vec![
            1, 2, 3, 4, 5, 6, // dst_addr
            7, 8, 9, 10, 11, 12, // src_addr
            13, 14, // ether_type
            99, 99, // trailing bytes
        ];
        let (mut hdr, body) = Ref::<_, EthernetIIHdr>::from_prefix(&mut bytes[..])
            .expect("cannot create ethernet header.");
        assert_eq!(hdr.da, MacAddr::from([1u8, 2, 3, 4, 5, 6]));
        assert_eq!(hdr.sa, MacAddr::from([7u8, 8, 9, 10, 11, 12]));
        assert_eq!(hdr.ether_type.get(), 13 << 8 | 14);
        assert_eq!(hdr.ether_type.to_bytes(), [13u8, 14]);
        assert_eq!(body, [99, 99]);

        hdr.ether_type.set(0x888e);
        assert_eq!(hdr.ether_type.to_bytes(), [0x88, 0x8e]);
        #[rustfmt::skip]
        assert_eq!(
            &[1u8, 2, 3, 4, 5, 6,
            7, 8, 9, 10, 11, 12,
            0x88, 0x8e,
            99, 99],
            &bytes[..]);
    }
}
