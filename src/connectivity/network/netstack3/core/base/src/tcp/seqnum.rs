// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TCP sequence numbers and operations on them.

use core::convert::TryFrom as _;
use core::num::TryFromIntError;
use core::ops;

use explicit::ResultExt as _;

/// Sequence number of a transferred TCP segment.
///
/// Per https://tools.ietf.org/html/rfc793#section-3.3:
///   This space ranges from 0 to 2**32 - 1. Since the space is finite, all
///   arithmetic dealing with sequence numbers must be performed modulo 2**32.
///   This unsigned arithmetic preserves the relationship of sequence numbers
///   as they cycle from 2**32 - 1 to 0 again.  There are some subtleties to
///   computer modulo arithmetic, so great care should be taken in programming
///   the comparison of such values.
///
/// For any sequence number, there are 2**31 - 1 numbers after it and 2**31
/// numbers before it. It is an arbitrary decision to make the "before" region
/// larger than the "after" region. The TCP RFC does not specify what the
/// relationship should be between two sequence numbers that are 2**31 apart.
/// Per https://datatracker.ietf.org/doc/html/rfc1982#section-3.2, this
/// relationship is UNDEFINED, and implementations are free to decide.
///
/// Using a larger "before" region matches Linux semantics.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct SeqNum(u32);

impl ops::Add<i32> for SeqNum {
    type Output = SeqNum;

    fn add(self, rhs: i32) -> Self::Output {
        let Self(lhs) = self;
        Self(lhs.wrapping_add_signed(rhs))
    }
}

impl ops::Sub<i32> for SeqNum {
    type Output = SeqNum;

    fn sub(self, rhs: i32) -> Self::Output {
        let Self(lhs) = self;
        Self(lhs.wrapping_add_signed(rhs.wrapping_neg()))
    }
}

impl ops::Add<u32> for SeqNum {
    type Output = SeqNum;

    fn add(self, rhs: u32) -> Self::Output {
        let Self(lhs) = self;
        Self(lhs.wrapping_add(rhs))
    }
}

impl ops::Sub<u32> for SeqNum {
    type Output = SeqNum;

    fn sub(self, rhs: u32) -> Self::Output {
        let Self(lhs) = self;
        Self(lhs.wrapping_sub(rhs))
    }
}

impl ops::Sub<WindowSize> for SeqNum {
    type Output = SeqNum;

    fn sub(self, WindowSize(wnd): WindowSize) -> Self::Output {
        // The conversion from u32 to i32 will never truncate because the
        // maximum window size is less than 2^30, which will comfortably fit
        // into an i32.
        self - i32::try_from(wnd).unwrap()
    }
}

impl ops::Add<usize> for SeqNum {
    type Output = SeqNum;

    fn add(self, rhs: usize) -> Self::Output {
        // The following `as` coercion is sound because:
        // 1. if `u32` is wider than `usize`, the unsigned extension will
        //    result in the same number.
        // 2. if `usize` is wider than `u32`, then `rhs` can be written as
        //    `A * 2 ^ 32 + B`. Because of the wrapping nature of sequnce
        //    numbers, the effect of adding `rhs` is the same as adding `B`
        //    which is the number after the truncation, i.e., `rhs as u32`.
        self + (rhs as u32)
    }
}

impl ops::Sub for SeqNum {
    // `i32` is more intuitive than `u32`, since subtraction may yield negative
    // values.
    type Output = i32;

    fn sub(self, rhs: Self) -> Self::Output {
        let Self(lhs) = self;
        let Self(rhs) = rhs;
        // The following `as` coercion is sound because:
        // Rust uses 2's complement for signed integers [1], meaning when cast
        // to an `i32, an `u32` >= 1<<32 becomes negative and an `u32` < 1<<32
        // becomes positive. `wrapping_sub` ensures that if `rhs` is a `SeqNum`
        // after `lhs`, the result will wrap into the `u32` space > 1<<32.
        // Recall that `SeqNums` are only valid for a `WindowSize` < 1<<31; this
        // prevents the difference of `wrapping_sub` from being so large that it
        // wraps into the `u32` space < 1<<32.
        // [1]: https://doc.rust-lang.org/reference/types/numeric.html
        lhs.wrapping_sub(rhs) as i32
    }
}

impl From<u32> for SeqNum {
    fn from(x: u32) -> Self {
        Self::new(x)
    }
}

impl From<SeqNum> for u32 {
    fn from(x: SeqNum) -> Self {
        let SeqNum(x) = x;
        x
    }
}

impl SeqNum {
    /// Creates a new sequence number.
    pub const fn new(x: u32) -> Self {
        Self(x)
    }
}

