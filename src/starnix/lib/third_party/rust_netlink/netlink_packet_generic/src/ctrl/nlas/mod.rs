// SPDX-License-Identifier: MIT

use crate::constants::*;
use anyhow::Context;
use byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::DecodeError;
use netlink_packet_utils::nla::{Nla, NlaBuffer, NlasIterator};
use netlink_packet_utils::parsers::*;
use netlink_packet_utils::traits::*;
use std::mem::size_of_val;

mod mcast;
mod oppolicy;
mod ops;
mod policy;

pub use mcast::*;
pub use oppolicy::*;
pub use ops::*;
pub use policy::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenlCtrlAttrs {
    FamilyId(u16),
    FamilyName(String),
    Version(u32),
    HdrSize(u32),
    MaxAttr(u32),
    Ops(Vec<Vec<OpAttrs>>),
    McastGroups(Vec<Vec<McastGrpAttrs>>),
    Policy(PolicyAttr),
    OpPolicy(OppolicyAttr),
    Op(u32),
}

impl Nla for GenlCtrlAttrs {
    fn value_len(&self) -> usize {
        use GenlCtrlAttrs::*;
        match self {
            FamilyId(v) => size_of_val(v),
            FamilyName(s) => s.len() + 1,
            Version(v) => size_of_val(v),
            HdrSize(v) => size_of_val(v),
            MaxAttr(v) => size_of_val(v),
            Ops(nlas) => OpList::from(nlas).as_slice().buffer_len(),
            McastGroups(nlas) => McastGroupList::from(nlas).as_slice().buffer_len(),
            Policy(nla) => nla.buffer_len(),
            OpPolicy(nla) => nla.buffer_len(),
            Op(v) => size_of_val(v),
        }
    }

