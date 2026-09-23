// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! The definition of a TCP segment.

use core::convert::TryFrom as _;
use core::fmt::Debug;
use core::mem::MaybeUninit;
use core::num::TryFromIntError;
use core::ops::Range;

use arrayvec::ArrayVec;
use log::info;
use net_types::ip::IpAddress;
use packet::InnerSerializer;
use packet_formats::tcp::options::{TcpOptions, TcpOptionsBuilder, TcpSackBlock};
use packet_formats::tcp::{TcpSegment, TcpSegmentBuilder, TcpSegmentBuilderWithOptions};
use thiserror::Error;

use super::base::{Control, Mss};
use super::seqnum::{SeqNum, UnscaledWindowSize, WindowScale, WindowSize};
use super::timestamp::TimestampOption;

/// A TCP segment.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Segment<P> {
    /// The non-payload information of the segment.
    header: SegmentHeader,
    /// The data carried by the segment.
    ///
    /// It is guaranteed that data.len() plus the length of the control flag
    /// (SYN or FIN) is <= MAX_PAYLOAD_AND_CONTROL_LEN
    data: P,
}

/// All non-data portions of a TCP segment.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SegmentHeader {
    /// The sequence number of the segment.
    pub seq: SeqNum,
    /// The acknowledge number of the segment. [`None`] if not present.
    pub ack: Option<SeqNum>,
    /// The advertised window size.
    pub wnd: UnscaledWindowSize,
    /// The control flag of the segment.
    pub control: Option<Control>,
    /// Indicates whether the PSH bit is set.
    pub push: bool,
    /// Options carried by this segment.
    pub options: Options,
}

/// Contains all supported TCP options.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Options {
    /// Options present in a handshake segment.
    Handshake(HandshakeOptions),
    /// Options present in a data/ack segment.
    Segment(SegmentOptions),
    /// Options present in a reset segment.
    Reset(ResetOptions),
}

impl Options {
    /// Creates a new [`Options`] with default values based on the control flag.
    fn new(control: Option<&Control>) -> Self {
        match control {
            None | Some(Control::FIN) => Options::Segment(Default::default()),
            Some(Control::SYN) => Options::Handshake(Default::default()),
            Some(Control::RST) => Options::Reset(Default::default()),
        }
    }
}

impl From<HandshakeOptions> for Options {
    fn from(value: HandshakeOptions) -> Self {
        Self::Handshake(value)
    }
}

impl From<SegmentOptions> for Options {
    fn from(value: SegmentOptions) -> Self {
        Self::Segment(value)
    }
}

impl From<ResetOptions> for Options {
    fn from(value: ResetOptions) -> Self {
        Self::Reset(value)
    }
}

impl Options {
    fn as_handshake(&self) -> Option<&HandshakeOptions> {
        match self {
            Self::Handshake(h) => Some(h),
            Self::Segment(_) | Self::Reset(_) => None,
        }
    }

    fn as_segment(&self) -> Option<&SegmentOptions> {
        match self {
            Self::Handshake(_) | Self::Reset(_) => None,
            Self::Segment(s) => Some(s),
        }
    }

    /// Creates a new [`Options`] from the packet-formats representation.
    pub fn from_options(control: Option<&Control>, input_options: impl TcpOptions) -> Self {
        let mut options = Options::new(control);

        // Fetch options based on the type of segment being handled.
        // Spurious options (e.g. "WindowScale" for an `Options::Segment`) are
        // ignored.
        match options {
            Options::Handshake(ref mut h) => {
                let HandshakeOptions { mss, window_scale, sack_permitted, timestamp } = h;
                *timestamp = input_options.timestamp().map(Into::into);
                *mss = input_options.mss().map(|mss| Mss::new(mss).unwrap_or(Mss::MIN));
                *window_scale = input_options.window_scale().map(|ws| {
                    WindowScale::new(ws).unwrap_or_else(|| {
                        // Per RFC 7323 Section 2.3:
                        //   If a Window Scale option is received with a shift.cnt
                        //   value larger than 14, the TCP SHOULD log the error but
                        //   MUST use 14 instead of the specified value.
                        info!(
                            "received an out-of-range window scale: {}, want < {}",
                            ws,
                            WindowScale::MAX.get()
                        );
                        WindowScale::MAX
                    })
                });
                *sack_permitted = input_options.sack_permitted();
            }
            Options::Segment(ref mut s) => {
                let SegmentOptions { sack_blocks, timestamp } = s;
                *timestamp = input_options.timestamp().map(Into::into);
                *sack_blocks = match input_options.sack_blocks() {
                    None => SackBlocks::EMPTY,
                    Some(sack_blocks) => SackBlocks::from_option(sack_blocks),
                };
            }
            Options::Reset(ref mut r) => {
                let ResetOptions { timestamp } = r;
                *timestamp = input_options.timestamp().map(Into::into);
            }
        }

        options
    }

    /// Creates a new [`Options`] from the packet-formats representation.
    pub fn try_from_options<'a, A: IpAddress>(
        builder: &TcpSegmentBuilder<A>,
        options: impl TcpOptions,
    ) -> Result<Self, MalformedFlags> {
        let control =
            Flags { syn: builder.syn_set(), fin: builder.fin_set(), rst: builder.rst_set() }
                .control()?;
        Ok(Options::from_options(control.as_ref(), options))
    }

    /// Reads the window scale if this is an [`Options::Handshake`].
    pub fn window_scale(&self) -> Option<WindowScale> {
        self.as_handshake().and_then(|h| h.window_scale)
    }

    /// Reads the mss option if this is an [`Options::Handshake`].
    pub fn mss(&self) -> Option<Mss> {
        self.as_handshake().and_then(|h| h.mss)
    }

    /// Returns true IFF this is an [`Options::Handshake`] and its
    /// [`HandShakeOptions::sack_permitted`] is set.
    pub fn sack_permitted(&self) -> bool {
        self.as_handshake().is_some_and(|o| o.sack_permitted)
    }

    /// Returns the segment's selective ack blocks.
    ///
    /// Returns a reference to empty blocks if this is not [`Options::Segment`].
    pub fn sack_blocks(&self) -> &SackBlocks {
        const EMPTY_REF: &'static SackBlocks = &SackBlocks::EMPTY;
        self.as_segment().map(|s| &s.sack_blocks).unwrap_or(EMPTY_REF)
    }

    /// Returns the segment's TCP timestamp option, if there is one.
    pub fn timestamp(&self) -> &Option<TimestampOption> {
        match self {
            Options::Handshake(h) => &h.timestamp,
            Options::Segment(s) => &s.timestamp,
            Options::Reset(r) => &r.timestamp,
        }
    }

    /// Returns a builder of TCP Options to be used during serialization.
    pub fn builder(&self) -> TcpOptionsBuilder<'_> {
        match self {
            Options::Handshake(h) => h.builder(),
            Options::Segment(s) => s.builder(),
            Options::Reset(r) => r.builder(),
        }
    }
}

