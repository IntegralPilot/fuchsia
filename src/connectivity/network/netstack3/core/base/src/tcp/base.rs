// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The Transmission Control Protocol (TCP).

use core::iter::FromIterator;
use core::ops::Range;

use alloc::vec::Vec;
use core::mem::MaybeUninit;
use core::num::NonZeroU16;
use net_types::ip::{Ip, IpVersion};
use packet::InnerPacketBuilder;
use static_assertions::const_assert;

use crate::ip::Mms;
use crate::tcp::segment::{Payload, PayloadLen, SegmentOptions};

/// Control flags that can alter the state of a TCP control block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Corresponds to the SYN bit in a TCP segment.
    SYN,
    /// Corresponds to the FIN bit in a TCP segment.
    FIN,
    /// Corresponds to the RST bit in a TCP segment.
    RST,
}

impl Control {
    /// Returns whether the control flag consumes one byte from the sequence
    /// number space.
    pub fn has_sequence_no(self) -> bool {
        match self {
            Control::SYN | Control::FIN => true,
            Control::RST => false,
        }
    }
}

const TCP_HEADER_LEN: u32 = packet_formats::tcp::HDR_PREFIX_LEN as u32;

/// Maximum segment size, that is the maximum TCP payload one segment can carry.
///
/// `Mss` also acts as a witness that the contained value is >= `Mss::MIN`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct Mss(u16);

const_assert!(Mss::MIN.get() <= Mss::DEFAULT_IPV4.get());
const_assert!(Mss::MIN.get() <= Mss::DEFAULT_IPV6.get());
const_assert!(Mss::MIN.get() as usize >= packet_formats::tcp::MAX_OPTIONS_LEN);

impl Mss {
    /// The minimum MSS allowed by TCP.
    ///
    /// Although enforcing a minimum MSS is outside the recommendations of any
    /// RFC, it is a common practice on other platforms and has multiple
    /// benefits:
    ///   1) Ensures there is enough space to transmit TCP Options & IP Options.
    ///      See RFC 6691 section 2, which clarifies that
    ///          The TCP MSS OPTION [...] SHOULD NOT be decreased to account for
    ///          any possible IP or TCP options; conversely, the sender MUST
    ///          reduce the TCP data length to account for any IP or TCP options
    ///          that it is including in the packets that it sends.
    ///   2) Protects against DOS attacks in which the attacker initiates TCP
    ///      connections with an intentionally small MSS to incur additional
    ///      packet processing overhead on the victim. See
    ///      * FreeBSD: https://www.cve.org/CVERecord?id=CVE-2004-0002
    ///      * Linux: https://www.cve.org/CVERecord?id=CVE-2019-11479
    ///
    /// Here, the value 216 is inspired by FreeBSD. It's large enough to satisfy
    /// points 1 & 2 from above, while remaining small enough to support all
    /// link-layer technologies on the open Internet.
    pub const MIN: Mss = Mss(216);

    /// Per RFC 9293 Section 3.7.1:
    ///  If an MSS Option is not received at connection setup, TCP
    ///  implementations MUST assume a default send MSS of 536 (576 - 40) for
    ///  IPv4.
    pub const DEFAULT_IPV4: Mss = Mss(536);

    /// Per RFC 9293 Section 3.7.1:
    ///  If an MSS Option is not received at connection setup, TCP
    ///  implementations MUST assume a default send MSS of [...] 1220
    /// (1280 - 60) for IPv6 (MUST-15).
    pub const DEFAULT_IPV6: Mss = Mss(1220);

    /// Creates `Mss`, provided the given value satisfies the requirements.
    pub const fn new(mss: u16) -> Option<Self> {
        if mss < Self::MIN.get() { None } else { Some(Mss(mss)) }
    }

    /// Creates MSS from the maximum message size of the IP layer.
    pub fn from_mms(mms: Mms) -> Option<Self> {
        let mss = u16::try_from(mms.get().get().saturating_sub(TCP_HEADER_LEN)).unwrap_or(u16::MAX);
        Self::new(mss)
    }

    /// Create a new [`Mss`] with the IP-version default value, as defined by RFC 9293.
    pub const fn default<I: Ip>() -> Self {
        match I::VERSION {
            IpVersion::V4 => Self::DEFAULT_IPV4,
            IpVersion::V6 => Self::DEFAULT_IPV6,
        }
    }

    /// Gets the numeric value of the MSS.
    pub const fn get(&self) -> u16 {
        let Self(mss) = *self;
        mss
    }
}