    fn kind(&self) -> u16 {
        use GenlCtrlAttrs::*;
        match self {
            FamilyId(_) => CTRL_ATTR_FAMILY_ID,
            FamilyName(_) => CTRL_ATTR_FAMILY_NAME,
            Version(_) => CTRL_ATTR_VERSION,
            HdrSize(_) => CTRL_ATTR_HDRSIZE,
            MaxAttr(_) => CTRL_ATTR_MAXATTR,
            Ops(_) => CTRL_ATTR_OPS,
            McastGroups(_) => CTRL_ATTR_MCAST_GROUPS,
            Policy(_) => CTRL_ATTR_POLICY,
            OpPolicy(_) => CTRL_ATTR_OP_POLICY,
            Op(_) => CTRL_ATTR_OP,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use GenlCtrlAttrs::*;
        match self {
            FamilyId(v) => NativeEndian::write_u16(buffer, *v),
            FamilyName(s) => {
                buffer[..s.len()].copy_from_slice(s.as_bytes());
                buffer[s.len()] = 0;
            }
            Version(v) => NativeEndian::write_u32(buffer, *v),
            HdrSize(v) => NativeEndian::write_u32(buffer, *v),
            MaxAttr(v) => NativeEndian::write_u32(buffer, *v),
            Ops(nlas) => {
                OpList::from(nlas).as_slice().emit(buffer);
            }
            McastGroups(nlas) => {
                McastGroupList::from(nlas).as_slice().emit(buffer);
            }
            Policy(nla) => nla.emit_value(buffer),
            OpPolicy(nla) => nla.emit_value(buffer),
            Op(v) => NativeEndian::write_u32(buffer, *v),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for GenlCtrlAttrs {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        Ok(match buf.kind() {
            CTRL_ATTR_FAMILY_ID => {
                Self::FamilyId(parse_u16(payload).context("invalid CTRL_ATTR_FAMILY_ID value")?)
            }
            CTRL_ATTR_FAMILY_NAME => Self::FamilyName(
                parse_string(payload).context("invalid CTRL_ATTR_FAMILY_NAME value")?,
            ),
            CTRL_ATTR_VERSION => {
                Self::Version(parse_u32(payload).context("invalid CTRL_ATTR_VERSION value")?)
            }
            CTRL_ATTR_HDRSIZE => {
                Self::HdrSize(parse_u32(payload).context("invalid CTRL_ATTR_HDRSIZE value")?)
            }
            CTRL_ATTR_MAXATTR => {
                Self::MaxAttr(parse_u32(payload).context("invalid CTRL_ATTR_MAXATTR value")?)
            }
            CTRL_ATTR_OPS => {
                let ops = NlasIterator::new(payload)
                    .map(|nlas| {
                        nlas.map_err(|err| DecodeError::from(err)).and_then(|nlas| {
                            NlasIterator::new(nlas.value())
                                .map(|nla| {
                                    nla.map_err(|err| DecodeError::from(err))
                                        .and_then(|nla| OpAttrs::parse(&nla))
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                    })
                    .collect::<Result<Vec<Vec<_>>, _>>()
                    .context("failed to parse CTRL_ATTR_OPS")?;
                Self::Ops(ops)
            }
            CTRL_ATTR_MCAST_GROUPS => {
                let groups = NlasIterator::new(payload)
                    .map(|nlas| {
                        nlas.map_err(|err| DecodeError::from(err)).and_then(|nlas| {
                            NlasIterator::new(nlas.value())
                                .map(|nla| {
                                    nla.map_err(|err| DecodeError::from(err))
                                        .and_then(|nla| McastGrpAttrs::parse(&nla))
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                    })
                    .collect::<Result<Vec<Vec<_>>, _>>()
                    .context("failed to parse CTRL_ATTR_MCAST_GROUPS")?;
                Self::McastGroups(groups)
            }
            CTRL_ATTR_POLICY => Self::Policy(
                PolicyAttr::parse(&NlaBuffer::new(payload)?)
                    .context("failed to parse CTRL_ATTR_POLICY")?,
            ),
            CTRL_ATTR_OP_POLICY => Self::OpPolicy(
                OppolicyAttr::parse(&NlaBuffer::new(payload)?)
                    .context("failed to parse CTRL_ATTR_OP_POLICY")?,
            ),
            CTRL_ATTR_OP => Self::Op(parse_u32(payload)?),
            kind => return Err(DecodeError::from(format!("Unknown NLA type: {kind}"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcast_groups_parse() {
        let mcast_bytes: [u8; 24] = [
            24, 0, // Netlink header length
            7, 0, // Netlink header kind (Mcast groups)
            20, 0, // Mcast group nested NLA length
            1, 0, // Mcast group kind
            8, 0, // Id length
            2, 0, // Id kind
            1, 0, 0, 0, // Id
            8, 0, // Name length
            1, 0, // Name kind
            b't', b'e', b's', b't', // Name
        ];
        let nla_buffer = NlaBuffer::new(&mcast_bytes[..]).expect("Failed to create NlaBuffer");
        let result_attr =
            GenlCtrlAttrs::parse(&nla_buffer).expect("Failed to parse encoded McastGroups");
        let expected_attr = GenlCtrlAttrs::McastGroups(vec![vec![
            McastGrpAttrs::Id(1),
            McastGrpAttrs::Name("test".to_string()),
        ]]);
        assert_eq!(expected_attr, result_attr);
    }

    #[test]
    fn mcast_groups_emit() {
        let mcast_attr = GenlCtrlAttrs::McastGroups(vec![
            vec![McastGrpAttrs::Id(7), McastGrpAttrs::Name("group1".to_string())],
            vec![McastGrpAttrs::Id(8), McastGrpAttrs::Name("group2".to_string())],
        ]);
        let expected_bytes: [u8; 52] = [
            52, 0, // Netlink header length
            7, 0, // Netlink header kind (Mcast groups)
            24, 0, // Mcast group nested NLA length
            1, 0, // Mcast group kind (index 1)
            8, 0, // Id length
            2, 0, // Id kind
            7, 0, 0, 0, // Id
            11, 0, // Name length
            1, 0, // Name kind
            b'g', b'r', b'o', b'u', b'p', b'1', 0, // Name
            0, // mcast group padding
            24, 0, // Mcast group nested NLA length
            2, 0, // Mcast group kind (index 2)
            8, 0, // Id length
            2, 0, // Id kind
            8, 0, 0, 0, // Id
            11, 0, // Name length
            1, 0, // Name kind
            b'g', b'r', b'o', b'u', b'p', b'2', 0, // Name
            0, // padding
        ];
        let mut buf = vec![0u8; 100];
        mcast_attr.emit(&mut buf);

        assert_eq!(&expected_bytes[..], &buf[..expected_bytes.len()]);
    }

    #[test]
    fn ops_parse() {
        let ops_bytes: [u8; 24] = [
            24, 0, // Netlink header length
            6, 0, // Netlink header kind (Ops)
            20, 0, // Op nested NLA length
            0, 0, // Op kind
            8, 0, // Id length
            1, 0, // Id kind
            1, 0, 0, 0, // Id
            8, 0, // Flags length
            2, 0, // Flags kind
            123, 0, 0, 0, // Flags
        ];
        let nla_buffer = NlaBuffer::new(&ops_bytes[..]).expect("Failed to create NlaBuffer");
        let result_attr =
            GenlCtrlAttrs::parse(&nla_buffer).expect("Failed to parse encoded McastGroups");
        let expected_attr = GenlCtrlAttrs::Ops(vec![vec![OpAttrs::Id(1), OpAttrs::Flags(123)]]);
        assert_eq!(expected_attr, result_attr);
    }

    #[test]
    fn ops_emit() {
        let ops = GenlCtrlAttrs::Ops(vec![
            vec![OpAttrs::Id(1), OpAttrs::Flags(11)],
            vec![OpAttrs::Id(3), OpAttrs::Flags(33)],
        ]);
        let expected_bytes: [u8; 44] = [
            44, 0, // Netlink header length
            6, 0, // Netlink header kind (Ops)
            20, 0, // Op nested NLA length
            1, 0, // Op kind
            8, 0, // Id length
            1, 0, // Id kind
            1, 0, 0, 0, // Id
            8, 0, // Flags length
            2, 0, // Flags kind
            11, 0, 0, 0, // Flags
            20, 0, // Op nested NLA length
            2, 0, // Op kind
            8, 0, // Id length
            1, 0, // Id kind
            3, 0, 0, 0, // Id
            8, 0, // Flags length
            2, 0, // Flags kind
            33, 0, 0, 0, // Flags
        ];
        let mut buf = vec![0u8; 100];
        ops.emit(&mut buf);

        assert_eq!(&expected_bytes[..], &buf[..expected_bytes.len()]);
    }
}