/// Segment options available on handshake segments.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct HandshakeOptions {
    /// The MSS option.
    pub mss: Option<Mss>,

    /// The WS option.
    pub window_scale: Option<WindowScale>,

    /// The SACK permitted option.
    pub sack_permitted: bool,

    /// The timestamp option.
    pub timestamp: Option<TimestampOption>,
}

impl HandshakeOptions {
    fn builder(&self) -> TcpOptionsBuilder<'_> {
        let Self { mss, window_scale, sack_permitted, timestamp } = self;
        TcpOptionsBuilder {
            mss: mss.map(|mss| mss.get()),
            window_scale: window_scale.map(|ws| ws.get()),
            sack_permitted: *sack_permitted,
            timestamp: timestamp.as_ref().map(Into::into),
            ..Default::default()
        }
    }
}

/// Segment options available on data segments.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct SegmentOptions {
    /// The SACK option.
    pub sack_blocks: SackBlocks,

    /// The timestamp option.
    pub timestamp: Option<TimestampOption>,
}

impl SegmentOptions {
    /// Returns a builder of TCP Options to be used during serialization.
    pub fn builder(&self) -> TcpOptionsBuilder<'_> {
        let Self { timestamp, sack_blocks } = self;
        TcpOptionsBuilder {
            timestamp: timestamp.as_ref().map(Into::into),
            sack_blocks: sack_blocks.as_slice(),
            ..Default::default()
        }
    }
}

/// Segment options available on reset segments.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ResetOptions {
    /// The timestamp option.
    pub timestamp: Option<TimestampOption>,
}

impl ResetOptions {
    fn builder(&self) -> TcpOptionsBuilder<'_> {
        let Self { timestamp } = self;
        TcpOptionsBuilder { timestamp: timestamp.as_ref().map(Into::into), ..Default::default() }
    }
}

const MAX_SACK_BLOCKS: usize = 4;
/// Blocks of selective ACKs.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct SackBlocks(ArrayVec<TcpSackBlock, MAX_SACK_BLOCKS>);

impl SackBlocks {
    /// A constant empty instance of SACK blocks.
    pub const EMPTY: Self = SackBlocks(ArrayVec::new_const());

    /// The maximum number of selective ack blocks that can be in a TCP segment.
    ///
    /// See [RFC 2018 section 3].
    ///
    /// [RFC 2018 section 3] https://www.rfc-editor.org/rfc/rfc2018#section-3
    pub const MAX_BLOCKS: usize = MAX_SACK_BLOCKS;

    /// The maximum number of selective ack blocks that can be in a TCP segment
    /// that includes the timestamp option.
    ///
    /// See [RFC 2018 section 3].
    ///
    /// [RFC 2018 section 3] https://www.rfc-editor.org/rfc/rfc2018#section-3
    pub const MAX_BLOCKS_WITH_TIMESTAMP: usize = 3;

    /// Returns the contained selective ACKs as a slice of `TcpSackBlock`
    ///
    /// Returns `None` if this [`SackBlocks`] is empty.
    pub fn as_slice(&self) -> Option<&[TcpSackBlock]> {
        let Self(inner) = self;
        if inner.is_empty() {
            return None;
        }

        Some(inner.as_slice())
    }

    /// Returns an iterator over the *valid* [`SackBlock`]s contained in this
    /// option.
    pub fn iter_skip_invalid(&self) -> impl Iterator<Item = SackBlock> + '_ {
        self.try_iter().filter_map(|r| match r {
            Ok(s) => Some(s),
            Err(InvalidSackBlockError(_, _)) => None,
        })
    }

    /// Returns an iterator yielding the results of converting the blocks in
    /// this option to valid [`SackBlock`]s.
    pub fn try_iter(&self) -> impl Iterator<Item = Result<SackBlock, InvalidSackBlockError>> + '_ {
        let Self(inner) = self;
        inner.iter().map(|block| SackBlock::try_from(*block))
    }

    /// Creates a new [`SackBlocks`] option from a slice of blocks seen in a TCP
    /// segment.
    ///
    /// Ignores any blocks past [`SackBlocks::MAX_BLOCKS`].
    pub fn from_option(blocks: &[TcpSackBlock]) -> Self {
        Self(blocks.iter().take(Self::MAX_BLOCKS).copied().collect())
    }

    /// Returns `true` if there are no blocks present.
    pub fn is_empty(&self) -> bool {
        let Self(inner) = self;
        inner.is_empty()
    }

    /// Drops all blocks.
    pub fn clear(&mut self) {
        let Self(inner) = self;
        inner.clear()
    }
}

/// Creates a new [`SackBlocks`] option from an iterator of [`SackBlock`].
///
/// Ignores any blocks past [`SackBlocks::MAX_BLOCKS`].
impl FromIterator<SackBlock> for SackBlocks {
    fn from_iter<T: IntoIterator<Item = SackBlock>>(iter: T) -> Self {
        Self(iter.into_iter().take(Self::MAX_BLOCKS).map(|b| b.into()).collect())
    }
}

mod sack_block {
    use super::*;

    /// A selective ACK block.
    ///
    /// Contains the left and right markers for a received data segment. It is a
    /// witness for a valid non empty open range of `SeqNum`.
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    pub struct SackBlock {
        // NB: We don't use core::ops::Range here because it doesn't implement Copy.
        left: SeqNum,
        right: SeqNum,
    }

    impl SackBlock {
        /// Attempts to create a new [`SackBlock`] with the range `[left, right)`.
        ///
        /// Returns an error if `right` is at or before `left`.
        pub fn try_new(left: SeqNum, right: SeqNum) -> Result<Self, InvalidSackBlockError> {
            if right.after(left) {
                Ok(Self { left, right })
            } else {
                Err(InvalidSackBlockError(left, right))
            }
        }

        /// Creates a new [`SackBlock`] without checking that `right` is
        /// strictly after `left`.
        ///
        /// # Safety
        ///
        /// Caller must guarantee that `right.after(left)`.
        pub unsafe fn new_unchecked(left: SeqNum, right: SeqNum) -> Self {
            Self { left, right }
        }

        /// Consumes this [`SackBlock`] returning a [`Range`] representation.
        pub fn into_range(self) -> Range<SeqNum> {
            let Self { left, right } = self;
            Range { start: left, end: right }
        }