/// Like [`Mss`], but smaller to account for fixed-size TCP Options.
///
/// This corresponds to the "effective send MSS" as defined in RFC 9293 section
/// 3.7.1:
///   Eff.snd.MSS = min(SendMSS+20, MMS_S) - TCPhdrsize - IPoptionsize
///   where:
///     [...]
///     * TCPhdrsize is the size of the fixed TCP header and any options.
///
/// Both [`Mss`] and [`EffectiveMss`] have their place in TCP. For example,
/// the TCP MSS option has [`Mss`] semantics, while the MSS used to calculate
/// receive windows & congestion windows has [`EffectiveMss`] semantics. When
/// implementing a TCP feature, refer to the feature's RFC to determine which
/// MSS semantics are appropriate to use.
///
/// Note: this implementation accounts for all fixed-sized TCP Options that are
/// part of [`SegmentOptions`]. SACK blocks are ignored, because they are
/// variable sized. Variable sized options pose a problem when calculating the
/// [`EffectiveMss`] because they vary from segment to segment, whereas the
/// [`EffectiveMss`] should be stable throughout the lifetime of the connection.
/// While, no RFC explicitly states how to account for variable sized options,
/// we take inspiration from Linux's TCP implementation and choose to ignore
/// them until it comes time to actually calculate payload sizes for a given
/// segment.
// TODO(https://fxbug.dev/441271979): Account for fixed-size IP Options.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EffectiveMss {
    mss: Mss,
    fixed_tcp_options_size: u16,
}

impl EffectiveMss {
    /// Constructs an [`EffectiveMss`] from an [`Mss`]
    pub const fn from_mss(mss: Mss, size_limits: MssSizeLimiters) -> Self {
        let MssSizeLimiters { timestamp_enabled } = size_limits;
        // NB: When adding additional fixed size options in the future, authors
        // should take care to account for the alignment only once.
        let fixed_tcp_options_size = if timestamp_enabled {
            packet_formats::tcp::options::ALIGNED_TIMESTAMP_OPTION_LENGTH as u16
        } else {
            0
        };
        EffectiveMss { mss, fixed_tcp_options_size }
    }

    /// Computes the amount of payload data to include in a segment.
    ///
    /// Accounts for the size of any variable-sized options present in the
    /// segment.
    pub fn payload_size(&self, options: &SegmentOptions) -> NonZeroU16 {
        let Self { mss, fixed_tcp_options_size } = self;
        // Determine the length of the TCP Options. If we don't have any
        // variable-sized options, we can use the already-known fixed size.
        // otherwise we need to recalculate.
        let SegmentOptions { timestamp: _, sack_blocks } = options;
        let tcp_options_len = if sack_blocks.is_empty() {
            *fixed_tcp_options_size
        } else {
            // NB: Safe to unwrap here because TCP options have a fixed maximum
            // size < u16::MAX.
            u16::try_from(options.builder().bytes_len()).unwrap()
        };

        // NB: Safe to unwrap here because MSS has a minimum value large enough
        // to fit all TCP options.
        NonZeroU16::new(mss.get() - tcp_options_len).unwrap()
    }

    /// Returns the original [`Mss`] used to compute this [`EffectiveMss`].
    pub fn mss(&self) -> &Mss {
        &self.mss
    }

    /// Replaces the held [`Mss`] with a new value.
    pub fn update_mss(&mut self, new: Mss) {
        self.mss = new
    }

    /// Gets the numeric value of the MSS.
    pub const fn get(&self) -> u16 {
        let Self { mss, fixed_tcp_options_size } = *self;
        mss.get() - fixed_tcp_options_size
    }
}

/// Factors that may limit the space available from the MSS.
pub struct MssSizeLimiters {
    /// True if the TCP Timestamp Option is enabled.
    pub timestamp_enabled: bool,
}

impl From<EffectiveMss> for u32 {
    fn from(mss: EffectiveMss) -> Self {
        u32::from(mss.get())
    }
}

impl From<EffectiveMss> for usize {
    fn from(mss: EffectiveMss) -> Self {
        usize::from(mss.get())
    }
}

/// An implementation of [`Payload`] backed by up to `N` byte slices.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FragmentedPayload<'a, const N: usize> {
    storage: [&'a [u8]; N],
    // NB: Not using `Range` because it is not `Copy`.
    //
    // Start is inclusive, end is exclusive; so this is equivalent to
    // `start..end` ranges.
    start: usize,
    end: usize,
}

