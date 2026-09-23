// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Common types and utilities between MLDv2 and IGMPv3.
//!
//! See [`crate::igmp`] and [`crate::icmp::mld`] for implementations.

use core::borrow::Borrow;
use core::fmt::Debug;
use core::num::NonZeroUsize;
use core::time::Duration;

use net_types::MulticastAddr;
use net_types::ip::IpAddress;

/// Creates a bitmask of [n] bits, [n] must be <= 31.
/// E.g. for n = 12 yields 0xFFF.
const fn bitmask(n: u8) -> u32 {
    assert!((n as u32) < u32::BITS);
    (1 << n) - 1
}

/// Requested value doesn't fit the representation.
#[derive(Debug, Eq, PartialEq)]
pub struct OverflowError;

/// Exact conversion failed.
#[derive(Debug, Eq, PartialEq)]
pub enum ExactConversionError {
    /// Equivalent to [`OverflowError`].
    Overflow,
    /// An exact representation is not possible.
    NotExact,
}

impl From<OverflowError> for ExactConversionError {
    fn from(OverflowError: OverflowError) -> Self {
        Self::Overflow
    }
}

/// The trait converts a code to a floating point value: in a linear fashion up
/// to `SWITCHPOINT` and then using a floating point representation to allow the
/// conversion of larger values. In MLD and IGMP there are different codes that
/// follow this pattern, e.g. QQIC, ResponseDelay ([RFC 3376 section 4.1], [RFC
/// 3810 section 5.1]), which all convert a code with the following underlying
/// structure:
///
///       0    NUM_EXP_BITS       NUM_MANT_BITS
///      +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///      |X|      exp      |          mant         |
///      +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
///
/// This trait simplifies the implementation by providing methods to perform the
/// conversion.
///
/// [RFC 3376 section 4.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1
/// [RFC 3810 section 5.1]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1
pub(crate) trait LinExpConversion<C: Debug + PartialEq + Copy + Clone>:
    Into<C> + Copy + Clone + Sized
{
    // Specified by Implementors
    /// Number of bits used for the mantissa.
    const NUM_MANT_BITS: u8;
    /// Number of bits used for the exponent.
    const NUM_EXP_BITS: u8;
    /// Perform a lossy conversion from the `C` type.
    ///
    /// Not all values in `C` can be exactly represented using the code and they
    /// will be rounded to a code that represents a value close the provided
    /// one.
    fn lossy_try_from(value: C) -> Result<Self, OverflowError>;

    // Provided for Implementors.
    /// How much the exponent needs to be incremented when performing the
    /// exponential conversion.
    const EXP_INCR: u32 = 3;
    /// Bitmask for the mantissa.
    const MANT_BITMASK: u32 = bitmask(Self::NUM_MANT_BITS);
    /// Bitmask for the exponent.
    const EXP_BITMASK: u32 = bitmask(Self::NUM_EXP_BITS);
    /// First value for which we start the exponential conversion.
    const SWITCHPOINT: u32 = 0x1 << (Self::NUM_MANT_BITS + Self::NUM_EXP_BITS);
    /// Prefix for capturing the mantissa.
    const MANT_PREFIX: u32 = 0x1 << Self::NUM_MANT_BITS;
    /// Maximum value the code supports.
    const MAX_VALUE: u32 =
        (Self::MANT_BITMASK | Self::MANT_PREFIX) << (Self::EXP_INCR + Self::EXP_BITMASK);

    /// Converts the provided code to a value: in a linear way until
    /// [Self::SWITCHPOINT] and using a floating representation for larger
    /// values.
    fn to_expanded(code: u16) -> u32 {
        let code = code.into();
        if code < Self::SWITCHPOINT {
            code
        } else {
            let mant = code & Self::MANT_BITMASK;
            let exp = (code >> Self::NUM_MANT_BITS) & Self::EXP_BITMASK;
            (mant | Self::MANT_PREFIX) << (Self::EXP_INCR + exp)
        }
    }

    /// Performs a lossy conversion from `value`.
    ///
    /// The function will always succeed for values within the valid range.
    /// However, the code might not exactly represent the provided input. E.g. a
    /// value of `MAX_VALUE - 1` cannot be exactly represented with a
    /// corresponding code, due the exponential representation. However, the
    /// function will be able to provide a code representing a value close to
    /// the provided one.
    ///
    /// If stronger guarantees are needed consider using
    /// [`LinExpConversion::exact_try_from`].
    fn lossy_try_from_expanded(value: u32) -> Result<u16, OverflowError> {
        if value > Self::MAX_VALUE {
            Err(OverflowError)
        } else if value < Self::SWITCHPOINT {
            // Given that Value is < Self::SWITCHPOINT, unwrapping here is safe.
            let code = value.try_into().unwrap();
            Ok(code)
        } else {
            let msb = (u32::BITS - value.leading_zeros()) - 1;
            let exp = msb - u32::from(Self::NUM_MANT_BITS);
            let mant = (value >> exp) & Self::MANT_BITMASK;
            // Unwrap guaranteed by the structure of the built int:
            let code = (Self::SWITCHPOINT | ((exp - Self::EXP_INCR) << Self::NUM_MANT_BITS) | mant)
                .try_into()
                .unwrap();
            Ok(code)
        }
    }

    /// Attempts an exact conversion from `value`.
    ///
    /// The function will succeed only for values within the valid range that
    /// can be exactly represented by the produced code. E.g. a value of
    /// `FLOATING_POINT_MAX_VALUE - 1` cannot be exactly represented with a
    /// corresponding, code due the exponential representation. In this case,
    /// the function will return an error.
    ///
    /// If a lossy conversion can be tolerated consider using
    /// [`LinExpConversion::lossy_try_from_expanded`].
    ///
    /// If the conversion is attempt is lossy, returns `Ok(None)`.
    fn exact_try_from(value: C) -> Result<Self, ExactConversionError> {
        let res = Self::lossy_try_from(value)?;
        if value == res.into() { Ok(res) } else { Err(ExactConversionError::NotExact) }
    }
}