        /// Consumes this [`SackBlock`] returning a [`Range`] representation
        /// unwrapping the [`SeqNum`] representation into `u32`.
        pub fn into_range_u32(self) -> Range<u32> {
            let Self { left, right } = self;
            Range { start: left.into(), end: right.into() }
        }

        /// Returns the left (inclusive) edge of the block.
        pub fn left(&self) -> SeqNum {
            self.left
        }

        /// Returns the right (exclusive) edge of the block.
        pub fn right(&self) -> SeqNum {
            self.right
        }

        /// Returns a tuple of the left (inclusive) and right (exclusive) edges
        /// of the block.
        pub fn into_parts(self) -> (SeqNum, SeqNum) {
            let Self { left, right } = self;
            (left, right)
        }
    }

    /// Error returned when attempting to create a [`SackBlock`] with an invalid
    /// range (i.e. right edge <= left edge).
    #[derive(Debug, Eq, PartialEq, Clone, Copy)]
    pub struct InvalidSackBlockError(pub SeqNum, pub SeqNum);

    impl From<SackBlock> for TcpSackBlock {
        fn from(value: SackBlock) -> Self {
            let SackBlock { left, right } = value;
            TcpSackBlock::new(left.into(), right.into())
        }
    }

    impl TryFrom<TcpSackBlock> for SackBlock {
        type Error = InvalidSackBlockError;

        fn try_from(value: TcpSackBlock) -> Result<Self, Self::Error> {
            Self::try_new(value.left_edge().into(), value.right_edge().into())
        }
    }

    impl From<SackBlock> for Range<SeqNum> {
        fn from(value: SackBlock) -> Self {
            value.into_range()
        }
    }

    impl TryFrom<Range<SeqNum>> for SackBlock {
        type Error = InvalidSackBlockError;

        fn try_from(value: Range<SeqNum>) -> Result<Self, Self::Error> {
            let Range { start, end } = value;
            Self::try_new(start, end)
        }
    }
}
pub use sack_block::{InvalidSackBlockError, SackBlock};

/// The maximum length that the sequence number doesn't wrap around.
pub const MAX_PAYLOAD_AND_CONTROL_LEN: usize = 1 << 31;
// The following `as` is sound because it is representable by `u32`.
const MAX_PAYLOAD_AND_CONTROL_LEN_U32: u32 = MAX_PAYLOAD_AND_CONTROL_LEN as u32;

impl<P: Payload> Segment<P> {
    /// Creates a new segment with the provided header and data.
    ///
    /// Returns the segment along with how many bytes were removed to make sure
    /// sequence numbers don't wrap around, i.e., `seq.before(seq + seg.len())`.
    pub fn new(header: SegmentHeader, data: P) -> (Self, usize) {
        let SegmentHeader { seq, ack, wnd, control, push, options } = header;
        let has_control_len = control.map(Control::has_sequence_no).unwrap_or(false);

        let data_len = data.len();
        let discarded_len =
            data_len.saturating_sub(MAX_PAYLOAD_AND_CONTROL_LEN - usize::from(has_control_len));

        // Only keep the PSH bit if data is not empty.
        let push = push && data_len != 0;

        let (control, data) = if discarded_len > 0 {
            // If we have to truncate the segment, the FIN flag must be removed
            // because it is logically the last octet of the segment.
            let (control, control_len) = if control == Some(Control::FIN) {
                (None, 0)
            } else {
                (control, has_control_len.into())
            };
            // The following slice will not panic because `discarded_len > 0`,
            // thus `data.len() > MAX_PAYLOAD_AND_CONTROL_LEN - control_len`.
            (control, data.slice(0..MAX_PAYLOAD_AND_CONTROL_LEN_U32 - control_len))
        } else {
            (control, data)
        };

        (
            Segment { header: SegmentHeader { seq, ack, wnd, control, push, options }, data: data },
            discarded_len,
        )
    }

    /// Returns a borrow of the segment's header.
    pub fn header(&self) -> &SegmentHeader {
        &self.header
    }

    /// Returns a borrow of the data payload in this segment.
    pub fn data(&self) -> &P {
        &self.data
    }

    /// Destructures self into its inner parts: The segment header and the data
    /// payload.
    pub fn into_parts(self) -> (SegmentHeader, P) {
        let Self { header, data } = self;
        (header, data)
    }

    /// Maps the payload in the segment with `f`.
    pub fn map_payload<R, F: FnOnce(P) -> R>(self, f: F) -> Segment<R> {
        let Segment { header, data } = self;
        Segment { header, data: f(data) }
    }

    /// Returns the length of the segment in sequence number space.
    ///
    /// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-25):
    ///   SEG.LEN = the number of octets occupied by the data in the segment
    ///   (counting SYN and FIN)
    pub fn len(&self) -> u32 {
        self.header.len(self.data.len())
    }