impl SeqNum {
    /// A predicate for whether a sequence number is before the other.
    ///
    /// Please refer to [`SeqNum`] for the defined order.
    pub fn before(self, other: SeqNum) -> bool {
        self - other < 0
    }

    /// A predicate for whether a sequence number is equal to or before the
    /// other.
    ///
    /// Please refer to [`SeqNum`] for the defined order.
    pub fn before_or_eq(self, other: SeqNum) -> bool {
        self - other <= 0
    }

    /// A predicate for whether a sequence number is after the other.
    ///
    /// Please refer to [`SeqNum`] for the defined order.
    pub fn after(self, other: SeqNum) -> bool {
        self - other > 0
    }

    /// A predicate for whether a sequence number is equal to or after the
    /// other.
    ///
    /// Please refer to [`SeqNum`] for the defined order.
    pub fn after_or_eq(self, other: SeqNum) -> bool {
        self - other >= 0
    }

    /// Returns the earliest sequence number between `self` and `other`.
    ///
    /// This is equivalent to [`Ord::min`], but keeps within the temporal
    /// instead of numeric semantics.
    pub fn earliest(self, other: SeqNum) -> SeqNum {
        if self.before(other) { self } else { other }
    }

    /// Returns the latest sequence number between `self` and `other`.
    ///
    /// This is equivalent to [`Ord::max`], but keeps within the temporal
    /// instead of numeric semantics.
    pub fn latest(self, other: SeqNum) -> SeqNum {
        if self.after(other) { self } else { other }
    }
}

/// A witness type for TCP window size.
///
/// Per [RFC 7323 Section 2.3]:
/// > ..., the above constraints imply that two times the maximum window size
/// > must be less than 2^31, or
/// >                    max window < 2^30
///
/// [RFC 7323 Section 2.3]: https://tools.ietf.org/html/rfc7323#section-2.3
#[derive(Debug, PartialEq, Eq, Clone, Copy, PartialOrd, Ord)]
pub struct WindowSize(u32);

impl WindowSize {
    /// The largest possible window size.
    pub const MAX: WindowSize = WindowSize((1 << 30) - 1);
    /// The smallest possible window size.
    pub const ZERO: WindowSize = WindowSize(0);
    /// A window size of 1, the smallest nonzero window size.
    pub const ONE: WindowSize = WindowSize(1);

    /// The Netstack3 default window size.
    // TODO(https://github.com/rust-lang/rust/issues/67441): put this constant
    // in the state module once `Option::unwrap` is stable.
    pub const DEFAULT: WindowSize = WindowSize(65535);

    /// Create a new `WindowSize` from the provided `u32`.
    ///
    /// If the provided window size is out of range, then `None` is returned.
    pub const fn from_u32(wnd: u32) -> Option<Self> {
        let WindowSize(max) = Self::MAX;
        if wnd > max { None } else { Some(Self(wnd)) }
    }

    /// Add a `u32` to this WindowSize, saturating at [`WindowSize::MAX`].
    pub fn saturating_add(self, rhs: u32) -> Self {
        Self::from_u32(u32::from(self).saturating_add(rhs)).unwrap_or(Self::MAX)
    }

    /// Create a new [`WindowSize`], returning `None` if the argument is out of range.
    pub fn new(wnd: usize) -> Option<Self> {
        u32::try_from(wnd).ok_checked::<TryFromIntError>().and_then(WindowSize::from_u32)
    }

    /// Subtract `diff` from `self`, returning `None` if the result would be negative.
    pub fn checked_sub(self, diff: usize) -> Option<Self> {
        // The call to Self::new will never return None.
        //
        // If diff is larger than self, the checked_sub will return None. Otherwise the result must
        // be less than Self::MAX, since the value of self before subtraction must be less than or
        // equal to Self::MAX.
        usize::from(self).checked_sub(diff).and_then(Self::new)
    }

    /// Subtract `diff` from `self` returning [`WindowSize::ZERO`] if the result
    /// would be negative.
    pub fn saturating_sub(self, diff: usize) -> Self {
        self.checked_sub(diff).unwrap_or(WindowSize::ZERO)
    }

    /// The window scale that needs to be advertised during the handshake.
    pub fn scale(self) -> WindowScale {
        let WindowSize(size) = self;
        let effective_bits = u8::try_from(32 - u32::leading_zeros(size)).unwrap();
        let scale = WindowScale(effective_bits.saturating_sub(16));
        scale
    }

    /// Returns this `WindowSize` with a halved value
    pub fn halved(self) -> WindowSize {
        let WindowSize(size) = self;
        WindowSize(size >> 1)
    }
}

