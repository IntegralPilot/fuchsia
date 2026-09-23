// SPDX-License-Identifier: MIT

use crate::address::{AddressError, AddressHeader, AddressMessage, AddressMessageBuffer};
use crate::link::{LinkMessage, LinkMessageBuffer};
use crate::neighbour::{NeighbourError, NeighbourMessage, NeighbourMessageBuffer};
use crate::neighbour_discovery_user_option::{
    NeighbourDiscoveryUserOptionError, NeighbourDiscoveryUserOptionMessage,
    NeighbourDiscoveryUserOptionMessageBuffer,
};
use crate::neighbour_table::{
    NeighbourTableError, NeighbourTableMessage, NeighbourTableMessageBuffer,
};
use crate::nsid::{NsidError, NsidMessage, NsidMessageBuffer};
use crate::prefix::{PrefixError, PrefixMessage, PrefixMessageBuffer};
use crate::route::{RouteError, RouteHeader, RouteMessage, RouteMessageBuffer};
use crate::rule::{RuleError, RuleMessage, RuleMessageBuffer};
use crate::tc::{TcError, TcMessage, TcMessageBuffer};
use netlink_packet_core::{
    NetlinkDeserializable, NetlinkHeader, NetlinkPayload, NetlinkSerializable,
};
use netlink_packet_utils::nla::NlaParseMode;
use netlink_packet_utils::{DecodeError, Emitable, ParseableParametrized};
use thiserror::Error;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_SETLINK: u16 = 19;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;
const RTM_GETROUTE: u16 = 26;
const RTM_NEWNEIGH: u16 = 28;
const RTM_DELNEIGH: u16 = 29;
const RTM_GETNEIGH: u16 = 30;
const RTM_NEWRULE: u16 = 32;
const RTM_DELRULE: u16 = 33;
const RTM_GETRULE: u16 = 34;
const RTM_NEWQDISC: u16 = 36;
const RTM_DELQDISC: u16 = 37;
const RTM_GETQDISC: u16 = 38;
const RTM_NEWTCLASS: u16 = 40;
const RTM_DELTCLASS: u16 = 41;
const RTM_GETTCLASS: u16 = 42;
const RTM_NEWTFILTER: u16 = 44;
const RTM_DELTFILTER: u16 = 45;
const RTM_GETTFILTER: u16 = 46;
// const RTM_NEWACTION: u16 = 48;
// const RTM_DELACTION: u16 = 49;
// const RTM_GETACTION: u16 = 50;
const RTM_NEWPREFIX: u16 = 52;
// const RTM_GETMULTICAST: u16 = 58;
// const RTM_GETANYCAST: u16 = 62;
const RTM_NEWNEIGHTBL: u16 = 64;
const RTM_GETNEIGHTBL: u16 = 66;
const RTM_SETNEIGHTBL: u16 = 67;
const RTM_NEWNDUSEROPT: u16 = 68;
// const RTM_NEWADDRLABEL: u16 = 72;
// const RTM_DELADDRLABEL: u16 = 73;
// const RTM_GETADDRLABEL: u16 = 74;
// const RTM_GETDCB: u16 = 78;
// const RTM_SETDCB: u16 = 79;
// const RTM_NEWNETCONF: u16 = 80;
// const RTM_DELNETCONF: u16 = 81;
// const RTM_GETNETCONF: u16 = 82;
// const RTM_NEWMDB: u16 = 84;
// const RTM_DELMDB: u16 = 85;
// const RTM_GETMDB: u16 = 86;
const RTM_NEWNSID: u16 = 88;
const RTM_DELNSID: u16 = 89;
const RTM_GETNSID: u16 = 90;
// const RTM_NEWSTATS: u16 = 92;
// const RTM_GETSTATS: u16 = 94;
// const RTM_NEWCACHEREPORT: u16 = 96;
const RTM_NEWCHAIN: u16 = 100;
const RTM_DELCHAIN: u16 = 101;
const RTM_GETCHAIN: u16 = 102;
const RTM_NEWLINKPROP: u16 = 108;
const RTM_DELLINKPROP: u16 = 109;

buffer!(RouteNetlinkMessageBuffer);

