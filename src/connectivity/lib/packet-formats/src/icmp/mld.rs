// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Multicast Listener Discovery Protocol.
//!
//! Wire serialization and deserialization functions.

use core::borrow::Borrow;
use core::fmt::Debug;
use core::mem::size_of;
use core::ops::Deref;
use core::time::Duration;

use net_types::ip::{Ip, IpAddress as _, Ipv6, Ipv6Addr};
use net_types::{MulticastAddr, Witness as _};
use packet::BufferView;
use packet::records::{ParsedRecord, RecordParseResult, Records, RecordsImpl, RecordsImplLayout};
use packet::serialize::InnerPacketBuilder;
use zerocopy::byteorder::network_endian::U16;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice, Unaligned};

use crate::error::{ParseError, ParseResult, UnrecognizedProtocolCode};
use crate::gmp::{GmpReportGroupRecord, InvalidConstraintsError, LinExpConversion, OverflowError};
use crate::icmp::{
    IcmpIpExt, IcmpMessage, IcmpPacket, IcmpPacketRaw, IcmpSenderZeroCode, MessageBody,
};

// TODO(https://github.com/google/zerocopy/issues/1528): Use std::convert::Infallible.
/// A record that can never be instantiated. Trying to instantiate this will result in a compile
/// error.
///
/// At time of writing, [std::convert::Infallible] does not implement [Immutable] nor [IntoBytes]
/// therefore this enum was created.
#[derive(Debug, Immutable)]
pub enum UninstantiableRecord {}

// We have to implement `only_derive_is_allowed_to_implement_this_trait` because
// `#[derive(IntoBytes)]` works only if we can have a `repr` for that type, but since that the type
// is empty we cannot have `repr`.
unsafe impl IntoBytes for UninstantiableRecord {
    fn only_derive_is_allowed_to_implement_this_trait() {
        panic!("UninstantiableRecord cannot be instantiated");
    }
}

/// An ICMPv6 packet with an MLD message.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum MldPacket<B: SplitByteSlice> {
    MulticastListenerQuery(IcmpPacket<Ipv6, B, MulticastListenerQuery>),
    MulticastListenerReport(IcmpPacket<Ipv6, B, MulticastListenerReport>),
    MulticastListenerDone(IcmpPacket<Ipv6, B, MulticastListenerDone>),
    MulticastListenerQueryV2(IcmpPacket<Ipv6, B, MulticastListenerQueryV2>),
    MulticastListenerReportV2(IcmpPacket<Ipv6, B, MulticastListenerReportV2>),
}

/// A raw ICMPv6 packet with an MLD message.
#[allow(missing_docs)]
#[derive(Debug)]
pub enum MldPacketRaw<B: SplitByteSlice> {
    MulticastListenerQuery(IcmpPacketRaw<Ipv6, B, MulticastListenerQuery>),
    MulticastListenerReport(IcmpPacketRaw<Ipv6, B, MulticastListenerReport>),
    MulticastListenerDone(IcmpPacketRaw<Ipv6, B, MulticastListenerDone>),
    MulticastListenerQueryV2(IcmpPacketRaw<Ipv6, B, MulticastListenerQueryV2>),
    MulticastListenerReportV2(IcmpPacketRaw<Ipv6, B, MulticastListenerReportV2>),
}

/// Multicast Record Types as defined in [RFC 3810 section 5.2.12].
///
/// Aliased to shared GMP implementation for convenience.
///
/// [RFC 3810 section 5.2.12]:
///     https://www.rfc-editor.org/rfc/rfc3810#section-5.2.12
pub type Mldv2MulticastRecordType = crate::gmp::GroupRecordType;

/// Fixed information for an MLDv2 Report's Multicast Record, per
/// [RFC 3810 section 5.2].
///
/// [RFC 3810 section 5.2]: https://www.rfc-editor.org/rfc/rfc3810#section-5.2
#[derive(Copy, Clone, Debug, IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned)]
#[repr(C)]
pub struct Mldv2ReportRecordHeader {
    record_type: u8,
    aux_data_len: u8,
    number_of_sources: U16,
    multicast_address: Ipv6Addr,
}

impl Mldv2ReportRecordHeader {
    /// Create a `Mldv2ReportRecordHeader`.
    pub fn new(
        record_type: Mldv2MulticastRecordType,
        aux_data_len: u8,
        number_of_sources: u16,
        multicast_address: Ipv6Addr,
    ) -> Self {
        Mldv2ReportRecordHeader {
            record_type: record_type.into(),
            aux_data_len,
            number_of_sources: number_of_sources.into(),
            multicast_address,
        }
    }

    /// Returns the number of sources.
    pub fn number_of_sources(&self) -> u16 {
        self.number_of_sources.get()
    }

    /// Returns the type of the record.
    pub fn record_type(&self) -> Result<Mldv2MulticastRecordType, UnrecognizedProtocolCode<u8>> {
        Mldv2MulticastRecordType::try_from(self.record_type)
    }

    /// Returns the multicast address.
    pub fn multicast_addr(&self) -> &Ipv6Addr {
        &self.multicast_address
    }
}

/// Wire representation of an MLDv2 Report's Multicast Record, per
/// [RFC 3810 section 5.2].
///
/// [RFC 3810 section 5.2]: https://www.rfc-editor.org/rfc/rfc3810#section-5.2
pub struct MulticastRecord<B> {
    header: Ref<B, Mldv2ReportRecordHeader>,
    sources: Ref<B, [Ipv6Addr]>,
}

impl<B: SplitByteSlice> MulticastRecord<B> {
    /// Returns the multicast record header.
    pub fn header(&self) -> &Mldv2ReportRecordHeader {
        self.header.deref()
    }

    /// Returns the multicast record's sources.
    pub fn sources(&self) -> &[Ipv6Addr] {
        self.sources.deref()
    }
}

/// An implementation of MLDv2 report's records parsing.
#[derive(Copy, Clone, Debug)]
pub enum Mldv2ReportRecords {}

impl RecordsImplLayout for Mldv2ReportRecords {
    type Context = usize;
    type Error = ParseError;
}

impl RecordsImpl for Mldv2ReportRecords {
    type Record<'a> = MulticastRecord<&'a [u8]>;

    fn parse_with_context<'a, BV: BufferView<&'a [u8]>>(
        data: &mut BV,
        ctx: &mut usize,
    ) -> RecordParseResult<MulticastRecord<&'a [u8]>, ParseError> {
        if *ctx == 0 {
            return Ok(ParsedRecord::Done);
        }
        let header = data
            .take_obj_front::<Mldv2ReportRecordHeader>()
            .ok_or_else(debug_err_fn!(ParseError::Format, "Can't take multicast record header"))?;
        let sources = data
            .take_slice_front::<Ipv6Addr>(header.number_of_sources().into())
            .ok_or_else(debug_err_fn!(ParseError::Format, "Can't take multicast record sources"))?;
        // every record may have aux_data_len 32-bit words at the end.
        // we need to update our buffer view to reflect that.
        let _ = data
            .take_front(usize::from(header.aux_data_len) * 4)
            .ok_or_else(debug_err_fn!(ParseError::Format, "Can't skip auxiliary data"))?;

        *ctx -= 1;
        Ok(ParsedRecord::Parsed(Self::Record { header, sources }))
    }
}

