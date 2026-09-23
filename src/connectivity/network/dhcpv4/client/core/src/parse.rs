// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parsing and serialization of DHCP messages

use dhcp_protocol::{AtLeast, AtMostBytes};
use diagnostics_traits::Inspector;
use packet::{
    InnerPacketBuilder, NestableSerializer as _, NoOpSerializationContext, ParseBuffer as _,
    Serializer,
};
use packet_formats::ip::IpPacket as _;
use std::net::Ipv4Addr;
use std::num::{NonZeroU16, NonZeroU32, TryFromIntError};

use crate::inspect::Counter;

#[derive(thiserror::Error, Debug)]
pub(crate) enum ParseError {
    #[error("parsing IPv4 packet: {0}")]
    Ipv4(packet_formats::error::ParseError),
    #[error("IPv4 packet protocol was not UDP")]
    NotUdp,
    #[error("parsing UDP datagram: {0}")]
    Udp(packet_formats::error::ParseError),
    #[error("incoming packet destined for wrong port: {0}")]
    WrongPort(NonZeroU16),
    #[error("incoming packet has wrong source address: {0}")]
    WrongSource(std::net::SocketAddr),
    #[error("parsing DHCP message: {0}")]
    Dhcp(dhcp_protocol::ProtocolError),
}

/// Parses a DHCP message from the bytes of an IP packet. This function does not
/// expect to parse a packet with link-layer headers; the buffer may only
/// include bytes for the IP layer and above.
/// NOTE: does not handle IP fragmentation.
pub(crate) fn parse_dhcp_message_from_ip_packet(
    mut bytes: &[u8],
    expected_dst_port: NonZeroU16,
) -> Result<(net_types::ip::Ipv4Addr, dhcp_protocol::Message), ParseError> {
    let ip_packet =
        bytes.parse::<packet_formats::ipv4::Ipv4Packet<_>>().map_err(ParseError::Ipv4)?;

    let src_addr = ip_packet.src_ip();

    match ip_packet.proto() {
        packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Udp) => (),
        packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Tcp)
        | packet_formats::ip::Ipv4Proto::Icmp
        | packet_formats::ip::Ipv4Proto::Igmp
        | packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Reserved)
        | packet_formats::ip::Ipv4Proto::Other(_) => return Err(ParseError::NotUdp),
    };
    let mut ip_packet_body = ip_packet.body();

    let udp_packet = ip_packet_body
        .parse_with::<_, packet_formats::udp::UdpPacket<_>>(packet_formats::udp::UdpParseArgs::new(
            ip_packet.src_ip(),
            ip_packet.dst_ip(),
        ))
        .map_err(ParseError::Udp)?;
    let dst_port = udp_packet.dst_port();
    if dst_port != expected_dst_port {
        return Err(ParseError::WrongPort(dst_port));
    }
    dhcp_protocol::Message::from_buffer(udp_packet.body())
        .map(|msg| (src_addr, msg))
        .map_err(ParseError::Dhcp)
}

const DEFAULT_TTL: u8 = 64;

/// Serializes a DHCP message to the bytes of an IP packet. Includes IP header
/// but not link-layer headers.
pub(crate) fn serialize_dhcp_message_to_ip_packet(
    message: dhcp_protocol::Message,
    src_ip: impl Into<net_types::ip::Ipv4Addr>,
    src_port: NonZeroU16,
    dst_ip: impl Into<net_types::ip::Ipv4Addr>,
    dst_port: NonZeroU16,
) -> impl AsRef<[u8]> {
    let message = message.serialize();
    let src_ip = src_ip.into();
    let dst_ip = dst_ip.into();

    let udp_builder =
        packet_formats::udp::UdpPacketBuilder::new(src_ip, dst_ip, Some(src_port), dst_port);

    let ipv4_builder = packet_formats::ipv4::Ipv4PacketBuilder::new(
        src_ip,
        dst_ip,
        DEFAULT_TTL,
        packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Udp),
    );

    match message
        .into_serializer()
        .wrap_in(udp_builder)
        .wrap_in(ipv4_builder)
        .serialize_vec_outer(&mut NoOpSerializationContext)
    {
        Ok(buf) => buf,
        Err(e) => {
            let (e, _serializer) = e;
            match e {
                packet::SerializeError::SizeLimitExceeded => {
                    unreachable!("no MTU constraints on serializer")
                }
            }
        }
    }
}

#[derive(derive_builder::Builder, Debug, PartialEq)]
#[builder(private, build_fn(error = "CommonIncomingMessageError"))]
struct CommonIncomingMessageFields {
    message_type: dhcp_protocol::MessageType,
    #[builder(setter(custom), default)]
    server_identifier: Option<net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>>,
    #[builder(setter(custom), default)]
    yiaddr: Option<net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>>,
    #[builder(setter(strip_option), default)]
    ip_address_lease_time_secs: Option<NonZeroU32>,
    // While it's nonsensical to have a 0-valued lease time, it's somewhat more
    // reasonable to set the renewal time value to 0 (prompting the client to
    // begin the renewal process immediately).
    #[builder(setter(strip_option), default)]
    renewal_time_value_secs: Option<u32>,
    // Same holds for the rebinding time.
    #[builder(setter(strip_option), default)]
    rebinding_time_value_secs: Option<u32>,
    #[builder(default)]
    parameters: Vec<dhcp_protocol::DhcpOption>,
    #[builder(setter(strip_option), default)]
    message: Option<String>,
    #[builder(setter(strip_option), default)]
    client_identifier: Option<AtLeast<2, AtMostBytes<{ dhcp_protocol::U8_MAX_AS_USIZE }, Vec<u8>>>>,
    #[builder(setter(custom))]
    seen_option_codes: OptionCodeSet,
}

#[derive(thiserror::Error, Debug, PartialEq)]
pub(crate) enum CommonIncomingMessageError {
    #[error("got op = {0}, want op = BOOTREPLY")]
    NotBootReply(dhcp_protocol::OpCode),
    #[error("server identifier was the unspecified address")]
    UnspecifiedServerIdentifier,
    #[error("missing: {0}")]
    BuilderMissingField(&'static str),
}

impl From<derive_builder::UninitializedFieldError> for CommonIncomingMessageError {
    fn from(value: derive_builder::UninitializedFieldError) -> Self {
        // `derive_builder::UninitializedFieldError` cannot be destructured
        // because its fields are private.
        Self::BuilderMissingField(value.field_name())
    }
}

/// Counters for reasons an incoming message was discarded.
#[derive(Default, Debug)]
pub(crate) struct CommonIncomingMessageErrorCounters {
    /// The incoming message was a BOOTREQUEST rather than a BOOTREPLY.
    pub(crate) not_boot_reply: Counter,
    /// The incoming message provided the unspecified address as the server
    /// identifier.
    pub(crate) unspecified_server_identifier: Counter,
    /// The parser was unable to populate a required field while consuming the
    /// message.
    pub(crate) parser_missing_field: Counter,
}

impl CommonIncomingMessageErrorCounters {
    /// Records the counters.
    fn record(&self, inspector: &mut impl Inspector) {
        let Self { not_boot_reply, unspecified_server_identifier, parser_missing_field } = self;
        inspector.record_usize("NotBootReply", not_boot_reply.load());
        inspector.record_usize("UnspecifiedServerIdentifier", unspecified_server_identifier.load());
        inspector.record_usize("ParserMissingField", parser_missing_field.load());
    }