    /// Returns the part of the incoming segment within the receive window.
    pub fn overlap(self, rnxt: SeqNum, rwnd: WindowSize) -> Option<Segment<P>> {
        let len = self.len();
        let Segment { header: SegmentHeader { seq, ack, wnd, control, options, push }, data } =
            self;

        // RFC 793 (https://tools.ietf.org/html/rfc793#page-69):
        //   There are four cases for the acceptability test for an incoming
        //   segment:
        //       Segment Receive  Test
        //       Length  Window
        //       ------- -------  -------------------------------------------
        //          0       0     SEG.SEQ = RCV.NXT
        //          0      >0     RCV.NXT =< SEG.SEQ < RCV.NXT+RCV.WND
        //         >0       0     not acceptable
        //         >0      >0     RCV.NXT =< SEG.SEQ < RCV.NXT+RCV.WND
        //                     or RCV.NXT =< SEG.SEQ+SEG.LEN-1 < RCV.NXT+RCV.WND
        let overlap = match (len, rwnd) {
            (0, WindowSize::ZERO) => seq == rnxt,
            (0, rwnd) => !rnxt.after(seq) && seq.before(rnxt + rwnd),
            (_len, WindowSize::ZERO) => false,
            (len, rwnd) => {
                (!rnxt.after(seq) && seq.before(rnxt + rwnd))
                    // Note: here we use RCV.NXT <= SEG.SEQ+SEG.LEN instead of
                    // the condition as quoted above because of the following
                    // text immediately after the above table:
                    //   One could tailor actual segments to fit this assumption by
                    //   trimming off any portions that lie outside the window
                    //   (including SYN and FIN), and only processing further if
                    //   the segment then begins at RCV.NXT.
                    // This is essential for TCP simultaneous open to work,
                    // otherwise, the state machine would reject the SYN-ACK
                    // sent by the peer.
                    || (!(seq + len).before(rnxt) && !(seq + len).after(rnxt + rwnd))
            }
        };
        overlap.then(move || {
            // We deliberately don't define `PartialOrd` for `SeqNum`, so we use
            // `cmp` below to utilize `cmp::{max,min}_by`.
            let cmp = |lhs: &SeqNum, rhs: &SeqNum| (*lhs - *rhs).cmp(&0);
            let new_seq = core::cmp::max_by(seq, rnxt, cmp);
            let new_len = core::cmp::min_by(seq + len, rnxt + rwnd, cmp) - new_seq;
            // The following unwrap won't panic because:
            // 1. if `seq` is after `rnxt`, then `start` would be 0.
            // 2. the interesting case is when `rnxt` is after `seq`, in that
            // case, we have `rnxt - seq > 0`, thus `new_seq - seq > 0`.
            let start = u32::try_from(new_seq - seq).unwrap();
            // The following unwrap won't panic because:
            // 1. The witness on `Segment` and `WindowSize` guarantees that
            // `len <= 2^31` and `rwnd <= 2^30-1` thus
            // `seq <= seq + len` and `rnxt <= rnxt + rwnd`.
            // 2. We are in the closure because `overlap` is true which means
            // `seq <= rnxt + rwnd` and `rnxt <= seq + len`.
            // With these two conditions combined, `new_len` can't be negative
            // so the unwrap can't panic.
            let new_len = u32::try_from(new_len).unwrap();
            let (new_control, new_data) = {
                match control {
                    Some(Control::SYN) => {
                        if start == 0 {
                            (Some(Control::SYN), data.slice(start..start + new_len - 1))
                        } else {
                            (None, data.slice(start - 1..start + new_len - 1))
                        }
                    }
                    Some(Control::FIN) => {
                        if len == start + new_len {
                            if new_len > 0 {
                                (Some(Control::FIN), data.slice(start..start + new_len - 1))
                            } else {
                                (None, data.slice(start - 1..start - 1))
                            }
                        } else {
                            (None, data.slice(start..start + new_len))
                        }
                    }
                    Some(Control::RST) | None => (control, data.slice(start..start + new_len)),
                }
            };
            Segment {
                header: SegmentHeader {
                    seq: new_seq,
                    ack,
                    wnd,
                    control: new_control,
                    options,
                    push,
                },
                data: new_data,
            }
        })
    }

    /// Creates a segment with no data.
    pub fn new_empty(header: SegmentHeader) -> Self {
        // All of the checks on lengths are optimized out:
        // https://godbolt.org/z/KPd537G6Y
        let (seg, truncated) = Self::new(header, P::new_empty());
        debug_assert_eq!(truncated, 0);
        seg
    }

    /// Creates an ACK segment.
    pub fn ack(seq: SeqNum, ack: SeqNum, wnd: UnscaledWindowSize, options: SegmentOptions) -> Self {
        Segment::new_empty(SegmentHeader {
            seq,
            ack: Some(ack),
            wnd,
            control: None,
            push: false,
            options: options.into(),
        })
    }

    /// Creates a SYN segment.
    pub fn syn(seq: SeqNum, wnd: UnscaledWindowSize, options: HandshakeOptions) -> Self {
        Segment::new_empty(SegmentHeader {
            seq,
            ack: None,
            wnd,
            control: Some(Control::SYN),
            push: false,
            options: options.into(),
        })
    }

    /// Creates a SYN-ACK segment.
    pub fn syn_ack(
        seq: SeqNum,
        ack: SeqNum,
        wnd: UnscaledWindowSize,
        options: HandshakeOptions,
    ) -> Self {
        Segment::new_empty(SegmentHeader {
            seq,
            ack: Some(ack),
            wnd,
            control: Some(Control::SYN),
            push: false,
            options: options.into(),
        })
    }

    /// Creates a RST segment.
    pub fn rst(seq: SeqNum, options: ResetOptions) -> Self {
        Segment::new_empty(SegmentHeader {
            seq,
            ack: None,
            wnd: UnscaledWindowSize::from(0),
            control: Some(Control::RST),
            push: false,
            options: options.into(),
        })
    }

    /// Creates a RST-ACK segment.
    pub fn rst_ack(seq: SeqNum, ack: SeqNum, options: ResetOptions) -> Self {
        Segment::new_empty(SegmentHeader {
            seq,
            ack: Some(ack),
            wnd: UnscaledWindowSize::from(0),
            control: Some(Control::RST),
            push: false,
            options: options.into(),
        })
    }
}

impl Segment<()> {
    /// Converts this segment with `()` data into any `P` payload's `new_empty`
    /// form.
    pub fn into_empty<P: Payload>(self) -> Segment<P> {
        self.map_payload(|()| P::new_empty())
    }
}

impl SegmentHeader {
    /// Returns the length of the segment in sequence number space.
    ///
    /// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-25):
    ///   SEG.LEN = the number of octets occupied by the data in the segment
    ///   (counting SYN and FIN)
    pub fn len(&self, payload_len: usize) -> u32 {
        // The following unwrap and addition are fine because:
        // - `u32::from(has_control_len)` is 0 or 1.
        // - `self.data.len() <= 2^31`.
        let has_control_len = self.control.map(Control::has_sequence_no).unwrap_or(false);
        u32::try_from(payload_len).unwrap() + u32::from(has_control_len)
    }

    /// Create a `SegmentHeader` from the provided builder and data length.  The
    /// options will be set to their default values.
    pub fn from_builder<A: IpAddress>(
        builder: &TcpSegmentBuilder<A>,
    ) -> Result<Self, MalformedFlags> {
        let control =
            Flags { syn: builder.syn_set(), fin: builder.fin_set(), rst: builder.rst_set() }
                .control()?;
        let options = Options::new(control.as_ref());
        Self::from_builder_options(builder, options)
    }

    /// Create a `SegmentHeader` from the provided builder, options, and data length.
    pub fn from_builder_options<A: IpAddress>(
        builder: &TcpSegmentBuilder<A>,
        options: Options,
    ) -> Result<Self, MalformedFlags> {
        Ok(SegmentHeader {
            seq: SeqNum::new(builder.seq_num()),
            ack: builder.ack_num().map(SeqNum::new),
            control: Flags {
                syn: builder.syn_set(),
                fin: builder.fin_set(),
                rst: builder.rst_set(),
            }
            .control()?,
            wnd: UnscaledWindowSize::from(builder.window_size()),
            push: builder.psh_set(),
            options: options,
        })
    }
}

/// A TCP payload that only allows for getting the length of the payload.
pub trait PayloadLen {
    /// Returns the length of the payload.
    fn len(&self) -> usize;
}