/// The layout for an MLDv2 report message header, per [RFC 3810 section 5.2].
///
/// [RFC 3810 section 5.2]: https://www.rfc-editor.org/rfc/rfc3810#section-5.2
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct Mldv2ReportHeader {
    /// Initialized to zero by the sender; ignored by receivers.
    _reserved: [u8; 2],
    /// The number of multicast address records found in this message.
    num_mcast_addr_records: U16,
}

impl Mldv2ReportHeader {
    /// Create a `Mldv2ReportHeader`.
    pub fn new(num_mcast_addr_records: u16) -> Self {
        Mldv2ReportHeader {
            _reserved: [0, 0],
            num_mcast_addr_records: U16::from(num_mcast_addr_records),
        }
    }
    /// Returns the number of multicast address records found in this message.
    pub fn num_mcast_addr_records(&self) -> u16 {
        self.num_mcast_addr_records.get()
    }
}

/// The on-wire structure for the body of an MLDv2 report message, per
/// [RFC 3910 section 5.2].
///
/// [RFC 3810 section 5.2]: https://www.rfc-editor.org/rfc/rfc3810#section-5.2
#[derive(Debug)]
pub struct Mldv2ReportBody<B: SplitByteSlice> {
    header: Ref<B, Mldv2ReportHeader>,
    records: Records<B, Mldv2ReportRecords>,
}

impl<B: SplitByteSlice> Mldv2ReportBody<B> {
    /// Returns the header.
    pub fn header(&self) -> &Mldv2ReportHeader {
        self.header.deref()
    }

    /// Returns an iterator over the multicast address records.
    pub fn iter_multicast_records(&self) -> impl Iterator<Item = MulticastRecord<&'_ [u8]>> {
        self.records.iter()
    }
}

impl<B: SplitByteSlice> MessageBody for Mldv2ReportBody<B> {
    type B = B;
    fn parse(bytes: B) -> ParseResult<Self> {
        let (header, bytes) =
            Ref::<_, Mldv2ReportHeader>::from_prefix(bytes).map_err(|_| ParseError::Format)?;
        let records = Records::parse_with_context(bytes, header.num_mcast_addr_records().into())?;
        Ok(Mldv2ReportBody { header, records })
    }

    fn len(&self) -> usize {
        let (inner_header, inner_body) = self.bytes();
        // We know this is a V2 Report message and that it must have a variable sized body, it's
        // therefore safe to unwrap.
        inner_header.len() + inner_body.unwrap().len()
    }

    fn bytes(&self) -> (&[u8], Option<&[u8]>) {
        (Ref::bytes(&self.header), Some(self.records.bytes()))
    }
}

/// Multicast Listener Report V2 Message.
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct MulticastListenerReportV2;

impl_icmp_message!(
    Ipv6,
    MulticastListenerReportV2,
    MulticastListenerReportV2,
    IcmpSenderZeroCode,
    Mldv2ReportBody<B>
);

/// Multicast Listener Query V1 Message.
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct MulticastListenerQuery;

/// Multicast Listener Report V1 Message.
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct MulticastListenerReport;

/// Multicast Listener Done V1 Message.
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct MulticastListenerDone;

/// The trait for all MLDv1 Messages.
pub trait Mldv1MessageType {
    /// The type used to represent maximum response delay.
    ///
    /// It should be `()` for Report and Done messages,
    /// and be `Mldv1ResponseDelay` for Query messages.
    type MaxRespDelay: MaxCode<U16> + Debug + Copy;
    /// The type used to represent the group_addr in the message.
    ///
    /// For Query Messages, it is just `Ipv6Addr` because
    /// general queries will have this field to be zero, which
    /// is not a multicast address, for Report and Done messages,
    /// this should be `MulticastAddr<Ipv6Addr>`.
    type GroupAddr: Into<Ipv6Addr> + Debug + Copy;
}

/// The trait for all ICMPv6 messages holding MLDv1 messages.
pub trait IcmpMldv1MessageType:
    Mldv1MessageType + IcmpMessage<Ipv6, Code = IcmpSenderZeroCode>
{
}

/// The trait for MLD codes that can be further interpreted using different methods e.g. QQIC.
///
/// The type implementing this trait should be able
/// to convert itself from/to `T`
pub trait MaxCode<T: Default + Debug + FromBytes + IntoBytes> {
    /// Convert to `T`
    #[allow(clippy::wrong_self_convention)]
    fn as_code(self) -> T;

    /// Convert from `T`
    fn from_code(code: T) -> Self;
}

impl<T: Default + Debug + FromBytes + IntoBytes> MaxCode<T> for () {
    fn as_code(self) -> T {
        T::default()
    }

    fn from_code(_: T) -> Self {}
}

/// Maximum Response Delay used in Query messages.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct Mldv1ResponseDelay(u16);

impl MaxCode<U16> for Mldv1ResponseDelay {
    fn as_code(self) -> U16 {
        U16::new(self.0)
    }

    fn from_code(code: U16) -> Self {
        Mldv1ResponseDelay(code.get())
    }
}

impl From<Mldv1ResponseDelay> for Duration {
    fn from(code: Mldv1ResponseDelay) -> Self {
        Duration::from_millis(code.0.into())
    }
}

impl TryFrom<Duration> for Mldv1ResponseDelay {
    type Error = OverflowError;
    fn try_from(period: Duration) -> Result<Self, Self::Error> {
        Ok(Mldv1ResponseDelay(u16::try_from(period.as_millis()).map_err(|_| OverflowError)?))
    }
}

/// The layout for an MLDv1 message body.
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct Mldv1Message {
    /// Max Response Delay, in units of milliseconds.
    pub max_response_delay: U16,
    /// Initialized to zero by the sender; ignored by receivers.
    _reserved: U16,
    /// In a Query message, the Multicast Address field is set to zero when
    /// sending a General Query, and set to a specific IPv6 multicast address
    /// when sending a Multicast-Address-Specific Query.
    ///
    /// In a Report or Done message, the Multicast Address field holds a
    /// specific IPv6 multicast address to which the message sender is
    /// listening or is ceasing to listen, respectively.
    pub group_addr: Ipv6Addr,
}

impl Mldv1Message {
    /// Gets the response delay value.
    pub fn max_response_delay(&self) -> Duration {
        Mldv1ResponseDelay(self.max_response_delay.get()).into()
    }
}

/// The on-wire structure for the body of an MLDv1 message.
#[derive(Debug)]
pub struct Mldv1Body<B: SplitByteSlice>(Ref<B, Mldv1Message>);