    /// Increments the counter corresponding to the error.
    fn increment(&self, error: &CommonIncomingMessageError) {
        let Self { not_boot_reply, unspecified_server_identifier, parser_missing_field } = self;
        match error {
            CommonIncomingMessageError::NotBootReply(_) => not_boot_reply.increment(),
            CommonIncomingMessageError::UnspecifiedServerIdentifier => {
                unspecified_server_identifier.increment()
            }
            CommonIncomingMessageError::BuilderMissingField(_) => parser_missing_field.increment(),
        }
    }
}

/// The set of recoverable errors that were encountered during parsing.
#[derive(Debug, Default)]
pub(crate) struct SoftParseErrors {
    /// The message included an illegal option.
    pub(crate) illegal_option: bool,
}

impl CommonIncomingMessageFieldsBuilder {
    fn ignore_unused_result(&mut self) {}

    fn add_requested_parameter(&mut self, option: dhcp_protocol::DhcpOption) {
        let parameters = self.parameters.get_or_insert_with(Default::default);
        parameters.push(option)
    }

    fn add_seen_option_and_return_whether_newly_added(
        &mut self,
        option_code: dhcp_protocol::OptionCode,
    ) -> bool {
        self.seen_option_codes.get_or_insert_with(Default::default).insert(option_code)
    }

    fn server_identifier(&mut self, addr: Ipv4Addr) -> Result<(), CommonIncomingMessageError> {
        self.server_identifier = Some(Some(
            net_types::SpecifiedAddr::new(net_types::ip::Ipv4Addr::from(addr))
                .ok_or(CommonIncomingMessageError::UnspecifiedServerIdentifier)?,
        ));
        Ok(())
    }

    fn yiaddr(&mut self, addr: Ipv4Addr) {
        match net_types::SpecifiedAddr::new(net_types::ip::Ipv4Addr::from(addr)) {
            None => {
                // Unlike with the Server Identifier option, it is not an error
                // to set `yiaddr` to the unspecified address, as there is no
                // other way to indicate its absence (it has its own field in
                // the DHCP message rather than appearing in the list of
                // options).
            }
            Some(specified_addr) => {
                self.yiaddr = Some(Some(specified_addr));
            }
        }
    }
}

/// Represents a `Map<OptionCode, T>` as an array of booleans.
#[derive(Clone, PartialEq, Debug)]
pub struct OptionCodeMap<T> {
    inner: [Option<T>; dhcp_protocol::U8_MAX_AS_USIZE],
}

impl<T: Copy> OptionCodeMap<T> {
    /// Constructs an empty `OptionCodeMap`.
    pub fn new() -> Self {
        OptionCodeMap { inner: [None; dhcp_protocol::U8_MAX_AS_USIZE] }
    }

    /// Puts `(option_code, value)` into the map, returning the previously-associated
    /// value if is one.
    pub fn put(&mut self, option_code: dhcp_protocol::OptionCode, value: T) -> Option<T> {
        std::mem::replace(&mut self.inner[usize::from(u8::from(option_code))], Some(value))
    }

    /// Gets the value associated with `option_code` from the map, if there is one.
    pub fn get(&self, option_code: dhcp_protocol::OptionCode) -> Option<T> {
        self.inner[usize::from(u8::from(option_code))]
    }

    /// Checks if `option_code` is present in the map.
    pub fn contains(&self, option_code: dhcp_protocol::OptionCode) -> bool {
        self.get(option_code).is_some()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (dhcp_protocol::OptionCode, T)> + '_ {
        self.inner.iter().enumerate().filter_map(|(index, value)| {
            let option_code = u8::try_from(index)
                .ok()
                .and_then(|i| dhcp_protocol::OptionCode::try_from(i).ok())?;
            let value = *value.as_ref()?;
            Some((option_code, value))
        })
    }

    pub(crate) fn iter_keys(&self) -> impl Iterator<Item = dhcp_protocol::OptionCode> + '_ {
        self.iter().map(|(key, _)| key)
    }
}

impl<V: Copy> FromIterator<(dhcp_protocol::OptionCode, V)> for OptionCodeMap<V> {
    fn from_iter<T: IntoIterator<Item = (dhcp_protocol::OptionCode, V)>>(iter: T) -> Self {
        let mut map = Self::new();
        for (option_code, value) in iter {
            let _: Option<_> = map.put(option_code, value);
        }
        map
    }
}

impl<T: Copy> Default for OptionCodeMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionCodeMap<OptionRequested> {
    fn iter_required(&self) -> impl Iterator<Item = dhcp_protocol::OptionCode> + '_ {
        self.iter().filter_map(|(key, val)| match val {
            OptionRequested::Required => Some(key),
            OptionRequested::Optional => None,
        })
    }

    /// Converts `self` into the representation required for
    /// `DhcpOption::ParameterRequestList`.
    ///
    /// Returns None if `self` is empty.
    pub(crate) fn try_to_parameter_request_list(
        &self,
    ) -> Option<
        AtLeast<1, AtMostBytes<{ dhcp_protocol::U8_MAX_AS_USIZE }, Vec<dhcp_protocol::OptionCode>>>,
    > {
        match AtLeast::try_from(self.iter_keys().collect::<Vec<_>>()) {
            Ok(parameters) => Some(parameters),
            Err((dhcp_protocol::SizeConstrainedError::SizeConstraintViolated, parameters)) => {
                // This can only have happened because parameters is empty.
                assert_eq!(parameters, Vec::new());
                // Thus, we must omit the ParameterRequestList option.
                None
            }
        }
    }
}

/// Represents a set of OptionCodes as an array of booleans.
pub type OptionCodeSet = OptionCodeMap<()>;

impl OptionCodeSet {
    /// Inserts `option_code` into the set, returning whether it was newly added.
    pub fn insert(&mut self, option_code: dhcp_protocol::OptionCode) -> bool {
        self.put(option_code, ()).is_none()
    }
}

impl FromIterator<dhcp_protocol::OptionCode> for OptionCodeSet {
    fn from_iter<T: IntoIterator<Item = dhcp_protocol::OptionCode>>(iter: T) -> Self {
        let mut set = Self::new();
        for code in iter {
            let _: bool = set.insert(code);
        }
        set
    }
}

/// Denotes whether a requested option is required or optional.
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum OptionRequested {
    /// The option is required; incoming DHCPOFFERs and DHCPACKs lacking this
    /// option will be discarded.
    Required,
    /// The option is optional.
    Optional,
}