/// A TCP payload that operates around `u32` instead of `usize`.
pub trait Payload: PayloadLen + Sized {
    /// Creates a slice of the payload, reducing it to only the bytes within
    /// `range`.
    ///
    /// # Panics
    ///
    /// Panics if the provided `range` is not within the bounds of this
    /// `Payload`, or if the range is nonsensical (the end precedes
    /// the start).
    fn slice(self, range: Range<u32>) -> Self;

    /// Copies part of the payload beginning at `offset` into `dst`.
    ///
    /// # Panics
    ///
    /// Panics if offset is too large or we couldn't fill the `dst` slice.
    fn partial_copy(&self, offset: usize, dst: &mut [u8]);

    /// Copies part of the payload beginning at `offset` into `dst`.
    ///
    /// # Panics
    ///
    /// Panics if offset is too large or we couldn't fill the `dst` slice.
    fn partial_copy_uninit(&self, offset: usize, dst: &mut [MaybeUninit<u8>]);

    /// Creates a new empty payload.
    ///
    /// An empty payload must report 0 as its length.
    fn new_empty() -> Self;
}

impl PayloadLen for &[u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }
}

impl Payload for &[u8] {
    fn slice(self, Range { start, end }: Range<u32>) -> Self {
        // The following `unwrap`s are ok because:
        // `usize::try_from(x)` fails when `x > usize::MAX`; given that
        // `self.len() <= usize::MAX`, panic would be expected because `range`
        // exceeds the bound of `self`.
        let start = usize::try_from(start).unwrap_or_else(|TryFromIntError { .. }| {
            panic!("range start index {} out of range for slice of length {}", start, self.len())
        });
        let end = usize::try_from(end).unwrap_or_else(|TryFromIntError { .. }| {
            panic!("range end index {} out of range for slice of length {}", end, self.len())
        });
        &self[start..end]
    }

    fn partial_copy(&self, offset: usize, dst: &mut [u8]) {
        dst.copy_from_slice(&self[offset..offset + dst.len()])
    }

    fn partial_copy_uninit(&self, offset: usize, dst: &mut [MaybeUninit<u8>]) {
        let _ = dst.write_copy_of_slice(&self[offset..offset + dst.len()]);
    }

    fn new_empty() -> Self {
        &[]
    }
}

impl PayloadLen for () {
    fn len(&self) -> usize {
        0
    }
}

impl Payload for () {
    fn slice(self, Range { start, end }: Range<u32>) -> Self {
        if start != 0 {
            panic!("range start index {} out of range for slice of length 0", start);
        }
        if end != 0 {
            panic!("range end index {} out of range for slice of length 0", end);
        }
        ()
    }

    fn partial_copy(&self, offset: usize, dst: &mut [u8]) {
        if dst.len() != 0 || offset != 0 {
            panic!(
                "source slice length (0) does not match destination slice length ({})",
                dst.len()
            );
        }
    }

    fn partial_copy_uninit(&self, offset: usize, dst: &mut [MaybeUninit<u8>]) {
        if dst.len() != 0 || offset != 0 {
            panic!(
                "source slice length (0) does not match destination slice length ({})",
                dst.len()
            );
        }
    }

    fn new_empty() -> Self {
        ()
    }
}

impl<I: PayloadLen, B> PayloadLen for InnerSerializer<I, B> {
    fn len(&self) -> usize {
        PayloadLen::len(self.inner())
    }
}

/// Error returned when mutually exclusive TCP control flags are set together.
#[derive(Error, Debug, PartialEq, Eq)]
#[error("multiple mutually exclusive flags are set: syn: {syn}, fin: {fin}, rst: {rst}")]
pub struct MalformedFlags {
    syn: bool,
    fin: bool,
    rst: bool,
}

struct Flags {
    syn: bool,
    fin: bool,
    rst: bool,
}

impl Flags {
    fn control(&self) -> Result<Option<Control>, MalformedFlags> {
        if usize::from(self.syn) + usize::from(self.fin) + usize::from(self.rst) > 1 {
            return Err(MalformedFlags { syn: self.syn, fin: self.fin, rst: self.rst });
        }

        let syn = self.syn.then_some(Control::SYN);
        let fin = self.fin.then_some(Control::FIN);
        let rst = self.rst.then_some(Control::RST);

        Ok(syn.or(fin).or(rst))
    }
}

/// A TCP segment that has been verified to have valid flags. Can be converted
/// to a `Segment`.
pub struct VerifiedTcpSegment<'a> {
    segment: TcpSegment<&'a [u8]>,
    control: Option<Control>,
}

impl<'a> VerifiedTcpSegment<'a> {
    /// Returns the underlying [`TcpSegment`].
    pub fn tcp_segment(&self) -> &TcpSegment<&'a [u8]> {
        &self.segment
    }

    /// Returns the control flag of the segment.
    pub fn control(&self) -> Option<Control> {
        self.control
    }
}

impl<'a> TryFrom<TcpSegment<&'a [u8]>> for VerifiedTcpSegment<'a> {
    type Error = MalformedFlags;

    fn try_from(segment: TcpSegment<&'a [u8]>) -> Result<Self, Self::Error> {
        let control =
            Flags { syn: segment.syn(), fin: segment.fin(), rst: segment.rst() }.control()?;
        Ok(VerifiedTcpSegment { segment, control })
    }
}

impl<'a> From<&'a VerifiedTcpSegment<'a>> for Segment<&'a [u8]> {
    fn from(from: &'a VerifiedTcpSegment<'a>) -> Segment<&'a [u8]> {
        let VerifiedTcpSegment { segment, control } = from;
        let options = Options::from_options(control.as_ref(), segment.options());
        let (to, discarded) = Segment::new(
            SegmentHeader {
                seq: segment.seq_num().into(),
                ack: segment.ack_num().map(Into::into),
                wnd: UnscaledWindowSize::from(segment.window_size()),
                control: *control,
                push: segment.psh(),
                options,
            },
            from.segment.body(),
        );
        debug_assert_eq!(discarded, 0);
        to
    }
}

impl<A> TryFrom<&TcpSegmentBuilder<A>> for SegmentHeader
where
    A: IpAddress,
{
    type Error = MalformedFlags;

    fn try_from(from: &TcpSegmentBuilder<A>) -> Result<Self, Self::Error> {
        SegmentHeader::from_builder(from)
    }
}

impl<'a, A: IpAddress> TryFrom<&TcpSegmentBuilderWithOptions<A, TcpOptionsBuilder<'a>>>
    for SegmentHeader
{
    type Error = MalformedFlags;

    fn try_from(
        from: &TcpSegmentBuilderWithOptions<A, TcpOptionsBuilder<'a>>,
    ) -> Result<Self, Self::Error> {
        let prefix_builder = from.prefix_builder();
        let options = Options::try_from_options(&prefix_builder, from.options())?;
        Self::from_builder_options(prefix_builder, options)
    }
}