#[derive(Debug, Error)]
pub enum RouteNetlinkMessageParseError {
    #[error("Invalid link message")]
    InvalidLinkMessage(#[source] DecodeError),

    #[error(transparent)]
    InvalidRouteMessage(#[from] RouteError),

    #[error(transparent)]
    InvalidAddrMessage(#[from] AddressError),

    #[error(transparent)]
    InvalidPrefixMessage(#[from] PrefixError),

    #[error(transparent)]
    InvalidFibRuleMessage(#[from] RuleError),

    #[error(transparent)]
    InvalidTcMessage(#[from] TcError),

    #[error(transparent)]
    InvalidNsidMessage(#[from] NsidError),

    #[error(transparent)]
    InvalidNeighbourMessage(#[from] NeighbourError),

    #[error(transparent)]
    InvalidNeighbourTableMessage(#[from] NeighbourTableError),

    #[error(transparent)]
    InvalidNeighbourDiscoveryUserOptionMessage(#[from] NeighbourDiscoveryUserOptionError),

    #[error("Unknown message type: {0}")]
    UnknownMessageType(u16),

    #[error("Parse buffer: {0}")]
    ParseBuffer(#[source] DecodeError),
}

impl From<RouteNetlinkMessageParseError> for DecodeError {
    fn from(e: RouteNetlinkMessageParseError) -> Self {
        DecodeError::Other(e.into())
    }
}

/// Describes how to handle errors when parsing `NETLINK_ROUTE` messages.
#[derive(Default)]
pub enum RouteNetlinkMessageParseMode {
    /// Parsing fails if fields are not correctly initialized. Corresponds to
    /// the `NETLINK_GET_STRICT_CHK` option.
    #[default]
    Strict,
    /// Parsing may not fail if e.g. irrelevant fields are set or
    /// unknown/invalid attributes are provided.
    Relaxed,
}

impl From<RouteNetlinkMessageParseMode> for NlaParseMode {
    fn from(mode: RouteNetlinkMessageParseMode) -> Self {
        match mode {
            RouteNetlinkMessageParseMode::Strict => NlaParseMode::Strict,
            RouteNetlinkMessageParseMode::Relaxed => NlaParseMode::Relaxed,
        }
    }
}

struct RouteNetlinkMessageParseConfig {
    message_type: u16,
    parse_mode: RouteNetlinkMessageParseMode,
}

impl<'a, T: AsRef<[u8]> + ?Sized>
    ParseableParametrized<RouteNetlinkMessageBuffer<&'a T>, RouteNetlinkMessageParseConfig>
    for RouteNetlinkMessage
{
    type Error = RouteNetlinkMessageParseError;

    fn parse_with_param(
        buf: &RouteNetlinkMessageBuffer<&'a T>,
        RouteNetlinkMessageParseConfig { message_type, parse_mode }: RouteNetlinkMessageParseConfig,
    ) -> Result<Self, Self::Error> {
        let message = match message_type {
            // Link messages
            RTM_NEWLINK | RTM_GETLINK | RTM_DELLINK | RTM_SETLINK => {
                let msg = match LinkMessageBuffer::new(&buf.inner()) {
                    Ok(buf) => LinkMessage::parse_with_param(&buf, parse_mode)
                        .map_err(RouteNetlinkMessageParseError::InvalidLinkMessage)?,
                    // HACK: iproute2 sends invalid RTM_GETLINK message, where
                    // the header is limited to the
                    // interface family (1 byte) and 3 bytes of padding.
                    Err(e) => {
                        if buf.inner().len() == 4 && message_type == RTM_GETLINK {
                            let mut msg = LinkMessage::default();
                            msg.header.interface_family = buf.inner()[0].into();
                            msg
                        } else {
                            return Err(RouteNetlinkMessageParseError::InvalidLinkMessage(e));
                        }
                    }
                };
                match message_type {
                    RTM_NEWLINK => RouteNetlinkMessage::NewLink(msg),
                    RTM_GETLINK => RouteNetlinkMessage::GetLink(msg),
                    RTM_DELLINK => RouteNetlinkMessage::DelLink(msg),
                    RTM_SETLINK => RouteNetlinkMessage::SetLink(msg),
                    _ => unreachable!(),
                }
            }

            // Address messages
            RTM_NEWADDR | RTM_GETADDR | RTM_DELADDR => {
                let msg = match AddressMessageBuffer::new(&buf.inner()) {
                    Ok(buffer) => AddressMessage::parse_with_param(&buffer, parse_mode)?,
                    // HACK: iproute2 sends invalid RTM_GETADDR message, where
                    // the header is limited to the
                    // interface family (1 byte) and 3 bytes of padding.
                    Err(e) => {
                        if buf.inner().len() == 4 && message_type == RTM_GETADDR {
                            let mut msg = AddressMessage {
                                header: AddressHeader::default(),
                                attributes: vec![],
                            };
                            msg.header.family = buf.inner()[0].into();
                            msg
                        } else {
                            return Err(RouteNetlinkMessageParseError::InvalidAddrMessage(
                                AddressError::FailedBufferInit(e),
                            ));
                        }
                    }
                };
                match message_type {
                    RTM_NEWADDR => RouteNetlinkMessage::NewAddress(msg),
                    RTM_GETADDR => RouteNetlinkMessage::GetAddress(msg),
                    RTM_DELADDR => RouteNetlinkMessage::DelAddress(msg),
                    _ => unreachable!(),
                }
            }

            // Neighbour messages
            RTM_NEWNEIGH | RTM_GETNEIGH | RTM_DELNEIGH => {
                let buf_inner = buf.inner();
                let buffer = NeighbourMessageBuffer::new(&buf_inner)
                    .map_err(RouteNetlinkMessageParseError::ParseBuffer)?;
                let msg = NeighbourMessage::parse_with_param(&buffer, parse_mode)?;
                match message_type {
                    RTM_GETNEIGH => RouteNetlinkMessage::GetNeighbour(msg),
                    RTM_NEWNEIGH => RouteNetlinkMessage::NewNeighbour(msg),
                    RTM_DELNEIGH => RouteNetlinkMessage::DelNeighbour(msg),
                    _ => unreachable!(),
                }
            }

            // Neighbour table messages
            RTM_NEWNEIGHTBL | RTM_GETNEIGHTBL | RTM_SETNEIGHTBL => {
                let buf_inner = buf.inner();
                let buffer = NeighbourTableMessageBuffer::new(&buf_inner)
                    .map_err(RouteNetlinkMessageParseError::ParseBuffer)?;
                let msg = NeighbourTableMessage::parse_with_param(&buffer, parse_mode)
                    .map_err(RouteNetlinkMessageParseError::InvalidNeighbourTableMessage)?;
                match message_type {
                    RTM_GETNEIGHTBL => RouteNetlinkMessage::GetNeighbourTable(msg),
                    RTM_NEWNEIGHTBL => RouteNetlinkMessage::NewNeighbourTable(msg),
                    RTM_SETNEIGHTBL => RouteNetlinkMessage::SetNeighbourTable(msg),
                    _ => unreachable!(),
                }
            }

            RTM_NEWNDUSEROPT => {
                let msg = NeighbourDiscoveryUserOptionMessage::parse_with_param(
                    &NeighbourDiscoveryUserOptionMessageBuffer::new(&buf.inner())
                        .map_err(NeighbourDiscoveryUserOptionError::ParseBuffer)?,
                    parse_mode,
                )?;
                RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(msg)
            }

            // Route messages
            RTM_NEWROUTE | RTM_GETROUTE | RTM_DELROUTE => {
                let msg = match RouteMessageBuffer::new(&buf.inner()) {
                    Ok(buffer) => RouteMessage::parse_with_param(&buffer, parse_mode)?,
                    // HACK: iproute2 sends invalid RTM_GETROUTE message, where
                    // the header is limited to the
                    // interface family (1 byte) and 3 bytes of padding.
                    Err(e) => {
                        // Not only does iproute2 sends invalid messages, it's
                        // also inconsistent in
                        // doing so: for link and address messages, the length
                        // advertised in the
                        // netlink header includes the 3 bytes of padding but it
                        // does not seem to be the case
                        // for the route message, hence the buf.length() == 1
                        // check.
                        if (buf.inner().len() == 4 || buf.inner().len() == 1)
                            && message_type == RTM_GETROUTE
                        {
                            let mut msg =
                                RouteMessage { header: RouteHeader::default(), attributes: vec![] };
                            msg.header.address_family = buf.inner()[0].into();
                            msg
                        } else {
                            return Err(RouteNetlinkMessageParseError::ParseBuffer(e));
                        }
                    }
                };
                match message_type {
                    RTM_NEWROUTE => RouteNetlinkMessage::NewRoute(msg),
                    RTM_GETROUTE => RouteNetlinkMessage::GetRoute(msg),
                    RTM_DELROUTE => RouteNetlinkMessage::DelRoute(msg),
                    _ => unreachable!(),
                }
            }

            // Prefix messages
            RTM_NEWPREFIX => {
                let buf_inner = buf.inner();
                let buffer = PrefixMessageBuffer::new(&buf_inner)
                    .map_err(RouteNetlinkMessageParseError::ParseBuffer)?;
                RouteNetlinkMessage::NewPrefix(PrefixMessage::parse_with_param(
                    &buffer, parse_mode,
                )?)
            }
            RTM_NEWRULE | RTM_GETRULE | RTM_DELRULE => {
                let buf_inner = buf.inner();
                let buffer = RuleMessageBuffer::new(&buf_inner)
                    .map_err(RouteNetlinkMessageParseError::ParseBuffer)?;
                let msg = RuleMessage::parse_with_param(&buffer, parse_mode)?;
                match message_type {
                    RTM_NEWRULE => RouteNetlinkMessage::NewRule(msg),
                    RTM_DELRULE => RouteNetlinkMessage::DelRule(msg),
                    RTM_GETRULE => RouteNetlinkMessage::GetRule(msg),
                    _ => unreachable!(),
                }
            }
            // TC Messages
            RTM_NEWQDISC | RTM_DELQDISC | RTM_GETQDISC | RTM_NEWTCLASS | RTM_DELTCLASS
            | RTM_GETTCLASS | RTM_NEWTFILTER | RTM_DELTFILTER | RTM_GETTFILTER | RTM_NEWCHAIN
            | RTM_DELCHAIN | RTM_GETCHAIN => {
                let buf_inner = buf.inner();
                let buffer = TcMessageBuffer::new(&buf_inner)
                    .map_err(RouteNetlinkMessageParseError::ParseBuffer)?;
                let msg = TcMessage::parse_with_param(&buffer, parse_mode)?;
                match message_type {
                    RTM_NEWQDISC => RouteNetlinkMessage::NewQueueDiscipline(msg),
                    RTM_DELQDISC => RouteNetlinkMessage::DelQueueDiscipline(msg),
                    RTM_GETQDISC => RouteNetlinkMessage::GetQueueDiscipline(msg),
                    RTM_NEWTCLASS => RouteNetlinkMessage::NewTrafficClass(msg),
                    RTM_DELTCLASS => RouteNetlinkMessage::DelTrafficClass(msg),
                    RTM_GETTCLASS => RouteNetlinkMessage::GetTrafficClass(msg),
                    RTM_NEWTFILTER => RouteNetlinkMessage::NewTrafficFilter(msg),
                    RTM_DELTFILTER => RouteNetlinkMessage::DelTrafficFilter(msg),
                    RTM_GETTFILTER => RouteNetlinkMessage::GetTrafficFilter(msg),
                    RTM_NEWCHAIN => RouteNetlinkMessage::NewTrafficChain(msg),
                    RTM_DELCHAIN => RouteNetlinkMessage::DelTrafficChain(msg),
                    RTM_GETCHAIN => RouteNetlinkMessage::GetTrafficChain(msg),
                    _ => unreachable!(),
                }
            }

            // ND ID Messages
            RTM_NEWNSID | RTM_GETNSID | RTM_DELNSID => {
                let buf_inner = buf.inner();
                let buffer = NsidMessageBuffer::new(&buf_inner)
                    .map_err(RouteNetlinkMessageParseError::ParseBuffer)?;
                let msg = NsidMessage::parse_with_param(&buffer, parse_mode)?;
                match message_type {
                    RTM_NEWNSID => RouteNetlinkMessage::NewNsId(msg),
                    RTM_DELNSID => RouteNetlinkMessage::DelNsId(msg),
                    RTM_GETNSID => RouteNetlinkMessage::GetNsId(msg),
                    _ => unreachable!(),
                }
            }

            _ => return Err(RouteNetlinkMessageParseError::UnknownMessageType(message_type)),
        };
        Ok(message)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RouteNetlinkMessage {
    NewLink(LinkMessage),
    DelLink(LinkMessage),
    GetLink(LinkMessage),
    SetLink(LinkMessage),
    NewLinkProp(LinkMessage),
    DelLinkProp(LinkMessage),
    NewAddress(AddressMessage),
    DelAddress(AddressMessage),
    GetAddress(AddressMessage),
    NewNeighbour(NeighbourMessage),
    GetNeighbour(NeighbourMessage),
    DelNeighbour(NeighbourMessage),
    NewNeighbourTable(NeighbourTableMessage),
    GetNeighbourTable(NeighbourTableMessage),
    SetNeighbourTable(NeighbourTableMessage),
    NewNeighbourDiscoveryUserOption(NeighbourDiscoveryUserOptionMessage),
    NewRoute(RouteMessage),
    DelRoute(RouteMessage),
    GetRoute(RouteMessage),
    NewPrefix(PrefixMessage),
    NewQueueDiscipline(TcMessage),
    DelQueueDiscipline(TcMessage),
    GetQueueDiscipline(TcMessage),
    NewTrafficClass(TcMessage),
    DelTrafficClass(TcMessage),
    GetTrafficClass(TcMessage),
    NewTrafficFilter(TcMessage),
    DelTrafficFilter(TcMessage),
    GetTrafficFilter(TcMessage),
    NewTrafficChain(TcMessage),
    DelTrafficChain(TcMessage),
    GetTrafficChain(TcMessage),
    NewNsId(NsidMessage),
    DelNsId(NsidMessage),
    GetNsId(NsidMessage),
    NewRule(RuleMessage),
    DelRule(RuleMessage),
    GetRule(RuleMessage),
}

impl RouteNetlinkMessage {
    pub fn is_new_link(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewLink(_))
    }

    pub fn is_del_link(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelLink(_))
    }

    pub fn is_get_link(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetLink(_))
    }

    pub fn is_set_link(&self) -> bool {
        matches!(self, RouteNetlinkMessage::SetLink(_))
    }

    pub fn is_new_address(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewAddress(_))
    }

    pub fn is_del_address(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelAddress(_))
    }

    pub fn is_get_address(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetAddress(_))
    }

    pub fn is_get_neighbour(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetNeighbour(_))
    }

    pub fn is_new_route(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewRoute(_))
    }

    pub fn is_new_neighbour(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewNeighbour(_))
    }

    pub fn is_get_route(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetRoute(_))
    }

    pub fn is_del_neighbour(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelNeighbour(_))
    }

    pub fn is_new_neighbour_table(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewNeighbourTable(_))
    }

    pub fn is_get_neighbour_table(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetNeighbourTable(_))
    }