impl<B: SplitByteSlice> Deref for Mldv1Body<B> {
    type Target = Mldv1Message;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl<B: SplitByteSlice> MessageBody for Mldv1Body<B> {
    type B = B;
    fn parse(bytes: B) -> ParseResult<Self> {
        Ref::from_bytes(bytes).map_or(Err(ParseError::Format), |body| Ok(Mldv1Body(body)))
    }

    fn len(&self) -> usize {
        let (inner_header, _inner_body) = self.bytes();
        debug_assert!(_inner_body.is_none());
        inner_header.len()
    }

    fn bytes(&self) -> (&[u8], Option<&[u8]>) {
        (Ref::bytes(&self.0), None)
    }
}

macro_rules! impl_mldv1_message {
    ($msg:ident, $resp_code:ty, $group_addr:ty) => {
        impl_icmp_message!(Ipv6, $msg, $msg, IcmpSenderZeroCode, Mldv1Body<B>);
        impl Mldv1MessageType for $msg {
            type MaxRespDelay = $resp_code;
            type GroupAddr = $group_addr;
        }
        impl IcmpMldv1MessageType for $msg {}
    };
}

impl_mldv1_message!(MulticastListenerQuery, Mldv1ResponseDelay, Ipv6Addr);
impl_mldv1_message!(MulticastListenerReport, (), MulticastAddr<Ipv6Addr>);
impl_mldv1_message!(MulticastListenerDone, (), MulticastAddr<Ipv6Addr>);

/// The builder for MLDv1 Messages.
#[derive(Debug)]
pub struct Mldv1MessageBuilder<M: Mldv1MessageType> {
    max_resp_delay: M::MaxRespDelay,
    group_addr: M::GroupAddr,
}

impl<M: Mldv1MessageType<MaxRespDelay = ()>> Mldv1MessageBuilder<M> {
    /// Create an `Mldv1MessageBuilder` without a `max_resp_delay`
    /// for Report and Done messages.
    pub fn new(group_addr: M::GroupAddr) -> Self {
        Mldv1MessageBuilder { max_resp_delay: (), group_addr }
    }
}

impl<M: Mldv1MessageType> Mldv1MessageBuilder<M> {
    /// Create an `Mldv1MessageBuilder` with a `max_resp_delay`
    /// for Query messages.
    pub fn new_with_max_resp_delay(
        group_addr: M::GroupAddr,
        max_resp_delay: M::MaxRespDelay,
    ) -> Self {
        Mldv1MessageBuilder { max_resp_delay, group_addr }
    }

    fn serialize_message(&self, mut buf: &mut [u8]) {
        use packet::BufferViewMut;
        let mut bytes = &mut buf;
        bytes
            .write_obj_front(&Mldv1Message {
                max_response_delay: self.max_resp_delay.as_code(),
                _reserved: U16::ZERO,
                group_addr: self.group_addr.into(),
            })
            .expect("too few bytes for MLDv1 message");
    }
}

impl<M: Mldv1MessageType> InnerPacketBuilder for Mldv1MessageBuilder<M> {
    fn bytes_len(&self) -> usize {
        size_of::<Mldv1Message>()
    }