#[cfg(any(test, feature = "testutils"))]
mod testutils {
    use super::*;

    /// Provide a handy default implementation for tests only.
    impl Default for SegmentHeader {
        fn default() -> Self {
            Self {
                seq: SeqNum::new(0),
                ack: None,
                control: None,
                wnd: UnscaledWindowSize::from(0),
                options: Options::new(None),
                push: false,
            }
        }
    }

    impl<P: Payload> Segment<P> {
        /// Like [`Segment::new`] but asserts that no bytes were discarded from
        /// `data`.
        #[track_caller]
        pub fn new_assert_no_discard(header: SegmentHeader, data: P) -> Self {
            let (seg, discard) = Self::new(header, data);
            assert_eq!(discard, 0);
            seg
        }
    }

    impl<'a> Segment<&'a [u8]> {
        /// Create a new segment with the given seq, ack, and data.
        pub fn with_fake_data(seq: SeqNum, ack: SeqNum, data: &'a [u8]) -> Self {
            Self::new_assert_no_discard(
                SegmentHeader {
                    seq,
                    ack: Some(ack),
                    control: None,
                    wnd: UnscaledWindowSize::from(u16::MAX),
                    options: Options::new(None),
                    push: false,
                },
                data,
            )
        }
    }

    impl<P: Payload> Segment<P> {
        /// Creates a new segment with the provided data.
        pub fn with_data(
            seq: SeqNum,
            ack: SeqNum,
            wnd: UnscaledWindowSize,
            options: SegmentOptions,
            data: P,
        ) -> Segment<P> {
            Segment::new_assert_no_discard(
                SegmentHeader {
                    seq,
                    ack: Some(ack),
                    control: None,
                    wnd,
                    push: false,
                    options: Options::Segment(options),
                },
                data,
            )
        }

        /// Creates a new FIN segment with the provided data.
        pub fn piggybacked_fin(
            seq: SeqNum,
            ack: SeqNum,
            wnd: UnscaledWindowSize,
            options: SegmentOptions,
            data: P,
        ) -> Segment<P> {
            Segment::new_assert_no_discard(
                SegmentHeader {
                    seq,
                    ack: Some(ack),
                    control: Some(Control::FIN),
                    wnd,
                    push: false,
                    options: Options::Segment(options),
                },
                data,
            )
        }

        /// Creates a new FIN segment.
        pub fn fin(
            seq: SeqNum,
            ack: SeqNum,
            wnd: UnscaledWindowSize,
            options: SegmentOptions,
        ) -> Self {
            Segment::new_empty(SegmentHeader {
                seq,
                ack: Some(ack),
                control: Some(Control::FIN),
                wnd,
                push: false,
                options: Options::Segment(options),
            })
        }
    }
}

#[cfg(test)]
mod test {

    use crate::tcp::timestamp::Timestamp;
    use assert_matches::assert_matches;
    use core::num::NonZeroU16;
    use ip_test_macro::ip_test;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::ip::{Ipv4, Ipv6};
    use packet_formats::ip::IpExt;
    use test_case::test_case;

    use super::*;

    #[test_case(None, &[][..] => (0, &[][..]); "empty")]
    #[test_case(None, &[1][..] => (1, &[1][..]); "no control")]
    #[test_case(Some(Control::SYN), &[][..] => (1, &[][..]); "empty slice with syn")]
    #[test_case(Some(Control::SYN), &[1][..] => (2, &[1][..]); "non-empty slice with syn")]
    #[test_case(Some(Control::FIN), &[][..] => (1, &[][..]); "empty slice with fin")]
    #[test_case(Some(Control::FIN), &[1][..] => (2, &[1][..]); "non-empty slice with fin")]
    #[test_case(Some(Control::RST), &[][..] => (0, &[][..]); "empty slice with rst")]
    #[test_case(Some(Control::RST), &[1][..] => (1, &[1][..]); "non-empty slice with rst")]
    fn segment_len(control: Option<Control>, data: &[u8]) -> (u32, &[u8]) {
        let (seg, truncated) = Segment::new(
            SegmentHeader {
                seq: SeqNum::new(1),
                ack: Some(SeqNum::new(1)),
                wnd: UnscaledWindowSize::from(0),
                control,
                push: false,
                options: Options::new(None),
            },
            data,
        );
        assert_eq!(truncated, 0);
        (seg.len(), seg.data)
    }

