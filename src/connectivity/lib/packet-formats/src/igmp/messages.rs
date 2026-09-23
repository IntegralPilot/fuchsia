// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implementation of IGMP Messages.

use core::borrow::Borrow as _;
use core::ops::Deref;

use net_types::ip::{Ip as _, Ipv4, Ipv4Addr};
use net_types::{MulticastAddr, Witness as _};
use packet::records::{ParsedRecord, RecordParseResult, Records, RecordsImpl, RecordsImplLayout};
use packet::{BufferView, FragmentedByteSlice, InnerPacketBuilder, ParsablePacket, ParseMetadata};
use zerocopy::byteorder::network_endian::U16;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, Unaligned};

use super::{
    IgmpMessage, IgmpNonEmptyBody, IgmpResponseTimeV2, IgmpResponseTimeV3, peek_message_type,
};
use crate::error::{ParseError, UnrecognizedProtocolCode};
use crate::gmp::{GmpReportGroupRecord, InvalidConstraintsError};
use crate::igmp::{IgmpPacketBuilder, MessageType};

create_protocol_enum!(
    /// An IGMP message type.
    #[allow(missing_docs)]
    #[derive(PartialEq, Copy, Clone)]
    pub enum IgmpMessageType: u8 {
        MembershipQuery, 0x11, "Membership Query";
        MembershipReportV1,0x12, "Membership Report V1";
        MembershipReportV2,0x16, "Membership Report V2";
        MembershipReportV3,0x22, "Membership Report V3";
        LeaveGroup, 0x17, "Leave Group";
    }
);

macro_rules! impl_igmp_simple_message_type {
    ($type:ident, $code:tt, $fixed_header:ident) => {
        impl<B> MessageType<B> for $type {
            type FixedHeader = $fixed_header;
            const TYPE: IgmpMessageType = IgmpMessageType::$code;
            type MaxRespTime = ();
            declare_no_body!();
        }
    };
}

macro_rules! declare_no_body {
    () => {
        type VariableBody = ();

        fn parse_body<BV: BufferView<B>>(
            _header: &Self::FixedHeader,
            bytes: BV,
        ) -> Result<Self::VariableBody, ParseError>
        where
            B: SplitByteSlice,
        {
            if bytes.len() != 0 { Err(ParseError::NotExpected) } else { Ok(()) }
        }

        fn body_bytes(_body: &Self::VariableBody) -> &[u8]
        where
            B: SplitByteSlice,
        {
            &[]
        }
    };
}

/// IGMPv2 Membership Query message.
///
/// `IgmpMembershipQueryV2` implements `MessageType`, providing the intended
/// behavior for IGMPv2 Membership Queries as defined in [RFC 2236].
///
/// There are two sub-types of Membership Query messages:
/// - General Query, used to learn which groups have members on an
///   attached network.
/// - Group-Specific Query, used to learn if a particular group
///   has any members on an attached network.
///
/// These two messages are differentiated by the Group Address, as
/// described in [RFC 2236 section 1.4].
///
/// [RFC 2236]: https://tools.ietf.org/html/rfc2236
/// [RFC 2236 section 1.4]: https://tools.ietf.org/html/rfc2236#section-1.4
#[derive(Copy, Clone, Debug)]
pub struct IgmpMembershipQueryV2;

impl<B> MessageType<B> for IgmpMembershipQueryV2 {
    type FixedHeader = Ipv4Addr;
    type MaxRespTime = IgmpResponseTimeV2;
    const TYPE: IgmpMessageType = IgmpMessageType::MembershipQuery;

    declare_no_body!();
}

impl<B: SplitByteSlice> IgmpMessage<B, IgmpMembershipQueryV2> {
    /// Returns true if this is an IGMPv1 query.
    ///
    /// Defined in [RFC 2236 section 4.1]:
    ///
    /// > The IGMPv1 router will send General Queries with the Max Response Time
    /// > set to 0...
    ///
    /// [RFC 2236 section 4.1]:
    ///     https://datatracker.ietf.org/doc/html/rfc2236#section-4
    pub fn is_igmpv1_query(&self) -> bool {
        self.prefix.max_resp_code == 0
    }
}

/// Fixed information in IGMPv3 Membership Queries.
///
/// A `MembershipQueryData` struct represents the fixed data in IGMPv3
/// Membership Queries.
/// It is defined as the `FixedHeader` type for `IgmpMembershipQueryV3`.
#[derive(Copy, Clone, Debug, IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct MembershipQueryData {
    group_address: Ipv4Addr,
    sqrv: u8,
    qqic: u8,
    number_of_sources: U16,
}

impl MembershipQueryData {
    const S_FLAG: u8 = (1 << 3);
    const QRV_MSK: u8 = 0x07;

    /// Returns the the number of sources.
    pub fn number_of_sources(&self) -> u16 {
        self.number_of_sources.get()
    }

    /// Returns the query's group address.
    pub fn group_address(&self) -> Ipv4Addr {
        self.group_address
    }

    /// Gets value of `S Flag`.
    ///
    /// Indicates to any receiving multicast routers that they are to suppress
    /// the normal timer updates they perform upon hearing a Query. It does not,
    /// however, suppress the querier election or the normal "host-side"
    /// processing of a Query that a router may be required to perform as a
    /// consequence of itself being a group member.
    pub fn suppress_router_side_processing(&self) -> bool {
        (self.sqrv & Self::S_FLAG) != 0
    }

    /// Gets querier robustness variable (QRV).
    ///
    /// If non-zero, the QRV field contains the *Robustness Variable* value
    /// used by the querier, i.e., the sender of the Query.  If the querier's
    /// *Robustness Variable* exceeds 7, the maximum value of the QRV field,
    /// the QRV is set to zero.  Routers adopt the QRV value from the most
    /// recently received Query as their own *Robustness Variable* value,
    /// unless that most recently received QRV was zero, in which case the
    /// receivers use the default *Robustness Variable* value specified in
    /// [RFC 3376 section 8.1] (defined as `DEFAULT_QRV`) or a statically
    /// configured value.
    ///
    /// [RFC 3376 section 8.1]: https://tools.ietf.org/html/rfc3376#section-8.1
    pub fn querier_robustness_variable(&self) -> u8 {
        self.sqrv & Self::QRV_MSK
    }