    fn serialize(&self, buf: &mut [u8]) {
        self.serialize_message(buf);
    }
}

/// The builder for MLDv2 Query Messages.
#[derive(Debug)]
pub struct Mldv2QueryMessageBuilder<I> {
    max_response_delay: Mldv2ResponseDelay,
    group_addr: Option<MulticastAddr<Ipv6Addr>>,
    s_flag: bool,
    qrv: Mldv2QRV,
    qqic: Mldv2QQIC,
    sources: I,
}

impl<I> Mldv2QueryMessageBuilder<I> {
    /// Creates a new [`Mldv2QueryMessageBuilder`].
    pub fn new(
        max_response_delay: Mldv2ResponseDelay,
        group_addr: Option<MulticastAddr<Ipv6Addr>>,
        s_flag: bool,
        qrv: Mldv2QRV,
        qqic: Mldv2QQIC,
        sources: I,
    ) -> Self {
        Self { max_response_delay, group_addr, s_flag, qrv, qqic, sources }
    }
}

impl<I> InnerPacketBuilder for Mldv2QueryMessageBuilder<I>
where
    I: Iterator<Item: Borrow<Ipv6Addr>> + Clone,
{
    fn bytes_len(&self) -> usize {
        core::mem::size_of::<Mldv2QueryMessageHeader>()
            + self.sources.clone().count() * core::mem::size_of::<Ipv6Addr>()
    }

    fn serialize(&self, mut buf: &mut [u8]) {
        use packet::BufferViewMut;
        let mut bytes = &mut buf;
        let mut header = bytes
            .take_obj_front_zero::<Mldv2QueryMessageHeader>()
            .expect("too few bytes for header");
        let Mldv2QueryMessageHeader {
            max_response_code,
            _reserved,
            group_addr,
            sqrv,
            qqic,
            number_of_sources,
        } = &mut *header;
        let Self {
            max_response_delay,
            group_addr: wr_group_addr,
            s_flag,
            qrv,
            qqic: wr_qqic,
            sources,
        } = self;
        *max_response_code = max_response_delay.as_code();
        *group_addr =
            wr_group_addr.as_ref().map(|addr| addr.get()).unwrap_or(Ipv6::UNSPECIFIED_ADDRESS);
        // sqrv contains 4 reserved bits, the s_flag and the qrv,
        // see https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.
        *sqrv = (u8::from(*s_flag) << 3) | (Mldv2QueryMessageHeader::QRV_MASK & u8::from(*qrv));
        *qqic = wr_qqic.as_code();
        let mut count: u16 = 0;
        for src in sources.clone() {
            count = count.checked_add(1).expect("overflowed number of sources");
            bytes.write_obj_front(src.borrow()).expect("too few bytes for source");
        }
        *number_of_sources = count.into();
    }
}

/// The builder for MLDv2 Report Messages.
#[derive(Debug)]
pub struct Mldv2ReportMessageBuilder<I> {
    groups: I,
}

impl<I> Mldv2ReportMessageBuilder<I> {
    /// Creates a new [`Mldv2ReportMessageBuilder`].
    pub fn new(groups: I) -> Self {
        Self { groups }
    }
}

impl<I> Mldv2ReportMessageBuilder<I>
where
    I: Iterator<Item: GmpReportGroupRecord<Ipv6Addr> + Clone> + Clone,
{
    /// Transform this builder into an iterator of builders with a given
    /// `max_len` for each generated packet.
    ///
    /// `max_len` is the maximum length each builder yielded by the returned
    /// iterator can have. The groups used to create this builder are split into
    /// multiple reports in order to meet this length. Note that this length
    /// does _not_ account for the IP *or* the shared ICMP header.
    ///
    /// Returns `Err` if `max_len` is not large enough to meet minimal
    /// constraints for each report.
    pub fn with_len_limits(
        self,
        max_len: usize,
    ) -> Result<
        impl Iterator<
            Item = Mldv2ReportMessageBuilder<
                impl Iterator<Item: GmpReportGroupRecord<Ipv6Addr>> + Clone,
            >,
        >,
        InvalidConstraintsError,
    > {
        let Self { groups } = self;
        crate::gmp::group_record_split_iterator(
            max_len.saturating_sub(core::mem::size_of::<Mldv2ReportHeader>()),
            core::mem::size_of::<Mldv2ReportRecordHeader>(),
            groups,
        )
        .map(|iter| iter.map(|groups| Mldv2ReportMessageBuilder { groups }))
    }
}

impl<I> InnerPacketBuilder for Mldv2ReportMessageBuilder<I>
where
    I: Iterator<Item: GmpReportGroupRecord<Ipv6Addr>> + Clone,
{
    fn bytes_len(&self) -> usize {
        core::mem::size_of::<Mldv2ReportHeader>()
            + self
                .groups
                .clone()
                .map(|g| {
                    core::mem::size_of::<Mldv2ReportRecordHeader>()
                        + g.sources().count() * core::mem::size_of::<Ipv6Addr>()
                })
                .sum::<usize>()
    }

    fn serialize(&self, mut buf: &mut [u8]) {
        use packet::BufferViewMut;
        let mut bytes = &mut buf;
        let mut header =
            bytes.take_obj_front_zero::<Mldv2ReportHeader>().expect("too few bytes for header");
        let Mldv2ReportHeader { _reserved, num_mcast_addr_records } = &mut *header;
        let mut mcast_count: u16 = 0;
        for group in self.groups.clone() {
            mcast_count = mcast_count.checked_add(1).expect("multicast groups count overflows");
            let mut header = bytes
                .take_obj_front_zero::<Mldv2ReportRecordHeader>()
                .expect("too few bytes for record header");
            let Mldv2ReportRecordHeader {
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
        *num_mcast_addr_records = mcast_count.into();
    }
}

/// Maximum Response Delay used in Queryv2 messages, defined in [RFC 3810
/// section 5.1.3].
///
/// [RFC 3810 section 5.1.3]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.3
#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub struct Mldv2ResponseDelay(u16);

impl LinExpConversion<Duration> for Mldv2ResponseDelay {
    const NUM_MANT_BITS: u8 = 12;
    const NUM_EXP_BITS: u8 = 3;

    fn lossy_try_from(value: Duration) -> Result<Self, OverflowError> {
        let millis: u32 = value.as_millis().try_into().map_err(|_| OverflowError)?;
        Self::lossy_try_from_expanded(millis).map(Self)
    }
}

impl MaxCode<U16> for Mldv2ResponseDelay {
    fn as_code(self) -> U16 {
        U16::new(self.0)
    }

    fn from_code(code: U16) -> Self {
        Mldv2ResponseDelay(code.get())
    }
}

impl From<Mldv2ResponseDelay> for Duration {
    fn from(code: Mldv2ResponseDelay) -> Self {
        Duration::from_millis(Mldv2ResponseDelay::to_expanded(code.0).into())
    }
}

/// QRV (Querier's Robustness Variable) used in Queryv2 messages, defined in
/// [RFC 3810 section 5.1.8].
///
/// Aliased to shared GMP implementation for convenience.
///
/// [RFC 3810 section 5.1.8]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.8
pub type Mldv2QRV = crate::gmp::QRV;

/// QQIC (Querier's Query Interval Code) used in Queryv2 messages, defined in
/// [RFC 3810 section 5.1.9].
///
/// Aliased to shared GMP implementation for convenience.
///
/// [RFC 3810 section 5.1.9]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.9
pub type Mldv2QQIC = crate::gmp::QQIC;

impl MaxCode<u8> for Mldv2QQIC {
    fn as_code(self) -> u8 {
        self.into()
    }

    fn from_code(code: u8) -> Self {
        code.into()
    }
}

/// The layout for an MLDv2 Query message header.
///
/// It tracks the fixed part of the message as defined in [RFC 3810 section 5.1]:
///
///    | ...                                                           |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    |    Maximum Response Code      |           Reserved            |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    |                                                               |
///    *                                                               *
///    |                                                               |
///    *                       Multicast Address                       *
///    |                                                               |
///    *                                                               *
///    |                                                               |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    | Resv  |S| QRV |     QQIC      |     Number of Sources (N)     |
///    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///    | ...                                                           |
///
/// [RFC 3810 section 5.1]: https://datatracker.ietf.org/doc/html/rfc3810#section-5.1
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct Mldv2QueryMessageHeader {
    /// Max Response Code
    max_response_code: U16,
    /// Initialized to zero by the sender; ignored by receivers.
    _reserved: U16,
    /// In a Query message, the Multicast Address field is set to zero when
    /// sending a General Query, and set to a specific IPv6 multicast address
    /// when sending a Multicast-Address-Specific Query.
    group_addr: Ipv6Addr,

    /// Tracks 4 reserved bits, the s_flag defined in [RFC 3810 section 5.1.7]
    /// and the qrv defined in [RFC 3810 section 5.1.8].
    ///
    /// [RFC 3810 section 5.1.7]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.7
    /// [RFC 3810 section 5.1.8]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.8
    sqrv: u8,
    /// Querier's Query Interval Code.
    qqic: u8,
    /// Number of Sources i.e. how many source addresses are present in the
    /// Query.
    number_of_sources: U16,
}

impl Mldv2QueryMessageHeader {
    const S_FLAG_MASK: u8 = (1 << 3);
    const QRV_MASK: u8 = 0x07;

    /// Gets the response delay for this query.
    pub fn max_response_delay(&self) -> Mldv2ResponseDelay {
        Mldv2ResponseDelay(self.max_response_code.get())
    }

    /// Returns the query's group address.
    pub fn group_address(&self) -> Ipv6Addr {
        self.group_addr
    }

    /// Returns the number of sources.
    pub fn number_of_sources(&self) -> u16 {
        self.number_of_sources.get()
    }

    /// Returns the S Flag (Suppress Router-Side Processing).
    pub fn suppress_router_side_processing(&self) -> bool {
        (self.sqrv & Self::S_FLAG_MASK) != 0
    }

    /// Returns the Querier's Robustness Variable.
    pub fn querier_robustness_variable(&self) -> u8 {
        self.sqrv & Self::QRV_MASK
    }

    /// Returns the Querier's Query Interval Code.
    pub fn querier_query_interval(&self) -> Duration {
        Mldv2QQIC::from(self.qqic).into()
    }
}

/// The on-wire structure for the body of an MLDv2 report message, per
/// [RFC 3910 section 5.1].
///
/// [RFC 3810 section 5.1]: https://www.rfc-editor.org/rfc/rfc3810#section-5.1
#[derive(Debug)]
pub struct Mldv2QueryBody<B: SplitByteSlice> {
    header: Ref<B, Mldv2QueryMessageHeader>,
    sources: Ref<B, [Ipv6Addr]>,
}

impl<B: SplitByteSlice> Mldv2QueryBody<B> {
    /// Returns the header.
    pub fn header(&self) -> &Mldv2QueryMessageHeader {
        self.header.deref()
    }

    /// Returns the sources.
    pub fn sources(&self) -> &[Ipv6Addr] {
        self.sources.deref()
    }

    /// Reinterprets this [`Mldv2QueryBody`] message as an
    /// [`Mldv1Body`] message in an MLDv1 query.
    ///
    /// Given this crate parses the version separately, users desiring to
    /// operate in MLDv1 mode *SHOULD* reinterpret V2 queries as the
    /// older version.
    ///
    /// See [RFC 3810 section 8.2.1] and [RFC 2236 section 2.5].
    ///
    /// [RFC 3810 section 8.2.1]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-8.2.1
    /// [RFC 2710 section 3.7]:
    ///     https://datatracker.ietf.org/doc/html/rfc2710#section-3.7
    pub fn as_v1_query(&self) -> Mldv1Body<&[u8]> {
        let Self { header, sources: _ } = self;
        // This unwrap is okay because we know Mldv1Message is effectively the
        // prefix within Mldv2QueryBody.
        let (msg, _rest) = Ref::from_prefix(header.as_bytes()).unwrap();
        Mldv1Body(msg)
    }
}

impl<B: SplitByteSlice> MessageBody for Mldv2QueryBody<B> {
    type B = B;
    fn parse(bytes: B) -> ParseResult<Self> {
        let (header, bytes) = Ref::<_, Mldv2QueryMessageHeader>::from_prefix(bytes)
            .map_err(|_| ParseError::Format)?;
        let num_sources_bytes =
            usize::from(Ipv6Addr::BYTES) * usize::from(header.number_of_sources());
        // Read exactly `number_of_sources` addresses from the body. Any
        // trailing bytes MUST be ignored per [RFC 3810 section 5.1.12].
        //
        // [RFC 3810 section 5.1.12]:
        //     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.12
        let (sources_bytes, _rest) =
            bytes.split_at(num_sources_bytes).map_err(|_| ParseError::Format)?;
        let sources =
            Ref::<B, [Ipv6Addr]>::from_bytes(sources_bytes).map_err(|_| ParseError::Format)?;
        Ok(Mldv2QueryBody { header, sources })
    }

    fn len(&self) -> usize {
        let (inner_header, inner_body) = self.bytes();
        // We know this is a V2 Query message and that it must have a variable sized body, it's
        // therefore safe to unwrap.
        inner_header.len() + inner_body.unwrap().len()
    }

    fn bytes(&self) -> (&[u8], Option<&[u8]>) {
        (Ref::bytes(&self.header), Some(Ref::bytes(&self.sources)))
    }
}

/// Multicast Query V2 Message.
#[repr(C)]
#[derive(IntoBytes, KnownLayout, FromBytes, Immutable, Unaligned, Copy, Clone, Debug)]
pub struct MulticastListenerQueryV2;

impl_icmp_message!(
    Ipv6,
    MulticastListenerQueryV2,
    MulticastListenerQuery,
    IcmpSenderZeroCode,
    Mldv2QueryBody<B>
);

#[cfg(test)]
mod tests {
    use packet::{NestableSerializer as _, NoOpSerializationContext, ParseBuffer, Serializer};
    use test_case::test_case;

    use super::*;
    use crate::gmp::{ExactConversionError, GroupRecordType};
    use crate::icmp::{IcmpPacketBuilder, IcmpParseArgs};
    use crate::ip::Ipv6Proto;
    use crate::ipv6::ext_hdrs::{
        ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData, Ipv6ExtensionHeader,
    };
    use crate::ipv6::{Ipv6Header, Ipv6Packet, Ipv6PacketBuilder, Ipv6PacketBuilderWithHbhOptions};
    use net_declare::net_ip_v6;

    fn serialize_to_bytes<B: SplitByteSlice + Debug, M: IcmpMessage<Ipv6> + Debug>(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        icmp: &IcmpPacket<Ipv6, B, M>,
    ) -> Vec<u8> {
        let ip = Ipv6PacketBuilder::new(src_ip, dst_ip, 1, Ipv6Proto::Icmpv6);
        let with_options = Ipv6PacketBuilderWithHbhOptions::new(
            ip,
            &[HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 0 },
            }],
        )
        .unwrap();
        let (header, body) = icmp.message_body.bytes();
        let body = if let Some(b) = body { b } else { &[] };
        let complete_msg = &[header, body].concat();
        complete_msg
            .into_serializer()
            .wrap_in(icmp.builder(src_ip, dst_ip))
            .wrap_in(with_options)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn test_parse_and_serialize<
        M: IcmpMessage<Ipv6> + Debug,
        F: FnOnce(&Ipv6Packet<&[u8]>),
        G: for<'a> FnOnce(&IcmpPacket<Ipv6, &'a [u8], M>),
    >(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        mut req: &[u8],
        check_ip: F,
        check_icmp: G,
    ) {
        let orig_req = req;

        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip);
        let icmp =
            req.parse_with::<_, IcmpPacket<_, _, M>>(IcmpParseArgs::new(src_ip, dst_ip)).unwrap();
        check_icmp(&icmp);

        let data = serialize_to_bytes(src_ip, dst_ip, &icmp);
        assert_eq!(&data[..], orig_req);
    }