    #[test_case(&[1, 2, 3, 4, 5][..], 0..4 => [1, 2, 3, 4])]
    #[test_case((), 0..0 => [0, 0, 0, 0])]
    fn payload_slice_copy(data: impl Payload, range: Range<u32>) -> [u8; 4] {
        let sliced = data.slice(range);
        let mut buffer = [0; 4];
        sliced.partial_copy(0, &mut buffer[..sliced.len()]);
        buffer
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TestPayload(Range<u32>);

    impl TestPayload {
        fn new(len: usize) -> Self {
            Self(0..u32::try_from(len).unwrap())
        }
    }

    impl PayloadLen for TestPayload {
        fn len(&self) -> usize {
            self.0.len()
        }
    }

    impl Payload for TestPayload {
        fn slice(self, range: Range<u32>) -> Self {
            let Self(this) = self;
            assert!(range.start >= this.start && range.end <= this.end);
            TestPayload(range)
        }

        fn partial_copy(&self, _offset: usize, _dst: &mut [u8]) {
            unimplemented!("TestPayload doesn't carry any data");
        }

        fn partial_copy_uninit(&self, _offset: usize, _dst: &mut [MaybeUninit<u8>]) {
            unimplemented!("TestPayload doesn't carry any data");
        }

        fn new_empty() -> Self {
            Self(0..0)
        }
    }

    #[test_case(100, Some(Control::SYN) => (100, Some(Control::SYN), 0))]
    #[test_case(100, Some(Control::FIN) => (100, Some(Control::FIN), 0))]
    #[test_case(100, Some(Control::RST) => (100, Some(Control::RST), 0))]
    #[test_case(100, None => (100, None, 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::SYN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::SYN), 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::FIN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::FIN), 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::RST)
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::RST), 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN - 1, None
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, None, 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN, Some(Control::SYN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::SYN), 1))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN, Some(Control::FIN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN, None, 1))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN, Some(Control::RST)
    => (MAX_PAYLOAD_AND_CONTROL_LEN, Some(Control::RST), 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN, None
    => (MAX_PAYLOAD_AND_CONTROL_LEN, None, 0))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN + 1, Some(Control::SYN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::SYN), 2))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN + 1, Some(Control::FIN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN, None, 2))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN + 1, Some(Control::RST)
    => (MAX_PAYLOAD_AND_CONTROL_LEN, Some(Control::RST), 1))]
    #[test_case(MAX_PAYLOAD_AND_CONTROL_LEN + 1, None
    => (MAX_PAYLOAD_AND_CONTROL_LEN, None, 1))]
    #[test_case(u32::MAX as usize, Some(Control::SYN)
    => (MAX_PAYLOAD_AND_CONTROL_LEN - 1, Some(Control::SYN), 1 << 31))]
    fn segment_truncate(len: usize, control: Option<Control>) -> (usize, Option<Control>, usize) {
        let (seg, truncated) = Segment::new(
            SegmentHeader {
                seq: SeqNum::new(0),
                ack: None,
                wnd: UnscaledWindowSize::from(0),
                control,
                push: false,
                options: Options::new(None),
            },
            TestPayload::new(len),
        );
        (seg.data.len(), seg.header.control, truncated)
    }

    struct OverlapTestArgs {
        seg_seq: u32,
        control: Option<Control>,
        data_len: u32,
        rcv_nxt: u32,
        rcv_wnd: usize,
    }
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: 0,
        rcv_nxt: 0,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 0,
    } => Some((SeqNum::new(1), None, 0..0)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: 0,
        rcv_nxt: 2,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::SYN),
        data_len: 0,
        rcv_nxt: 2,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::SYN),
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::SYN),
        data_len: 0,
        rcv_nxt: 0,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::FIN),
        data_len: 0,
        rcv_nxt: 2,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::FIN),
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::FIN),
        data_len: 0,
        rcv_nxt: 0,
        rcv_wnd: 0,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 0,
        control: None,
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 0..0)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 2,
        control: None,
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 0,
        control: None,
        data_len: 1,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 1..1)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 0,
        control: Some(Control::SYN),
        data_len: 0,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 0..0)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 2,
        control: None,
        data_len: 1,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => None)]
    #[test_case(OverlapTestArgs{
        seg_seq: 0,
        control: None,
        data_len: 2,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 1..2)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: 2,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 0..1)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 0,
        control: Some(Control::SYN),
        data_len: 1,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 0..1)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::SYN),
        data_len: 1,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), Some(Control::SYN), 0..0)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 0,
        control: Some(Control::FIN),
        data_len: 1,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), Some(Control::FIN), 1..1)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::FIN),
        data_len: 1,
        rcv_nxt: 1,
        rcv_wnd: 1,
    } => Some((SeqNum::new(1), None, 0..1)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: MAX_PAYLOAD_AND_CONTROL_LEN_U32,
        rcv_nxt: 1,
        rcv_wnd: 10,
    } => Some((SeqNum::new(1), None, 0..10)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 10,
        control: None,
        data_len: MAX_PAYLOAD_AND_CONTROL_LEN_U32,
        rcv_nxt: 1,
        rcv_wnd: 10,
    } => Some((SeqNum::new(10), None, 0..1)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: None,
        data_len: 10,
        rcv_nxt: 1,
        rcv_wnd: WindowSize::MAX.into(),
    } => Some((SeqNum::new(1), None, 0..10)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 10,
        control: None,
        data_len: 10,
        rcv_nxt: 1,
        rcv_wnd: WindowSize::MAX.into(),
    } => Some((SeqNum::new(10), None, 0..10)))]
    #[test_case(OverlapTestArgs{
        seg_seq: 1,
        control: Some(Control::FIN),
        data_len: 1,
        rcv_nxt: 3,
        rcv_wnd: 10,
    } => Some((SeqNum::new(3), None, 1..1)); "regression test for https://fxbug.dev/42061750")]
    fn segment_overlap(
        OverlapTestArgs { seg_seq, control, data_len, rcv_nxt, rcv_wnd }: OverlapTestArgs,
    ) -> Option<(SeqNum, Option<Control>, Range<u32>)> {
        let (seg, discarded) = Segment::new(
            SegmentHeader {
                seq: SeqNum::new(seg_seq),
                ack: None,
                control,
                wnd: UnscaledWindowSize::from(0),
                push: false,
                options: Options::new(None),
            },
            TestPayload(0..data_len),
        );
        assert_eq!(discarded, 0);
        seg.overlap(SeqNum::new(rcv_nxt), WindowSize::new(rcv_wnd).unwrap()).map(
            |Segment { header: SegmentHeader { seq, control, .. }, data: TestPayload(range) }| {
                (seq, control, range)
            },
        )
    }

    pub trait TestIpExt: IpExt {
        const SRC_IP: Self::Addr;
        const DST_IP: Self::Addr;
    }

    impl TestIpExt for Ipv4 {
        const SRC_IP: Self::Addr = net_ip_v4!("192.0.2.1");
        const DST_IP: Self::Addr = net_ip_v4!("192.0.2.2");
    }

    impl TestIpExt for Ipv6 {
        const SRC_IP: Self::Addr = net_ip_v6!("2001:db8::1");
        const DST_IP: Self::Addr = net_ip_v6!("2001:db8::2");
    }

    const SRC_PORT: NonZeroU16 = NonZeroU16::new(1234).unwrap();
    const DST_PORT: NonZeroU16 = NonZeroU16::new(9876).unwrap();

    #[ip_test(I)]
    fn from_segment_builder<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.syn(true);

        let converted_header =
            SegmentHeader::try_from(&builder).expect("failed to convert serializer");

        let expected_header = SegmentHeader {
            seq: SeqNum::new(1),
            ack: Some(SeqNum::new(2)),
            wnd: UnscaledWindowSize::from(3u16),
            control: Some(Control::SYN),
            options: HandshakeOptions::default().into(),
            push: false,
        };

        assert_eq!(converted_header, expected_header);
    }

    #[ip_test(I)]
    fn from_segment_builder_failure<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.syn(true);
        builder.fin(true);

        assert_matches!(
            SegmentHeader::try_from(&builder),
            Err(MalformedFlags { syn: true, fin: true, rst: false })
        );
    }

    #[ip_test(I)]
    fn from_segment_builder_with_options_handshake<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.syn(true);

        const OPTIONS: HandshakeOptions = HandshakeOptions {
            mss: Some(Mss::new(1024).unwrap()),
            window_scale: Some(WindowScale::new(10).unwrap()),
            sack_permitted: true,
            timestamp: Some(TimestampOption {
                ts_val: Timestamp::new(1),
                ts_echo_reply: Timestamp::new(0),
            }),
        };

        let builder = TcpSegmentBuilderWithOptions::new(builder, OPTIONS.builder())
            .expect("failed to create tcp segment builder");

        let converted_header =
            SegmentHeader::try_from(&builder).expect("failed to convert serializer");

        let expected_header = SegmentHeader {
            seq: SeqNum::new(1),
            ack: Some(SeqNum::new(2)),
            wnd: UnscaledWindowSize::from(3u16),
            control: Some(Control::SYN),
            push: false,
            options: Options::Handshake(OPTIONS),
        };

        assert_eq!(converted_header, expected_header);
    }

    #[ip_test(I)]
    fn mss_option_clamps_to_minimum<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.syn(true);

        let builder = TcpSegmentBuilderWithOptions::new(
            builder,
            // An MSS option below the minimum should be increased to the
            // minimum.
            TcpOptionsBuilder { mss: Some(Mss::MIN.get() - 1), ..Default::default() },
        )
        .expect("failed to create tcp segment builder");

        let converted_header =
            SegmentHeader::try_from(&builder).expect("failed to convert serializer");

        let expected_header = SegmentHeader {
            seq: SeqNum::new(1),
            ack: Some(SeqNum::new(2)),
            wnd: UnscaledWindowSize::from(3u16),
            control: Some(Control::SYN),
            push: false,
            options: HandshakeOptions {
                mss: Some(Mss::MIN),
                window_scale: None,
                sack_permitted: false,
                timestamp: None,
            }
            .into(),
        };

        assert_eq!(converted_header, expected_header);
    }

    #[ip_test(I)]
    fn from_segment_builder_with_options_segment<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.psh(true);

        let sack_blocks = [TcpSackBlock::new(1, 2), TcpSackBlock::new(4, 6)];
        let options = SegmentOptions {
            timestamp: Some(TimestampOption {
                ts_val: Timestamp::new(1234),
                ts_echo_reply: Timestamp::new(4321),
            }),
            sack_blocks: SackBlocks::from_option(&sack_blocks),
        };

        let builder = TcpSegmentBuilderWithOptions::new(builder, options.builder())
            .expect("failed to create tcp segment builder");

        let converted_header =
            SegmentHeader::try_from(&builder).expect("failed to convert serializer");

        let expected_header = SegmentHeader {
            seq: SeqNum::new(1),
            ack: Some(SeqNum::new(2)),
            wnd: UnscaledWindowSize::from(3u16),
            control: None,
            push: true,
            options: Options::Segment(options),
        };

        assert_eq!(converted_header, expected_header);
    }

    #[ip_test(I)]
    fn from_segment_builder_with_options_failure<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.syn(true);
        builder.fin(true);

        let builder = TcpSegmentBuilderWithOptions::new(
            builder,
            TcpOptionsBuilder { mss: Some(1024), window_scale: Some(10), ..Default::default() },
        )
        .expect("failed to create tcp segment builder");

        assert_matches!(
            SegmentHeader::try_from(&builder),
            Err(MalformedFlags { syn: true, fin: true, rst: false })
        );
    }

    #[ip_test(I)]
    fn from_segment_builder_with_options_reset<I: TestIpExt>() {
        let mut builder =
            TcpSegmentBuilder::new(I::SRC_IP, I::DST_IP, SRC_PORT, DST_PORT, 1, Some(2), 3);
        builder.rst(true);

        const OPTIONS: ResetOptions = ResetOptions {
            timestamp: Some(TimestampOption {
                ts_val: Timestamp::new(1234),
                ts_echo_reply: Timestamp::new(4321),
            }),
        };

        let builder = TcpSegmentBuilderWithOptions::new(builder, OPTIONS.builder())
            .expect("failed to create tcp segment builder");

        let converted_header =
            SegmentHeader::try_from(&builder).expect("failed to convert serializer");

        let expected_header = SegmentHeader {
            seq: SeqNum::new(1),
            ack: Some(SeqNum::new(2)),
            wnd: UnscaledWindowSize::from(3u16),
            control: Some(Control::RST),
            push: false,
            options: Options::Reset(OPTIONS),
        };

        assert_eq!(converted_header, expected_header);
    }

    #[test_case(Flags {
            syn: false,
            fin: false,
            rst: false,
        } => Ok(None))]
    #[test_case(Flags {
            syn: true,
            fin: false,
            rst: false,
        } => Ok(Some(Control::SYN)))]
    #[test_case(Flags {
            syn: false,
            fin: true,
            rst: false,
        } => Ok(Some(Control::FIN)))]
    #[test_case(Flags {
            syn: false,
            fin: false,
            rst: true,
        } => Ok(Some(Control::RST)))]
    #[test_case(Flags {
            syn: true,
            fin: true,
            rst: false,
        } => Err(MalformedFlags {
            syn: true,
            fin: true,
            rst: false,
        }))]
    #[test_case(Flags {
            syn: true,
            fin: false,
            rst: true,
        } => Err(MalformedFlags {
            syn: true,
            fin: false,
            rst: true,
        }))]
    #[test_case(Flags {
            syn: false,
            fin: true,
            rst: true,
        } => Err(MalformedFlags {
            syn: false,
            fin: true,
            rst: true,
        }))]
    #[test_case(Flags {
            syn: true,
            fin: true,
            rst: true,
        } => Err(MalformedFlags {
            syn: true,
            fin: true,
            rst: true,
        }))]
    fn flags_to_control(input: Flags) -> Result<Option<Control>, MalformedFlags> {
        input.control()
    }

    #[test]
    fn sack_block_try_new() {
        assert_matches!(SackBlock::try_new(SeqNum::new(1), SeqNum::new(2)), Ok(_));
        assert_matches!(
            SackBlock::try_new(SeqNum::new(0u32.wrapping_sub(1)), SeqNum::new(2)),
            Ok(_)
        );
        assert_eq!(
            SackBlock::try_new(SeqNum::new(1), SeqNum::new(1)),
            Err(InvalidSackBlockError(SeqNum::new(1), SeqNum::new(1)))
        );
        assert_eq!(
            SackBlock::try_new(SeqNum::new(2), SeqNum::new(1)),
            Err(InvalidSackBlockError(SeqNum::new(2), SeqNum::new(1)))
        );
        assert_eq!(
            SackBlock::try_new(SeqNum::new(0), SeqNum::new(0u32.wrapping_sub(1))),
            Err(InvalidSackBlockError(SeqNum::new(0), SeqNum::new(0u32.wrapping_sub(1))))
        );
    }

    #[test]
    fn psh_bit_cleared_if_no_data() {
        let seg =
            Segment::new_assert_no_discard(SegmentHeader { push: true, ..Default::default() }, ());
        assert_eq!(seg.header().push, false);
        let seg = Segment::new_assert_no_discard(
            SegmentHeader { push: true, ..Default::default() },
            &[1u8, 2, 3, 4][..],
        );
        assert_eq!(seg.header().push, true);
    }
}