/// Creates a new `FragmentedPayload` possibly without using the entire
/// storage capacity `N`.
///
/// # Panics
///
/// Panics if the iterator contains more than `N` items.
impl<'a, const N: usize> FromIterator<&'a [u8]> for FragmentedPayload<'a, N> {
    fn from_iter<T>(iter: T) -> Self
    where
        T: IntoIterator<Item = &'a [u8]>,
    {
        let Self { storage, start, end } = Self::new_empty();
        let (storage, end) = iter.into_iter().fold((storage, end), |(mut storage, end), sl| {
            storage[end] = sl;
            (storage, end + 1)
        });
        Self { storage, start, end }
    }
}

impl<'a, const N: usize> FragmentedPayload<'a, N> {
    /// Creates a new `FragmentedPayload` with the slices in `values`.
    pub fn new(values: [&'a [u8]; N]) -> Self {
        Self { storage: values, start: 0, end: N }
    }

    /// Creates a new `FragmentedPayload` with a single contiguous slice.
    pub fn new_contiguous(value: &'a [u8]) -> Self {
        core::iter::once(value).collect()
    }

    /// Converts this [`FragmentedPayload`] into an owned `Vec`.
    pub fn to_vec(self) -> Vec<u8> {
        self.slices().concat()
    }

    fn slices(&self) -> &[&'a [u8]] {
        let Self { storage, start, end } = self;
        &storage[*start..*end]
    }

    /// Extracted function to implement [`Payload::partial_copy`] and
    /// [`Payload::partial_copy_uninit`].
    fn apply_copy<T, F: Fn(&[u8], &mut [T])>(
        &self,
        mut offset: usize,
        mut dst: &mut [T],
        apply: F,
    ) {
        let mut slices = self.slices().iter();
        while let Some(sl) = slices.next() {
            let l = sl.len();
            if offset >= l {
                offset -= l;
                continue;
            }
            let sl = &sl[offset..];
            let cp = sl.len().min(dst.len());
            let (target, new_dst) = dst.split_at_mut(cp);
            apply(&sl[..cp], target);

            // We're done.
            if new_dst.len() == 0 {
                return;
            }

            dst = new_dst;
            offset = 0;
        }
        assert_eq!(dst.len(), 0, "failed to fill dst");
    }
}

impl<'a, const N: usize> PayloadLen for FragmentedPayload<'a, N> {
    fn len(&self) -> usize {
        self.slices().iter().map(|s| s.len()).sum()
    }
}

impl<'a, const N: usize> Payload for FragmentedPayload<'a, N> {
    fn slice(self, byte_range: Range<u32>) -> Self {
        let Self { mut storage, start: mut self_start, end: mut self_end } = self;
        let Range { start: byte_start, end: byte_end } = byte_range;
        let byte_start =
            usize::try_from(byte_start).expect("range start index out of range for usize");
        let byte_end = usize::try_from(byte_end).expect("range end index out of range for usize");
        assert!(byte_end >= byte_start);
        let mut storage_iter =
            (&mut storage[self_start..self_end]).iter_mut().scan(0, |total_len, slice| {
                let slice_len = slice.len();
                let item = Some((*total_len, slice));
                *total_len += slice_len;
                item
            });

        // Keep track of whether the start was inside the range, we should panic
        // even on an empty range out of start bounds.
        let mut start_offset = None;
        let mut final_len = 0;
        while let Some((sl_offset, sl)) = storage_iter.next() {
            let orig_len = sl.len();

            // Advance until the start of the specified range, discarding unused
            // slices.
            if sl_offset + orig_len < byte_start {
                *sl = &[];
                self_start += 1;
                continue;
            }
            // Discard any empty slices at the end.
            if sl_offset >= byte_end {
                *sl = &[];
                self_end -= 1;
                continue;
            }

            let sl_start = byte_start.saturating_sub(sl_offset);
            let sl_end = sl.len().min(byte_end - sl_offset);
            *sl = &sl[sl_start..sl_end];

            match start_offset {
                Some(_) => (),
                None => {
                    // Keep track of the start offset of the first slice.
                    start_offset = Some(sl_offset + sl_start);
                    // Avoid producing an empty slice if we haven't added
                    // anything yet.
                    if sl.len() == 0 {
                        self_start += 1;
                    }
                }
            }
            final_len += sl.len();
        }
        // Verify that the entire range was consumed.
        assert_eq!(
            // If we didn't use start_offset the only valid value for
            // `byte_start` is zero.
            start_offset.unwrap_or(0),
            byte_start,
            "range start index out of range {byte_range:?}"
        );
        assert_eq!(byte_start + final_len, byte_end, "range end index out of range {byte_range:?}");

        // Canonicalize an empty payload.
        if self_start == self_end {
            self_start = 0;
            self_end = 0;
        }
        Self { storage, start: self_start, end: self_end }
    }