    pub fn is_set_neighbour_table(&self) -> bool {
        matches!(self, RouteNetlinkMessage::SetNeighbourTable(_))
    }

    pub fn is_del_route(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelRoute(_))
    }

    pub fn is_new_qdisc(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewQueueDiscipline(_))
    }

    pub fn is_del_qdisc(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelQueueDiscipline(_))
    }

    pub fn is_get_qdisc(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetQueueDiscipline(_))
    }

    pub fn is_new_class(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewTrafficClass(_))
    }

    pub fn is_del_class(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelTrafficClass(_))
    }

    pub fn is_get_class(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetTrafficClass(_))
    }

    pub fn is_new_filter(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewTrafficFilter(_))
    }

    pub fn is_del_filter(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelTrafficFilter(_))
    }

    pub fn is_get_filter(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetTrafficFilter(_))
    }

    pub fn is_new_chain(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewTrafficChain(_))
    }

    pub fn is_del_chain(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelTrafficChain(_))
    }

    pub fn is_get_chain(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetTrafficChain(_))
    }

    pub fn is_new_nsid(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewNsId(_))
    }

    pub fn is_get_nsid(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetNsId(_))
    }

    pub fn is_del_nsid(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelNsId(_))
    }

    pub fn is_get_rule(&self) -> bool {
        matches!(self, RouteNetlinkMessage::GetRule(_))
    }

    pub fn is_new_rule(&self) -> bool {
        matches!(self, RouteNetlinkMessage::NewRule(_))
    }

    pub fn is_del_rule(&self) -> bool {
        matches!(self, RouteNetlinkMessage::DelRule(_))
    }

    pub fn message_type(&self) -> u16 {
        use self::RouteNetlinkMessage::*;

        match self {
            NewLink(_) => RTM_NEWLINK,
            DelLink(_) => RTM_DELLINK,
            GetLink(_) => RTM_GETLINK,
            SetLink(_) => RTM_SETLINK,
            NewLinkProp(_) => RTM_NEWLINKPROP,
            DelLinkProp(_) => RTM_DELLINKPROP,
            NewAddress(_) => RTM_NEWADDR,
            DelAddress(_) => RTM_DELADDR,
            GetAddress(_) => RTM_GETADDR,
            GetNeighbour(_) => RTM_GETNEIGH,
            NewNeighbour(_) => RTM_NEWNEIGH,
            DelNeighbour(_) => RTM_DELNEIGH,
            GetNeighbourTable(_) => RTM_GETNEIGHTBL,
            NewNeighbourTable(_) => RTM_NEWNEIGHTBL,
            SetNeighbourTable(_) => RTM_SETNEIGHTBL,
            NewNeighbourDiscoveryUserOption(_) => RTM_NEWNDUSEROPT,
            NewRoute(_) => RTM_NEWROUTE,
            DelRoute(_) => RTM_DELROUTE,
            GetRoute(_) => RTM_GETROUTE,
            NewPrefix(_) => RTM_NEWPREFIX,
            NewQueueDiscipline(_) => RTM_NEWQDISC,
            DelQueueDiscipline(_) => RTM_DELQDISC,
            GetQueueDiscipline(_) => RTM_GETQDISC,
            NewTrafficClass(_) => RTM_NEWTCLASS,
            DelTrafficClass(_) => RTM_DELTCLASS,
            GetTrafficClass(_) => RTM_GETTCLASS,
            NewTrafficFilter(_) => RTM_NEWTFILTER,
            DelTrafficFilter(_) => RTM_DELTFILTER,
            GetTrafficFilter(_) => RTM_GETTFILTER,
            NewTrafficChain(_) => RTM_NEWCHAIN,
            DelTrafficChain(_) => RTM_DELCHAIN,
            GetTrafficChain(_) => RTM_GETCHAIN,
            GetNsId(_) => RTM_GETNSID,
            NewNsId(_) => RTM_NEWNSID,
            DelNsId(_) => RTM_DELNSID,
            GetRule(_) => RTM_GETRULE,
            NewRule(_) => RTM_NEWRULE,
            DelRule(_) => RTM_DELRULE,
        }
    }
}