    fn serialize_to_bytes_with_builder<M: IcmpMldv1MessageType + Debug>(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        msg: M,
        group_addr: M::GroupAddr,
        max_resp_delay: M::MaxRespDelay,
    ) -> Vec<u8> {
        let ip = Ipv6PacketBuilder::new(src_ip, dst_ip, 1, Ipv6Proto::Icmpv6);
        let with_options = Ipv6PacketBuilderWithHbhOptions::new(
            ip,
            &[HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 0 },
            }],
        )
        .unwrap();
        // Serialize an MLD(ICMPv6) packet using the builder.
        Mldv1MessageBuilder::<M>::new_with_max_resp_delay(group_addr, max_resp_delay)
            .into_serializer()
            .wrap_in(IcmpPacketBuilder::new(src_ip, dst_ip, IcmpSenderZeroCode, msg))
            .wrap_in(with_options)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn serialize_to_bytes_with_builder_v2<
        M: IcmpMessage<Ipv6, Code = IcmpSenderZeroCode> + Debug,
        B: InnerPacketBuilder + Debug,
    >(
        src_ip: Ipv6Addr,
        dst_ip: Ipv6Addr,
        msg: M,
        builder: B,
    ) -> Vec<u8> {
        let ip = Ipv6PacketBuilder::new(src_ip, dst_ip, 1, Ipv6Proto::Icmpv6);
        let with_options = Ipv6PacketBuilderWithHbhOptions::new(
            ip,
            &[HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 0 },
            }],
        )
        .unwrap();
        //Serialize an MLD(ICMPv6) packet using the builder.

        builder
            .into_serializer()
            .wrap_in(IcmpPacketBuilder::new(src_ip, dst_ip, IcmpSenderZeroCode, msg))
            .wrap_in(with_options)
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .as_ref()
            .to_vec()
    }

    fn check_ip<B: SplitByteSlice>(ip: &Ipv6Packet<B>, src_ip: Ipv6Addr, dst_ip: Ipv6Addr) {
        assert_eq!(ip.src_ip(), src_ip);
        assert_eq!(ip.dst_ip(), dst_ip);
        assert_eq!(ip.iter_extension_hdrs().count(), 1);
        let hbh = ip.iter_extension_hdrs().next().unwrap();
        match hbh {
            Ipv6ExtensionHeader::HopByHopOptions { options } => {
                assert_eq!(options.iter().count(), 1);
                assert_eq!(
                    options.iter().next().unwrap(),
                    HopByHopOption {
                        action: ExtensionHeaderOptionAction::SkipAndContinue,
                        mutable: false,
                        data: HopByHopOptionData::RouterAlert { data: 0 },
                    }
                );
            }
            _ => panic!("Wrong extension header"),
        }
    }

    fn check_mld_v1<
        B: SplitByteSlice,
        M: IcmpMessage<Ipv6, Body<B> = Mldv1Body<B>> + Mldv1MessageType + Debug,
    >(
        icmp: &IcmpPacket<Ipv6, B, M>,
        max_resp_code: u16,
        group_addr: Ipv6Addr,
    ) {
        assert_eq!(icmp.message_body._reserved.get(), 0);
        assert_eq!(icmp.message_body.max_response_delay.get(), max_resp_code);
        assert_eq!(icmp.message_body.group_addr, group_addr);
    }

    fn check_mld_query_v2<
        'a,
        B: SplitByteSlice,
        M: IcmpMessage<Ipv6, Body<B> = Mldv2QueryBody<B>> + Debug,
    >(
        icmp: &IcmpPacket<Ipv6, B, M>,
        max_resp_code: u16,
        group_addr: Ipv6Addr,
        sources: &[Ipv6Addr],
    ) {
        assert_eq!(icmp.message_body.header._reserved.get(), 0);
        assert_eq!(icmp.message_body.header.max_response_code.get(), max_resp_code);
        assert_eq!(icmp.message_body.header.group_addr, group_addr);
        assert_eq!(icmp.message_body.sources.len(), sources.len());
        for (expected, actual) in sources.iter().zip(icmp.message_body.sources.iter()) {
            assert_eq!(actual, expected);
        }

        // When interpreted as a v1 body we should get valid results.
        let Mldv1Body(v1) = icmp.message_body.as_v1_query();
        assert_eq!(v1.max_response_delay.get(), max_resp_code);
        assert_eq!(v1.group_addr, group_addr);
    }

    fn check_mld_report_v2<
        'a,
        B: SplitByteSlice,
        M: IcmpMessage<Ipv6, Body<B> = Mldv2ReportBody<B>> + Debug,
    >(
        icmp: &IcmpPacket<Ipv6, B, M>,
        expected_records_header: &[(Mldv2MulticastRecordType, Ipv6Addr)],
        expected_records_sources: &[&[Ipv6Addr]],
    ) {
        assert_eq!(
            icmp.message_body.header.num_mcast_addr_records.get(),
            u16::try_from(expected_records_header.len()).unwrap()
        );
        let expected_records = expected_records_header.iter().zip(expected_records_sources.iter());
        for (expected_record, actual_record) in
            expected_records.zip(icmp.message_body.iter_multicast_records())
        {
            let (expected_header, expected_sources) = expected_record;
            let (expected_record_type, expected_multicast_addr) = expected_header;
            assert_eq!(
                expected_record_type,
                &actual_record.header.record_type().expect("valid record type")
            );

            assert_eq!(
                u16::try_from(expected_sources.len()).unwrap(),
                actual_record.header.number_of_sources()
            );
            assert_eq!(expected_multicast_addr, actual_record.header.multicast_addr());
            assert_eq!(*expected_sources, actual_record.sources());
        }
    }

    #[test]
    fn test_mld_parse_and_serialize_query() {
        use crate::icmp::mld::MulticastListenerQuery;
        use crate::testdata::mld_router_query::*;
        test_parse_and_serialize::<MulticastListenerQuery, _, _>(
            SRC_IP,
            DST_IP,
            QUERY,
            |ip| {
                check_ip(ip, SRC_IP, DST_IP);
            },
            |icmp| {
                check_mld_v1(icmp, MAX_RESP_CODE, HOST_GROUP_ADDRESS);
            },
        );
    }

    #[test]
    fn test_mld_parse_and_serialize_report() {
        use crate::icmp::mld::MulticastListenerReport;
        use crate::testdata::mld_router_report::*;
        test_parse_and_serialize::<MulticastListenerReport, _, _>(
            SRC_IP,
            DST_IP,
            REPORT,
            |ip| {
                check_ip(ip, SRC_IP, DST_IP);
            },
            |icmp| {
                check_mld_v1(icmp, 0, HOST_GROUP_ADDRESS);
            },
        );
    }

    #[test]
    fn test_mld_parse_and_serialize_done() {
        use crate::icmp::mld::MulticastListenerDone;
        use crate::testdata::mld_router_done::*;
        test_parse_and_serialize::<MulticastListenerDone, _, _>(
            SRC_IP,
            DST_IP,
            DONE,
            |ip| {
                check_ip(ip, SRC_IP, DST_IP);
            },
            |icmp| {
                check_mld_v1(icmp, 0, HOST_GROUP_ADDRESS);
            },
        );
    }

    #[test]
    fn test_mld_parse_and_serialize_query_v2() {
        use crate::icmp::mld::MulticastListenerQueryV2;
        use crate::testdata::mld_router_query::*;
        test_parse_and_serialize::<MulticastListenerQueryV2, _, _>(
            SRC_IP,
            DST_IP,
            QUERY_V2,
            |ip| {
                check_ip(ip, SRC_IP, DST_IP);
            },
            |icmp| {
                check_mld_query_v2(icmp, MAX_RESP_CODE, HOST_GROUP_ADDRESS, SOURCES);
            },
        );
    }

    #[test]
    fn test_mld_parse_and_serialize_report_v2() {
        use crate::icmp::mld::MulticastListenerReportV2;
        use crate::testdata::mld_router_report_v2::*;
        test_parse_and_serialize::<MulticastListenerReportV2, _, _>(
            SRC_IP,
            DST_IP,
            REPORT,
            |ip| {
                check_ip(ip, SRC_IP, DST_IP);
            },
            |icmp| {
                check_mld_report_v2(icmp, RECORDS_HEADERS, RECORDS_SOURCES);
            },
        );
    }

    #[test]
    fn test_mld_serialize_and_parse_query() {
        use crate::icmp::mld::MulticastListenerQuery;
        use crate::testdata::mld_router_query::*;
        let bytes = serialize_to_bytes_with_builder::<_>(
            SRC_IP,
            DST_IP,
            MulticastListenerQuery,
            HOST_GROUP_ADDRESS,
            Duration::from_secs(1).try_into().unwrap(),
        );
        assert_eq!(&bytes[..], QUERY);
        let mut req = &bytes[..];
        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = req
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerQuery>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();
        check_mld_v1(&icmp, MAX_RESP_CODE, HOST_GROUP_ADDRESS);
    }

    #[test]
    fn test_mld_serialize_and_parse_report() {
        use crate::icmp::mld::MulticastListenerReport;
        use crate::testdata::mld_router_report::*;
        let bytes = serialize_to_bytes_with_builder::<_>(
            SRC_IP,
            DST_IP,
            MulticastListenerReport,
            MulticastAddr::new(HOST_GROUP_ADDRESS).unwrap(),
            (),
        );
        assert_eq!(&bytes[..], REPORT);
        let mut req = &bytes[..];
        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = req
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerReport>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();
        check_mld_v1(&icmp, 0, HOST_GROUP_ADDRESS);
    }

    #[test]
    fn test_mld_serialize_and_parse_done() {
        use crate::icmp::mld::MulticastListenerDone;
        use crate::testdata::mld_router_done::*;
        let bytes = serialize_to_bytes_with_builder::<_>(
            SRC_IP,
            DST_IP,
            MulticastListenerDone,
            MulticastAddr::new(HOST_GROUP_ADDRESS).unwrap(),
            (),
        );
        assert_eq!(&bytes[..], DONE);
        let mut req = &bytes[..];
        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = req
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerDone>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();
        check_mld_v1(&icmp, 0, HOST_GROUP_ADDRESS);
    }

    #[test]
    fn test_mld_serialize_and_parse_query_v2() {
        use crate::icmp::mld::{Mldv2QRV, Mldv2ResponseDelay, MulticastListenerQueryV2};
        use crate::testdata::mld_router_query::*;
        use core::time::Duration;

        let builder = Mldv2QueryMessageBuilder::new(
            Mldv2ResponseDelay::lossy_try_from(Duration::from_millis(MAX_RESP_CODE.into()))
                .unwrap(),
            MulticastAddr::new(HOST_GROUP_ADDRESS),
            S_FLAG,
            Mldv2QRV::new(QRV),
            Mldv2QQIC::lossy_try_from(Duration::from_secs(QQIC.into())).unwrap(),
            SOURCES.iter(),
        );

        let bytes =
            serialize_to_bytes_with_builder_v2(SRC_IP, DST_IP, MulticastListenerQueryV2, builder);
        assert_eq!(&bytes[..], QUERY_V2);
        let mut req = &bytes[..];
        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = req
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerQueryV2>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();

        check_mld_query_v2(&icmp, MAX_RESP_CODE, HOST_GROUP_ADDRESS, SOURCES);
    }

    #[test]
    fn test_mld_serialize_and_parse_report_v2() {
        use crate::icmp::mld::MulticastListenerReportV2;
        use crate::testdata::mld_router_report_v2::*;

        let builder = Mldv2ReportMessageBuilder::new(
            RECORDS_HEADERS.iter().zip(RECORDS_SOURCES.iter()).map(|record| {
                let (record_header, record_sources) = record;
                let (record_type, multicast_addr) = record_header;
                (MulticastAddr::new(*multicast_addr).unwrap(), *record_type, record_sources.iter())
            }),
        );

        let bytes =
            serialize_to_bytes_with_builder_v2(SRC_IP, DST_IP, MulticastListenerReportV2, builder);
        assert_eq!(&bytes[..], REPORT);
        let mut req = &bytes[..];
        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = req
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerReportV2>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();

        check_mld_report_v2(&icmp, RECORDS_HEADERS, RECORDS_SOURCES);
    }

    #[test_case(&[0x11, 0x22, 0x33, 0x44]; "extra bytes")]
    #[test_case(Ipv6Addr::new([1, 2, 3, 4, 5, 6, 7, 8]).bytes(); "extra addr")]
    fn test_mld_query_v2_trailing_bytes_ignored(extend_bytes: &[u8]) {
        use crate::testdata::mld_router_query::*;

        let mut body_with_trailing = QUERY_V2.to_vec();
        body_with_trailing.extend_from_slice(extend_bytes);

        let mut req = &body_with_trailing[..];
        let ip = req.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = req
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerQueryV2>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();

        check_mld_query_v2(&icmp, MAX_RESP_CODE, HOST_GROUP_ADDRESS, SOURCES);
    }

    // Test that our maximum sources accounting matches an equivalent example in
    // RFC 3810 section 5.1.10:
    //
    //  For example, on an Ethernet link with an MTU of 1500 octets, the IPv6
    //  header (40 octets) together with the Hop-By-Hop Extension Header (8
    //  octets) that includes the Router Alert option consume 48 octets; the MLD
    //  fields up to the Number of Sources (N) field consume 28 octets; thus,
    //  there are 1424 octets left for source addresses, which limits the number
    //  of source addresses to 89 (1424/16)
    //
    // This example is for queries, but reports have the same prefix length so
    // we can use the same numbers.
    #[test]
    fn report_v2_split_many_sources() {
        use crate::testdata::mld_router_report_v2::*;
        use packet::{NestablePacketBuilder as _, NestableSerializer as _};

        const ETH_MTU: usize = 1500;
        const MAX_SOURCES: usize = 89;

        let ip_builder = Ipv6PacketBuilderWithHbhOptions::new(
            Ipv6PacketBuilder::new(SRC_IP, DST_IP, 1, Ipv6Proto::Icmpv6),
            &[HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 0 },
            }],
        )
        .unwrap();
        let icmp_builder =
            IcmpPacketBuilder::new(SRC_IP, DST_IP, IcmpSenderZeroCode, MulticastListenerReportV2);

        let avail_len = ETH_MTU
            - ip_builder.constraints().header_len()
            - icmp_builder.constraints().header_len();

        let src_ip = |i: usize| Ipv6Addr::new([0x2000, 0, 0, 0, 0, 0, 0, i as u16]);
        let group_addr = MulticastAddr::new(RECORDS_HEADERS[0].1).unwrap();
        let reports = Mldv2ReportMessageBuilder::new(
            [(
                group_addr,
                Mldv2MulticastRecordType::ModeIsInclude,
                (0..MAX_SOURCES).into_iter().map(|i| src_ip(i)),
            )]
            .into_iter(),
        )
        .with_len_limits(avail_len)
        .unwrap();

        let mut reports = reports.map(|builder| {
            builder
                .into_serializer()
                .wrap_in(icmp_builder.clone())
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
        let ip = buffer.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = buffer
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerReportV2>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();

        let mut groups = icmp.body().iter_multicast_records();
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

        let reports = Mldv2ReportMessageBuilder::new(
            [(
                group_addr,
                Mldv2MulticastRecordType::ModeIsInclude,
                core::iter::repeat(SRC_IP).take(MAX_SOURCES + 1),
            )]
            .into_iter(),
        )
        .with_len_limits(avail_len)
        .unwrap();
        // 2 reports are generated with one extra source.
        assert_eq!(
            reports
                .map(|r| r.groups.map(|group| group.sources().count()).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![vec![MAX_SOURCES], vec![1]]
        );
    }

    // Like report_v2_split_many_sources but we calculate how many groups with
    // no sources specified we can have in the same 1500 Ethernet MTU.
    //
    // * 48 bytes for IPv6 header + Hop-by-Hop router alert.
    // * 8 bytes for ICMPv6 header + report up to number of groups.
    // * 20 bytes per group with no sources.
    //
    // So we should be able to fit (1500 - 48 - 8)/20 = 72.2 groups. 72
    // groups result in a a 1496 byte-long message.
    #[test]
    fn report_v2_split_many_groups() {
        use crate::testdata::mld_router_report_v2::*;
        use packet::{NestablePacketBuilder as _, NestableSerializer as _};

        const ETH_MTU: usize = 1500;
        const EXPECT_SERIALIZED: usize = 1496;
        const MAX_GROUPS: usize = 72;

        let ip_builder = Ipv6PacketBuilderWithHbhOptions::new(
            Ipv6PacketBuilder::new(SRC_IP, DST_IP, 1, Ipv6Proto::Icmpv6),
            &[HopByHopOption {
                action: ExtensionHeaderOptionAction::SkipAndContinue,
                mutable: false,
                data: HopByHopOptionData::RouterAlert { data: 0 },
            }],
        )
        .unwrap();
        let icmp_builder =
            IcmpPacketBuilder::new(SRC_IP, DST_IP, IcmpSenderZeroCode, MulticastListenerReportV2);

        let avail_len = ETH_MTU
            - ip_builder.constraints().header_len()
            - icmp_builder.constraints().header_len();

        let group_ip = |i: usize| {
            MulticastAddr::new(Ipv6Addr::new([0xff02, 0, 0, 0, 0, 0, 0, i as u16])).unwrap()
        };
        let reports = Mldv2ReportMessageBuilder::new((0..MAX_GROUPS).into_iter().map(|i| {
            (group_ip(i), Mldv2MulticastRecordType::ModeIsExclude, core::iter::empty::<Ipv6Addr>())
        }))
        .with_len_limits(avail_len)
        .unwrap();

        let mut reports = reports.map(|builder| {
            builder
                .into_serializer()
                .wrap_in(icmp_builder.clone())
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
        let ip = buffer.parse_with::<_, Ipv6Packet<_>>(()).unwrap();
        check_ip(&ip, SRC_IP, DST_IP);
        let icmp = buffer
            .parse_with::<_, IcmpPacket<_, _, MulticastListenerReportV2>>(IcmpParseArgs::new(
                SRC_IP, DST_IP,
            ))
            .unwrap();
        assert_eq!(usize::from(icmp.body().header().num_mcast_addr_records()), MAX_GROUPS);
        for (i, group) in icmp.body().iter_multicast_records().enumerate() {
            assert_eq!(group.header.number_of_sources.get(), 0);
            assert_eq!(group.header.multicast_addr(), &group_ip(i).get());
        }
        // Only one report is generated.
        assert_eq!(reports.next(), None);

        let reports = Mldv2ReportMessageBuilder::new((0..MAX_GROUPS + 1).into_iter().map(|i| {
            (group_ip(i), Mldv2MulticastRecordType::ModeIsExclude, core::iter::empty::<Ipv6Addr>())
        }))
        .with_len_limits(avail_len)
        .unwrap();
        // 2 reports are generated with one extra group.
        assert_eq!(reports.map(|r| r.groups.count()).collect::<Vec<_>>(), vec![MAX_GROUPS, 1]);
    }

    #[test]
    fn test_mld_parse_and_serialize_response_delay_v2_linear() {
        // Linear code:duration mapping
        for code in 0..(Mldv2ResponseDelay::SWITCHPOINT as u16) {
            let response_delay = Mldv2ResponseDelay::from_code(U16::from(code));
            let duration = Duration::from(response_delay);
            assert_eq!(duration.as_millis(), code.into());

            let duration = Duration::from_millis(code.into());
            let response_delay_code: u16 =
                Mldv2ResponseDelay::lossy_try_from(duration).unwrap().as_code().into();
            assert_eq!(response_delay_code, code);

            let duration = Duration::from_millis(code.into());
            let response_delay_code: u16 =
                Mldv2ResponseDelay::exact_try_from(duration).unwrap().as_code().into();
            assert_eq!(response_delay_code, code);
        }
    }

    #[test_case(Mldv2ResponseDelay::SWITCHPOINT, 0x8000; "min exponential value")]
    #[test_case(32784,                           0x8002; "exponental value 32784")]
    #[test_case(227744,                          0xABCD; "exponental value 227744")]
    #[test_case(1821184,                         0xDBCA; "exponental value 1821184")]
    #[test_case(8385536,                         0xFFFD; "exponental value 8385536")]
    #[test_case(Mldv2ResponseDelay::MAX_VALUE,   0xFFFF; "max exponential value")]
    fn test_mld_parse_and_serialize_response_delay_v2_exponential_exact(
        duration_millis: u32,
        resp_code: u16,
    ) {
        let response_delay = Mldv2ResponseDelay::from_code(resp_code.into());
        let duration = Duration::from(response_delay);
        assert_eq!(duration.as_millis(), duration_millis.into());

        let response_delay_code: u16 =
            Mldv2ResponseDelay::lossy_try_from(duration).unwrap().as_code().into();
        assert_eq!(response_delay_code, resp_code);

        let response_delay_code: u16 =
            Mldv2ResponseDelay::exact_try_from(duration).unwrap().as_code().into();
        assert_eq!(response_delay_code, resp_code);
    }

    #[test]
    fn test_mld_parse_and_serialize_response_delay_v2_errors() {
        let duration = Duration::from_millis((Mldv2ResponseDelay::MAX_VALUE + 1).into());
        assert_eq!(Mldv2ResponseDelay::lossy_try_from(duration), Err(OverflowError));

        let duration = Duration::from_millis((Mldv2ResponseDelay::MAX_VALUE + 1).into());
        assert_eq!(
            Mldv2ResponseDelay::exact_try_from(duration),
            Err(ExactConversionError::Overflow)
        );

        let duration = Duration::from_millis((Mldv2ResponseDelay::MAX_VALUE - 1).into());
        assert_eq!(
            Mldv2ResponseDelay::exact_try_from(duration),
            Err(ExactConversionError::NotExact)
        );
    }

    #[test]
    fn test_mld_parse_and_serialize_response_qqic_v2_linear() {
        // Linear code:duration mapping
        for code in 0..(Mldv2QQIC::SWITCHPOINT as u8) {
            let response_delay = Mldv2QQIC::from_code(code);
            let duration = Duration::from(response_delay);
            assert_eq!(duration.as_secs(), code.into());

            let duration = Duration::from_secs(code.into());
            let response_delay_code: u8 =
                Mldv2QQIC::lossy_try_from(duration).unwrap().as_code().into();
            assert_eq!(response_delay_code, code);

            let duration = Duration::from_secs(code.into());
            let response_delay_code: u8 =
                Mldv2QQIC::exact_try_from(duration).unwrap().as_code().into();
            assert_eq!(response_delay_code, code);
        }
    }

    #[test_case(Mldv2QQIC::SWITCHPOINT, 0x80; "min exponential value")]
    #[test_case(144,                    0x82; "exponental value 144")]
    #[test_case(928,                    0xAD; "exponental value 928")]
    #[test_case(6656,                   0xDA; "exponental value 6656")]
    #[test_case(29696,                  0xFD; "exponental value 29696")]
    #[test_case(Mldv2QQIC::MAX_VALUE,   0xFF; "max exponential value")]
    fn test_mld_parse_and_serialize_response_qqic_v2_exponential_exact(
        duration_secs: u32,
        resp_code: u8,
    ) {
        let response_delay = Mldv2QQIC::from_code(resp_code.into());
        let duration = Duration::from(response_delay);
        assert_eq!(duration.as_secs(), duration_secs.into());

        let response_delay_code: u8 = Mldv2QQIC::lossy_try_from(duration).unwrap().as_code().into();
        assert_eq!(response_delay_code, resp_code);

        let response_delay_code: u8 = Mldv2QQIC::exact_try_from(duration).unwrap().as_code().into();
        assert_eq!(response_delay_code, resp_code);
    }

    #[test]
    fn test_mld_parse_and_serialize_response_qqic_v2_errors() {
        let duration = Duration::from_secs((Mldv2QQIC::MAX_VALUE + 1).into());
        assert_eq!(Mldv2QQIC::lossy_try_from(duration), Err(OverflowError));

        let duration = Duration::from_secs((Mldv2QQIC::MAX_VALUE + 1).into());
        assert_eq!(Mldv2QQIC::exact_try_from(duration), Err(ExactConversionError::Overflow));

        let duration = Duration::from_secs((Mldv2QQIC::MAX_VALUE - 1).into());
        assert_eq!(Mldv2QQIC::exact_try_from(duration), Err(ExactConversionError::NotExact));
    }

    #[test]
    fn test_mld_too_few_records() {
        let src_ip = net_ip_v6!("::1");
        let group_addr = MulticastAddr::new(net_ip_v6!("ff02::2")).unwrap();

        // Serialize 1 record.
        let builder = Mldv2ReportMessageBuilder::new(
            [(group_addr, GroupRecordType::ModeIsInclude, core::iter::once(src_ip))].into_iter(),
        );
        let serialized = builder
            .into_serializer()
            .serialize_vec_outer(&mut NoOpSerializationContext)
            .unwrap()
            .unwrap_b()
            .into_inner();

        // Skip MLDv2 report header to get only the record bytes.
        let header_size = core::mem::size_of::<Mldv2ReportHeader>();
        let record_bytes = &serialized[header_size..];

        let mut ctx = 2; // Expect 2 records, but only 1 is present in record_bytes
        let res = Records::<_, Mldv2ReportRecords>::parse_with_mut_context(record_bytes, &mut ctx);
        assert_eq!(res.unwrap_err(), ParseError::Format);
        assert_eq!(ctx, 1); // Parsed one record, so counter decremented to 1
    }
}