    /// Gets the querier's query interval (QQI).
    ///
    /// Multicast routers that are not the current querier adopt the QQI
    /// value from the most recently received Query as their own *Query
    /// Interval* value, unless that most recently received QQI was zero, in
    /// which case the receiving routers use the default *Query Interval*
    /// value specified in [RFC 3376 section 8.2], defined as
    /// `DEFAULT_QUERY_INTERVAL`.
    ///
    /// [RFC 3376 section 8.2]: https://tools.ietf.org/html/rfc3376#section-8.2
    pub fn querier_query_interval(&self) -> core::time::Duration {
        // qqic is represented in a packed floating point format and interpreted
        // as units of seconds.
        Igmpv3QQIC::from(self.qqic).into()
    }
}

/// QRV (Querier's Robustness Variable) defined
/// in [RFC 3376 section 4.1.6].
///
/// Aliased to shared GMP implementation for convenience.
///
/// [RFC 3376 section 4.1.6]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1.6
pub type Igmpv3QRV = crate::gmp::QRV;

/// QQIC (Querier's Query Interval Code) defined in [RFC 3376 section 4.1.7].
///
/// Aliased to shared GMP implementation for convenience.
///
/// [RFC 3376 section 4.1.7]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1.7
pub type Igmpv3QQIC = crate::gmp::QQIC;

/// IGMPv3 Membership Query message.
///
/// `IgmpMembershipQueryV3` implements `MessageType`, providing the intended
/// behavior for IGMPv3 Membership Queries as defined in
/// [RFC 3376 section 4.1].
///
/// Membership Queries are sent by IP multicast routers to query the
/// multicast reception state of neighboring interfaces.
///
/// [RFC 3376 section 4.1]: https://tools.ietf.org/html/rfc3376#section-4.1
#[derive(Copy, Clone, Debug)]
pub struct IgmpMembershipQueryV3;

impl<B> IgmpNonEmptyBody for Ref<B, [Ipv4Addr]> {}

impl<B> MessageType<B> for IgmpMembershipQueryV3 {
    type FixedHeader = MembershipQueryData;
    type VariableBody = Ref<B, [Ipv4Addr]>;
    type MaxRespTime = IgmpResponseTimeV3;
    const TYPE: IgmpMessageType = IgmpMessageType::MembershipQuery;

    fn parse_body<BV: BufferView<B>>(
        header: &Self::FixedHeader,
        mut bytes: BV,
    ) -> Result<Self::VariableBody, ParseError>
    where
        B: SplitByteSlice,
    {
        bytes
            .take_slice_front::<Ipv4Addr>(header.number_of_sources() as usize)
            .ok_or(ParseError::Format)
    }

    fn body_bytes(body: &Self::VariableBody) -> &[u8]
    where
        B: SplitByteSlice,
    {
        Ref::bytes(body)
    }
}

impl<B: SplitByteSlice> IgmpMessage<B, IgmpMembershipQueryV3> {
    /// Reinterprets this [`IgmpMembershipQueryV3`] message as an
    /// [`IgmpMembershipQueryV2`] message.
    ///
    /// Given this crate parses the version separately, users desiring to
    /// operate in IGMPv2 or IGMPv1 modes *SHOULD* reinterpret V3 queries as the
    /// older version.
    ///
    /// See [RFC 3376 section 7.2.1] and [RFC 2236 section 2.5].
    ///
    /// [RFC 3376 section 7.2.1]:
    ///     https://datatracker.ietf.org/doc/html/rfc3376#section-7.2.1
    /// [RFC 2236 section 2.5]:
    ///     https://datatracker.ietf.org/doc/html/rfc2236#section-2.5
    pub fn as_v2_query(&self) -> IgmpMessage<&[u8], IgmpMembershipQueryV2> {
        let Self { prefix, header, body: _ } = self;
        // Unwraps are okay here, we know the sizes and alignments must fit.
        let prefix = Ref::from_bytes(prefix.as_bytes()).unwrap();
        let (header, _rest) = Ref::from_prefix(header.as_bytes()).unwrap();
        IgmpMessage { prefix, header, body: () }
    }
}

/// Fixed information in IGMPv3 Membership Reports.
///
/// A `MembershipReportV3Data` struct represents the fixed data in IGMPv3
/// Membership Reports.
/// It is defined as the `FixedHeader` type for `IgmpMembershipReportV3`.
#[derive(Copy, Clone, Debug, IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct MembershipReportV3Data {
    _reserved: [u8; 2],
    number_of_group_records: U16,
}

impl MembershipReportV3Data {
    /// Returns the number of group records.
    pub fn number_of_group_records(self) -> u16 {
        self.number_of_group_records.get()
    }
}

/// Group Record Types as defined in [RFC 3376 section 4.2.12]
///
/// Aliased to shared GMP implementation for convenience.
///
/// [RFC 3376 section 4.2.12]:
///     https://tools.ietf.org/html/rfc3376#section-4.2.12
pub type IgmpGroupRecordType = crate::gmp::GroupRecordType;

/// Fixed information for IGMPv3 Membership Report's Group Records.
///
/// A `GroupRecordHeader` struct represents the fixed header of a Group Record
/// in IGMPv3 Membership Report messages as defined in [RFC 3376 section 4.2.4].
///
/// The `GroupRecordHeader` is followed by a series of source IPv4 addresses.
///
/// [RFC 3376 section 4.2.4]: https://tools.ietf.org/html/rfc3376#section-4.2.4
#[derive(Copy, Clone, Debug, IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct GroupRecordHeader {
    record_type: u8,
    aux_data_len: u8,
    number_of_sources: U16,
    multicast_address: Ipv4Addr,
}

impl GroupRecordHeader {
    /// Returns the number of sources.
    pub fn number_of_sources(&self) -> u16 {
        self.number_of_sources.get()
    }

