// SPDX-License-Identifier: MIT

use crate::constants::*;
use anyhow::Context;
use byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::{Nla, NlaBuffer};
use netlink_packet_utils::parsers::*;
use netlink_packet_utils::traits::*;
use std::mem::size_of_val;
use std::ops::Deref;

pub struct OpList(Vec<Op>);

impl Deref for OpList {
    type Target = Vec<Op>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&Vec<Vec<OpAttrs>>> for OpList {
    fn from(ops: &Vec<Vec<OpAttrs>>) -> Self {
        Self(
            ops.iter()
                .cloned()
                .enumerate()
                .map(|(index, nlas)| Op { index: index as u16, nlas })
                .collect(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub index: u16,
    pub nlas: Vec<OpAttrs>,
}

impl Nla for Op {
    fn value_len(&self) -> usize {
        self.nlas.as_slice().buffer_len()
    }

    fn kind(&self) -> u16 {
        self.index + 1
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        self.nlas.as_slice().emit(buffer);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpAttrs {
    Id(u32),
    Flags(u32),
}

impl Nla for OpAttrs {
    fn value_len(&self) -> usize {
        use OpAttrs::*;
        match self {
            Id(v) => size_of_val(v),
            Flags(v) => size_of_val(v),
        }
    }

    fn kind(&self) -> u16 {
        use OpAttrs::*;
        match self {
            Id(_) => CTRL_ATTR_OP_ID,
            Flags(_) => CTRL_ATTR_OP_FLAGS,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use OpAttrs::*;
        match self {
            Id(v) => NativeEndian::write_u32(buffer, *v),
            Flags(v) => NativeEndian::write_u32(buffer, *v),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for OpAttrs {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        Ok(match buf.kind() {
            CTRL_ATTR_OP_ID => {
                Self::Id(parse_u32(payload).context("invalid CTRL_ATTR_OP_ID value")?)
            }
            CTRL_ATTR_OP_FLAGS => {
                Self::Flags(parse_u32(payload).context("invalid CTRL_ATTR_OP_FLAGS value")?)
            }
            kind => return Err(DecodeError::from(format!("Unknown NLA type: {kind}"))),
        })
    }
}