create_protocol_enum!(
    /// Group/Multicast Record Types as defined in [RFC 3376 section 4.2.12] and
    /// [RFC 3810 section 5.2.12].
    ///
    /// [RFC 3376 section 4.2.12]:
    ///     https://tools.ietf.org/html/rfc3376#section-4.2.12
    /// [RFC 3810 section 5.2.12]:
    ///     https://www.rfc-editor.org/rfc/rfc3810#section-5.2.12
    #[allow(missing_docs)]
    #[derive(PartialEq, Eq, Copy, Clone, PartialOrd, Ord)]
    pub enum GroupRecordType: u8 {
        ModeIsInclude, 0x01, "Mode Is Include";
        ModeIsExclude, 0x02, "Mode Is Exclude";
        ChangeToIncludeMode, 0x03, "Change To Include Mode";
        ChangeToExcludeMode, 0x04, "Change To Exclude Mode";
        AllowNewSources, 0x05, "Allow New Sources";
        BlockOldSources, 0x06, "Block Old Sources";
    }
);

impl GroupRecordType {
    /// Returns `true` if this record type allows the record to be split into
    /// multiple reports.
    ///
    /// If `false`, then the list of sources should be truncated instead.
    ///
    /// From [RFC 3810 section 5.2.15]:
    ///
    /// > if its Type is not IS_EX or TO_EX, it is split into multiple Multicast
    /// > Address Records; each such record contains a different subset of the
    /// > source addresses, and is sent in a separate Report.
    ///
    /// > if its Type is IS_EX or TO_EX, a single Multicast Address Record is
    /// > sent, with as many source addresses as can fit; the remaining source
    /// > addresses are not reported.
    ///
    /// Text is equivalent in [RFC 3376 section 4.2.16]:
    ///
    /// > If a single Group Record contains so many source addresses that it
    /// > does not fit within the size limit of a single Report message, if its
    /// > Type is not MODE_IS_EXCLUDE or CHANGE_TO_EXCLUDE_MODE, it is split
    /// > into multiple Group Records, each containing a different subset of the
    /// > source addresses and each sent in a separate Report message.  If its
    /// > Type is MODE_IS_EXCLUDE or CHANGE_TO_EXCLUDE_MODE, a single Group
    /// > Record is sent, containing as many source addresses as can fit, and
    /// > the remaining source addresses are not reported;
    ///
    /// [RFC 3810 section 5.2.15]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.2.15
    /// [RFC 3376 section 4.2.16]:
    ///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.2.16
    fn allow_split(&self) -> bool {
        match self {
            GroupRecordType::ModeIsInclude
            | GroupRecordType::ChangeToIncludeMode
            | GroupRecordType::AllowNewSources
            | GroupRecordType::BlockOldSources => true,
            GroupRecordType::ModeIsExclude | GroupRecordType::ChangeToExcludeMode => false,
        }
    }
}