    /// Returns the type of the record.
    pub fn record_type(&self) -> Result<IgmpGroupRecordType, UnrecognizedProtocolCode<u8>> {
        IgmpGroupRecordType::try_from(self.record_type)
    }

    /// Returns the multicast address.
    pub fn multicast_addr(&self) -> &Ipv4Addr {
        &self.multicast_address
    }
}

/// Wire representation of an IGMPv3 Membership Report's Group Records.
///
/// A `GroupRecord` struct is composed of a fixed part provided by
/// `GroupRecordHeader` and a variable part, which is a list of IPv4 addresses.
///
/// Each Group Record is a block of fields containing information
/// pertaining to the sender's membership in a single multicast group on
/// the interface from which the Report is sent.
///
/// The structure of a Group Record includes Auxiliary Data, as defined in
/// [RFC 3376 section 4.2.10]. As the information in Auxiliary Data is supposed
/// to be ignored, when parsing a `GroupRecord` struct from a buffer, the
/// information in Auxiliary Data, if any, is discarded.
///
/// [RFC 3376 section 4.2.10]: https://tools.ietf.org/html/rfc3376#section-4.2.10
pub struct GroupRecord<B> {
    header: Ref<B, GroupRecordHeader>,
    sources: Ref<B, [Ipv4Addr]>,
}

impl<B: SplitByteSlice> GroupRecord<B> {
    /// Returns the group record header.
    pub fn header(&self) -> &GroupRecordHeader {
        self.header.deref()
    }

    /// Returns the group record's sources.
    pub fn sources(&self) -> &[Ipv4Addr] {
        self.sources.deref()
    }
}

/// IGMPv3 Membership Report message.
///
/// `IgmpMembershipReportV3` implements `MessageType`, providing the intended
/// behavior for IGMPv3 Membership Reports as defined in
/// [RFC 3376 section 4.2].
///
/// Version 3 Membership Reports are sent by IP systems to report (to
/// neighboring routers) the current multicast reception state, or
/// changes in the multicast reception state, of their interfaces.
///
/// [RFC 3376 section 4.2]: https://tools.ietf.org/html/rfc3376#section-4.2
#[derive(Copy, Clone, Debug)]
pub struct IgmpMembershipReportV3;

impl<B> IgmpNonEmptyBody for Records<B, IgmpMembershipReportV3> {}

impl<B> MessageType<B> for IgmpMembershipReportV3 {
    type FixedHeader = MembershipReportV3Data;
    type VariableBody = Records<B, IgmpMembershipReportV3>;
    type MaxRespTime = ();
    const TYPE: IgmpMessageType = IgmpMessageType::MembershipReportV3;

    fn parse_body<BV: BufferView<B>>(
        header: &Self::FixedHeader,
        bytes: BV,
    ) -> Result<Self::VariableBody, ParseError>
    where
        B: SplitByteSlice,
    {
        Records::parse_with_context(bytes.into_rest(), header.number_of_group_records().into())
    }

    fn body_bytes(body: &Self::VariableBody) -> &[u8]
    where
        B: SplitByteSlice,
    {
        body.bytes()
    }
}

impl RecordsImplLayout for IgmpMembershipReportV3 {
    type Context = usize;
    type Error = ParseError;
}

impl RecordsImpl for IgmpMembershipReportV3 {
    type Record<'a> = GroupRecord<&'a [u8]>;

    fn parse_with_context<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        ctx: &mut usize,
    ) -> RecordParseResult<GroupRecord<&'a [u8]>, ParseError> {
        if *ctx == 0 {
            return Ok(ParsedRecord::Done);
        }
        let header = data
            .take_obj_front::<GroupRecordHeader>()
            .ok_or_else(debug_err_fn!(ParseError::Format, "Can't take group record header"))?;
        let sources = data
            .take_slice_front::<Ipv4Addr>(header.number_of_sources().into())
            .ok_or_else(debug_err_fn!(ParseError::Format, "Can't group record sources"))?;
        // every record may have aux_data_len 32-bit words at the end.
        // we need to update our buffer view to reflect that.
        let _ = data
            .take_front(usize::from(header.aux_data_len) * 4)
            .ok_or_else(debug_err_fn!(ParseError::Format, "Can't skip auxiliary data"))?;

        *ctx -= 1;
        Ok(ParsedRecord::Parsed(Self::Record { header, sources }))
    }
}

/// IGMPv1 Membership Report message.
///
/// `IgmpMembershipReportV1` implements `MessageType`, providing the intended
/// behavior for IGMPv1 Membership Reports as defined in [RFC 1112].
///
/// In a Host Membership Report message, the group address field (expressed in
/// `FixedHeader`) holds the IP host group address of the group being reported.
///
/// Hosts respond to a Query by generating Host Membership Reports, reporting
/// each host group to which they belong on the network interface from which the
/// Query was received.
///
/// [RFC 1112]: https://tools.ietf.org/html/rfc1112
#[derive(Debug)]
pub struct IgmpMembershipReportV1;

impl_igmp_simple_message_type!(IgmpMembershipReportV1, MembershipReportV1, Ipv4Addr);

/// IGMPv2 Membership Report message.
///
/// `IgmpMembershipReportV2` implements `MessageType`, providing the intended
/// behavior for IGMPv2 Membership Reports as defined in [RFC 2236].
///
/// In a Membership Report message, the group address field (expressed in
/// `FixedHeader`) holds the IP multicast group address of the group being
/// reported.
///
/// Hosts respond to a Query by generating Host Membership Reports, reporting
/// each host group to which they belong on the network interface from which the
/// Query was received.
///
/// [RFC 2236]: https://tools.ietf.org/html/rfc2236
#[derive(Debug)]
pub struct IgmpMembershipReportV2;

impl_igmp_simple_message_type!(IgmpMembershipReportV2, MembershipReportV2, Ipv4Addr);