impl Emitable for RouteNetlinkMessage {
    #[rustfmt::skip]
    fn buffer_len(&self) -> usize {
        use self::RouteNetlinkMessage::*;
        match self {
            | NewLink(msg)
            | DelLink(msg)
            | GetLink(msg)
            | SetLink(msg)
            | NewLinkProp(msg)
            | DelLinkProp(msg)
            =>  msg.buffer_len(),

            | NewAddress(msg)
            | DelAddress(msg)
            | GetAddress(msg)
            => msg.buffer_len(),

            | NewNeighbour(msg)
            | GetNeighbour(msg)
            | DelNeighbour(msg)
            => msg.buffer_len(),

            | NewNeighbourTable(msg)
            | GetNeighbourTable(msg)
            | SetNeighbourTable(msg)
            => msg.buffer_len(),

            | NewNeighbourDiscoveryUserOption(msg) => msg.buffer_len(),

            | NewRoute(msg)
            | DelRoute(msg)
            | GetRoute(msg)
            => msg.buffer_len(),

            NewPrefix(msg) => msg.buffer_len(),

            | NewQueueDiscipline(msg)
            | DelQueueDiscipline(msg)
            | GetQueueDiscipline(msg)
            | NewTrafficClass(msg)
            | DelTrafficClass(msg)
            | GetTrafficClass(msg)
            | NewTrafficFilter(msg)
            | DelTrafficFilter(msg)
            | GetTrafficFilter(msg)
            | NewTrafficChain(msg)
            | DelTrafficChain(msg)
            | GetTrafficChain(msg)
            => msg.buffer_len(),

            | NewNsId(msg)
            | DelNsId(msg)
            | GetNsId(msg)
            => msg.buffer_len(),

            | NewRule(msg)
            | DelRule(msg)
            | GetRule(msg)
            => msg.buffer_len()
        }
    }