/// QQIC (Querier's Query Interval Code) used in IGMPv3/MLDv2 messages, defined
/// in [RFC 3376 section 4.1.7] and [RFC 3810 section 5.1.9].
///
/// [RFC 3376 section 4.1.7]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1.7
/// [RFC 3810 section 5.1.9]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.9
#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub struct QQIC(u8);

impl QQIC {
    /// Creates a new `QQIC` allowing lossy conversion from `value`.
    pub fn new_lossy(value: Duration) -> Result<Self, OverflowError> {
        Self::lossy_try_from(value)
    }

    /// Creates a new `QQIC` rejecting lossy conversion from `value`.
    pub fn new_exact(value: Duration) -> Result<Self, ExactConversionError> {
        Self::exact_try_from(value)
    }
}

impl LinExpConversion<Duration> for QQIC {
    const NUM_MANT_BITS: u8 = 4;
    const NUM_EXP_BITS: u8 = 3;

    fn lossy_try_from(value: Duration) -> Result<Self, OverflowError> {
        let secs: u32 = value.as_secs().try_into().map_err(|_| OverflowError)?;
        let code = Self::lossy_try_from_expanded(secs)?.try_into().map_err(|_| OverflowError)?;
        Ok(Self(code))
    }
}

impl From<QQIC> for Duration {
    fn from(code: QQIC) -> Self {
        let secs: u64 = QQIC::to_expanded(code.0.into()).into();
        Duration::from_secs(secs)
    }
}

impl From<QQIC> for u8 {
    fn from(QQIC(v): QQIC) -> Self {
        v
    }
}

impl From<u8> for QQIC {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

/// QRV (Querier's Robustness Variable) used in IGMPv3/MLDv2 messages, defined
/// in [RFC 3376 section 4.1.6] and [RFC 3810 section 5.1.8].
///
/// [RFC 3376 section 4.1.6]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1.6
/// [RFC 3810 section 5.1.8]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.8
#[derive(PartialEq, Eq, Debug, Clone, Copy, Default)]
pub struct QRV(u8);

impl QRV {
    const QRV_MAX: u8 = 7;

    /// Returns the Querier's Robustness Variable.
    ///
    /// From [RFC 3376 section 4.1.6]: If the querier's [Robustness Variable]
    /// exceeds 7, the maximum value of the QRV field, the QRV is set to zero.
    ///
    /// From [RFC 3810 section 5.1.8]: If the Querier's [Robustness Variable]
    /// exceeds 7 (the maximum value of the QRV field), the QRV field is set to
    /// zero.
    ///
    /// [RFC 3376 section 4.1.6]:
    ///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1.6
    ///
    /// [RFC 3810 section 5.1.8]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1.8
    pub fn new(robustness_value: u8) -> Self {
        if robustness_value > Self::QRV_MAX {
            return QRV(0);
        }
        QRV(robustness_value)
    }
}

impl From<QRV> for u8 {
    fn from(qrv: QRV) -> u8 {
        qrv.0
    }
}

/// A trait abstracting a multicast group record in MLDv2 or IGMPv3.
///
/// This trait facilitates the nested iterators required for implementing group
/// records (iterator of groups, each of which with an iterator of sources)
/// without propagating the inner iterator types far up.
///
/// An implementation for tuples of `(group, record_type, iterator)` is
/// provided.
pub trait GmpReportGroupRecord<A: IpAddress> {
    /// Returns the multicast group this report refers to.
    fn group(&self) -> MulticastAddr<A>;

    /// Returns record type to insert in the record entry.
    fn record_type(&self) -> GroupRecordType;

    /// Returns an iterator over the sources in the report.
    fn sources(&self) -> impl Iterator<Item: Borrow<A>> + '_;
}

impl<A, I> GmpReportGroupRecord<A> for (MulticastAddr<A>, GroupRecordType, I)
where
    A: IpAddress,
    I: Iterator<Item: Borrow<A>> + Clone,
{
    fn group(&self) -> MulticastAddr<A> {
        self.0
    }

    fn record_type(&self) -> GroupRecordType {
        self.1
    }

    fn sources(&self) -> impl Iterator<Item: Borrow<A>> + '_ {
        self.2.clone()
    }
}