/// IGMP Leave Group message.
///
/// `IgmpLeaveGroup` implements `MessageType`, providing the intended behavior
/// for IGMP LeaveGroup as defined in [RFC 2236].
///
/// In a Leave Group message, the group address field (expressed in
/// `FixedHeader`) holds the IP multicast group address of the group being
/// left.
///
/// When a host leaves a multicast group, if it was the last host to reply to a
/// Query with a Membership Report for that group, it SHOULD send a Leave Group
/// message to the all-routers multicast group (224.0.0.2).
///
/// [RFC 2236]: https://tools.ietf.org/html/rfc2236
#[derive(Debug)]
pub struct IgmpLeaveGroup;

impl_igmp_simple_message_type!(IgmpLeaveGroup, LeaveGroup, Ipv4Addr);

/// An IGMP packet with a dynamic message type.
///
/// Each enum variant contains an `IgmpMessage` of
/// the appropriate static type, making it easier to call `parse` without
/// knowing the message type ahead of time while still getting the benefits of a
/// statically-typed packet struct after parsing is complete.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum IgmpPacket<B: SplitByteSlice> {
    MembershipQueryV2(IgmpMessage<B, IgmpMembershipQueryV2>),
    MembershipQueryV3(IgmpMessage<B, IgmpMembershipQueryV3>),
    MembershipReportV1(IgmpMessage<B, IgmpMembershipReportV1>),
    MembershipReportV2(IgmpMessage<B, IgmpMembershipReportV2>),
    MembershipReportV3(IgmpMessage<B, IgmpMembershipReportV3>),
    LeaveGroup(IgmpMessage<B, IgmpLeaveGroup>),
}

impl<B: SplitByteSlice> ParsablePacket<B, ()> for IgmpPacket<B> {
    type Error = ParseError;

    fn parse_metadata(&self) -> ParseMetadata {
        use self::IgmpPacket::*;
        match self {
            MembershipQueryV2(p) => p.parse_metadata(),
            MembershipQueryV3(p) => p.parse_metadata(),
            MembershipReportV1(p) => p.parse_metadata(),
            MembershipReportV2(p) => p.parse_metadata(),
            MembershipReportV3(p) => p.parse_metadata(),
            LeaveGroup(p) => p.parse_metadata(),
        }
    }

    fn parse<BV: BufferView<B>>(buffer: BV, args: ()) -> Result<Self, ParseError> {
        macro_rules! mtch {
            ($buffer:expr, $args:expr, $( ($code:ident, $long:tt) => $type:ty, $variant:ident )*) => {
                match peek_message_type($buffer.as_ref())? {
                    $( (IgmpMessageType::$code, $long) => {
                        let packet = <IgmpMessage<B,$type> as ParsablePacket<_, _>>::parse($buffer, $args)?;
                        IgmpPacket::$variant(packet)
                    })*,
                }
            }
        }

        Ok(mtch!(
            buffer,
            args,
            (MembershipQuery, false) => IgmpMembershipQueryV2, MembershipQueryV2
            (MembershipQuery, true) => IgmpMembershipQueryV3, MembershipQueryV3
            (MembershipReportV1, _) => IgmpMembershipReportV1, MembershipReportV1
            (MembershipReportV2, _) => IgmpMembershipReportV2, MembershipReportV2
            (MembershipReportV3, _) => IgmpMembershipReportV3, MembershipReportV3
            (LeaveGroup, _) => IgmpLeaveGroup, LeaveGroup
        ))
    }
}

/// A builder for IGMPv3 membership queries.
///
/// This differs from [`IgmpPacketBuilder`] in that it implements
/// [`InnerPacketBuilder`] directly, and provides a more ergonomic way of
/// building queries than nesting builders.
#[derive(Debug)]
pub struct IgmpMembershipQueryV3Builder<I> {
    max_resp_time: IgmpResponseTimeV3,
    group_addr: Option<MulticastAddr<Ipv4Addr>>,
    s_flag: bool,
    qrv: Igmpv3QRV,
    qqic: Igmpv3QQIC,
    sources: I,
}

impl<I> IgmpMembershipQueryV3Builder<I> {
    /// Creates a new [`IgmpMembershipQueryV3Builder`].
    pub fn new(
        max_resp_time: IgmpResponseTimeV3,
        group_addr: Option<MulticastAddr<Ipv4Addr>>,
        s_flag: bool,
        qrv: Igmpv3QRV,
        qqic: Igmpv3QQIC,
        sources: I,
    ) -> Self {
        Self { max_resp_time, group_addr, s_flag, qrv, qqic, sources }
    }

    const HEADER_SIZE: usize = super::total_header_size::<MembershipQueryData>();
}

impl<I> InnerPacketBuilder for IgmpMembershipQueryV3Builder<I>
where
    I: Iterator<Item = Ipv4Addr> + Clone,
{
    fn bytes_len(&self) -> usize {
        Self::HEADER_SIZE + self.sources.clone().count() * core::mem::size_of::<Ipv4Addr>()
    }

    fn serialize(&self, buf: &mut [u8]) {
        use packet::BufferViewMut;

        let Self { max_resp_time, group_addr, s_flag, qrv, qqic, sources } = self;
        let (header, body) = buf.split_at_mut(Self::HEADER_SIZE);
        let mut bytes = &mut body[..];
        let mut bytes = &mut bytes;
        // Serialize the body first, counting the records.
        let mut count: u16 = 0;
        for src in sources.clone() {
            count = count.checked_add(1).expect("overflowed number of sources");
            bytes.write_obj_front(&src).expect("too few bytes for source");
        }
        let builder = IgmpPacketBuilder::<&mut [u8], IgmpMembershipQueryV3>::new_with_resp_time(
            MembershipQueryData {
                group_address: group_addr
                    .as_ref()
                    .map(|a| a.get())
                    .unwrap_or(Ipv4::UNSPECIFIED_ADDRESS),
                sqrv: (u8::from(*s_flag) << 3) | (MembershipQueryData::QRV_MSK & u8::from(*qrv)),
                qqic: (*qqic).into(),
                number_of_sources: count.into(),
            },
            *max_resp_time,
        );
        builder.serialize_headers(header, FragmentedByteSlice::new(&mut [body][..]));
    }
}

