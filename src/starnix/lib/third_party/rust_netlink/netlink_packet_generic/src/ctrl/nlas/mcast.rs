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

pub(crate) struct McastGroupList(Vec<McastGroup>);

impl Deref for McastGroupList {
    type Target = Vec<McastGroup>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&Vec<Vec<McastGrpAttrs>>> for McastGroupList {
    fn from(groups: &Vec<Vec<McastGrpAttrs>>) -> Self {
        Self(
            groups
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, nlas)| McastGroup { index: index as u16, nlas })
                .collect(),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McastGroup {
    pub index: u16,
    pub nlas: Vec<McastGrpAttrs>,
}

impl Nla for McastGroup {
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
pub enum McastGrpAttrs {
    Name(String),
    Id(u32),
}

impl Nla for McastGrpAttrs {
    fn value_len(&self) -> usize {
        use McastGrpAttrs::*;
        match self {
            Name(s) => s.as_bytes().len() + 1,
            Id(v) => size_of_val(v),
        }
    }

    fn kind(&self) -> u16 {
        use McastGrpAttrs::*;
        match self {
            Name(_) => CTRL_ATTR_MCAST_GRP_NAME,
            Id(_) => CTRL_ATTR_MCAST_GRP_ID,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use McastGrpAttrs::*;
        match self {
            Name(s) => {
                buffer[..s.len()].copy_from_slice(s.as_bytes());
                buffer[s.len()] = 0;
            }
            Id(v) => NativeEndian::write_u32(buffer, *v),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for McastGrpAttrs {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        Ok(match buf.kind() {
            CTRL_ATTR_MCAST_GRP_NAME => {
                Self::Name(parse_string(payload).context("invalid CTRL_ATTR_MCAST_GRP_NAME value")?)
            }
            CTRL_ATTR_MCAST_GRP_ID => {
                Self::Id(parse_u32(payload).context("invalid CTRL_ATTR_MCAST_GRP_ID value")?)
            }
            kind => return Err(DecodeError::from(format!("Unknown NLA type: {kind}"))),
        })
    }
}