#[derive(Clone)]
struct OverrideGroupRecordSources<R> {
    record: R,
    limit: NonZeroUsize,
    skip: usize,
}

impl<R, A> GmpReportGroupRecord<A> for OverrideGroupRecordSources<R>
where
    A: IpAddress,
    R: GmpReportGroupRecord<A>,
{
    fn group(&self) -> MulticastAddr<A> {
        self.record.group()
    }

    fn record_type(&self) -> GroupRecordType {
        self.record.record_type()
    }

    fn sources(&self) -> impl Iterator<Item: Borrow<A>> + '_ {
        self.record.sources().skip(self.skip).take(self.limit.get())
    }
}

/// The error returned when size constraints can't fit records.
#[derive(Debug, Eq, PartialEq)]
pub struct InvalidConstraintsError;

pub(crate) fn group_record_split_iterator<A, I>(
    max_len: usize,
    group_header: usize,
    groups: I,
) -> Result<
    impl Iterator<Item: Iterator<Item: GmpReportGroupRecord<A>> + Clone>,
    InvalidConstraintsError,
>
where
    A: IpAddress,
    I: Iterator<Item: GmpReportGroupRecord<A> + Clone> + Clone,
{
    // We need a maximum length that can fit at least one group with one source.
    if group_header + core::mem::size_of::<A>() > max_len {
        return Err(InvalidConstraintsError);
    }
    // These are the mutable state given to the iterator.
    //
    // `groups` is the main iterator that is moved forward whenever we've fully
    // yielded a group out on a `next` call.
    let mut groups = groups.peekable();
    // `skip` is saved in case the first group of a next iteration needs to skip
    // sources entries.
    let mut skip = 0;
    Ok(core::iter::from_fn(move || {
        let start = groups.clone();
        let mut take = 0;
        let mut len = 0;
        loop {
            let group = match groups.peek() {
                Some(group) => group,
                None => break,
            };
            len += group_header;
            // Can't even fit the header.
            if len > max_len {
                break;
            }

            // `skip` is only going to be valid for the first group we look at,
            // so always reset it to zero.
            let skipped = core::mem::replace(&mut skip, 0);
            let sources = group.sources();
            if take == 0 {
                // If this is the first group, we should be able to split this
                // into multiple reports as necessary. Alternatively, if we have
                // skipped records from a previous yield we should produce the
                // rest of the records here.
                let mut sources = sources.skip(skipped).enumerate();
                loop {
                    // NB: This is not written as a `while` or `for` loop so we
                    // don't create temporaries that are holding on to borrows
                    // of groups, which then allows us to drive the main
                    // iterator before exiting here.
                    let Some((i, _)) = sources.next() else { break };

                    len += core::mem::size_of::<A>();
                    if len > max_len {
                        // We're ensured to always be able to fit at least one
                        // group with one source per report, so we should never
                        // hit max length on the first source.
                        let limit = NonZeroUsize::new(i).expect("can't fit a single source");
                        let record = if group.record_type().allow_split() {
                            // Update skip so we yield the rest of the message
                            // on the next iteration.
                            skip = skipped + i;
                            group.clone()
                        } else {
                            // Use the current limit and just ignore any further
                            // sources. We known unwrap is okay here we just
                            // peeked.
                            drop(sources);
                            groups.next().unwrap()
                        };
                        return Some(either::Either::Left(core::iter::once(
                            OverrideGroupRecordSources { record, limit, skip: skipped },
                        )));
                    }
                }
                // If we need to skip any records, yield a single entry. It's a
                // bit too complicated to insert this group in a report with
                // other groups, so let's just issue the rest of its sources in
                // its own report.
                if skipped != 0 {
                    // Consume this current group. Unwrap is safe we just
                    // peeked.
                    drop(sources);
                    let group = groups.next().unwrap();
                    return Some(either::Either::Left(core::iter::once(
                        OverrideGroupRecordSources {
                            record: group,
                            limit: NonZeroUsize::MAX,
                            skip: skipped,
                        },
                    )));
                }
            } else {
                // We can't handle skipped sources here.
                assert_eq!(skipped, 0);
                // If not the first group only account for it if we can take all
                // sources.
                len += sources.count() * core::mem::size_of::<A>();
                if len > max_len {
                    break;
                }
            }

            // This entry fits account for it.
            let _: Option<_> = groups.next();
            take += 1;
        }

        if take == 0 {
            None
        } else {
            Some(either::Either::Right(start.take(take).map(|record| OverrideGroupRecordSources {
                record,
                limit: NonZeroUsize::MAX,
                skip: 0,
            })))
        }
    }))
}