/// A builder for IGMPv3 membership reports.
///
/// This differs from [`IgmpPacketBuilder`] in that it implements
/// [`InnerPacketBuilder`\ directly, and provides a more ergonomic way of
/// building reports than nesting builders.
#[derive(Debug)]
pub struct IgmpMembershipReportV3Builder<I> {
    groups: I,
}

impl<I> IgmpMembershipReportV3Builder<I> {
    /// Creates a new [`IgmpMembershipReportV3Builder`].
    pub fn new(groups: I) -> Self {
        Self { groups }
    }

    const HEADER_SIZE: usize = super::total_header_size::<MembershipReportV3Data>();
}

impl<I> IgmpMembershipReportV3Builder<I>
where
    I: Iterator<Item: GmpReportGroupRecord<Ipv4Addr> + Clone> + Clone,
{
    /// Transform this builder into an iterator of builders with a given
    /// `max_len` for each generated packet.
    ///
    /// `max_len` is the maximum length each builder yielded by the returned
    /// iterator can have. The groups used to create this builder are split into
    /// multiple reports in order to meet this length. Note that this length
    /// does _not_ account for the IP header, but it accounts for the entire
    /// IGMP packet.
    ///
    /// Returns `Err` if `max_len` is not large enough to meet minimal
    /// constraints for each report.
    pub fn with_len_limits(
        self,
        max_len: usize,
    ) -> Result<
        impl Iterator<
            Item = IgmpMembershipReportV3Builder<
                impl Iterator<Item: GmpReportGroupRecord<Ipv4Addr>> + Clone,
            >,
        >,
        InvalidConstraintsError,
    > {
        let Self { groups } = self;
        crate::gmp::group_record_split_iterator(
            max_len.saturating_sub(Self::HEADER_SIZE),
            core::mem::size_of::<GroupRecordHeader>(),
            groups,
        )
        .map(|iter| iter.map(|groups| IgmpMembershipReportV3Builder { groups }))
    }
}

impl<I> InnerPacketBuilder for IgmpMembershipReportV3Builder<I>
where
    I: Iterator<Item: GmpReportGroupRecord<Ipv4Addr>> + Clone,
{
    fn bytes_len(&self) -> usize {
        Self::HEADER_SIZE
            + self
                .groups
                .clone()
                .map(|g| {
                    core::mem::size_of::<GroupRecordHeader>()
                        + g.sources().count() * core::mem::size_of::<Ipv4Addr>()
                })
                .sum::<usize>()
    }

    fn serialize(&self, buf: &mut [u8]) {
        use packet::BufferViewMut;

        let Self { groups } = self;
        let (header, body) = buf.split_at_mut(Self::HEADER_SIZE);
        let mut bytes = &mut body[..];
        let mut bytes = &mut bytes;
        // Serialize the body first, counting the records.
        let mut count: u16 = 0;
        for group in groups.clone() {
            count = count.checked_add(1).expect("multicast groups count overflows");
            let mut header = bytes
                .take_obj_front_zero::<GroupRecordHeader>()
                .expect("too few bytes for record header");
            let GroupRecordHeader {
                record_type,
                aux_data_len,
                number_of_sources,
                multicast_address,
            } = &mut *header;
            *record_type = group.record_type().into();
            *aux_data_len = 0;
            *multicast_address = group.group().into();
            let mut source_count: u16 = 0;
            for src in group.sources() {
                source_count = source_count.checked_add(1).expect("sources count overflows");
                bytes.write_obj_front(src.borrow()).expect("too few bytes for source");
            }
            *number_of_sources = source_count.into();
        }

        let builder =
            IgmpPacketBuilder::<&mut [u8], IgmpMembershipReportV3>::new(MembershipReportV3Data {
                _reserved: [0, 0],
                number_of_group_records: count.into(),
            });
        builder.serialize_headers(header, FragmentedByteSlice::new(&mut [body][..]));
    }
}

#[cfg(test)]
mod tests {
    use core::fmt::Debug;
    use core::time::Duration;

    use packet::{NestablePacketBuilder as _, NoOpSerializationContext, ParseBuffer, Serializer};

    use super::*;
    use crate::igmp::IgmpMaxRespCode;
    use crate::igmp::testdata::*;
    use crate::ip::Ipv4Proto;
    use crate::ipv4::options::Ipv4Option;
    use crate::ipv4::{Ipv4Packet, Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions};
    use crate::testutil::set_logger_for_test;
    use net_declare::net_ip_v4;

    const ALL_BUFFERS: [&[u8]; 6] = [
        igmp_router_queries::v2::QUERY,
        igmp_router_queries::v3::QUERY,
        igmp_reports::v1::MEMBER_REPORT,
        igmp_reports::v2::MEMBER_REPORT,
        igmp_reports::v3::MEMBER_REPORT,
        igmp_leave_group::LEAVE_GROUP,
    ];