fn collect_common_fields<T: Copy>(
    requested_parameters: &OptionCodeMap<T>,
    dhcp_protocol::Message {
        op,
        xid: _,
        secs: _,
        bdcast_flag: _,
        ciaddr: _,
        yiaddr,
        siaddr: _,
        giaddr: _,
        chaddr: _,
        sname: _,
        file: _,
        options,
    }: dhcp_protocol::Message,
) -> Result<(CommonIncomingMessageFields, SoftParseErrors), CommonIncomingMessageError> {
    use dhcp_protocol::DhcpOption;

    match op {
        dhcp_protocol::OpCode::BOOTREQUEST => {
            return Err(CommonIncomingMessageError::NotBootReply(op));
        }
        dhcp_protocol::OpCode::BOOTREPLY => (),
    };

    let mut builder = CommonIncomingMessageFieldsBuilder::default();
    builder.yiaddr(yiaddr);

    let mut soft_errors = SoftParseErrors::default();

    for option in options {
        let code = option.code();
        let newly_seen = builder.add_seen_option_and_return_whether_newly_added(code);
        if !newly_seen && !option_can_be_duplicated(code) {
            // Adhere to a principle of maximum conformance: ignore options
            // that the server unexpectedly repeated rather than rejecting the
            // message entirely. Should the multiple instance of this option
            // have different values, we'll use the first instance's value.
            log::warn!("DHCP option {code} was unexpectedly repeated: {option:?}");
            continue;
        }

        // From RFC 2131 section 4.3.1:
        // """
        // Option                    DHCPOFFER    DHCPACK               DHCPNAK
        // ------                    ---------    -------               -------
        // Requested IP address      MUST NOT     MUST NOT              MUST NOT
        // IP address lease time     MUST         MUST (DHCPREQUEST)    MUST NOT
        //                                        MUST NOT (DHCPINFORM)
        // Use 'file'/'sname' fields MAY          MAY                   MUST NOT
        // DHCP message type         DHCPOFFER    DHCPACK               DHCPNAK
        // Parameter request list    MUST NOT     MUST NOT              MUST NOT
        // Message                   SHOULD       SHOULD                SHOULD
        // Client identifier         MUST NOT     MUST NOT              MAY
        // Vendor class identifier   MAY          MAY                   MAY
        // Server identifier         MUST         MUST                  MUST
        // Maximum message size      MUST NOT     MUST NOT              MUST NOT
        // All others                MAY          MAY                   MUST NOT
        //
        //            Table 3:  Fields and options used by DHCP servers
        // """

        match &option {
            DhcpOption::IpAddressLeaseTime(value) => match NonZeroU32::try_from(*value) {
                Err(e) => {
                    let _: TryFromIntError = e;
                    log::warn!("dropping 0 lease time");
                }
                Ok(value) => {
                    builder.ip_address_lease_time_secs(value).ignore_unused_result();
                }
            },
            DhcpOption::DhcpMessageType(message_type) => {
                builder.message_type(*message_type).ignore_unused_result()
            }
            DhcpOption::ServerIdentifier(value) => {
                builder.server_identifier(*value)?;
            }
            DhcpOption::Message(message) => builder.message(message.clone()).ignore_unused_result(),
            DhcpOption::RenewalTimeValue(value) => {
                builder.renewal_time_value_secs(*value).ignore_unused_result()
            }
            DhcpOption::RebindingTimeValue(value) => {
                builder.rebinding_time_value_secs(*value).ignore_unused_result()
            }
            DhcpOption::ClientIdentifier(value) => {
                builder.client_identifier(value.clone()).ignore_unused_result();
            }
            DhcpOption::ParameterRequestList(_)
            | DhcpOption::RequestedIpAddress(_)
            | DhcpOption::MaxDhcpMessageSize(_) => {
                // Per the RFC citation above, a DHCP server MUST NOT include
                // these options in any of it's messages. However, for the
                // sake of robustness, our client will ignore these options and
                // continue parsing the rest of the message.
                soft_errors.illegal_option = true;
                log::warn!("ignoring illegal DHCP option {option:?}");
            }
            DhcpOption::Pad()
            | DhcpOption::End()
            | DhcpOption::SubnetMask(_)
            | DhcpOption::TimeOffset(_)
            | DhcpOption::Router(_)
            | DhcpOption::TimeServer(_)
            | DhcpOption::NameServer(_)
            | DhcpOption::DomainNameServer(_)
            | DhcpOption::LogServer(_)
            | DhcpOption::CookieServer(_)
            | DhcpOption::LprServer(_)
            | DhcpOption::ImpressServer(_)
            | DhcpOption::ResourceLocationServer(_)
            | DhcpOption::HostName(_)
            | DhcpOption::BootFileSize(_)
            | DhcpOption::MeritDumpFile(_)
            | DhcpOption::DomainName(_)
            | DhcpOption::SwapServer(_)
            | DhcpOption::RootPath(_)
            | DhcpOption::ExtensionsPath(_)
            | DhcpOption::IpForwarding(_)
            | DhcpOption::NonLocalSourceRouting(_)
            | DhcpOption::PolicyFilter(_)
            | DhcpOption::MaxDatagramReassemblySize(_)
            | DhcpOption::DefaultIpTtl(_)
            | DhcpOption::PathMtuAgingTimeout(_)
            | DhcpOption::PathMtuPlateauTable(_)
            | DhcpOption::InterfaceMtu(_)
            | DhcpOption::AllSubnetsLocal(_)
            | DhcpOption::BroadcastAddress(_)
            | DhcpOption::PerformMaskDiscovery(_)
            | DhcpOption::MaskSupplier(_)
            | DhcpOption::PerformRouterDiscovery(_)
            | DhcpOption::RouterSolicitationAddress(_)
            | DhcpOption::StaticRoute(_)
            | DhcpOption::TrailerEncapsulation(_)
            | DhcpOption::ArpCacheTimeout(_)
            | DhcpOption::EthernetEncapsulation(_)
            | DhcpOption::TcpDefaultTtl(_)
            | DhcpOption::TcpKeepaliveInterval(_)
            | DhcpOption::TcpKeepaliveGarbage(_)
            | DhcpOption::NetworkInformationServiceDomain(_)
            | DhcpOption::NetworkInformationServers(_)
            | DhcpOption::NetworkTimeProtocolServers(_)
            | DhcpOption::VendorSpecificInformation(_)
            | DhcpOption::NetBiosOverTcpipNameServer(_)
            | DhcpOption::NetBiosOverTcpipDatagramDistributionServer(_)
            | DhcpOption::NetBiosOverTcpipNodeType(_)
            | DhcpOption::NetBiosOverTcpipScope(_)
            | DhcpOption::XWindowSystemFontServer(_)
            | DhcpOption::XWindowSystemDisplayManager(_)
            | DhcpOption::NetworkInformationServicePlusDomain(_)
            | DhcpOption::NetworkInformationServicePlusServers(_)
            | DhcpOption::MobileIpHomeAgent(_)
            | DhcpOption::SmtpServer(_)
            | DhcpOption::Pop3Server(_)
            | DhcpOption::NntpServer(_)
            | DhcpOption::DefaultWwwServer(_)
            | DhcpOption::DefaultFingerServer(_)
            | DhcpOption::DefaultIrcServer(_)
            | DhcpOption::StreetTalkServer(_)
            | DhcpOption::StreetTalkDirectoryAssistanceServer(_)
            | DhcpOption::OptionOverload(_)
            | DhcpOption::TftpServerName(_)
            | DhcpOption::BootfileName(_)
            | DhcpOption::VendorClassIdentifier(_) => (),
        };

        if requested_parameters.contains(option.code()) {
            builder.add_requested_parameter(option);
        }
    }
    Ok((builder.build()?, soft_errors))
}