    fn new_empty() -> Self {
        Self { storage: [&[]; N], start: 0, end: 0 }
    }

    fn partial_copy(&self, offset: usize, dst: &mut [u8]) {
        self.apply_copy(offset, dst, |src, dst| {
            dst.copy_from_slice(src);
        });
    }

    fn partial_copy_uninit(&self, offset: usize, dst: &mut [MaybeUninit<u8>]) {
        self.apply_copy(offset, dst, |src, dst| {
            let _ = dst.write_copy_of_slice(src);
        });
    }
}

impl<'a, const N: usize> InnerPacketBuilder for FragmentedPayload<'a, N> {
    fn bytes_len(&self) -> usize {
        self.len()
    }

    fn serialize(&self, buffer: &mut [u8]) {
        self.partial_copy(0, buffer);
    }
}

#[cfg(any(test, feature = "testutils"))]
mod testutil {
    use super::*;

    impl From<Mss> for u32 {
        fn from(Mss(mss): Mss) -> Self {
            u32::from(mss)
        }
    }

    impl From<Mss> for usize {
        fn from(Mss(mss): Mss) -> Self {
            usize::from(mss)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use packet::Serializer as _;
    use proptest::test_runner::Config;
    use proptest::{prop_assert_eq, proptest};
    use proptest_support::failed_seeds_no_std;
    use test_case::test_case;

    use crate::{
        NetworkSerializationContext, SackBlock, SackBlocks, SeqNum, Timestamp, TimestampOption,
    };

    const EXAMPLE_DATA: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
    #[test_case(FragmentedPayload::new([&EXAMPLE_DATA[..]]); "contiguous")]
    #[test_case(FragmentedPayload::new([&EXAMPLE_DATA[0..2], &EXAMPLE_DATA[2..]]); "split once")]
    #[test_case(FragmentedPayload::new([
        &EXAMPLE_DATA[0..2],
        &EXAMPLE_DATA[2..5],
        &EXAMPLE_DATA[5..],
    ]); "split twice")]
    #[test_case(FragmentedPayload::<4>::from_iter([
        &EXAMPLE_DATA[0..2],
        &EXAMPLE_DATA[2..5],
        &EXAMPLE_DATA[5..],
    ]); "partial twice")]
    fn fragmented_payload_serializer_data<const N: usize>(payload: FragmentedPayload<'_, N>) {
        let serialized = payload
            .into_serializer()
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .expect("should serialize")
            .unwrap_b()
            .into_inner();
        assert_eq!(&serialized[..], EXAMPLE_DATA);
    }

    #[test]
    #[should_panic(expected = "range start index out of range")]
    fn slice_start_out_of_bounds() {
        let len = u32::try_from(EXAMPLE_DATA.len()).unwrap();
        let bad_len = len + 1;
        // Like for standard slices, this shouldn't succeed if the start length
        // is out of bounds, even if the total range is empty.
        let _ = FragmentedPayload::<2>::new_contiguous(&EXAMPLE_DATA).slice(bad_len..bad_len);
    }

    #[test]
    #[should_panic(expected = "range end index out of range")]
    fn slice_end_out_of_bounds() {
        let len = u32::try_from(EXAMPLE_DATA.len()).unwrap();
        let bad_len = len + 1;
        let _ = FragmentedPayload::<2>::new_contiguous(&EXAMPLE_DATA).slice(0..bad_len);
    }

    #[test]
    fn canon_empty_payload() {
        let len = u32::try_from(EXAMPLE_DATA.len()).unwrap();
        assert_eq!(
            FragmentedPayload::<1>::new_contiguous(&EXAMPLE_DATA).slice(len..len),
            FragmentedPayload::new_empty()
        );
        assert_eq!(
            FragmentedPayload::<2>::new_contiguous(&EXAMPLE_DATA).slice(len..len),
            FragmentedPayload::new_empty()
        );
        assert_eq!(
            FragmentedPayload::<2>::new_contiguous(&EXAMPLE_DATA).slice(2..2),
            FragmentedPayload::new_empty()
        );
    }

    const TEST_BYTES: &'static [u8] = b"Hello World!";
    proptest! {
        #![proptest_config(Config {
            // Add all failed seeds here.
            failure_persistence: failed_seeds_no_std!(),
            ..Config::default()
        })]

        #[test]
        fn fragmented_payload_to_vec(payload in fragmented_payload::with_payload()) {
            prop_assert_eq!(payload.to_vec(), &TEST_BYTES[..]);
        }

        #[test]
        fn fragmented_payload_len(payload in fragmented_payload::with_payload()) {
            prop_assert_eq!(payload.len(), TEST_BYTES.len())
        }

        #[test]
        fn fragmented_payload_slice((payload, (start, end)) in fragmented_payload::with_range()) {
            let want = &TEST_BYTES[start..end];
            let start = u32::try_from(start).unwrap();
            let end = u32::try_from(end).unwrap();
            prop_assert_eq!(payload.clone().slice(start..end).to_vec(), want);
        }

        #[test]
        fn fragmented_payload_partial_copy((payload, (start, end)) in fragmented_payload::with_range()) {
            let mut buffer = [0; TEST_BYTES.len()];
            let buffer = &mut buffer[0..(end-start)];
            payload.partial_copy(start, buffer);
            prop_assert_eq!(buffer, &TEST_BYTES[start..end]);
        }
    }

    mod fragmented_payload {
        use super::*;

        use proptest::prelude::Rng as _;
        use proptest::strategy::{Just, Strategy};

        const TEST_STORAGE: usize = 5;
        type TestFragmentedPayload = FragmentedPayload<'static, TEST_STORAGE>;
        pub(super) fn with_payload() -> impl Strategy<Value = TestFragmentedPayload> {
            (1..=TEST_STORAGE).prop_perturb(|slices, mut rng| {
                (0..slices)
                    .scan(0, |st, slice| {
                        let len = if slice == slices - 1 {
                            TEST_BYTES.len() - *st
                        } else {
                            rng.random_range(0..=(TEST_BYTES.len() - *st))
                        };
                        let start = *st;
                        *st += len;
                        Some(&TEST_BYTES[start..*st])
                    })
                    .collect()
            })
        }

        pub(super) fn with_range() -> impl Strategy<Value = (TestFragmentedPayload, (usize, usize))>
        {
            (
                with_payload(),
                (0..TEST_BYTES.len()).prop_flat_map(|start| (Just(start), start..TEST_BYTES.len())),
            )
        }
    }

    #[test_case(true; "timestamp_enabled")]
    #[test_case(false; "timestamp_disabled")]
    fn effective_mss_accounts_for_fixed_size_tcp_options(timestamp_enabled: bool) {
        const SIZE: u16 = 1000;
        let mss =
            EffectiveMss::from_mss(Mss::new(SIZE).unwrap(), MssSizeLimiters { timestamp_enabled });
        if timestamp_enabled {
            assert_eq!(
                mss.get(),
                SIZE - packet_formats::tcp::options::ALIGNED_TIMESTAMP_OPTION_LENGTH as u16
            )
        } else {
            assert_eq!(mss.get(), SIZE);
        }
    }

    #[test_case(SegmentOptions {sack_blocks: SackBlocks::EMPTY, timestamp: None}; "empty")]
    #[test_case(SegmentOptions {
        sack_blocks: SackBlocks::from_iter([
            SackBlock::try_new(SeqNum::new(1), SeqNum::new(2)).unwrap(),
            SackBlock::try_new(SeqNum::new(4), SeqNum::new(6)).unwrap(),
        ]),
        timestamp: None
    }; "sack_blocks")]
    #[test_case(SegmentOptions {
        sack_blocks: SackBlocks::EMPTY,
        timestamp: Some(TimestampOption {
            ts_val: Timestamp::new(12345), ts_echo_reply: Timestamp::new(54321)
        }),
    }; "timestamp")]
    #[test_case(SegmentOptions {
        sack_blocks: SackBlocks::from_iter([
            SackBlock::try_new(SeqNum::new(1), SeqNum::new(2)).unwrap(),
            SackBlock::try_new(SeqNum::new(4), SeqNum::new(6)).unwrap(),
        ]),
        timestamp: Some(TimestampOption {
            ts_val: Timestamp::new(12345), ts_echo_reply: Timestamp::new(54321)
        }),
    }; "sack_blocks_and_timestamp")]

    fn effective_mss_accounts_for_variable_size_tcp_options(options: SegmentOptions) {
        const SIZE: u16 = 1000;
        let timestamp_enabled = options.timestamp.is_some();
        let mss =
            EffectiveMss::from_mss(Mss::new(SIZE).unwrap(), MssSizeLimiters { timestamp_enabled });
        let options_len = u16::try_from(options.builder().bytes_len()).unwrap();
        assert_eq!(mss.payload_size(&options).get(), SIZE - options_len);
    }
}