    fn serialize_to_bytes<B: SplitByteSlice + Debug, M: MessageType<B> + Debug>(
        igmp: &IgmpMessage<B, M>,
    ) -> Vec<u8>
    where
        M::VariableBody: IgmpNonEmptyBody,
    {
        igmp.builder()
            .wrap_body(M::body_bytes(&igmp.body).into_serializer())
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn serialize_to_bytes_inner<
        B: SplitByteSlice + Debug,
        M: MessageType<B, VariableBody = ()> + Debug,
    >(
        igmp: &IgmpMessage<B, M>,
    ) -> Vec<u8> {
        igmp.builder()
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn test_parse_and_serialize<
        B: SplitByteSlice + Debug,
        BV: BufferView<B>,
        M: MessageType<B> + Debug,
        F: FnOnce(&IgmpMessage<B, M>),
    >(
        req: BV,
        check: F,
    ) where
        M::VariableBody: IgmpNonEmptyBody,
    {
        let orig_req = req.as_ref().to_owned();

        let igmp = IgmpMessage::<_, M>::parse(req, ()).unwrap();
        check(&igmp);

        let data = serialize_to_bytes(&igmp);
        assert_eq!(data, orig_req);
    }

    fn test_parse_and_serialize_inner<
        M: for<'a> MessageType<&'a [u8], VariableBody = ()> + Debug,
        F: for<'a> FnOnce(&IgmpMessage<&'a [u8], M>),
    >(
        mut req: &[u8],
        check: F,
    ) {
        let orig_req = req;

        let igmp = req.parse_with::<_, IgmpMessage<_, M>>(()).unwrap();
        check(&igmp);

        let data = serialize_to_bytes_inner(&igmp);
        assert_eq!(&data[..], orig_req);
    }

    #[test]
    fn membership_query_v2_parse_and_serialize() {
        set_logger_for_test();
        test_parse_and_serialize_inner::<IgmpMembershipQueryV2, _>(
            igmp_router_queries::v2::QUERY,
            |igmp| {
                assert_eq!(
                    *igmp.header,
                    Ipv4Addr::new(igmp_router_queries::v2::HOST_GROUP_ADDRESS)
                );
                assert_eq!(igmp.prefix.max_resp_code, igmp_router_queries::v2::MAX_RESP_CODE);
            },
        );
    }

    #[test]
    fn membership_query_v3_parse_and_serialize() {
        set_logger_for_test();
        let mut req = igmp_router_queries::v3::QUERY;
        test_parse_and_serialize::<_, _, IgmpMembershipQueryV3, _>(&mut req, |igmp| {
            assert_eq!(igmp.prefix.max_resp_code, igmp_router_queries::v3::MAX_RESP_CODE);
            assert_eq!(
                igmp.header.group_address,
                Ipv4Addr::new(igmp_router_queries::v3::GROUP_ADDRESS)
            );
            assert_eq!(igmp.header.number_of_sources(), igmp_router_queries::v3::NUMBER_OF_SOURCES);
            assert_eq!(
                igmp.header.suppress_router_side_processing(),
                igmp_router_queries::v3::SUPPRESS_ROUTER_SIDE
            );
            assert_eq!(igmp.header.querier_robustness_variable(), igmp_router_queries::v3::QRV);
            assert_eq!(
                igmp.header.querier_query_interval().as_secs() as u32,
                igmp_router_queries::v3::QQIC_SECS
            );
            assert_eq!(igmp.body.len(), igmp_router_queries::v3::NUMBER_OF_SOURCES as usize);
            assert_eq!(igmp.body[0], Ipv4Addr::new(igmp_router_queries::v3::SOURCE));

            // When interpreted as a v2 query we should get the same values.
            let v2 = igmp.as_v2_query();
            assert_eq!(v2.prefix.max_resp_code, igmp_router_queries::v3::MAX_RESP_CODE);
            assert_eq!(*(v2.header), Ipv4Addr::new(igmp_router_queries::v3::GROUP_ADDRESS));
        });
    }

    #[test]
    fn membership_report_v3_parse_and_serialize() {
        use igmp_reports::v3::*;

        set_logger_for_test();
        let mut req = MEMBER_REPORT;
        test_parse_and_serialize::<_, _, IgmpMembershipReportV3, _>(&mut req, |igmp| {
            assert_eq!(igmp.header.number_of_group_records(), NUMBER_OF_RECORDS);
            assert_eq!(igmp.prefix.max_resp_code, MAX_RESP_CODE);
            let mut iter = igmp.body.iter();
            // look at first group record:
            let rec1 = iter.next().unwrap();
            assert_eq!(rec1.header().number_of_sources(), NUMBER_OF_SOURCES_1);
            assert_eq!(rec1.header().record_type, RECORD_TYPE_1);
            assert_eq!(rec1.header().multicast_address, Ipv4Addr::new(MULTICAST_ADDR_1));
            assert_eq!(rec1.header().record_type(), Ok(IgmpGroupRecordType::ModeIsInclude));
            assert_eq!(rec1.sources().len(), NUMBER_OF_SOURCES_1 as usize);
            assert_eq!(rec1.sources()[0], Ipv4Addr::new(SRC_1_1));
            assert_eq!(rec1.sources()[1], Ipv4Addr::new(SRC_1_2));

            // look at second group record:
            let rec2 = iter.next().unwrap();
            assert_eq!(rec2.header().number_of_sources(), NUMBER_OF_SOURCES_2);
            assert_eq!(rec2.header().record_type, RECORD_TYPE_2);
            assert_eq!(rec2.header().multicast_address, Ipv4Addr::new(MULTICAST_ADDR_2));
            assert_eq!(rec2.header().record_type(), Ok(IgmpGroupRecordType::ModeIsExclude));
            assert_eq!(rec2.sources().len(), NUMBER_OF_SOURCES_2 as usize);
            assert_eq!(rec2.sources()[0], Ipv4Addr::new(SRC_2_1));

            // assert that no other records came in:
            assert_eq!(iter.next().is_none(), true);
        });
    }

    #[test]
    fn membership_query_v3_builder() {
        set_logger_for_test();
        let builder = IgmpMembershipQueryV3Builder::new(
            IgmpResponseTimeV3::from_code(igmp_router_queries::v3::MAX_RESP_CODE),
            Some(
                MulticastAddr::new(Ipv4Addr::new(igmp_router_queries::v3::GROUP_ADDRESS)).unwrap(),
            ),
            igmp_router_queries::v3::SUPPRESS_ROUTER_SIDE,
            Igmpv3QRV::new(igmp_router_queries::v3::QRV),
            Igmpv3QQIC::new_exact(Duration::from_secs(igmp_router_queries::v3::QQIC_SECS.into()))
                .unwrap(),
            [Ipv4Addr::new(igmp_router_queries::v3::SOURCE)].into_iter(),
        );
        let serialized = builder
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b();
        assert_eq!(serialized.as_ref(), igmp_router_queries::v3::QUERY);
    }

    #[test]
    fn membership_report_v3_builder() {
        set_logger_for_test();
        use igmp_reports::v3::*;
        let builder = IgmpMembershipReportV3Builder::new(
            [
                (MULTICAST_ADDR_1, RECORD_TYPE_1, &[SRC_1_1, SRC_1_2][..]),
                (MULTICAST_ADDR_2, RECORD_TYPE_2, &[SRC_2_1][..]),
            ]
            .into_iter()
            .map(|(addr, rec_type, sources)| {
                (
                    MulticastAddr::new(Ipv4Addr::new(addr)).unwrap(),
                    IgmpGroupRecordType::try_from(rec_type).unwrap(),
                    sources.iter().copied().map(Ipv4Addr::new),
                )
            }),
        );
        let serialized = builder
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b();
        assert_eq!(serialized.as_ref(), MEMBER_REPORT);
    }

    // Test that our maximum sources accounting matches an equivalent example
    // in RFC 3376 section 4.1.8:
    //
    //  For example, on an Ethernet with an MTU of 1500 octets, the IP header
    //  including the Router Alert option consumes 24 octets, and the IGMP
    //  fields up to including the Number of Sources (N) field consume 12
    //  octets, leaving 1464 octets for source addresses, which limits the
    //  number of source addresses to 366 (1464/4).
    //
    // This example is for queries, reports have 8 octets of IGMP headers + 8
    // octets of group record header, hence (1500 - 24 - 16)/4 = 365.
    #[test]
    fn membership_report_v3_split_many_sources() {
        use igmp_reports::v3::*;
        use packet::{NestablePacketBuilder as _, NestableSerializer as _};
        const ETH_MTU: usize = 1500;
        const MAX_SOURCES: usize = 365;

        let src = Ipv4Addr::new(SRC_1_1);
        let ip_builder = Ipv4PacketBuilderWithOptions::new(
            Ipv4PacketBuilder::new(src, src, 1, Ipv4Proto::Igmp),
            &[Ipv4Option::RouterAlert { data: 0 }],
        )
        .unwrap();
        let ip_header = ip_builder.constraints().header_len();

        let src_ip = |i: usize| Ipv4Addr::new([10, 0, (i >> 8) as u8, i as u8]);
        let group_addr = MulticastAddr::new(Ipv4Addr::new(MULTICAST_ADDR_1)).unwrap();
        let reports = IgmpMembershipReportV3Builder::new(
            [(
                group_addr,
                IgmpGroupRecordType::ModeIsInclude,
                (0..MAX_SOURCES).into_iter().map(|i| src_ip(i)),
            )]
            .into_iter(),
        )
        .with_len_limits(ETH_MTU - ip_header)
        .unwrap();

        let mut reports = reports.map(|builder| {
            builder
                .into_serializer()
                .wrap_in(ip_builder.clone())
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap_or_else(|(err, _)| panic!("{err:?}"))
                .unwrap_b()
                .into_inner()
        });
        // We can generate a report at exactly ETH_MTU.
        let serialized = reports.next().unwrap();
        assert_eq!(serialized.len(), ETH_MTU);

        let mut buffer = &serialized[..];
        let _ip = buffer.parse_with::<_, Ipv4Packet<_>>(()).unwrap();
        let igmp = buffer.parse::<IgmpMessage<_, IgmpMembershipReportV3>>().unwrap();
        let mut groups = igmp.body.iter();
        let group = groups.next().expect("has group");
        assert_eq!(group.header.multicast_address, group_addr.get());
        assert_eq!(usize::from(group.header.number_of_sources()), MAX_SOURCES);
        assert_eq!(group.sources().len(), MAX_SOURCES);
        for (i, addr) in group.sources().iter().enumerate() {
            assert_eq!(*addr, src_ip(i));
        }
        assert_eq!(groups.next().map(|r| r.header.multicast_address), None);

        // Only one report is generated.
        assert_eq!(reports.next(), None);

        let reports = IgmpMembershipReportV3Builder::new(
            [(
                group_addr,
                IgmpGroupRecordType::ModeIsInclude,
                core::iter::repeat(src).take(MAX_SOURCES + 1),
            )]
            .into_iter(),
        )
        .with_len_limits(ETH_MTU - ip_header)
        .unwrap();
        // 2 reports are generated with one extra source.
        assert_eq!(
            reports
                .map(|r| r.groups.map(|group| group.sources().count()).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![vec![MAX_SOURCES], vec![1]]
        );
    }

    // Like membership_report_v3_split_many_sources but we calculate how many
    // groups with no sources specified we can have in the same 1500 Ethernet
    // MTU.
    //
    // * 24 bytes for IPv4 header + router alert option.
    // * 8 bytes for IGMP header + report up to number of groups.
    // * 8 bytes per group with no sources.
    //
    // So we should be able to fit (1500 - 24 - 8)/8 = 183.5 groups. 183
    // groups result in a a 1496 byte-long message.
    #[test]
    fn membership_report_v3_split_many_groups() {
        use igmp_reports::v3::*;
        use packet::{NestablePacketBuilder as _, NestableSerializer as _};

        const ETH_MTU: usize = 1500;
        const EXPECT_SERIALIZED: usize = 1496;
        const MAX_GROUPS: usize = 183;

        let src = Ipv4Addr::new(SRC_1_1);
        let ip_builder = Ipv4PacketBuilderWithOptions::new(
            Ipv4PacketBuilder::new(src, src, 1, Ipv4Proto::Igmp),
            &[Ipv4Option::RouterAlert { data: 0 }],
        )
        .unwrap();
        let ip_header = ip_builder.constraints().header_len();

        let group_ip = |i: usize| {
            MulticastAddr::new(Ipv4Addr::new([224, 0, (i >> 8) as u8, i as u8])).unwrap()
        };
        let reports = IgmpMembershipReportV3Builder::new((0..MAX_GROUPS).into_iter().map(|i| {
            (group_ip(i), IgmpGroupRecordType::ModeIsExclude, core::iter::empty::<Ipv4Addr>())
        }))
        .with_len_limits(ETH_MTU - ip_header)
        .unwrap();

        let mut reports = reports.map(|builder| {
            builder
                .into_serializer()
                .wrap_in(ip_builder.clone())
                .serialize_vec_outer(&mut NoOpSerializationContext)
                .unwrap_or_else(|(err, _)| panic!("{err:?}"))
                .unwrap_b()
                .into_inner()
        });
        // We can generate a report at exactly ETH_MTU.
        let serialized = reports.next().unwrap();
        assert_eq!(serialized.len(), EXPECT_SERIALIZED);

        let mut buffer = &serialized[..];
        let _ip = buffer.parse_with::<_, Ipv4Packet<_>>(()).unwrap();
        let igmp = buffer.parse::<IgmpMessage<_, IgmpMembershipReportV3>>().unwrap();
        assert_eq!(usize::from(igmp.header.number_of_group_records.get()), MAX_GROUPS);
        for (i, group) in igmp.body.iter().enumerate() {
            assert_eq!(group.header.multicast_address, group_ip(i).get());
            assert_eq!(group.header.number_of_sources.get(), 0);
        }

        // Only one report is generated.
        assert_eq!(reports.next(), None);

        let reports =
            IgmpMembershipReportV3Builder::new((0..MAX_GROUPS + 1).into_iter().map(|i| {
                (group_ip(i), IgmpGroupRecordType::ModeIsExclude, core::iter::empty::<Ipv4Addr>())
            }))
            .with_len_limits(ETH_MTU - ip_header)
            .unwrap();
        // 2 reports are generated with one extra source.
        assert_eq!(reports.map(|r| r.groups.count()).collect::<Vec<_>>(), vec![MAX_GROUPS, 1]);
    }

    #[test]
    fn membership_report_v1_parse_and_serialize() {
        use igmp_reports::v1;
        set_logger_for_test();
        test_parse_and_serialize_inner::<IgmpMembershipReportV1, _>(v1::MEMBER_REPORT, |igmp| {
            assert_eq!(*igmp.header, Ipv4Addr::new(v1::GROUP_ADDRESS));
        });
    }

    #[test]
    fn membership_report_v2_parse_and_serialize() {
        use igmp_reports::v2;
        set_logger_for_test();
        test_parse_and_serialize_inner::<IgmpMembershipReportV2, _>(v2::MEMBER_REPORT, |igmp| {
            assert_eq!(*igmp.header, Ipv4Addr::new(v2::GROUP_ADDRESS));
        });
    }

    #[test]
    fn leave_group_parse_and_serialize() {
        set_logger_for_test();
        test_parse_and_serialize_inner::<IgmpLeaveGroup, _>(
            igmp_leave_group::LEAVE_GROUP,
            |igmp| {
                assert_eq!(*igmp.header, Ipv4Addr::new(igmp_leave_group::GROUP_ADDRESS));
            },
        );
    }

    #[test]
    fn test_unknown_type() {
        let mut buff = igmp_invalid_buffers::UNKNOWN_TYPE.to_vec();
        let mut buff = buff.as_mut_slice();
        let packet = buff.parse_with::<_, IgmpPacket<_>>(());
        // we don't use expect_err here because IgmpPacket does not implement
        // core::fmt::Debug
        assert_eq!(packet.is_err(), true);
    }

    #[test]
    fn test_full_parses() {
        let mut bufs = ALL_BUFFERS.to_vec();
        for buff in bufs.iter_mut() {
            let orig_req = &buff[..];
            let packet = buff.parse_with::<_, IgmpPacket<_>>(()).unwrap();
            let msg_type = match packet {
                IgmpPacket::MembershipQueryV2(p) => p.prefix.msg_type,
                IgmpPacket::MembershipQueryV3(p) => p.prefix.msg_type,
                IgmpPacket::MembershipReportV1(p) => p.prefix.msg_type,
                IgmpPacket::MembershipReportV2(p) => p.prefix.msg_type,
                IgmpPacket::MembershipReportV3(p) => p.prefix.msg_type,
                IgmpPacket::LeaveGroup(p) => p.prefix.msg_type,
            };
            assert_eq!(msg_type, orig_req[0]);
        }
    }

    #[test]
    fn test_partial_parses() {
        // parsing a part of the buffer should always result in errors and
        // nothing panics.
        for buff in ALL_BUFFERS.iter() {
            for i in 0..buff.len() {
                let partial_buff = &mut &buff[0..i];
                let packet = partial_buff.parse_with::<_, IgmpPacket<_>>(());
                assert_eq!(packet.is_err(), true)
            }
        }
    }

    // Asserts that a `Message` without `VariableBody` should have the same length
    // as the given ground truth packet.
    fn assert_message_length<Message: for<'a> MessageType<&'a [u8], VariableBody = ()>>(
        mut ground_truth: &[u8],
    ) {
        let ground_truth_len = ground_truth.len();
        let igmp = ground_truth.parse_with::<_, IgmpMessage<&[u8], Message>>(()).unwrap();
        let builder_len = igmp.builder().bytes_len();
        assert_eq!(builder_len, ground_truth_len);
    }

    #[test]
    fn test_igmp_packet_length() {
        assert_message_length::<IgmpMembershipQueryV2>(igmp_router_queries::v2::QUERY);
        assert_message_length::<IgmpMembershipReportV1>(igmp_reports::v1::MEMBER_REPORT);
        assert_message_length::<IgmpMembershipReportV2>(igmp_reports::v2::MEMBER_REPORT);
        assert_message_length::<IgmpLeaveGroup>(igmp_leave_group::LEAVE_GROUP);
    }

    #[test]
    fn test_igmp_too_few_records() {
        let src = net_ip_v4!("192.168.0.1");
        let group_addr = MulticastAddr::new(net_ip_v4!("224.0.0.1")).unwrap();

        // Serialize 1 record.
        let builder = IgmpMembershipReportV3Builder::new(
            [(group_addr, IgmpGroupRecordType::ModeIsInclude, core::iter::once(src))].into_iter(),
        );
        let serialized = builder
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b()
            .into_inner();

        // Skip IGMPv3 report header to get only the record bytes.
        let header_size = crate::igmp::total_header_size::<MembershipReportV3Data>();
        let record_bytes = &serialized[header_size..];

        let mut ctx = 2; // Expect 2 records, but only 1 is present in record_bytes
        let res =
            Records::<_, IgmpMembershipReportV3>::parse_with_mut_context(record_bytes, &mut ctx);
        assert_eq!(res.unwrap_err(), ParseError::Format);
        assert_eq!(ctx, 1); // Parsed one record, so counter decremented to 1
    }
}