/// Returns whether the option is allowed to be specified multiple times.
///
/// Note: The DHCP RFC doesn't take a stance on whether options may or may not
/// be repeated. We take a pragmatic approach and allow options to be repeated
/// where appropriate (e.g. options that have list semantics). Repeats are
/// rejected on options for which they are nonsensical (e.g. the
/// DhcpMessageType).
fn option_can_be_duplicated(code: dhcp_protocol::OptionCode) -> bool {
    use dhcp_protocol::OptionCode::*;
    match code {
        SubnetMask
        | TimeOffset
        | HostName
        | BootFileSize
        | MeritDumpFile
        | DomainName
        | SwapServer
        | RootPath
        | ExtensionsPath
        | IpForwarding
        | NonLocalSourceRouting
        | MaxDatagramReassemblySize
        | DefaultIpTtl
        | PathMtuAgingTimeout
        | InterfaceMtu
        | AllSubnetsLocal
        | BroadcastAddress
        | PerformMaskDiscovery
        | MaskSupplier
        | PerformRouterDiscovery
        | RouterSolicitationAddress
        | TrailerEncapsulation
        | ArpCacheTimeout
        | EthernetEncapsulation
        | TcpDefaultTtl
        | TcpKeepaliveInterval
        | TcpKeepaliveGarbage
        | NetworkInformationServiceDomain
        | NetBiosOverTcpipNodeType
        | NetBiosOverTcpipScope
        | RequestedIpAddress
        | IpAddressLeaseTime
        | OptionOverload
        | DhcpMessageType
        | ServerIdentifier
        | Message
        | MaxDhcpMessageSize
        | RenewalTimeValue
        | RebindingTimeValue
        | VendorClassIdentifier
        | ClientIdentifier
        | NetworkInformationServicePlusDomain
        | TftpServerName
        | BootfileName
        | End => false,
        Pad
        | Router
        | TimeServer
        | NameServer
        | DomainNameServer
        | LogServer
        | CookieServer
        | LprServer
        | ImpressServer
        | ResourceLocationServer
        | PolicyFilter
        | PathMtuPlateauTable
        | StaticRoute
        | NetworkInformationServers
        | NetworkTimeProtocolServers
        | VendorSpecificInformation
        | NetBiosOverTcpipNameServer
        | NetBiosOverTcpipDatagramDistributionServer
        | XWindowSystemFontServer
        | XWindowSystemDisplayManager
        | NetworkInformationServicePlusServers
        | MobileIpHomeAgent
        | SmtpServer
        | Pop3Server
        | NntpServer
        | DefaultWwwServer
        | DefaultFingerServer
        | DefaultIrcServer
        | StreetTalkServer
        | StreetTalkDirectoryAssistanceServer
        | ParameterRequestList => true,
    }
}

/// Reasons that an incoming DHCP message might be discarded during Selecting
/// state.
#[derive(thiserror::Error, Debug, PartialEq)]
pub(crate) enum SelectingIncomingMessageError {
    #[error("{0}")]
    CommonError(#[from] CommonIncomingMessageError),
    /// Note that `NoServerIdentifier` is intentionally distinct from
    /// `CommonIncomingMessageError::UnspecifiedServerIdentifier`, as the latter
    /// refers to the Server Identifier being explicitly populated as the
    /// unspecified address, rather than simply omitted.
    #[error("no server identifier")]
    NoServerIdentifier,
    #[error("got DHCP message type = {0}, wanted DHCPOFFER")]
    NotDhcpOffer(dhcp_protocol::MessageType),
    #[error("yiaddr was the unspecified address")]
    UnspecifiedYiaddr,
    #[error("missing required option: {0:?}")]
    MissingRequiredOption(dhcp_protocol::OptionCode),
}

/// Counters for reasons a message was discarded while receiving in Selecting
/// state.
#[derive(Default, Debug)]
pub(crate) struct SelectingIncomingMessageErrorCounters {
    /// Common reasons across all states.
    pub(crate) common: CommonIncomingMessageErrorCounters,
    /// The message omitted the Server Identifier option.
    pub(crate) no_server_identifier: Counter,
    /// The message was not a DHCPOFFER.
    pub(crate) not_dhcp_offer: Counter,
    /// The message had yiaddr set to the unspecified address.
    pub(crate) unspecified_yiaddr: Counter,
    /// The message was missing a required option.
    pub(crate) missing_required_option: Counter,
}

impl SelectingIncomingMessageErrorCounters {
    /// Records the counters.
    pub(crate) fn record(&self, inspector: &mut impl Inspector) {
        let Self {
            common,
            no_server_identifier,
            not_dhcp_offer,
            unspecified_yiaddr,
            missing_required_option,
        } = self;
        common.record(inspector);
        inspector.record_usize("NoServerIdentifier", no_server_identifier.load());
        inspector.record_usize("NotDhcpOffer", not_dhcp_offer.load());
        inspector.record_usize("UnspecifiedYiaddr", unspecified_yiaddr.load());
        inspector.record_usize("MissingRequiredOption", missing_required_option.load());
    }