#[cfg(test)]
mod tests {
    use core::ops::Range;

    use super::*;

    use ip_test_macro::ip_test;
    use net_types::ip::{Ip, Ipv4Addr, Ipv6Addr};

    fn empty_iter<A: IpAddress>() -> impl Iterator<Item: GmpReportGroupRecord<A> + Clone> + Clone {
        core::iter::empty::<(MulticastAddr<A>, GroupRecordType, core::iter::Empty<A>)>()
    }

    fn addr<I: Ip>(i: u8) -> I::Addr {
        I::map_ip_out(
            i,
            |i| Ipv4Addr::new([0, 0, 0, i]),
            |i| Ipv6Addr::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, i]),
        )
    }

    fn mcast_addr<I: Ip>(i: u8) -> MulticastAddr<I::Addr> {
        MulticastAddr::new(I::map_ip_out(
            i,
            |i| Ipv4Addr::new([224, 0, 0, i]),
            |i| Ipv6Addr::from_bytes([0xFF, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, i]),
        ))
        .unwrap()
    }

    fn addr_iter_range<I: Ip>(range: Range<u8>) -> impl Iterator<Item = I::Addr> + Clone {
        range.into_iter().map(|i| addr::<I>(i))
    }

    fn collect<I, A>(iter: I) -> Vec<Vec<(MulticastAddr<A>, GroupRecordType, Vec<A>)>>
    where
        I: Iterator<Item: Iterator<Item: GmpReportGroupRecord<A>>>,
        A: IpAddress,
    {
        iter.map(|groups| {
            groups
                .map(|g| {
                    (
                        g.group(),
                        g.record_type(),
                        g.sources().map(|b| b.borrow().clone()).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
    }

    const GROUP_RECORD_HEADER: usize = 1;

    #[ip_test(I)]
    fn split_rejects_small_lengths<I: Ip>() {
        assert_eq!(
            group_record_split_iterator(
                GROUP_RECORD_HEADER,
                GROUP_RECORD_HEADER,
                empty_iter::<I::Addr>()
            )
            .map(collect),
            Err(InvalidConstraintsError)
        );
        assert_eq!(
            group_record_split_iterator(
                GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>() - 1,
                GROUP_RECORD_HEADER,
                empty_iter::<I::Addr>()
            )
            .map(collect),
            Err(InvalidConstraintsError)
        );
        // Works, doesn't yield anything because of empty iterator.
        assert_eq!(
            group_record_split_iterator(
                GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>(),
                GROUP_RECORD_HEADER,
                empty_iter::<I::Addr>()
            )
            .map(collect),
            Ok(vec![])
        );
    }

    #[ip_test(I)]
    fn basic_split<I: Ip>() {
        let iter = group_record_split_iterator(
            GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>() * 2,
            GROUP_RECORD_HEADER,
            [
                (mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(1..2)),
                (mcast_addr::<I>(2), GroupRecordType::ModeIsExclude, addr_iter_range::<I>(2..4)),
                (
                    mcast_addr::<I>(3),
                    GroupRecordType::ChangeToIncludeMode,
                    addr_iter_range::<I>(0..0),
                ),
                (
                    mcast_addr::<I>(4),
                    GroupRecordType::ChangeToExcludeMode,
                    addr_iter_range::<I>(0..0),
                ),
            ]
            .into_iter(),
        )
        .unwrap();

        let report1 = vec![(
            mcast_addr::<I>(1),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(1..2).collect::<Vec<_>>(),
        )];
        let report2 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsExclude,
            addr_iter_range::<I>(2..4).collect::<Vec<_>>(),
        )];
        let report3 = vec![
            (mcast_addr::<I>(3), GroupRecordType::ChangeToIncludeMode, vec![]),
            (mcast_addr::<I>(4), GroupRecordType::ChangeToExcludeMode, vec![]),
        ];
        assert_eq!(collect(iter), vec![report1, report2, report3]);
    }

    #[ip_test(I)]
    fn sources_split<I: Ip>() {
        let iter = group_record_split_iterator(
            GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>(),
            GROUP_RECORD_HEADER,
            [
                (mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(0..0)),
                (mcast_addr::<I>(2), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(0..3)),
                (mcast_addr::<I>(3), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(0..0)),
            ]
            .into_iter(),
        )
        .unwrap();

        let report1 = vec![(mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, vec![])];
        let report2 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(0..1).collect::<Vec<_>>(),
        )];
        let report3 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(1..2).collect::<Vec<_>>(),
        )];
        let report4 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(2..3).collect::<Vec<_>>(),
        )];
        let report5 = vec![(mcast_addr::<I>(3), GroupRecordType::ModeIsInclude, vec![])];
        assert_eq!(collect(iter), vec![report1, report2, report3, report4, report5]);
    }

    #[ip_test(I)]
    fn sources_truncate<I: Ip>() {
        let iter = group_record_split_iterator(
            GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>(),
            GROUP_RECORD_HEADER,
            [
                (mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(0..0)),
                (mcast_addr::<I>(2), GroupRecordType::ModeIsExclude, addr_iter_range::<I>(0..2)),
                (mcast_addr::<I>(3), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(2..3)),
            ]
            .into_iter(),
        )
        .unwrap();

        let report1 = vec![(mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, vec![])];
        // Only one report for the exclude mode is generated, sources are
        // truncated.
        let report2 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsExclude,
            addr_iter_range::<I>(0..1).collect::<Vec<_>>(),
        )];
        let report3 = vec![(
            mcast_addr::<I>(3),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(2..3).collect::<Vec<_>>(),
        )];
        assert_eq!(collect(iter), vec![report1, report2, report3]);
    }

    /// Tests for a current limitation of the iterator. We don't attempt to pack
    /// split sources, but rather possibly generate a short report.
    #[ip_test(I)]
    fn odd_split<I: Ip>() {
        let iter = group_record_split_iterator(
            GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>() * 4,
            GROUP_RECORD_HEADER,
            [
                (mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(0..5)),
                (mcast_addr::<I>(2), GroupRecordType::ModeIsExclude, addr_iter_range::<I>(5..6)),
            ]
            .into_iter(),
        )
        .unwrap();

        let report1 = vec![(
            mcast_addr::<I>(1),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(0..4).collect::<Vec<_>>(),
        )];
        let report2 = vec![(
            mcast_addr::<I>(1),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(4..5).collect::<Vec<_>>(),
        )];
        let report3 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsExclude,
            addr_iter_range::<I>(5..6).collect::<Vec<_>>(),
        )];
        assert_eq!(collect(iter), vec![report1, report2, report3]);
    }

    /// Tests that we prefer to keep a group together if we can, i.e., avoid
    /// splitting off a group that is not the first in a message.
    #[ip_test(I)]
    fn split_off_large_group<I: Ip>() {
        let iter = group_record_split_iterator(
            (GROUP_RECORD_HEADER + core::mem::size_of::<I::Addr>()) * 2,
            GROUP_RECORD_HEADER,
            [
                (mcast_addr::<I>(1), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(0..1)),
                // The beginning of this group should be in its own message.
                (mcast_addr::<I>(2), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(1..3)),
                (mcast_addr::<I>(3), GroupRecordType::ModeIsInclude, addr_iter_range::<I>(3..4)),
                // This group should be in its own message as opposed to
                // truncating together with the previous one.
                (mcast_addr::<I>(4), GroupRecordType::ModeIsExclude, addr_iter_range::<I>(4..6)),
            ]
            .into_iter(),
        )
        .unwrap();

        let report1 = vec![(
            mcast_addr::<I>(1),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(0..1).collect::<Vec<_>>(),
        )];
        let report2 = vec![(
            mcast_addr::<I>(2),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(1..3).collect::<Vec<_>>(),
        )];
        let report3 = vec![(
            mcast_addr::<I>(3),
            GroupRecordType::ModeIsInclude,
            addr_iter_range::<I>(3..4).collect::<Vec<_>>(),
        )];
        let report4 = vec![(
            mcast_addr::<I>(4),
            GroupRecordType::ModeIsExclude,
            addr_iter_range::<I>(4..6).collect::<Vec<_>>(),
        )];
        assert_eq!(collect(iter), vec![report1, report2, report3, report4]);
    }
}