impl ops::Add<WindowSize> for SeqNum {
    type Output = SeqNum;

    fn add(self, WindowSize(wnd): WindowSize) -> Self::Output {
        self + wnd
    }
}

impl From<WindowSize> for u32 {
    fn from(WindowSize(wnd): WindowSize) -> Self {
        wnd
    }
}

#[cfg(any(target_pointer_width = "32", target_pointer_width = "64"))]
impl From<WindowSize> for usize {
    fn from(WindowSize(wnd): WindowSize) -> Self {
        wnd as usize
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
/// This type is a witness for a valid window scale exponent value.
///
/// Per RFC 7323 Section 2.2, the restriction is as follows:
///   The maximum scale exponent is limited to 14 for a maximum permissible
///   receive window size of 1 GiB (2^(14+16)).
pub struct WindowScale(u8);

impl WindowScale {
    /// The largest possible [`WindowScale`].
    pub const MAX: WindowScale = WindowScale(14);
    /// The smallest possible [`WindowScale`].
    pub const ZERO: WindowScale = WindowScale(0);

    /// Creates a new `WindowScale`.
    ///
    /// Returns `None` if the input exceeds the maximum possible value.
    pub const fn new(ws: u8) -> Option<Self> {
        if ws <= Self::MAX.get() { Some(WindowScale(ws)) } else { None }
    }

    /// Returns the inner value.
    pub const fn get(&self) -> u8 {
        let Self(ws) = self;
        *ws
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
/// Window size that is used in the window field of a TCP segment.
///
/// For connections with window scaling enabled, the receiver has to scale this
/// value back to get the real window size advertised by the peer.
pub struct UnscaledWindowSize(u16);

impl ops::Shl<WindowScale> for UnscaledWindowSize {
    type Output = WindowSize;

    fn shl(self, WindowScale(scale): WindowScale) -> Self::Output {
        let UnscaledWindowSize(size) = self;
        // `scale` is guaranteed to be <= 14, so the result must fit in a u32.
        WindowSize::from_u32(u32::from(size) << scale).unwrap()
    }
}

impl ops::Shr<WindowScale> for WindowSize {
    type Output = UnscaledWindowSize;

    fn shr(self, WindowScale(scale): WindowScale) -> Self::Output {
        let WindowSize(size) = self;
        UnscaledWindowSize(u16::try_from(size >> scale).unwrap_or(u16::MAX))
    }
}

impl From<u16> for UnscaledWindowSize {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<UnscaledWindowSize> for u16 {
    fn from(UnscaledWindowSize(value): UnscaledWindowSize) -> Self {
        value
    }
}

impl UnscaledWindowSize {
    /// Helper to construct an [`UnscaledWindowSize`] in const contexts.
    // TODO(https://github.com/rust-lang/rust/issues/143773): Remove once
    // const_convert is stabilized.
    pub const fn from_u16(value: u16) -> Self {
        Self(value)
    }
}

#[cfg(feature = "testutils")]
mod testutils {
    use super::*;

    impl UnscaledWindowSize {
        /// Create a new UnscaledWindowSize.
        ///
        /// Panics if `size` is not in range.
        pub fn from_usize(size: usize) -> Self {
            UnscaledWindowSize::from(u16::try_from(size).unwrap())
        }

        /// Create a new UnscaledWindowSize.
        ///
        /// Panics if `size` is not in range.
        pub fn from_u32(size: u32) -> Self {
            UnscaledWindowSize::from(u16::try_from(size).unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::arbitrary::any;
    use proptest::strategy::{Just, Strategy};
    use proptest::test_runner::Config;
    use proptest::{prop_assert, prop_assert_eq, proptest};
    use proptest_support::failed_seeds_no_std;
    use test_case::test_case;

    use super::super::segment::MAX_PAYLOAD_AND_CONTROL_LEN;
    use super::*;

    fn arb_seqnum() -> impl Strategy<Value = SeqNum> {
        any::<u32>().prop_map(SeqNum::from)
    }

    // Generates a triple (a, b, c) s.t. a < b < a + 2^30 && b < c < a + 2^30.
    // This triple is used to verify that transitivity holds.
    fn arb_seqnum_trans_tripple() -> impl Strategy<Value = (SeqNum, SeqNum, SeqNum)> {
        arb_seqnum().prop_flat_map(|a| {
            (1..=MAX_PAYLOAD_AND_CONTROL_LEN).prop_flat_map(move |diff_a_b| {
                let b = a + diff_a_b;
                (1..=MAX_PAYLOAD_AND_CONTROL_LEN - diff_a_b).prop_flat_map(move |diff_b_c| {
                    let c = b + diff_b_c;
                    (Just(a), Just(b), Just(c))
                })
            })
        })
    }

    #[test_case(WindowSize::new(1).unwrap() => (UnscaledWindowSize::from(1), WindowScale::default()))]
    #[test_case(WindowSize::new(65535).unwrap() => (UnscaledWindowSize::from(65535), WindowScale::default()))]
    #[test_case(WindowSize::new(65536).unwrap() => (UnscaledWindowSize::from(32768), WindowScale::new(1).unwrap()))]
    #[test_case(WindowSize::new(65537).unwrap() => (UnscaledWindowSize::from(32768), WindowScale::new(1).unwrap()))]
    fn window_scale(size: WindowSize) -> (UnscaledWindowSize, WindowScale) {
        let scale = size.scale();
        (size >> scale, scale)
    }

    proptest! {
        #![proptest_config(Config {
            // Add all failed seeds here.
            failure_persistence: failed_seeds_no_std!(),
            ..Config::default()
        })]

        #[test]
        fn seqnum_ord_is_reflexive(a in arb_seqnum()) {
            prop_assert_eq!(a, a)
        }

        #[test]
        fn seqnum_ord_is_total(a in arb_seqnum(), b in arb_seqnum()) {
            if a == b {
                prop_assert!(!a.before(b) && !b.before(a))
            } else {
                prop_assert!(a.before(b) ^ b.before(a))
            }
        }

        #[test]
        fn seqnum_ord_is_transitive((a, b, c) in arb_seqnum_trans_tripple()) {
            prop_assert!(a.before(b) && b.before(c) && a.before(c));
        }

        #[test]
        fn seqnum_add_positive_greater(a in arb_seqnum(), b in 1..=i32::MAX) {
            prop_assert!(a.before(a + b))
        }

        #[test]
        fn seqnum_add_negative_smaller(a in arb_seqnum(), b in i32::MIN..=-1) {
            prop_assert!(a.after(a + b))
        }

        #[test]
        fn seqnum_sub_positive_smaller(a in arb_seqnum(), b in 1..=i32::MAX) {
            prop_assert!(a.after(a - b))
        }

        #[test]
        fn seqnum_sub_negative_greater(a in arb_seqnum(), b in i32::MIN..=-1) {
            prop_assert!(a.before(a - b))
        }

        #[test]
        fn seqnum_zero_identity(a in arb_seqnum()) {
            prop_assert_eq!(a, a + 0)
        }

        #[test]
        fn seqnum_before_after_inverse(a in arb_seqnum(), b in arb_seqnum()) {
            prop_assert_eq!(a.after(b), b.before(a))
        }

        #[test]
        fn seqnum_wraps_around_at_max_length(a in arb_seqnum()) {
            prop_assert!(a.before(a + MAX_PAYLOAD_AND_CONTROL_LEN));
            prop_assert!(a.after(a + MAX_PAYLOAD_AND_CONTROL_LEN + 1));
        }

        #[test]
        fn window_size_less_than_or_eq_to_max(wnd in 0..=WindowSize::MAX.0) {
            prop_assert_eq!(WindowSize::from_u32(wnd), Some(WindowSize(wnd)));
        }

        #[test]
        fn window_size_greater_than_max(wnd in WindowSize::MAX.0+1..=u32::MAX) {
            prop_assert_eq!(WindowSize::from_u32(wnd), None);
        }
    }

    /// Verify that the maximum value for [`WindowSize`] corresponds to
    /// appropriate values for [`UnscaledWindowSize`] and [`WindowScale`].
    #[test]
    fn max_window_size() {
        // Verify that constructing a `WindowSize` from it's maximum
        // constituents is valid.
        let window_size = UnscaledWindowSize(u16::MAX) << WindowScale::MAX;
        assert!(window_size <= WindowSize::MAX, "actual={window_size:?}");

        // Verify that deconstructing a maximum `WindowSize` into it's
        // constituents is valid.
        assert_eq!(WindowSize::MAX.scale(), WindowScale::MAX);
    }

    // Verify that every sequence number has 2^31 sequence numbers before it,
    // and 2^31-1 sequence numbers after it.
    #[test]
    fn seqnum_boundaries() {
        assert!(SeqNum((1 << 31) - 1).after(SeqNum(0)));
        assert!(!SeqNum((1 << 31) - 1).before(SeqNum(0)));
        assert!(SeqNum(1 << 31).before(SeqNum(0)));
        assert!(!SeqNum(1 << 31).after(SeqNum(0)));
    }
}
