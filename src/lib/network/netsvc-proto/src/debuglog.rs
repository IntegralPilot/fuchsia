// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Debuglog messaging types.
//!
//! The debuglog protocol is used to multicast log messages.

use crate::ValidStr;
use packet::{
    BufferView, BufferViewMut, FragmentedBytesMut, InnerPacketBuilder, NestablePacketBuilder,
    PacketBuilder, PacketConstraints, ParsablePacket, ParseMetadata, SerializationContext,
    SerializeTarget,
};
use std::num::NonZeroU16;
use zerocopy::byteorder::little_endian::U32;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, Unaligned};

pub const MULTICAST_PORT: NonZeroU16 = NonZeroU16::new(33337).unwrap();
pub const ACK_PORT: NonZeroU16 = NonZeroU16::new(33338).unwrap();

const MAGIC: u32 = 0xAEAE1123;
const MAX_LOG_DATA: usize = 1216;
const MAX_NODENAME_LENGTH: usize = 64;

pub const ACK_SIZE: usize = std::mem::size_of::<PacketHead>();

#[repr(C)]
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Unaligned)]
pub struct PacketHead {
    magic: U32,
    seqno: U32,
}

pub struct DebugLogPacket<B> {
    head: Ref<B, PacketHead>,
    nodename: ValidStr<B>,
    data: ValidStr<B>,
}

impl<B: SplitByteSlice> DebugLogPacket<B> {
    pub fn seqno(&self) -> u32 {
        self.head.seqno.get()
    }

    pub fn nodename(&self) -> &str {
        self.nodename.as_str()
    }

    pub fn data(&self) -> &str {
        self.data.as_str()
    }
}

#[derive(Debug)]
pub enum ParseError {
    BadMagic,
    Malformed,
    Encoding(std::str::Utf8Error),
}

impl<B> ParsablePacket<B, ()> for DebugLogPacket<B>
where
    B: SplitByteSlice,
{
    type Error = ParseError;

    fn parse<BV: BufferView<B>>(mut buffer: BV, _args: ()) -> Result<Self, Self::Error> {
        let head = buffer.take_obj_front::<PacketHead>().ok_or(ParseError::Malformed)?;
        if head.magic.get() != MAGIC {
            return Err(ParseError::BadMagic);
        }
        let nodename = buffer.take_front(MAX_NODENAME_LENGTH).ok_or(ParseError::Malformed)?;
        let nodename = ValidStr::new(nodename).map_err(ParseError::Encoding)?;
        let (nodename, _rest) = nodename.truncate_null();
        let data = ValidStr::new(buffer.into_rest()).map_err(ParseError::Encoding)?;

        Ok(Self { head, nodename, data })
    }

    fn parse_metadata(&self) -> ParseMetadata {
        // we only need ParseMetadata to undo parsing, not necessary to implement it for now
        unimplemented!()
    }
}

#[derive(Debug)]
pub struct AckPacketBuilder {
    seqno: u32,
}

impl AckPacketBuilder {
    pub fn new(seqno: u32) -> Self {
        Self { seqno }
    }
}

impl InnerPacketBuilder for AckPacketBuilder {
    fn bytes_len(&self) -> usize {
        std::mem::size_of::<PacketHead>()
    }

    fn serialize(&self, mut buffer: &mut [u8]) {
        let mut bv = crate::as_buffer_view_mut(&mut buffer);
        let mut head = bv.take_obj_front::<PacketHead>().unwrap();
        head.magic.set(MAGIC);
        head.seqno.set(self.seqno);
    }
}

#[derive(Debug)]
pub struct LogPacketBuilder<'a> {
    seqno: u32,
    nodename: &'a str,
}

impl<'a> LogPacketBuilder<'a> {
    pub fn new(seqno: u32, nodename: &'a str) -> Option<Self> {
        if nodename.len() <= MAX_NODENAME_LENGTH { Some(Self { seqno, nodename }) } else { None }
    }
}

impl<'a> NestablePacketBuilder for LogPacketBuilder<'a> {
    fn constraints(&self) -> PacketConstraints {
        PacketConstraints::new(
            /* header_len */ std::mem::size_of::<PacketHead>() + MAX_NODENAME_LENGTH,
            /* footer_len */ 0,
            /* min_body_len */ 0,
            /* max_body_len */ MAX_LOG_DATA,
        )
    }
}

impl<'a, C: SerializationContext> PacketBuilder<C> for LogPacketBuilder<'a> {
    fn serialize(
        &self,
        _context: &mut C,
        target: &mut SerializeTarget<'_>,
        _body: FragmentedBytesMut<'_, '_>,
    ) {
        let mut bv = crate::as_buffer_view_mut(&mut target.header);
        let mut head = bv.take_obj_front::<PacketHead>().unwrap();
        head.magic.set(MAGIC);
        head.seqno.set(self.seqno);
        let nodename_bytes = self.nodename.as_bytes();
        let nodename = bv.take_front_zero(MAX_NODENAME_LENGTH).unwrap();
        let (nodename, _rest) = nodename.split_at_mut(nodename_bytes.len());
        nodename.copy_from_slice(nodename_bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use packet::{NoOpSerializationContext, ParseBuffer as _, Serializer as _};

    /// Helper to convince the compiler we're holding buffer views.
    fn as_buffer_view<'a, B: packet::BufferView<&'a [u8]>>(
        v: B,
    ) -> impl packet::BufferView<&'a [u8]> {
        v
    }

    #[test]
    fn test_log_packet() {
        const LOG_DATA: &'static str = "some log data";
        const NODENAME: &'static str = "my node";
        let mut log = LogPacketBuilder::new(3, NODENAME)
            .unwrap()
            .wrap_body(LOG_DATA.as_bytes().into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("Failed to serialize"));
        let packet = log.parse::<DebugLogPacket<_>>().expect("Failed to parse");
        assert_eq!(packet.seqno(), 3);
        assert_eq!(packet.nodename(), NODENAME);
        assert_eq!(packet.data(), LOG_DATA);
    }

    #[test]
    fn test_ack_packet() {
        const SEQNO: u32 = 4;
        let ack = AckPacketBuilder::new(SEQNO)
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap_or_else(|_| panic!("Failed to serialize"));
        let mut bv = ack.as_ref();
        let head = as_buffer_view(&mut bv)
            .take_obj_front::<PacketHead>()
            .expect("failed to get serialized head");
        assert_eq!(head.magic.get(), MAGIC);
        assert_eq!(head.seqno.get(), SEQNO);
    }
}