    #[rustfmt::skip]
    fn emit(&self, buffer: &mut [u8]) {
        use self::RouteNetlinkMessage::*;
        match self {
            | NewLink(msg)
            | DelLink(msg)
            | GetLink(msg)
            | SetLink(msg)
            | NewLinkProp(msg)
            | DelLinkProp(msg)
            => msg.emit(buffer),

            | NewAddress(msg)
            | DelAddress(msg)
            | GetAddress(msg)
            => msg.emit(buffer),

            | GetNeighbour(msg)
            | NewNeighbour(msg)
            | DelNeighbour(msg)
            => msg.emit(buffer),

            | GetNeighbourTable(msg)
            | NewNeighbourTable(msg)
            | SetNeighbourTable(msg)
            => msg.emit(buffer),

            | NewNeighbourDiscoveryUserOption(msg) => msg.emit(buffer),

            | NewRoute(msg)
            | DelRoute(msg)
            | GetRoute(msg)
            => msg.emit(buffer),

            | NewPrefix(msg) => msg.emit(buffer),

            | NewQueueDiscipline(msg)
            | DelQueueDiscipline(msg)
            | GetQueueDiscipline(msg)
            | NewTrafficClass(msg)
            | DelTrafficClass(msg)
            | GetTrafficClass(msg)
            | NewTrafficFilter(msg)
            | DelTrafficFilter(msg)
            | GetTrafficFilter(msg)
            | NewTrafficChain(msg)
            | DelTrafficChain(msg)
            | GetTrafficChain(msg)
            => msg.emit(buffer),

            | NewNsId(msg)
            | DelNsId(msg)
            | GetNsId(msg)
            => msg.emit(buffer),

            | NewRule(msg)
            | DelRule(msg)
            | GetRule(msg)
            => msg.emit(buffer)
        }
    }
}

impl NetlinkSerializable for RouteNetlinkMessage {
    fn message_type(&self) -> u16 {
        self.message_type()
    }

    fn buffer_len(&self) -> usize {
        <Self as Emitable>::buffer_len(self)
    }

    fn serialize(&self, buffer: &mut [u8]) {
        self.emit(buffer)
    }
}

impl NetlinkDeserializable for RouteNetlinkMessage {
    type DeserializeOptions = RouteNetlinkMessageParseMode;
    type Error = RouteNetlinkMessageParseError;

    fn deserialize(
        header: &NetlinkHeader,
        payload: &[u8],
        options: RouteNetlinkMessageParseMode,
    ) -> Result<Self, Self::Error> {
        let buf = RouteNetlinkMessageBuffer::new(payload);
        match RouteNetlinkMessage::parse_with_param(
            &buf,
            RouteNetlinkMessageParseConfig {
                message_type: header.message_type,
                parse_mode: options,
            },
        ) {
            Err(e) => Err(e),
            Ok(message) => Ok(message),
        }
    }
}

impl From<RouteNetlinkMessage> for NetlinkPayload<RouteNetlinkMessage> {
    fn from(message: RouteNetlinkMessage) -> Self {
        NetlinkPayload::InnerMessage(message)
    }
}