    /// Increments the counter corresponding to the error.
    pub(crate) fn increment(&self, error: &SelectingIncomingMessageError) {
        let Self {
            common,
            no_server_identifier,
            not_dhcp_offer,
            unspecified_yiaddr,
            missing_required_option,
        } = self;
        match error {
            SelectingIncomingMessageError::CommonError(common_incoming_message_error) => {
                common.increment(common_incoming_message_error)
            }
            SelectingIncomingMessageError::NoServerIdentifier => no_server_identifier.increment(),
            SelectingIncomingMessageError::NotDhcpOffer(_) => not_dhcp_offer.increment(),
            SelectingIncomingMessageError::UnspecifiedYiaddr => unspecified_yiaddr.increment(),
            SelectingIncomingMessageError::MissingRequiredOption(_) => {
                missing_required_option.increment()
            }
        }
    }
}

/// Extracts the fields from a DHCP message incoming during Selecting state that
/// should be used during Requesting state.
pub(crate) fn fields_to_retain_from_selecting(
    requested_parameters: &OptionCodeMap<OptionRequested>,
    message: dhcp_protocol::Message,
) -> Result<(FieldsFromOfferToUseInRequest, SoftParseErrors), SelectingIncomingMessageError> {
    let (common_fields, soft_errors) = collect_common_fields(requested_parameters, message)?;
    let CommonIncomingMessageFields {
        message_type,
        server_identifier,
        yiaddr,
        ip_address_lease_time_secs,
        renewal_time_value_secs: _,
        rebinding_time_value_secs: _,
        parameters: _,
        seen_option_codes,
        message: _,
        client_identifier: _,
    } = common_fields;

    match message_type {
        dhcp_protocol::MessageType::DHCPOFFER => (),
        dhcp_protocol::MessageType::DHCPDISCOVER
        | dhcp_protocol::MessageType::DHCPREQUEST
        | dhcp_protocol::MessageType::DHCPDECLINE
        | dhcp_protocol::MessageType::DHCPACK
        | dhcp_protocol::MessageType::DHCPNAK
        | dhcp_protocol::MessageType::DHCPRELEASE
        | dhcp_protocol::MessageType::DHCPINFORM => {
            return Err(SelectingIncomingMessageError::NotDhcpOffer(message_type));
        }
    };

    if let Some(missing_option_code) =
        requested_parameters.iter_required().find(|code| !seen_option_codes.contains(*code))
    {
        return Err(SelectingIncomingMessageError::MissingRequiredOption(missing_option_code));
    }

    let offer_fields = FieldsFromOfferToUseInRequest {
        server_identifier: server_identifier
            .ok_or(SelectingIncomingMessageError::NoServerIdentifier)?,
        ip_address_lease_time_secs,
        ip_address_to_request: yiaddr.ok_or(SelectingIncomingMessageError::UnspecifiedYiaddr)?,
    };
    Ok((offer_fields, soft_errors))
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Fields from a DHCPOFFER that should be used while building a DHCPREQUEST.
pub(crate) struct FieldsFromOfferToUseInRequest {
    pub(crate) server_identifier: net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>,
    pub(crate) ip_address_lease_time_secs: Option<NonZeroU32>,
    pub(crate) ip_address_to_request: net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>,
}

impl FieldsFromOfferToUseInRequest {
    pub(crate) fn record(&self, inspector: &mut impl Inspector) {
        let Self { server_identifier, ip_address_lease_time_secs, ip_address_to_request } = self;
        inspector.record_ip_addr("ServerIdentifier", **server_identifier);
        if let Some(value) = ip_address_lease_time_secs {
            inspector.record_uint("IpAddressLeaseTimeSecs", value.get());
        }
        inspector.record_ip_addr("IpAddressToRequest", **ip_address_to_request);
    }
}

#[derive(Debug, PartialEq)]
// `ServerIdentifier`` is generic in order to allow for it to be optional for
// DHCPACKs received while in RENEWING state (since we already know the
// identifier of the server we're directly communicating with) but required in
// REBINDING state (because we've broadcast the request and need to record
// which server to send renewal requests to in the future).
pub(crate) enum IncomingResponseToRequest<ServerIdentifier> {
    Ack(FieldsToRetainFromAck<ServerIdentifier>),
    Nak(FieldsToRetainFromNak),
}

/// Reasons that an incoming response to a DHCPREQUEST might be discarded.
#[derive(thiserror::Error, Debug, PartialEq)]
pub(crate) enum IncomingResponseToRequestError {
    #[error("{0}")]
    CommonError(#[from] CommonIncomingMessageError),
    #[error("got DHCP message type = {0}, wanted DHCPACK or DHCPNAK")]
    NotDhcpAckOrNak(dhcp_protocol::MessageType),
    #[error("yiaddr was the unspecified address")]
    UnspecifiedYiaddr,
    #[error("no server identifier")]
    NoServerIdentifier,
    #[error("missing required option: {0:?}")]
    MissingRequiredOption(dhcp_protocol::OptionCode),
}

/// Counters for reasons a message was discarded while receiving in Requesting
/// state.
#[derive(Default, Debug)]
pub(crate) struct IncomingResponseToRequestErrorCounters {
    /// Common reasons across all states.
    pub(crate) common: CommonIncomingMessageErrorCounters,
    /// The message was not a DHCPACK or DHCPNAK.
    pub(crate) not_dhcp_ack_or_nak: Counter,
    /// The message had yiaddr set to the unspecified address.
    pub(crate) unspecified_yiaddr: Counter,
    /// The message had no server identifier.
    pub(crate) no_server_identifier: Counter,
    /// The message was missing a required option.
    pub(crate) missing_required_option: Counter,
}

impl IncomingResponseToRequestErrorCounters {
    /// Records the counters.
    pub(crate) fn record(&self, inspector: &mut impl Inspector) {
        let Self {
            common,
            not_dhcp_ack_or_nak,
            unspecified_yiaddr,
            no_server_identifier,
            missing_required_option,
        } = self;
        common.record(inspector);
        inspector.record_usize("NotDhcpAckOrNak", not_dhcp_ack_or_nak.load());
        inspector.record_usize("UnspecifiedYiaddr", unspecified_yiaddr.load());
        inspector.record_usize("NoServerIdentifier", no_server_identifier.load());
        inspector.record_usize("MissingRequiredOption", missing_required_option.load());
    }

    /// Increments the counter corresponding to the error.
    pub(crate) fn increment(&self, error: &IncomingResponseToRequestError) {
        let Self {
            common,
            not_dhcp_ack_or_nak,
            unspecified_yiaddr,
            no_server_identifier,
            missing_required_option,
        } = self;
        match error {
            IncomingResponseToRequestError::CommonError(common_incoming_message_error) => {
                common.increment(common_incoming_message_error)
            }
            IncomingResponseToRequestError::NotDhcpAckOrNak(_) => not_dhcp_ack_or_nak.increment(),
            IncomingResponseToRequestError::UnspecifiedYiaddr => unspecified_yiaddr.increment(),
            IncomingResponseToRequestError::NoServerIdentifier => no_server_identifier.increment(),
            IncomingResponseToRequestError::MissingRequiredOption(_) => {
                missing_required_option.increment()
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct FieldsToRetainFromAck<ServerIdentifier> {
    pub(crate) yiaddr: net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>,
    pub(crate) server_identifier: ServerIdentifier,
    // Strictly according to RFC 2131, the IP Address Lease Time MUST be
    // included in the DHCPACK. However, we've observed DHCP servers in the
    // field that fail to set the lease time (https://fxbug.dev/486403240).
    // Thus, we treat IP Address Lease Time as optional for DHCPACK.
    pub(crate) ip_address_lease_time_secs: Option<NonZeroU32>,
    pub(crate) renewal_time_value_secs: Option<u32>,
    pub(crate) rebinding_time_value_secs: Option<u32>,
    // Note: Options with list semantics may be repeated.
    pub(crate) parameters: Vec<dhcp_protocol::DhcpOption>,
}

impl<ServerIdentifier> FieldsToRetainFromAck<ServerIdentifier> {
    pub(crate) fn map_server_identifier<T, E>(
        self,
        f: impl FnOnce(ServerIdentifier) -> Result<T, E>,
    ) -> Result<FieldsToRetainFromAck<T>, E> {
        let Self {
            yiaddr,
            server_identifier,
            ip_address_lease_time_secs,
            renewal_time_value_secs,
            rebinding_time_value_secs,
            parameters,
        } = self;
        Ok(FieldsToRetainFromAck {
            yiaddr,
            server_identifier: f(server_identifier)?,
            ip_address_lease_time_secs,
            renewal_time_value_secs,
            rebinding_time_value_secs,
            parameters,
        })
    }
}

#[derive(Debug, PartialEq)]
pub(crate) struct FieldsToRetainFromNak {
    pub(crate) server_identifier: net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>,
    pub(crate) message: Option<String>,
    pub(crate) client_identifier: Option<
        AtLeast<
            { dhcp_protocol::CLIENT_IDENTIFIER_MINIMUM_LENGTH },
            AtMostBytes<{ dhcp_protocol::U8_MAX_AS_USIZE }, Vec<u8>>,
        >,
    >,
}

pub(crate) fn fields_to_retain_from_response_to_request(
    requested_parameters: &OptionCodeMap<OptionRequested>,
    message: dhcp_protocol::Message,
) -> Result<
    (
        IncomingResponseToRequest<
            // Strictly according to RFC 2131, the Server Identifier MUST be included in
            // the DHCPACK. However, we've observed DHCP servers in the field fail to
            // set the Server Identifier, instead expecting the client to remember it
            // from the DHCPOFFER (https://fxbug.dev/42064504). Thus, we treat Server
            // Identifier as optional for DHCPACK.
            Option<net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>>,
        >,
        SoftParseErrors,
    ),
    IncomingResponseToRequestError,
> {
    let (common_fields, soft_errors) = collect_common_fields(requested_parameters, message)?;
    let CommonIncomingMessageFields {
        message_type,
        server_identifier,
        yiaddr,
        ip_address_lease_time_secs,
        renewal_time_value_secs,
        rebinding_time_value_secs,
        parameters,
        seen_option_codes,
        message,
        client_identifier,
    } = common_fields;

    match message_type {
        dhcp_protocol::MessageType::DHCPACK => {
            // Only enforce required parameters for ACKs, since NAKs aren't
            // expected to include any configuration at all.

            if let Some(missing_option_code) =
                requested_parameters.iter_required().find(|code| !seen_option_codes.contains(*code))
            {
                return Err(IncomingResponseToRequestError::MissingRequiredOption(
                    missing_option_code,
                ));
            }
            let ack = IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
                yiaddr: yiaddr.ok_or(IncomingResponseToRequestError::UnspecifiedYiaddr)?,
                server_identifier,
                ip_address_lease_time_secs: ip_address_lease_time_secs,
                renewal_time_value_secs,
                rebinding_time_value_secs,
                parameters,
            });
            Ok((ack, soft_errors))
        }
        dhcp_protocol::MessageType::DHCPNAK => {
            let nak = IncomingResponseToRequest::Nak(FieldsToRetainFromNak {
                server_identifier: server_identifier
                    .ok_or(IncomingResponseToRequestError::NoServerIdentifier)?,
                message,
                client_identifier,
            });
            Ok((nak, soft_errors))
        }
        dhcp_protocol::MessageType::DHCPDISCOVER
        | dhcp_protocol::MessageType::DHCPOFFER
        | dhcp_protocol::MessageType::DHCPREQUEST
        | dhcp_protocol::MessageType::DHCPDECLINE
        | dhcp_protocol::MessageType::DHCPRELEASE
        | dhcp_protocol::MessageType::DHCPINFORM => {
            Err(IncomingResponseToRequestError::NotDhcpAckOrNak(message_type))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use assert_matches::assert_matches;
    use bstr::BString;
    use dhcp_protocol::{CLIENT_PORT, SERVER_PORT};
    use net_declare::net::prefix_length_v4;
    use net_declare::{net_ip_v4, net_mac, std_ip_v4};
    use net_types::ip::{Ip, Ipv4, PrefixLength};
    use std::net::Ipv4Addr;
    use test_case::test_case;

    #[test]
    fn serialize_parse_roundtrip() {
        let make_message = || dhcp_protocol::Message {
            op: dhcp_protocol::OpCode::BOOTREQUEST,
            xid: 124,
            secs: 99,
            bdcast_flag: false,
            ciaddr: net_ip_v4!("1.2.3.4").into(),
            yiaddr: net_ip_v4!("5.6.7.8").into(),
            siaddr: net_ip_v4!("9.10.11.12").into(),
            giaddr: net_ip_v4!("13.14.15.16").into(),
            chaddr: net_mac!("17:18:19:20:21:22"),
            sname: BString::from("this is a sname"),
            file: BString::from("this is the boot filename"),
            options: vec![
                dhcp_protocol::DhcpOption::DhcpMessageType(
                    dhcp_protocol::MessageType::DHCPDISCOVER,
                ),
                dhcp_protocol::DhcpOption::RequestedIpAddress(net_ip_v4!("5.6.7.8").into()),
            ],
        };
        let packet = serialize_dhcp_message_to_ip_packet(
            make_message(),
            Ipv4Addr::UNSPECIFIED,
            CLIENT_PORT,
            Ipv4Addr::BROADCAST,
            SERVER_PORT,
        );
        let (src_addr, parsed_message) =
            parse_dhcp_message_from_ip_packet(packet.as_ref(), SERVER_PORT).unwrap();

        assert_eq!(net_types::ip::Ipv4::UNSPECIFIED_ADDRESS, src_addr);
        assert_eq!(make_message(), parsed_message);
    }

    #[test]
    fn nonsense() {
        assert_matches!(
            parse_dhcp_message_from_ip_packet(
                &[0xD, 0xE, 0xA, 0xD, 0xB, 0xE, 0xE, 0xF],
                NonZeroU16::new(1).unwrap()
            ),
            Err(ParseError::Ipv4(parse_error)) => {
                assert_eq!(parse_error, packet_formats::error::ParseError::Format)
            }
        )
    }

    #[test]
    fn not_udp() {
        let src_ip = Ipv4Addr::UNSPECIFIED.into();
        let dst_ip = Ipv4Addr::BROADCAST.into();
        let tcp_builder: packet_formats::tcp::TcpSegmentBuilder<net_types::ip::Ipv4Addr> =
            packet_formats::tcp::TcpSegmentBuilder::new(
                src_ip,
                dst_ip,
                CLIENT_PORT,
                SERVER_PORT,
                0,
                None,
                0,
            );
        let ipv4_builder = packet_formats::ipv4::Ipv4PacketBuilder::new(
            src_ip,
            dst_ip,
            DEFAULT_TTL,
            packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Tcp),
        );
        let bytes = vec![1, 2, 3, 4, 5]
            .into_serializer()
            .wrap_in(tcp_builder)
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize error");

        assert_matches!(
            parse_dhcp_message_from_ip_packet(bytes.as_ref(), NonZeroU16::new(1).unwrap()),
            Err(ParseError::NotUdp)
        );
    }

    #[test]
    fn wrong_port() {
        let src_ip = Ipv4Addr::UNSPECIFIED.into();
        let dst_ip = Ipv4Addr::BROADCAST.into();

        let udp_builder: packet_formats::udp::UdpPacketBuilder<net_types::ip::Ipv4Addr> =
            packet_formats::udp::UdpPacketBuilder::new(
                src_ip,
                dst_ip,
                Some(CLIENT_PORT),
                SERVER_PORT,
            );
        let ipv4_builder = packet_formats::ipv4::Ipv4PacketBuilder::new(
            src_ip,
            dst_ip,
            DEFAULT_TTL,
            packet_formats::ip::Ipv4Proto::Proto(packet_formats::ip::IpProto::Udp),
        );

        let bytes = "hello_world"
            .bytes()
            .collect::<Vec<_>>()
            .into_serializer()
            .wrap_in(udp_builder)
            .wrap_in(ipv4_builder)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .expect("serialize error");

        let result = parse_dhcp_message_from_ip_packet(bytes.as_ref(), CLIENT_PORT);
        assert_matches!(result, Err(ParseError::WrongPort(port)) => assert_eq!(port, SERVER_PORT));
    }

    struct VaryingOfferFields {
        op: dhcp_protocol::OpCode,
        yiaddr: Ipv4Addr,
        message_type: Option<dhcp_protocol::MessageType>,
        server_identifier: Option<Ipv4Addr>,
        subnet_mask: Option<PrefixLength<Ipv4>>,
        lease_length_secs: Option<u32>,
        include_duplicate_option: bool,
        include_illegal_option: bool,
    }

    const SERVER_IP: Ipv4Addr = std_ip_v4!("192.168.1.1");
    const TEST_SUBNET_MASK: PrefixLength<Ipv4> = prefix_length_v4!(24);
    const LEASE_LENGTH_SECS: u32 = 100;
    const LEASE_LENGTH_SECS_NONZERO: NonZeroU32 = NonZeroU32::new(LEASE_LENGTH_SECS).unwrap();
    const YIADDR: Ipv4Addr = std_ip_v4!("192.168.1.5");

    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Ok(FieldsFromOfferToUseInRequest {
        server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
            .try_into()
            .expect("should be specified"),
        ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
        ip_address_to_request: net_types::ip::Ipv4Addr::from(YIADDR)
            .try_into()
            .expect("should be specified"),
    }); "accepts good offer with lease time")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: None,
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Ok(FieldsFromOfferToUseInRequest {
        server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
            .try_into()
            .expect("should be specified"),
        ip_address_lease_time_secs: None,
        ip_address_to_request: net_types::ip::Ipv4Addr::from(YIADDR)
            .try_into()
            .expect("should be specified"),
    }); "accepts good offer without lease time")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(Ipv4Addr::UNSPECIFIED),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(SelectingIncomingMessageError::CommonError(
        CommonIncomingMessageError::UnspecifiedServerIdentifier,
    )); "rejects offer with unspecified server identifier")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: None,
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(SelectingIncomingMessageError::MissingRequiredOption(
        dhcp_protocol::OptionCode::SubnetMask,
    )); "rejects offer without required subnet mask")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: None,
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(SelectingIncomingMessageError::NoServerIdentifier); "rejects offer with no server identifier option")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(SelectingIncomingMessageError::UnspecifiedYiaddr) ; "rejects offer with unspecified yiaddr")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREQUEST,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(SelectingIncomingMessageError::CommonError(
        CommonIncomingMessageError::NotBootReply(dhcp_protocol::OpCode::BOOTREQUEST),
    )); "rejects offer that isn't a bootreply")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPACK),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(
        SelectingIncomingMessageError::NotDhcpOffer(dhcp_protocol::MessageType::DHCPACK),
    ); "rejects offer with wrong DHCP message type")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: None,
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(SelectingIncomingMessageError::CommonError(
        CommonIncomingMessageError::BuilderMissingField("message_type"),
    )); "rejects offer with no DHCP message type option")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: true,
        include_illegal_option: false,
    } => Ok(FieldsFromOfferToUseInRequest {
        server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
            .try_into()
            .expect("should be specified"),
        ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
        ip_address_to_request: net_types::ip::Ipv4Addr::from(YIADDR)
            .try_into()
            .expect("should be specified"),
    }); "accepts good offer with duplicate options")]
    #[test_case(VaryingOfferFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        include_duplicate_option: false,
        include_illegal_option: true,
    } => Ok(FieldsFromOfferToUseInRequest {
        server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
            .try_into()
            .expect("should be specified"),
        ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
        ip_address_to_request: net_types::ip::Ipv4Addr::from(YIADDR)
            .try_into()
            .expect("should be specified"),
    }); "accepts good offer with illegal option")]
    fn fields_from_offer_to_use_in_request(
        offer_fields: VaryingOfferFields,
    ) -> Result<FieldsFromOfferToUseInRequest, SelectingIncomingMessageError> {
        use super::fields_to_retain_from_selecting as fields;
        use dhcp_protocol::DhcpOption;

        let VaryingOfferFields {
            op,
            yiaddr,
            message_type,
            server_identifier,
            subnet_mask,
            lease_length_secs,
            include_duplicate_option,
            include_illegal_option,
        } = offer_fields;

        let message = dhcp_protocol::Message {
            op,
            xid: 1,
            secs: 0,
            bdcast_flag: false,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: net_mac!("01:02:03:04:05:06"),
            sname: BString::default(),
            file: BString::default(),
            options: message_type
                .map(DhcpOption::DhcpMessageType)
                .into_iter()
                .chain(server_identifier.map(DhcpOption::ServerIdentifier))
                .chain(subnet_mask.map(DhcpOption::SubnetMask))
                .chain(lease_length_secs.map(DhcpOption::IpAddressLeaseTime))
                .chain(
                    include_duplicate_option
                        .then_some([
                            dhcp_protocol::DhcpOption::DomainName("example.com".to_owned()),
                            dhcp_protocol::DhcpOption::DomainName("example.com".to_owned()),
                        ])
                        .into_iter()
                        .flatten(),
                )
                .chain(
                    include_illegal_option
                        // It's illegal for the DHCP server to provide the
                        // "Requested IP Address" option in any of its messages.
                        .then_some(dhcp_protocol::DhcpOption::RequestedIpAddress(SERVER_IP)),
                )
                .collect(),
        };

        fields(
            &std::iter::once((dhcp_protocol::OptionCode::SubnetMask, OptionRequested::Required))
                .collect(),
            message,
        )
        .map(|(fields, soft_errors)| {
            let SoftParseErrors { illegal_option } = soft_errors;
            assert_eq!(illegal_option, include_illegal_option);
            fields
        })
    }

    struct VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode,
        yiaddr: Ipv4Addr,
        message_type: Option<dhcp_protocol::MessageType>,
        server_identifier: Option<Ipv4Addr>,
        subnet_mask: Option<PrefixLength<Ipv4>>,
        lease_length_secs: Option<u32>,
        renewal_time_secs: Option<u32>,
        rebinding_time_secs: Option<u32>,
        message: Option<String>,
        include_duplicate_option: bool,
        include_illegal_option: bool,
    }

    const DOMAIN_NAME: &str = "example.com";
    const MESSAGE: &str = "message explaining why the DHCPNAK was sent";
    const RENEWAL_TIME_SECS: u32 = LEASE_LENGTH_SECS / 2;
    const REBINDING_TIME_SECS: u32 = LEASE_LENGTH_SECS * 3 / 4;

    #[test_case(
        VaryingReplyToRequestFields {
            op: dhcp_protocol::OpCode::BOOTREPLY,
            yiaddr: YIADDR,
            message_type: Some(dhcp_protocol::MessageType::DHCPACK),
            server_identifier: Some(SERVER_IP),
            subnet_mask: Some(TEST_SUBNET_MASK),
            lease_length_secs: Some(LEASE_LENGTH_SECS),
            renewal_time_secs: None,
            rebinding_time_secs: None,
            message: None,
            include_duplicate_option: false,
            include_illegal_option: false,
        } => Ok(IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: Some(
                net_types::ip::Ipv4Addr::from(SERVER_IP)
                    .try_into()
                    .expect("should be specified"),
            ),
            ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
            parameters: vec![
                dhcp_protocol::DhcpOption::SubnetMask(TEST_SUBNET_MASK),
                dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())
            ],
            renewal_time_value_secs: None,
            rebinding_time_value_secs: None,
        })); "accepts good DHCPACK")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPACK),
        server_identifier: None,
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: None,
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Ok(IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
        yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
            .try_into()
            .expect("should be specified"),
        server_identifier: None,
        ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
        parameters: vec![
            dhcp_protocol::DhcpOption::SubnetMask(TEST_SUBNET_MASK),
            dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())
        ],
        renewal_time_value_secs: None,
        rebinding_time_value_secs: None,
    })); "accepts DHCPACK with no server identifier")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPACK),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        renewal_time_secs: Some(RENEWAL_TIME_SECS),
        rebinding_time_secs: Some(REBINDING_TIME_SECS),
        message: None,
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Ok(IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
        yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
            .try_into()
            .expect("should be specified"),
        server_identifier: Some(
            net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
        ),
        ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
        parameters: vec![
            dhcp_protocol::DhcpOption::SubnetMask(TEST_SUBNET_MASK),
            dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())
        ],
        renewal_time_value_secs: Some(RENEWAL_TIME_SECS),
        rebinding_time_value_secs: Some(REBINDING_TIME_SECS),
    })); "accepts DHCPACK with renew and rebind times")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPNAK),
        server_identifier: Some(SERVER_IP),
        subnet_mask: None,
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: Some(MESSAGE.to_owned()),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Ok(IncomingResponseToRequest::Nak(FieldsToRetainFromNak {
        server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
            .try_into()
            .expect("should be specified"),
        message: Some(MESSAGE.to_owned()),
        client_identifier: None,
    })); "accepts good DHCPNAK")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPACK),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: None,
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Ok(IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
        yiaddr: net_types::ip::Ipv4Addr::from(YIADDR).try_into().expect("should be specified"),
        server_identifier: Some(
            net_types::ip::Ipv4Addr::from(SERVER_IP).try_into().expect("should be specified")
        ),
        ip_address_lease_time_secs: None,
        parameters: vec![
            dhcp_protocol::DhcpOption::SubnetMask(TEST_SUBNET_MASK),
            dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())
        ],
        renewal_time_value_secs: None,
        rebinding_time_value_secs: None,
    })); "accepts DHCPACK with no lease time")]
    #[test_case(
        VaryingReplyToRequestFields {
            op: dhcp_protocol::OpCode::BOOTREPLY,
            yiaddr: YIADDR,
            message_type: Some(dhcp_protocol::MessageType::DHCPACK),
            server_identifier: Some(SERVER_IP),
            subnet_mask: None,
            lease_length_secs: Some(LEASE_LENGTH_SECS),
            renewal_time_secs: None,
            rebinding_time_secs: None,
            message: None,
            include_duplicate_option: false,
        include_illegal_option: false,
        } => Err(IncomingResponseToRequestError::MissingRequiredOption(
            dhcp_protocol::OptionCode::SubnetMask
        )); "rejects DHCPACK without required subnet mask")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: YIADDR,
        message_type: Some(dhcp_protocol::MessageType::DHCPACK),
        server_identifier: Some(Ipv4Addr::UNSPECIFIED),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        renewal_time_secs: Some(RENEWAL_TIME_SECS),
        rebinding_time_secs: Some(REBINDING_TIME_SECS),
        message: None,
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::CommonError(
        CommonIncomingMessageError::UnspecifiedServerIdentifier,
    )); "rejects DHCPACK with unspecified server identifier")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPACK),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: Some(LEASE_LENGTH_SECS),
        renewal_time_secs: Some(RENEWAL_TIME_SECS),
        rebinding_time_secs: Some(REBINDING_TIME_SECS),
        message: None,
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::UnspecifiedYiaddr); "rejects DHCPACK with unspecified yiaddr")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPNAK),
        server_identifier: Some(Ipv4Addr::UNSPECIFIED),
        subnet_mask: None,
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: Some(MESSAGE.to_owned()),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::CommonError(
        CommonIncomingMessageError::UnspecifiedServerIdentifier,
    )); "rejects DHCPNAK with unspecified server identifier")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPNAK),
        server_identifier: None,
        subnet_mask: None,
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: Some(MESSAGE.to_owned()),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::NoServerIdentifier) ; "rejects DHCPNAK with no server identifier")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREQUEST,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPNAK),
        server_identifier: Some(SERVER_IP),
        subnet_mask: None,
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: Some(MESSAGE.to_owned()),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::CommonError(
        CommonIncomingMessageError::NotBootReply(dhcp_protocol::OpCode::BOOTREQUEST),
    )) ; "rejects non-bootreply")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: Some(dhcp_protocol::MessageType::DHCPOFFER),
        server_identifier: Some(SERVER_IP),
        subnet_mask: Some(TEST_SUBNET_MASK),
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: Some(MESSAGE.to_owned()),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::NotDhcpAckOrNak(
        dhcp_protocol::MessageType::DHCPOFFER,
    )) ; "rejects non-DHCPACK or DHCPNAK")]
    #[test_case(VaryingReplyToRequestFields {
        op: dhcp_protocol::OpCode::BOOTREPLY,
        yiaddr: Ipv4Addr::UNSPECIFIED,
        message_type: None,
        server_identifier: Some(SERVER_IP),
        subnet_mask: None,
        lease_length_secs: None,
        renewal_time_secs: None,
        rebinding_time_secs: None,
        message: Some(MESSAGE.to_owned()),
        include_duplicate_option: false,
        include_illegal_option: false,
    } => Err(IncomingResponseToRequestError::CommonError(
        CommonIncomingMessageError::BuilderMissingField("message_type"),
    )) ; "rejects missing DHCP message type")]
    #[test_case( VaryingReplyToRequestFields {
            op: dhcp_protocol::OpCode::BOOTREPLY,
            yiaddr: YIADDR,
            message_type: Some(dhcp_protocol::MessageType::DHCPACK),
            server_identifier: Some(SERVER_IP),
            subnet_mask: Some(TEST_SUBNET_MASK),
            lease_length_secs: Some(LEASE_LENGTH_SECS),
            renewal_time_secs: Some(RENEWAL_TIME_SECS),
            rebinding_time_secs: Some(REBINDING_TIME_SECS),
            message: None,
            include_duplicate_option: true,
            include_illegal_option: false,
        } => Ok(IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: Some(
                net_types::ip::Ipv4Addr::from(SERVER_IP)
                    .try_into()
                    .expect("should be specified"),
            ),
            ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
            parameters: vec![
                dhcp_protocol::DhcpOption::SubnetMask(TEST_SUBNET_MASK),
                dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())
            ],
            renewal_time_value_secs: Some(RENEWAL_TIME_SECS),
            rebinding_time_value_secs: Some(REBINDING_TIME_SECS),
        })); "accepts good DHCPACK with duplicate option")]
    #[test_case( VaryingReplyToRequestFields {
            op: dhcp_protocol::OpCode::BOOTREPLY,
            yiaddr: YIADDR,
            message_type: Some(dhcp_protocol::MessageType::DHCPACK),
            server_identifier: Some(SERVER_IP),
            subnet_mask: Some(TEST_SUBNET_MASK),
            lease_length_secs: Some(LEASE_LENGTH_SECS),
            renewal_time_secs: None,
            rebinding_time_secs: None,
            message: None,
            include_duplicate_option: false,
            include_illegal_option: true,
        } => Ok(IncomingResponseToRequest::Ack(FieldsToRetainFromAck {
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: Some(
                net_types::ip::Ipv4Addr::from(SERVER_IP)
                    .try_into()
                    .expect("should be specified"),
            ),
            ip_address_lease_time_secs: Some(LEASE_LENGTH_SECS_NONZERO),
            parameters: vec![
                dhcp_protocol::DhcpOption::SubnetMask(TEST_SUBNET_MASK),
                dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())
            ],
            renewal_time_value_secs: None,
            rebinding_time_value_secs: None,
        })); "accepts good DHCPACK with illegal option")]
    fn fields_to_retain_during_requesting(
        incoming_fields: VaryingReplyToRequestFields,
    ) -> Result<
        IncomingResponseToRequest<Option<net_types::SpecifiedAddr<net_types::ip::Ipv4Addr>>>,
        IncomingResponseToRequestError,
    > {
        use super::fields_to_retain_from_response_to_request as fields;
        use dhcp_protocol::DhcpOption;

        let VaryingReplyToRequestFields {
            op,
            yiaddr,
            message_type,
            server_identifier,
            subnet_mask,
            lease_length_secs,
            renewal_time_secs,
            rebinding_time_secs,
            message,
            include_duplicate_option,
            include_illegal_option,
        } = incoming_fields;

        let message = dhcp_protocol::Message {
            op,
            xid: 1,
            secs: 0,
            bdcast_flag: false,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: net_mac!("01:02:03:04:05:06"),
            sname: BString::default(),
            file: BString::default(),
            options: std::iter::empty()
                .chain(message_type.map(DhcpOption::DhcpMessageType))
                .chain(server_identifier.map(DhcpOption::ServerIdentifier))
                .chain(subnet_mask.map(DhcpOption::SubnetMask))
                .chain(lease_length_secs.map(DhcpOption::IpAddressLeaseTime))
                .chain(renewal_time_secs.map(DhcpOption::RenewalTimeValue))
                .chain(rebinding_time_secs.map(DhcpOption::RebindingTimeValue))
                .chain(message.map(DhcpOption::Message))
                // Include a parameter that the client didn't request so that we can
                // assert that the client ignored it.
                .chain(std::iter::once(dhcp_protocol::DhcpOption::InterfaceMtu(1)))
                // Include a parameter that the client did request so that we can
                // check that it's included in the acquired parameters map.
                .chain(std::iter::once(dhcp_protocol::DhcpOption::DomainName(
                    DOMAIN_NAME.to_owned(),
                )))
                .chain(
                    include_duplicate_option
                        .then_some(dhcp_protocol::DhcpOption::DomainName(DOMAIN_NAME.to_owned())),
                )
                .chain(
                    include_illegal_option
                        // It's illegal for the DHCP server to provide the
                        // "Requested IP Address" option in any of its messages.
                        .then_some(dhcp_protocol::DhcpOption::RequestedIpAddress(SERVER_IP)),
                )
                .collect(),
        };

        fields(
            &[
                (dhcp_protocol::OptionCode::SubnetMask, OptionRequested::Required),
                (dhcp_protocol::OptionCode::DomainName, OptionRequested::Optional),
            ]
            .into_iter()
            .collect(),
            message,
        )
        .map(|(fields, soft_errors)| {
            let SoftParseErrors { illegal_option } = soft_errors;
            assert_eq!(illegal_option, include_illegal_option);
            fields
        })
    }
}
