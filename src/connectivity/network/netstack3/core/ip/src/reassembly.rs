// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Module for IP fragmented packet reassembly support.
//!
//! `reassembly` is a utility to support reassembly of fragmented IP packets.
//! Fragmented packets are associated by a combination of the packets' source
//! address, destination address and identification value. When a potentially
//! fragmented packet is received, this utility will check to see if the packet
//! is in fact fragmented or not. If it isn't fragmented, it will be returned as
//! is without any modification. If it is fragmented, this utility will capture
//! its body and store it in a cache while waiting for all the fragments for a
//! packet to arrive. The header information from a fragment with offset set to
//! 0 will also be kept to add to the final, reassembled packet. Once this
//! utility has received all the fragments for a combination of source address,
//! destination address and identification value, the implementer will need to
//! allocate a buffer of sufficient size to reassemble the final packet into and
//! pass it to this utility. This utility will then attempt to reassemble and
//! parse the packet, which will be returned to the caller. The caller should
//! then handle the returned packet as a normal IP packet. Note, there is a
//! timer from receipt of the first fragment to reassembly of the final packet.
//! See [`REASSEMBLY_TIMEOUT_SECONDS`].
//!
//! Note, this utility does not support reassembly of jumbogram packets.
//! According to the IPv6 Jumbogram RFC (RFC 2675), the jumbogram payload option
//! is relevant only for nodes that may be attached to links with a link MTU
//! greater than 65575 bytes. Note, the maximum size of a non-jumbogram IPv6
//! packet is also 65575 (as the payload length field for IP packets is 16 bits
//! + 40 byte IPv6 header). If a link supports an MTU greater than the maximum
//! size of a non-jumbogram packet, the packet should not be fragmented.

use alloc::collections::{BTreeSet, BinaryHeap};
use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt::Debug;
use core::hash::Hash;
use core::time::Duration;

use assert_matches::assert_matches;
use log::debug;
use net_types::ip::{GenericOverIp, Ip, IpAddr, IpVersionMarker, Ipv4, Ipv6};
use netstack3_base::{
    CoreTimerContext, HandleableTimer, InstantBindingsTypes, IpExt, LocalTimerHeap,
    TimerBindingsTypes, TimerContext,
};
use netstack3_hashmap::hash_map::{Entry, HashMap};
use packet::{BufferViewMut, ParsablePacket as _};
use packet_formats::ip::{IpPacket, Ipv4Proto};
use packet_formats::ipv4::{Ipv4Header, Ipv4Packet};
use packet_formats::ipv6::Ipv6Packet;
use packet_formats::ipv6::ext_hdrs::Ipv6ExtensionHeader;
use zerocopy::{SplitByteSlice, SplitByteSliceMut};

/// An IP extension trait supporting reassembly of fragments.
pub trait ReassemblyIpExt: IpExt {
    /// The maximum amount of time from receipt of the first fragment to
    /// reassembly of a packet. Note, "first fragment" does not mean a fragment
    /// with offset 0; it means the first fragment packet we receive with a new
    /// combination of source address, destination address and fragment
    /// identification value.
    const REASSEMBLY_TIMEOUT: Duration;

    /// An IP specific field that should be considered part of the
    /// [`FragmentCacheKey`].
    type FragmentCacheKeyPart: Copy + Clone + Debug + Hash + PartialEq + Eq;

    /// Returns the IP specific portion of the [`FragmentCacheKey`] from the
    /// packet.
    fn ip_specific_key_part<B: SplitByteSlice>(
        packet: &Self::Packet<B>,
    ) -> Self::FragmentCacheKeyPart;
}

impl ReassemblyIpExt for Ipv4 {
    /// This value is specified in RFC 729, section 3.1:
    ///   The current recommendation for the initial timer setting is 15
    ///   seconds.
    const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(15);

    /// IPv4 considers the inner protocol to be part of the fragmentation key.
    /// From RFC 791, section 2.3:
    ///   To assemble the fragments of an internet datagram, an internet
    ///   protocol module (for example at a destination host) combines
    ///   internet datagrams that all have the same value for the four fields:
    ///   identification, source, destination, and protocol.
    type FragmentCacheKeyPart = Ipv4Proto;

    fn ip_specific_key_part<B: SplitByteSlice>(
        packet: &Self::Packet<B>,
    ) -> Self::FragmentCacheKeyPart {
        IpPacket::proto(packet)
    }
}

impl ReassemblyIpExt for Ipv6 {
    /// This value is specified in RFC 8200, section 4.5:
    ///   If insufficient fragments are received to complete reassembly
    ///   of a packet within 60 seconds of the reception of the first-
    ///   arriving fragment of that packet, reassembly of that packet
    ///   must be abandoned and all the fragments that have been received
    ///   for that packet must be discarded.
    const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(60);

    /// Unlike IPv4, IPv6 allows reassembling fragments that have different
    /// inner protocols. From RFC 8200, section 4.5:
    ///   The Next Header values in the Fragment headers of different
    ///   fragments of the same original packet may differ.  Only the value
    ///   from the Offset zero fragment packet is used for reassembly.
    type FragmentCacheKeyPart = ();

    fn ip_specific_key_part<B: SplitByteSlice>(
        _packet: &Self::Packet<B>,
    ) -> Self::FragmentCacheKeyPart {
        ()
    }
}

/// Number of bytes per fragment block for IPv4 and IPv6.
///
/// IPv4 outlines the fragment block size in RFC 791 section 3.1, under the
/// fragment offset field's description: "The fragment offset is measured in
/// units of 8 octets (64 bits)".
///
/// IPv6 outlines the fragment block size in RFC 8200 section 4.5, under the
/// fragment offset field's description: "The offset, in 8-octet units, of the
/// data following this header".
const FRAGMENT_BLOCK_SIZE: u8 = 8;

/// Maximum number of fragment blocks an IPv4 or IPv6 packet can have.
///
/// We use this value because both IPv4 fixed header's fragment offset field and
/// IPv6 fragment extension header's fragment offset field are 13 bits wide.
const MAX_FRAGMENT_BLOCKS: u16 = 8191;

/// The state context for the fragment cache.
pub trait FragmentContext<I: Ip, BT: FragmentBindingsTypes> {
    /// Returns a mutable reference to the fragment cache.
    fn with_state_mut<O, F: FnOnce(&mut IpPacketFragmentCache<I, BT>) -> O>(&mut self, cb: F) -> O;
}

/// The bindings types for IP packet fragment reassembly.
pub trait FragmentBindingsTypes: TimerBindingsTypes + InstantBindingsTypes {}
impl<BT> FragmentBindingsTypes for BT where BT: TimerBindingsTypes + InstantBindingsTypes {}

/// The bindings execution context for IP packet fragment reassembly.
pub trait FragmentBindingsContext: TimerContext + FragmentBindingsTypes {}
impl<BC> FragmentBindingsContext for BC where BC: TimerContext + FragmentBindingsTypes {}

/// The timer ID for the fragment cache.
#[derive(Hash, Eq, PartialEq, Default, Clone, Debug, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub struct FragmentTimerId<I: Ip>(IpVersionMarker<I>);

/// An implementation of a fragment cache.
pub trait FragmentHandler<I: ReassemblyIpExt, BC> {
    /// Attempts to process a packet fragment.
    ///
    /// # Panics
    ///
    /// Panics if the packet has no fragment data.
    fn process_fragment<B: SplitByteSlice>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: I::Packet<B>,
    ) -> FragmentProcessingState<I, B>
    where
        I::Packet<B>: FragmentablePacket;

    /// Attempts to reassemble a packet.
    ///
    /// Attempts to reassemble a packet associated with a given
    /// `FragmentCacheKey`, `key`, and cancels the timer to reset reassembly
    /// data. The caller is expected to allocate a buffer of sufficient size
    /// (available from `process_fragment` when it returns a
    /// `FragmentProcessingState::Ready` value) and provide it to
    /// `reassemble_packet` as `buffer` where the packet will be reassembled
    /// into.
    ///
    /// Returns the size of the largest fragment received for this packet.
    ///
    /// # Panics
    ///
    /// Panics if the provided `buffer` does not have enough capacity for the
    /// reassembled packet. Also panics if a different `ctx` is passed to
    /// `reassemble_packet` from the one passed to `process_fragment` when
    /// processing a packet with a given `key` as `reassemble_packet` will fail
    /// to cancel the reassembly timer.
    fn reassemble_packet<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
        &mut self,
        bindings_ctx: &mut BC,
        key: &FragmentCacheKey<I>,
        buffer: BV,
    ) -> Result<usize, FragmentReassemblyError>;
}

impl<I: IpExt + ReassemblyIpExt, BC: FragmentBindingsContext, CC: FragmentContext<I, BC>>
    FragmentHandler<I, BC> for CC
{
    fn process_fragment<B: SplitByteSlice>(
        &mut self,
        bindings_ctx: &mut BC,
        packet: I::Packet<B>,
    ) -> FragmentProcessingState<I, B>
    where
        I::Packet<B>: FragmentablePacket,
    {
        self.with_state_mut(|cache| {
            let (res, timer_action) = cache.process_fragment(packet);

            if let Some(timer_action) = timer_action {
                match timer_action {
                    // TODO(https://fxbug.dev/414413500): for IPv4, use the
                    // fragment's TTL to determine the timeout.
                    CacheTimerAction::CreateNewTimer(key) => {
                        assert_eq!(
                            cache.timers.schedule_after(
                                bindings_ctx,
                                key,
                                (),
                                I::REASSEMBLY_TIMEOUT,
                            ),
                            None
                        )
                    }
                    CacheTimerAction::CancelExistingTimer(key) => {
                        assert_ne!(cache.timers.cancel(bindings_ctx, &key), None)
                    }
                }
            }

            res
        })
    }

    fn reassemble_packet<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
        &mut self,
        bindings_ctx: &mut BC,
        key: &FragmentCacheKey<I>,
        buffer: BV,
    ) -> Result<usize, FragmentReassemblyError> {
        self.with_state_mut(|cache| {
            let res = cache.reassemble_packet(key, buffer);

            match res {
                Ok(_) | Err(FragmentReassemblyError::PacketParsingError) => {
                    // Cancel the reassembly timer as we attempt reassembly which
                    // means we had all the fragments for the final packet, even
                    // if parsing the reassembled packet failed.
                    assert_matches!(cache.timers.cancel(bindings_ctx, key), Some(_));
                }
                Err(FragmentReassemblyError::InvalidKey)
                | Err(FragmentReassemblyError::MissingFragments) => {}
            }

            res
        })
    }
}

impl<I: ReassemblyIpExt, BC: FragmentBindingsContext, CC: FragmentContext<I, BC>>
    HandleableTimer<CC, BC> for FragmentTimerId<I>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, _: BC::UniqueTimerId) {
        let Self(IpVersionMarker { .. }) = self;
        core_ctx.with_state_mut(|cache| {
            let Some((key, ())) = cache.timers.pop(bindings_ctx) else {
                return;
            };

            // If a timer fired, the `key` must still exist in our fragment cache.
            let FragmentCacheData {
                missing_blocks: _,
                body_fragments,
                header: _,
                total_size,
                max_fragment_len: _,
            } = assert_matches!(cache.remove_data(&key), Some(c) => c);
            debug!(
                "reassembly for {key:?} \
                timed out with {} fragments and {total_size} bytes",
                body_fragments.len(),
            );
        });
    }
}

/// Trait that must be implemented by any packet type that is fragmentable.
pub trait FragmentablePacket {
    /// Return fragment identifier data.
    ///
    /// Returns the fragment identification, offset and more flag as `(a, b, c)`
    /// where `a` is the fragment identification value, `b` is the fragment
    /// offset and `c` is the more flag.
    ///
    /// # Panics
    ///
    /// Panics if the packet has no fragment data.
    fn fragment_data(&self) -> (u32, u16, bool);
}

impl<B: SplitByteSlice> FragmentablePacket for Ipv4Packet<B> {
    fn fragment_data(&self) -> (u32, u16, bool) {
        (u32::from(self.id()), self.fragment_offset().into_raw(), self.mf_flag())
    }
}

impl<B: SplitByteSlice> FragmentablePacket for Ipv6Packet<B> {
    fn fragment_data(&self) -> (u32, u16, bool) {
        for ext_hdr in self.iter_extension_hdrs() {
            if let Ipv6ExtensionHeader::Fragment { fragment_data } = ext_hdr {
                return (
                    fragment_data.identification(),
                    fragment_data.fragment_offset().into_raw(),
                    fragment_data.m_flag(),
                );
            }
        }

        unreachable!(
            "Should never call this function if the packet does not have a fragment header"
        );
    }
}

/// Possible return values for [`IpPacketFragmentCache::process_fragment`].
#[derive(Debug)]
pub enum FragmentProcessingState<I: ReassemblyIpExt, B: SplitByteSlice> {
    /// The provided packet is not fragmented so no processing is required.
    /// The packet is returned with this value without any modification.
    NotNeeded(I::Packet<B>),

    /// The provided packet is fragmented but it is malformed.
    ///
    /// Possible reasons for being malformed are:
    ///  1) Body is not a multiple of `FRAGMENT_BLOCK_SIZE` and  it is not the
    ///     last fragment (last fragment of a packet, not last fragment received
    ///     for a packet).
    ///  2) Overlaps with an existing fragment. This is explicitly not allowed
    ///     for IPv6 as per RFC 8200 section 4.5 (more details in RFC 5722). We
    ///     choose the same behaviour for IPv4 for the same reasons.
    ///  3) Packet's fragment offset + # of fragment blocks >
    ///     `MAX_FRAGMENT_BLOCKS`.
    InvalidFragment,

    /// Successfully processed the provided fragment. We are still waiting on
    /// more fragments for a packet to arrive before being ready to reassemble
    /// the packet.
    NeedMoreFragments,

    /// Cannot process the fragment because the cache's capacity is currently
    /// exceeded.
    OutOfMemory,

    /// Successfully processed the provided fragment. We now have all the
    /// fragments we need to reassemble the packet. The caller must create a
    /// buffer with capacity for at least `packet_len` bytes and provide the
    /// buffer and `key` to `reassemble_packet`.
    Ready { key: FragmentCacheKey<I>, packet_len: usize },
}

/// Possible errors when attempting to reassemble a packet.
#[derive(Debug, PartialEq, Eq)]
pub enum FragmentReassemblyError {
    /// At least one fragment for a packet has not arrived.
    MissingFragments,

    /// A `FragmentCacheKey` is not associated with any packet. This could be
    /// because either no fragment has yet arrived for a packet associated with
    /// a `FragmentCacheKey` or some fragments did arrive, but the reassembly
    /// timer expired and got discarded.
    InvalidKey,

    /// Packet parsing error.
    PacketParsingError,
}

/// Fragment Cache Key.
///
/// Composed of the original packet's source address, destination address,
/// and fragment id.
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq)]
pub struct FragmentCacheKey<I: ReassemblyIpExt> {
    src_ip: I::Addr,
    dst_ip: I::Addr,
    fragment_id: u32,
    ip_specific_fields: I::FragmentCacheKeyPart,
}

/// An inclusive-inclusive range of bytes within a reassembled packet.
// NOTE: We use this instead of `std::ops::RangeInclusive` because the latter
// provides getter methods which return references, and it adds a lot of
// unnecessary dereferences.
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
struct BlockRange {
    start: u16,
    end: u16,
}

/// Data required for fragmented packet reassembly.
#[derive(Debug)]
struct FragmentCacheData {
    /// List of non-overlapping inclusive ranges of fragment blocks required
    /// before being ready to reassemble a packet.
    ///
    /// When creating a new instance of `FragmentCacheData`, we will set
    /// `missing_blocks` to a list with a single element representing all
    /// blocks, (0, MAX_VALUE). In this case, MAX_VALUE will be set to
    /// `core::u16::MAX`.
    missing_blocks: BTreeSet<BlockRange>,

    /// Received fragment blocks.
    ///
    /// We use a binary heap for help when reassembling packets. When we
    /// reassemble packets, we will want to fill up a new buffer with all the
    /// body fragments. The easiest way to do this is in order, from the
    /// fragment with offset 0 to the fragment with the highest offset. Since we
    /// only need to enforce the order when reassembling, we use a min-heap so
    /// we have a defined order (increasing fragment offset values) when
    /// popping. `BinaryHeap` is technically a max-heap, but we use the negative
    /// of the offset values as the key for the heap. See
    /// [`PacketBodyFragment::new`].
    body_fragments: BinaryHeap<PacketBodyFragment>,

    /// The header data for the reassembled packet.
    ///
    /// The header of the fragment packet with offset 0 will be used as the
    /// header for the final, reassembled packet.
    header: Option<Vec<u8>>,

    /// Total number of bytes in the reassembled packet.
    ///
    /// This is used so that we don't have to iterated through `body_fragments`
    /// and sum the partial body sizes to calculate the reassembled packet's
    /// size.
    total_size: usize,

    /// Size of the largest fragment received for this packet.
    max_fragment_len: usize,
}

impl Default for FragmentCacheData {
    fn default() -> FragmentCacheData {
        FragmentCacheData {
            missing_blocks: core::iter::once(BlockRange { start: 0, end: u16::MAX }).collect(),
            body_fragments: BinaryHeap::new(),
            header: None,
            total_size: 0,
            max_fragment_len: 0,
        }
    }
}

impl FragmentCacheData {
    /// Attempts to find a gap where the provided `BlockRange` will fit in.
    fn find_gap(&self, BlockRange { start, end }: BlockRange) -> FindGapResult {
        let result = self.missing_blocks.iter().find_map(|gap| {
            // This gap completely contains the provided range.
            if gap.start <= start && gap.end >= end {
                return Some(FindGapResult::Found { gap: *gap });
            }

            // This gap is completely disjoint from the provided range.
            // Ignore it.
            if gap.start > end || gap.end < start {
                return None;
            }

            // If neither of the above are true, this gap must overlap with
            // the provided range.
            return Some(FindGapResult::Overlap);
        });

        match result {
            Some(result) => result,
            None => {
                // Searching the missing blocks didn't find a suitable gap nor
                // an overlap. Check for an out-of-bounds range before
                // concluding that this range must be a duplicate.

                // Note: `last` *must* exist and *must* represent the final
                // fragment. If we had not yet received the final fragment, the
                // search through the `missing_blocks` would be guaranteed to
                // return `Some` (because it would contain a range with an end
                // equal to u16::Max).
                let last = self.body_fragments.peek().unwrap();
                if last.offset < start {
                    FindGapResult::OutOfBounds
                } else {
                    FindGapResult::Duplicate
                }
            }
        }
    }
}

/// The result of calling [`FragmentCacheData::find_gap`].
enum FindGapResult {
    // The provided `BlockRange` fits inside of an existing gap. The gap may be
    // completely or partially filled by the provided `BlockRange`.
    Found {
        gap: BlockRange,
    },
    // The provided `BlockRange` overlaps with data we've already received.
    // Specifically, an overlap occurs if the provided `BlockRange` is partially
    // contained within a gap.
    Overlap,
    /// The provided `BlockRange` has an end beyond the known end of the packet.
    OutOfBounds,
    // The provided `BlockRange` has already been received. Specifically, a
    // duplicate occurs if the provided `BlockRange` is completely disjoint from
    // all known gaps.
    //
    // RFC 8200, Section 4.5 states:
    //   It should be noted that fragments may be duplicated in the
    //   network.  Instead of treating these exact duplicate fragments
    //   as overlapping fragments, an implementation may choose to
    //   detect this case and drop exact duplicate fragments while
    //   keeping the other fragments belonging to the same packet.
    //
    // Here we take a loose interpretation of "exact" and choose not to verify
    // that the *data* contained within the fragment matches the previously
    // received data. This is in the spirit of reducing the work performed by
    // the assembler, and is in line with the behavior of other platforms.
    Duplicate,
}

/// A cache of inbound IP packet fragments.
#[derive(Debug)]
pub struct IpPacketFragmentCache<I: ReassemblyIpExt, BT: FragmentBindingsTypes> {
    cache: HashMap<FragmentCacheKey<I>, FragmentCacheData>,
    // The bytes of data (e.g. IP Header + IP Payload) stored in `cache`.
    data_size: usize,
    // The number of individual fragments stored in `cache`.
    num_fragments: usize,
    capacity: FragmentCacheCapacity,
    timers: LocalTimerHeap<FragmentCacheKey<I>, (), BT>,
}

impl<I: ReassemblyIpExt, BC: FragmentBindingsContext> IpPacketFragmentCache<I, BC> {
    /// Creates a new `IpFragmentCache`.
    pub fn new<CC: CoreTimerContext<FragmentTimerId<I>, BC>>(
        bindings_ctx: &mut BC,
    ) -> IpPacketFragmentCache<I, BC> {
        IpPacketFragmentCache {
            cache: HashMap::new(),
            data_size: 0,
            num_fragments: 0,
            capacity: FragmentCacheCapacity::default(),
            timers: LocalTimerHeap::new(bindings_ctx, CC::convert_timer(Default::default())),
        }
    }
}

enum CacheTimerAction<I: ReassemblyIpExt> {
    CreateNewTimer(FragmentCacheKey<I>),
    CancelExistingTimer(FragmentCacheKey<I>),
}

impl<I: ReassemblyIpExt, BT: FragmentBindingsTypes> IpPacketFragmentCache<I, BT> {
    /// Attempts to process a packet fragment.
    ///
    /// # Panics
    ///
    /// Panics if the packet has no fragment data.
    fn process_fragment<B: SplitByteSlice>(
        &mut self,
        packet: I::Packet<B>,
    ) -> (FragmentProcessingState<I, B>, Option<CacheTimerAction<I>>)
    where
        I::Packet<B>: FragmentablePacket,
    {
        if self.above_capacity() {
            return (FragmentProcessingState::OutOfMemory, None);
        }

        // Get the fragment data.
        let (id, offset, m_flag) = packet.fragment_data();

        // Check if `packet` is actually fragmented. We know it is not
        // fragmented if the fragment offset is 0 (contains first fragment) and
        // we have no more fragments. This means the first fragment is the only
        // fragment, implying we have a full packet.
        if offset == 0 && !m_flag {
            return (FragmentProcessingState::NotNeeded(packet), None);
        }

        // Make sure packet's body isn't empty. Since at this point we know that
        // the packet is definitely fragmented (`offset` is not 0 or `m_flag` is
        // `true`), we simply let the caller know we need more fragments. This
        // should never happen, but just in case :).
        if packet.body().is_empty() {
            return (FragmentProcessingState::NeedMoreFragments, None);
        }

        // Make sure body is a multiple of `FRAGMENT_BLOCK_SIZE` bytes, or
        // `packet` contains the last fragment block which is allowed to be less
        // than `FRAGMENT_BLOCK_SIZE` bytes.
        if m_flag && (packet.body().len() % (FRAGMENT_BLOCK_SIZE as usize) != 0) {
            return (FragmentProcessingState::InvalidFragment, None);
        }

        // Key used to find this connection's fragment cache data.
        let key = FragmentCacheKey {
            src_ip: packet.src_ip(),
            dst_ip: packet.dst_ip(),
            fragment_id: id,
            ip_specific_fields: I::ip_specific_key_part(&packet),
        };

        // The number of fragment blocks `packet` contains.
        //
        // Note, we are calculating the ceiling of an integer division.
        // Essentially:
        //     ceil(packet.body.len() / FRAGMENT_BLOCK_SIZE)
        //
        // We need to calculate the ceiling of the division because the final
        // fragment block for a reassembled packet is allowed to contain less
        // than `FRAGMENT_BLOCK_SIZE` bytes.
        //
        // We know `packet.body().len() - 1` will never be less than 0 because
        // we already made sure that `packet`'s body is not empty, and it is
        // impossible to have a negative body size.
        let num_fragment_blocks = 1 + ((packet.body().len() - 1) / (FRAGMENT_BLOCK_SIZE as usize));
        assert!(num_fragment_blocks > 0);

        // The range of fragment blocks `packet` contains.
        //
        // The maximum number of fragment blocks a reassembled packet is allowed
        // to contain is `MAX_FRAGMENT_BLOCKS` so we make sure that the fragment
        // we received does not violate this.
        let fragment_blocks_range =
            if let Ok(offset_end) = u16::try_from((offset as usize) + num_fragment_blocks - 1) {
                if offset_end <= MAX_FRAGMENT_BLOCKS {
                    BlockRange { start: offset, end: offset_end }
                } else {
                    return (FragmentProcessingState::InvalidFragment, None);
                }
            } else {
                return (FragmentProcessingState::InvalidFragment, None);
            };

        // Get (or create) the fragment cache data.
        let (fragment_data, timer_not_yet_scheduled) = self.get_or_create(key);

        // Find the gap where `packet` belongs.
        let found_gap = match fragment_data.find_gap(fragment_blocks_range) {
            FindGapResult::Overlap | FindGapResult::OutOfBounds => {
                // Drop all reassembly data as per RFC 8200 section 4.5 (IPv6).
                // See RFC 5722 for more information.
                //
                // IPv4 (RFC 791) does not specify what to do for overlapped
                // fragments. RFC 1858 section 4.2 outlines a way to prevent an
                // overlapping fragment attack for IPv4, but this is primarily
                // for IP filtering since "no standard requires that an
                // overlap-safe reassemble algorithm be used" on hosts. In
                // practice, non-malicious nodes should not intentionally send
                // data for the same fragment block multiple times, so we will
                // do the same thing as IPv6 in this case.
                assert_matches!(self.remove_data(&key), Some(_));

                return (
                    FragmentProcessingState::InvalidFragment,
                    (!timer_not_yet_scheduled)
                        .then_some(CacheTimerAction::CancelExistingTimer(key)),
                );
            }
            FindGapResult::Duplicate => {
                // Ignore duplicate fragments as per RFC 8200 section 4.5
                // (IPv6):
                //   It should be noted that fragments may be duplicated in the
                //   network.  Instead of treating these exact duplicate fragments
                //   as overlapping fragments, an implementation may choose to
                //   detect this case and drop exact duplicate fragments while
                //   keeping the other fragments belonging to the same packet.
                //
                // Ipv4 (RFC 791) does not specify what to do for duplicate
                // fragments. As such we choose to do the same as IPv6 in this
                // case.
                return (FragmentProcessingState::NeedMoreFragments, None);
            }
            FindGapResult::Found { gap } => gap,
        };

        let timer_id = timer_not_yet_scheduled.then_some(CacheTimerAction::CreateNewTimer(key));

        if !m_flag && found_gap.end < u16::MAX {
            // There is another fragment after this one that is already present
            // in the cache. That means that this fragment can't be the last
            // one (must have `m_flag` set).
            return (FragmentProcessingState::InvalidFragment, timer_id);
        }

        // Remove `found_gap` since the gap as it exists will no longer be
        // valid.
        assert!(fragment_data.missing_blocks.remove(&found_gap));

        // If the received fragment blocks start after the beginning of
        // `found_gap`, create a new gap between the beginning of `found_gap`
        // and the first fragment block contained in `packet`.
        //
        // Example:
        //   `packet` w/ fragments [4, 7]
        //                 |-----|-----|-----|-----|
        //                    4     5     6     7
        //
        //   `found_gap` w/ fragments [X, 7] where 0 <= X < 4
        //     |-----| ... |-----|-----|-----|-----|
        //        X    ...    4     5     6     7
        //
        //   Here we can see that with a `found_gap` of [2, 7], `packet` covers
        //   [4, 7] but we are still missing [X, 3] so we create a new gap of
        //   [X, 3].
        if found_gap.start < fragment_blocks_range.start {
            assert!(fragment_data.missing_blocks.insert(BlockRange {
                start: found_gap.start,
                end: fragment_blocks_range.start - 1
            }));
        }

        // If the received fragment blocks end before the end of `found_gap` and
        // we expect more fragments, create a new gap between the last fragment
        // block contained in `packet` and the end of `found_gap`.
        //
        // Example 1:
        //   `packet` w/ fragments [4, 7] & m_flag = true
        //     |-----|-----|-----|-----|
        //        4     5     6     7
        //
        //   `found_gap` w/ fragments [4, Y] where 7 < Y <= `MAX_FRAGMENT_BLOCKS`.
        //     |-----|-----|-----|-----| ... |-----|
        //        4     5     6     7    ...    Y
        //
        //   Here we can see that with a `found_gap` of [4, Y], `packet` covers
        //   [4, 7] but we still expect more fragment blocks after the blocks in
        //   `packet` (as noted by `m_flag`) so we are still missing [8, Y] so
        //   we create a new gap of [8, Y].
        //
        // Example 2:
        //   `packet` w/ fragments [4, 7] & m_flag = false
        //     |-----|-----|-----|-----|
        //        4     5     6     7
        //
        //   `found_gap` w/ fragments [4, Y] where MAX = `MAX_FRAGMENT_BLOCKS`.
        //     |-----|-----|-----|-----| ... |-----|
        //        4     5     6     7    ...   MAX
        //
        //   Here we can see that with a `found_gap` of [4, MAX], `packet`
        //   covers [4, 7] and we don't expect more fragment blocks after the
        //   blocks in `packet` (as noted by `m_flag`) so we don't create a new
        //   gap. Note, if we encounter a `packet` where `m_flag` is false,
        //   `found_gap`'s end value must be MAX because we should only ever not
        //   create a new gap where the end is MAX when we are processing a
        //   packet with the last fragment block.
        if found_gap.end > fragment_blocks_range.end && m_flag {
            assert!(
                fragment_data.missing_blocks.insert(BlockRange {
                    start: fragment_blocks_range.end + 1,
                    end: found_gap.end
                })
            );
        } else {
            // Make sure that if we are not adding a fragment after the packet,
            // it is because `packet` goes up to the `found_gap`'s end boundary,
            // or this is the last fragment. If it is the last fragment for a
            // packet, we make sure that `found_gap`'s end value is
            // `core::u16::MAX`.
            assert!(
                found_gap.end == fragment_blocks_range.end
                    || (!m_flag && found_gap.end == u16::MAX),
                "found_gap: {:?}, fragment_blocks_range: {:?} offset: {:?}, m_flag: {:?}",
                found_gap,
                fragment_blocks_range,
                offset,
                m_flag
            );
        }

        let mut added_bytes = 0;
        let fragment_len = packet.parse_metadata().header_len() + packet.body().len();
        fragment_data.max_fragment_len = fragment_data.max_fragment_len.max(fragment_len);
        // Get header buffer from `packet` if its fragment offset equals to 0.
        if offset == 0 {
            assert_eq!(fragment_data.header, None);
            let header = get_header::<B, I>(&packet);
            added_bytes = header.len();
            fragment_data.header = Some(header);
        }

        // Add our `packet`'s body to the store of body fragments.
        let mut body = Vec::with_capacity(packet.body().len());
        body.extend_from_slice(packet.body());
        added_bytes += body.len();
        fragment_data.total_size += added_bytes;
        fragment_data.body_fragments.push(PacketBodyFragment::new(offset, body));

        // If we still have missing fragments, let the caller know that we are
        // still waiting on some fragments. Otherwise, we let them know we are
        // ready to reassemble and give them a key and the final packet length
        // so they can allocate a sufficient buffer and call
        // `reassemble_packet`.
        let result = if fragment_data.missing_blocks.is_empty() {
            FragmentProcessingState::Ready { key, packet_len: fragment_data.total_size }
        } else {
            FragmentProcessingState::NeedMoreFragments
        };

        self.track_fragment(added_bytes);
        (result, timer_id)
    }

    /// Attempts to reassemble a packet.
    ///
    /// Attempts to reassemble a packet associated with a given
    /// `FragmentCacheKey`, `key`, and cancels the timer to reset reassembly
    /// data. The caller is expected to allocate a buffer of sufficient size
    /// (available from `process_fragment` when it returns a
    /// `FragmentProcessingState::Ready` value) and provide it to
    /// `reassemble_packet` as `buffer` where the packet will be reassembled
    /// into.
    ///
    /// Returns the size of the largest fragment received for this packet.
    ///
    /// # Panics
    ///
    /// Panics if the provided `buffer` does not have enough capacity for the
    /// reassembled packet. Also panics if a different `ctx` is passed to
    /// `reassemble_packet` from the one passed to `process_fragment` when
    /// processing a packet with a given `key` as `reassemble_packet` will fail
    /// to cancel the reassembly timer.
    fn reassemble_packet<B: SplitByteSliceMut, BV: BufferViewMut<B>>(
        &mut self,
        key: &FragmentCacheKey<I>,
        buffer: BV,
    ) -> Result<usize, FragmentReassemblyError> {
        let entry = match self.cache.entry(*key) {
            Entry::Occupied(entry) => entry,
            Entry::Vacant(_) => return Err(FragmentReassemblyError::InvalidKey),
        };

        // Make sure we are not missing fragments.
        if !entry.get().missing_blocks.is_empty() {
            return Err(FragmentReassemblyError::MissingFragments);
        }
        // Remove the entry from the cache now that we've validated that we will
        // be able to reassemble it.
        let (_key, data) = entry.remove_entry();
        self.untrack_data(&data);

        // If we are not missing fragments, we must have header data.
        let header = data.header.expect("should have header if we're not missing fragments");

        // TODO(https://github.com/rust-lang/rust/issues/59278): Use
        // `BinaryHeap::into_iter_sorted`.
        let fragments = data.body_fragments.into_sorted_vec();
        let body_fragments = fragments.iter().map(|x| x.data.as_slice());
        I::Packet::reassemble_fragmented_packet(buffer, header.as_slice(), body_fragments)
            .map_err(|_| FragmentReassemblyError::PacketParsingError)?;
        Ok(data.max_fragment_len)
    }

    /// Gets or creates a new entry in the cache for a given `key`.
    ///
    /// Returns a tuple whose second component indicates whether a reassembly
    /// timer needs to be scheduled.
    fn get_or_create(&mut self, key: FragmentCacheKey<I>) -> (&mut FragmentCacheData, bool) {
        match self.cache.entry(key) {
            Entry::Occupied(e) => (e.into_mut(), false),
            Entry::Vacant(e) => {
                // We have no reassembly data yet so this fragment is the first
                // one associated with the given `key`. Create a new entry in
                // the hash table and let the caller know to schedule a timer to
                // reset the entry.
                (e.insert(FragmentCacheData::default()), true)
            }
        }
    }

    fn above_capacity(&self) -> bool {
        self.data_size >= self.capacity.max_data_bytes
            || self.num_fragments >= self.capacity.max_fragments
            || self.cache.len() >= self.capacity.max_keys
    }

    fn track_fragment(&mut self, added_bytes: usize) {
        self.data_size += added_bytes;
        self.num_fragments += 1;
    }

    fn untrack_data(&mut self, data: &FragmentCacheData) {
        self.data_size -= data.total_size;
        self.num_fragments -= data.body_fragments.len();
    }

    fn remove_data(&mut self, key: &FragmentCacheKey<I>) -> Option<FragmentCacheData> {
        let data = self.cache.remove(key)?;
        self.untrack_data(&data);
        Some(data)
    }
}

/// Gets the header bytes for a packet.
fn get_header<B: SplitByteSlice, I: IpExt>(packet: &I::Packet<B>) -> Vec<u8> {
    match packet.as_ip_addr_ref() {
        IpAddr::V4(packet) => packet.copy_header_bytes_for_fragment(),
        IpAddr::V6(packet) => {
            // We are guaranteed not to panic here because we will only panic if
            // `packet` does not have a fragment extension header. We can only get
            // here if `packet` is a fragment packet, so we know that `packet` has a
            // fragment extension header.
            packet.copy_header_bytes_for_fragment()
        }
    }
}

/// A fragment of a packet's body.
#[derive(Debug, PartialEq, Eq)]
struct PacketBodyFragment {
    offset: u16,
    data: Vec<u8>,
}

impl PacketBodyFragment {
    /// Constructs a new `PacketBodyFragment` to be stored in a `BinaryHeap`.
    fn new(offset: u16, data: Vec<u8>) -> Self {
        PacketBodyFragment { offset, data }
    }
}

// The ordering of a `PacketBodyFragment` is only dependant on the fragment
// offset.
impl PartialOrd for PacketBodyFragment {
    fn partial_cmp(&self, other: &PacketBodyFragment) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PacketBodyFragment {
    fn cmp(&self, other: &Self) -> Ordering {
        self.offset.cmp(&other.offset)
    }
}

/// Capacity limits for the IP packet fragment cache.
///
/// If none of these limits are currently exceeded, a new fragment can be cached
/// (even if this results in the cache newly exceeding the limits). If any
/// of the limits are currently exceeded, the incoming fragment is be dropped.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct FragmentCacheCapacity {
    /// Maximum number of bytes of all currently cached fragments.
    max_data_bytes: usize,
    /// Maximum number of keys in the cache.
    max_keys: usize,
    /// Maximum number of individual fragments in the cache.
    max_fragments: usize,
}

impl FragmentCacheCapacity {
    /// The default value for [`FragmentCacheCapacity.max_data_bytes`].
    ///
    /// 4 MiB.
    const DEFAULT_MAX_DATA_BYTES: usize = 4 * 1024 * 1024;

    /// The default value for [`FragmentCacheCapacity.max_keys`].
    ///
    /// With some fuzzy (and conservative) math, each key is expected to take
    /// < 512 bytes (after accounting for the [`FragmentCacheKey`],
    /// [`FragmentCacheData`], and the heap allocations created by the data).
    /// Thus 2048 keys should set a loose upper bound of 1MiB.
    const DEFAULT_MAX_KEYS: usize = 2048;

    /// The default value for [`FragmentCacheCapacity.max_fragments`].
    ///
    /// Allow on average 8 fragments per packet, with
    /// [`Self::DEFAULT_MAX_KEYS`]. A factor of 8 is quite conservative, and
    /// would only occur in the most extreme circumstances.
    const DEFAULT_MAX_FRAGMENTS: usize = Self::DEFAULT_MAX_KEYS * 8;
}

impl Default for FragmentCacheCapacity {
    fn default() -> Self {
        Self {
            max_data_bytes: Self::DEFAULT_MAX_DATA_BYTES,
            max_keys: Self::DEFAULT_MAX_KEYS,
            max_fragments: Self::DEFAULT_MAX_FRAGMENTS,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use net_declare::{net_ip_v4, net_ip_v6};
    use net_types::Witness;
    use net_types::ip::{IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeInstant, FakeTimerCtxExt, TEST_ADDRS_V4, TEST_ADDRS_V6,
        assert_empty,
    };
    use netstack3_base::{CtxPair, IntoCoreTimerCtx, NetworkSerializationContext};
    use packet::{Buf, NestablePacketBuilder as _, ParsablePacket, ParseBuffer, Serializer};
    use packet_formats::ip::{FragmentOffset, IpProto, Ipv6Proto};
    use packet_formats::ipv4::Ipv4PacketBuilder;
    use packet_formats::ipv6::ext_hdrs::IPV6_FRAGMENT_EXT_HDR_LEN;
    use packet_formats::ipv6::{Ipv6PacketBuilder, Ipv6PacketBuilderWithFragmentHeader};
    use test_case::test_case;

    use super::*;

    struct FakeFragmentContext<I: ReassemblyIpExt, BT: FragmentBindingsTypes> {
        cache: IpPacketFragmentCache<I, BT>,
    }

    impl<I: ReassemblyIpExt, BC: FragmentBindingsContext> FakeFragmentContext<I, BC>
    where
        BC::DispatchId: From<FragmentTimerId<I>>,
    {
        fn new(bindings_ctx: &mut BC) -> Self {
            Self { cache: IpPacketFragmentCache::new::<IntoCoreTimerCtx>(bindings_ctx) }
        }
    }

    type FakeCtxImpl<I> = CtxPair<FakeCoreCtxImpl<I>, FakeBindingsCtxImpl<I>>;
    type FakeBindingsCtxImpl<I> = FakeBindingsCtx<FragmentTimerId<I>, (), (), ()>;
    type FakeCoreCtxImpl<I> = FakeCoreCtx<FakeFragmentContext<I, FakeBindingsCtxImpl<I>>, (), ()>;

    impl<I: ReassemblyIpExt> FragmentContext<I, FakeBindingsCtxImpl<I>> for FakeCoreCtxImpl<I> {
        fn with_state_mut<
            O,
            F: FnOnce(&mut IpPacketFragmentCache<I, FakeBindingsCtxImpl<I>>) -> O,
        >(
            &mut self,
            cb: F,
        ) -> O {
            cb(&mut self.state.cache)
        }
    }

    /// The result `process_ipv4_fragment` or `process_ipv6_fragment` should
    /// expect after processing a fragment.
    #[derive(PartialEq)]
    enum ExpectedResult<I: ReassemblyIpExt> {
        /// After processing a packet fragment, we should be ready to reassemble
        /// the packet.
        ///
        /// `body_fragment_blocks` is in units of `FRAGMENT_BLOCK_SIZE`.
        Ready { body_fragment_blocks: u16, key: FragmentCacheKey<I> },

        /// After processing a packet fragment, we need more packet fragments
        /// before being ready to reassemble the packet.
        NeedMore,

        /// The packet fragment is invalid.
        Invalid,

        /// The Cache is full.
        OutOfMemory,
    }

    /// Get an IPv4 packet builder.
    fn get_ipv4_builder() -> Ipv4PacketBuilder {
        Ipv4PacketBuilder::new(
            TEST_ADDRS_V4.remote_ip,
            TEST_ADDRS_V4.local_ip,
            10,
            <Ipv4 as TestIpExt>::PROTOCOL,
        )
    }

    /// Get an IPv6 packet builder.
    fn get_ipv6_builder() -> Ipv6PacketBuilder {
        Ipv6PacketBuilder::new(
            TEST_ADDRS_V6.remote_ip,
            TEST_ADDRS_V6.local_ip,
            10,
            <Ipv6 as TestIpExt>::PROTOCOL,
        )
    }

    /// Validate that IpPacketFragmentCache has correct size.
    fn validate_size<I: ReassemblyIpExt, BT: FragmentBindingsTypes>(
        cache: &IpPacketFragmentCache<I, BT>,
    ) {
        let mut data_size: usize = 0;
        let mut num_fragments: usize = 0;

        for v in cache.cache.values() {
            data_size += v.total_size;
            num_fragments += v.body_fragments.len();
        }

        assert_eq!(data_size, cache.data_size);
        assert_eq!(num_fragments, cache.num_fragments);
    }

    struct FragmentSpec {
        /// The ID of the fragment.
        id: u16,
        /// The offset of the fragment, in units of `FRAGMENT_BLOCK_SIZE`.
        offset: u16,
        /// The size of the fragment, in units of `FRAGMENT_BLOCK_SIZE`.
        size: u16,
        /// The value of the M flag. "True" indicates more fragments.
        m_flag: bool,
    }

    fn expected_packet_size<I: TestIpExt>(num_fragment_blocks: u16) -> usize {
        usize::from(num_fragment_blocks) * usize::from(FRAGMENT_BLOCK_SIZE) + I::HEADER_LENGTH
    }

    /// Generates and processes an IPv4 fragment packet.
    fn process_ipv4_fragment<CC: FragmentContext<Ipv4, BC>, BC: FragmentBindingsContext>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        FragmentSpec { id, offset, size, m_flag }: FragmentSpec,
        mut builder: Ipv4PacketBuilder,
        expected_result: ExpectedResult<Ipv4>,
    ) {
        builder.id(id);
        builder.fragment_offset(FragmentOffset::new(offset).unwrap());
        builder.mf_flag(m_flag);
        let body = generate_body_fragment(
            id,
            offset,
            usize::from(size) * usize::from(FRAGMENT_BLOCK_SIZE),
        );

        let mut buffer = builder
            .wrap_body(Buf::new(body, ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv4Packet<_>>().unwrap();

        let actual_result =
            FragmentHandler::process_fragment::<&[u8]>(core_ctx, bindings_ctx, packet);
        match expected_result {
            ExpectedResult::Ready { body_fragment_blocks, key: expected_key } => {
                let (key, packet_len) = assert_matches!(
                    actual_result,
                    FragmentProcessingState::Ready {key, packet_len} => (key, packet_len)
                );
                assert_eq!(key, expected_key);
                assert_eq!(packet_len, expected_packet_size::<Ipv4>(body_fragment_blocks));
            }
            ExpectedResult::NeedMore => {
                assert_matches!(actual_result, FragmentProcessingState::NeedMoreFragments);
            }
            ExpectedResult::Invalid => {
                assert_matches!(actual_result, FragmentProcessingState::InvalidFragment);
            }
            ExpectedResult::OutOfMemory => {
                assert_matches!(actual_result, FragmentProcessingState::OutOfMemory);
            }
        }
    }

    /// Generates and processes an IPv6 fragment packet.
    ///
    /// `fragment_offset` and `size` are both in units of `FRAGMENT_BLOCK_SIZE`.
    fn process_ipv6_fragment<CC: FragmentContext<Ipv6, BC>, BC: FragmentBindingsContext>(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        FragmentSpec { id, offset, size, m_flag }: FragmentSpec,
        builder: Ipv6PacketBuilder,
        expected_result: ExpectedResult<Ipv6>,
    ) {
        let builder = Ipv6PacketBuilderWithFragmentHeader::new(
            builder,
            FragmentOffset::new(offset).unwrap(),
            m_flag,
            id.into(),
        );

        let body = generate_body_fragment(
            id,
            offset,
            usize::from(size) * usize::from(FRAGMENT_BLOCK_SIZE),
        );

        let mut buffer = builder
            .wrap_body(Buf::new(body, ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv6Packet<_>>().unwrap();

        let actual_result =
            FragmentHandler::process_fragment::<&[u8]>(core_ctx, bindings_ctx, packet);
        match expected_result {
            ExpectedResult::Ready { body_fragment_blocks, key: expected_key } => {
                let (key, packet_len) = assert_matches!(
                    actual_result,
                    FragmentProcessingState::Ready {key, packet_len} => (key, packet_len)
                );
                assert_eq!(key, expected_key);
                assert_eq!(packet_len, expected_packet_size::<Ipv6>(body_fragment_blocks));
            }
            ExpectedResult::NeedMore => {
                assert_matches!(actual_result, FragmentProcessingState::NeedMoreFragments);
            }
            ExpectedResult::Invalid => {
                assert_matches!(actual_result, FragmentProcessingState::InvalidFragment);
            }
            ExpectedResult::OutOfMemory => {
                assert_matches!(actual_result, FragmentProcessingState::OutOfMemory);
            }
        }
    }

    trait TestIpExt: IpExt + netstack3_base::testutil::TestIpExt + ReassemblyIpExt {
        const HEADER_LENGTH: usize;

        const PROTOCOL: Self::Proto;

        fn process_ip_fragment<CC: FragmentContext<Self, BC>, BC: FragmentBindingsContext>(
            core_ctx: &mut CC,
            bindings_ctx: &mut BC,
            spec: FragmentSpec,
            expected_result: ExpectedResult<Self>,
        );
    }

    impl TestIpExt for Ipv4 {
        const HEADER_LENGTH: usize = packet_formats::ipv4::HDR_PREFIX_LEN;

        const PROTOCOL: Ipv4Proto = Ipv4Proto::Proto(IpProto::Tcp);

        fn process_ip_fragment<CC: FragmentContext<Self, BC>, BC: FragmentBindingsContext>(
            core_ctx: &mut CC,
            bindings_ctx: &mut BC,
            spec: FragmentSpec,
            expected_result: ExpectedResult<Ipv4>,
        ) {
            process_ipv4_fragment(core_ctx, bindings_ctx, spec, get_ipv4_builder(), expected_result)
        }
    }
    impl TestIpExt for Ipv6 {
        const HEADER_LENGTH: usize = packet_formats::ipv6::IPV6_FIXED_HDR_LEN;

        const PROTOCOL: Ipv6Proto = Ipv6Proto::Proto(IpProto::Tcp);

        fn process_ip_fragment<CC: FragmentContext<Self, BC>, BC: FragmentBindingsContext>(
            core_ctx: &mut CC,
            bindings_ctx: &mut BC,
            spec: FragmentSpec,
            expected_result: ExpectedResult<Ipv6>,
        ) {
            process_ipv6_fragment(core_ctx, bindings_ctx, spec, get_ipv6_builder(), expected_result)
        }
    }

    fn expected_max_fragment_len<I: TestIpExt>(num_blocks: u16) -> usize {
        let mut len = I::HEADER_LENGTH + usize::from(num_blocks) * usize::from(FRAGMENT_BLOCK_SIZE);
        if I::VERSION == IpVersion::V6 {
            len += IPV6_FRAGMENT_EXT_HDR_LEN;
        }
        len
    }

    /// Tries to reassemble the packet with the given fragment ID.
    ///
    /// `body_fragment_blocks` is in units of `FRAGMENT_BLOCK_SIZE`.
    fn try_reassemble_ip_packet<
        I: TestIpExt + netstack3_base::IpExt,
        CC: FragmentContext<I, BC>,
        BC: FragmentBindingsContext,
    >(
        core_ctx: &mut CC,
        bindings_ctx: &mut BC,
        fragment_id: u16,
        body_fragment_blocks: u16,
        expected_max_fragment_len: usize,
    ) {
        let mut buffer: Vec<u8> = vec![
            0;
            usize::from(body_fragment_blocks)
                * usize::from(FRAGMENT_BLOCK_SIZE)
                + I::HEADER_LENGTH
        ];
        let mut buffer = &mut buffer[..];
        let key = test_key(fragment_id);
        let max_fragment_len =
            FragmentHandler::reassemble_packet(core_ctx, bindings_ctx, &key, &mut buffer).unwrap();
        assert_eq!(max_fragment_len, expected_max_fragment_len);
        let packet = I::Packet::parse_mut(&mut buffer, ()).unwrap();

        let expected_body = generate_body_fragment(
            fragment_id,
            0,
            usize::from(body_fragment_blocks) * usize::from(FRAGMENT_BLOCK_SIZE),
        );
        assert_eq!(packet.body(), &expected_body[..]);
    }

    /// Generates the body of a packet with the given fragment ID, offset, and
    /// length.
    ///
    /// Overlapping body bytes from different calls to `generate_body_fragment`
    /// are guaranteed to have the same values.
    fn generate_body_fragment(fragment_id: u16, fragment_offset: u16, len: usize) -> Vec<u8> {
        // The body contains increasing byte values which start at `fragment_id`
        // at byte 0. This ensures that different packets with different
        // fragment IDs contain bodies with different byte values.
        let start = usize::from(fragment_id)
            + usize::from(fragment_offset) * usize::from(FRAGMENT_BLOCK_SIZE);
        (start..start + len).map(|byte| byte as u8).collect()
    }

    /// Gets a `FragmentCacheKey` with hard coded test values.
    fn test_key<I: TestIpExt>(id: u16) -> FragmentCacheKey<I> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        struct Wrapper<I: ReassemblyIpExt>(I::FragmentCacheKeyPart);

        let Wrapper(ip_specific_fields) =
            I::map_ip_out((), |()| Wrapper(Ipv4::PROTOCOL), |()| Wrapper(()));

        FragmentCacheKey {
            src_ip: I::TEST_ADDRS.remote_ip.get(),
            dst_ip: I::TEST_ADDRS.local_ip.get(),
            fragment_id: id.into(),
            ip_specific_fields,
        }
    }

    fn new_context<I: ReassemblyIpExt>() -> FakeCtxImpl<I> {
        FakeCtxImpl::<I>::with_default_bindings_ctx(|bindings_ctx| {
            FakeCoreCtxImpl::with_state(FakeFragmentContext::new(bindings_ctx))
        })
    }

    #[test]
    fn test_ipv4_reassembly_not_needed() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv4>();

        // Test that we don't attempt reassembly if the packet is not
        // fragmented.

        let builder = get_ipv4_builder();
        let body = [1, 2, 3, 4, 5];
        let mut buffer = builder
            .wrap_body(Buf::new(body.to_vec(), ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv4Packet<_>>().unwrap();
        assert_matches!(
            FragmentHandler::process_fragment::<&[u8]>(&mut core_ctx, &mut bindings_ctx, packet),
            FragmentProcessingState::NotNeeded(unfragmented) if unfragmented.body() == body
        );
    }

    #[test]
    #[should_panic(
        expected = "internal error: entered unreachable code: Should never call this function if the packet does not have a fragment header"
    )]
    fn test_ipv6_reassembly_not_needed() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv6>();

        // Test that we panic if we call `fragment_data` on a packet that has no
        // fragment data.

        let builder = get_ipv6_builder();
        let mut buffer = builder
            .wrap_body(Buf::new(vec![1, 2, 3, 4, 5], ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv6Packet<_>>().unwrap();
        assert_matches!(
            FragmentHandler::process_fragment::<&[u8]>(&mut core_ctx, &mut bindings_ctx, packet),
            FragmentProcessingState::InvalidFragment
        );
    }

    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_ip_reassembly<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Test that we properly reassemble fragmented packets.

        // Process fragment #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: size, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 2 * size, size, m_flag: false },
            ExpectedResult::Ready { body_fragment_blocks: 3 * size, key: test_key(id) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id,
            3 * size,
            expected_max_fragment_len::<I>(size),
        );
    }

    #[test]
    fn test_ipv4_key_uniqueness() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv4>();

        const RIGHT_SRC: Ipv4Addr = net_ip_v4!("192.0.2.1");
        const WRONG_SRC: Ipv4Addr = net_ip_v4!("192.0.2.2");

        const RIGHT_DST: Ipv4Addr = net_ip_v4!("192.0.2.3");
        const WRONG_DST: Ipv4Addr = net_ip_v4!("192.0.2.4");

        const RIGHT_PROTO: Ipv4Proto = Ipv4Proto::Proto(IpProto::Tcp);
        const WRONG_PROTO: Ipv4Proto = Ipv4Proto::Proto(IpProto::Udp);

        const RIGHT_ID: u16 = 1;
        const WRONG_ID: u16 = 2;

        const TTL: u8 = 1;

        // Process fragment #0.
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: RIGHT_ID, offset: 0, size: 1, m_flag: true },
            Ipv4PacketBuilder::new(RIGHT_SRC, RIGHT_DST, TTL, RIGHT_PROTO),
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 under a different key, and verify it doesn't
        // complete the packet.
        for (id, src, dst, proto) in [
            (RIGHT_ID, RIGHT_SRC, RIGHT_DST, WRONG_PROTO),
            (RIGHT_ID, RIGHT_SRC, WRONG_DST, RIGHT_PROTO),
            (RIGHT_ID, WRONG_SRC, RIGHT_DST, RIGHT_PROTO),
            (WRONG_ID, RIGHT_SRC, RIGHT_DST, RIGHT_PROTO),
        ] {
            process_ipv4_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id, offset: 1, size: 1, m_flag: false },
                Ipv4PacketBuilder::new(src, dst, TTL, proto),
                ExpectedResult::NeedMore,
            );
        }

        // Finally, process fragment #1 under the correct key, and verify the
        // packet is completed.
        const KEY: FragmentCacheKey<Ipv4> = FragmentCacheKey {
            src_ip: RIGHT_SRC,
            dst_ip: RIGHT_DST,
            fragment_id: RIGHT_ID as u32,
            ip_specific_fields: RIGHT_PROTO,
        };
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: RIGHT_ID, offset: 1, size: 1, m_flag: false },
            Ipv4PacketBuilder::new(RIGHT_SRC, RIGHT_DST, TTL, RIGHT_PROTO),
            ExpectedResult::Ready { body_fragment_blocks: 2, key: KEY },
        );
        let mut buffer: Vec<u8> = vec![0; expected_packet_size::<Ipv4>(2)];
        let mut buffer = &mut buffer[..];
        let max_fragment_len =
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &KEY, &mut buffer)
                .expect("reassembly should succeed");
        assert_eq!(max_fragment_len, expected_max_fragment_len::<Ipv4>(1));
        let _packet = Ipv4Packet::parse_mut(&mut buffer, ()).expect("parse should succeed");
    }

    #[test]
    fn test_ipv6_key_uniqueness() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv6>();

        const RIGHT_SRC: Ipv6Addr = net_ip_v6!("2001:0db8::1");
        const WRONG_SRC: Ipv6Addr = net_ip_v6!("2001:0db8::2");

        const RIGHT_DST: Ipv6Addr = net_ip_v6!("2001:0db8::3");
        const WRONG_DST: Ipv6Addr = net_ip_v6!("2001:0db8::4");

        const RIGHT_ID: u16 = 1;
        const WRONG_ID: u16 = 2;

        const TTL: u8 = 1;

        // Process fragment #0.
        process_ipv6_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: RIGHT_ID, offset: 0, size: 1, m_flag: true },
            Ipv6PacketBuilder::new(RIGHT_SRC, RIGHT_DST, TTL, Ipv6::PROTOCOL),
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 under a different key, and verify it doesn't
        // complete the packet.
        for (id, src, dst) in [
            (RIGHT_ID, RIGHT_SRC, WRONG_DST),
            (RIGHT_ID, WRONG_SRC, RIGHT_DST),
            (WRONG_ID, RIGHT_SRC, RIGHT_DST),
        ] {
            process_ipv6_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id, offset: 1, size: 1, m_flag: false },
                Ipv6PacketBuilder::new(src, dst, TTL, Ipv6::PROTOCOL),
                ExpectedResult::NeedMore,
            );
        }

        // Finally, process fragment #1 under the correct key, and verify the
        // packet is completed.
        const KEY: FragmentCacheKey<Ipv6> = FragmentCacheKey {
            src_ip: RIGHT_SRC,
            dst_ip: RIGHT_DST,
            fragment_id: RIGHT_ID as u32,
            ip_specific_fields: (),
        };
        process_ipv6_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: RIGHT_ID, offset: 1, size: 1, m_flag: false },
            Ipv6PacketBuilder::new(RIGHT_SRC, RIGHT_DST, TTL, Ipv6::PROTOCOL),
            ExpectedResult::Ready { body_fragment_blocks: 2, key: KEY },
        );
        let mut buffer: Vec<u8> = vec![0; expected_packet_size::<Ipv6>(2)];
        let mut buffer = &mut buffer[..];
        let max_fragment_len =
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &KEY, &mut buffer)
                .expect("reassembly should succeed");
        assert_eq!(max_fragment_len, expected_max_fragment_len::<Ipv6>(1));
        let _packet = Ipv6Packet::parse_mut(&mut buffer, ()).expect("parse should succeed");
    }

    #[test]
    fn test_ipv6_reassemble_different_protocols() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv6>();

        const SRC: Ipv6Addr = net_ip_v6!("2001:0db8::1");
        const DST: Ipv6Addr = net_ip_v6!("2001:0db8::2");
        const ID: u16 = 1;
        const TTL: u8 = 1;

        const PROTO1: Ipv6Proto = Ipv6Proto::Proto(IpProto::Tcp);
        const PROTO2: Ipv6Proto = Ipv6Proto::Proto(IpProto::Udp);

        // Process fragment #0 (uses `PROTO1`).
        process_ipv6_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: ID, offset: 0, size: 1, m_flag: true },
            Ipv6PacketBuilder::new(SRC, DST, TTL, PROTO1),
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 (uses `PROTO2`).
        // The packet should successfully reassemble, using the protocol from
        // fragment #0 (i.e. `PROTO1`).
        const KEY: FragmentCacheKey<Ipv6> = FragmentCacheKey {
            src_ip: SRC,
            dst_ip: DST,
            fragment_id: ID as u32,
            ip_specific_fields: (),
        };
        process_ipv6_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: ID, offset: 1, size: 1, m_flag: false },
            Ipv6PacketBuilder::new(SRC, DST, TTL, PROTO2),
            ExpectedResult::Ready { body_fragment_blocks: 2, key: KEY },
        );
        let mut buffer: Vec<u8> = vec![0; expected_packet_size::<Ipv6>(2)];
        let mut buffer = &mut buffer[..];
        let max_fragment_len =
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &KEY, &mut buffer)
                .expect("reassembly should succeed");
        assert_eq!(max_fragment_len, expected_max_fragment_len::<Ipv6>(1));
        let packet = Ipv6Packet::parse_mut(&mut buffer, ()).expect("parse should succeed");
        assert_eq!(packet.proto(), PROTO1);
    }

    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_ip_reassemble_with_missing_blocks<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Test the error we get when we attempt to reassemble with missing
        // fragments.

        // Process fragment #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: size, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        let mut buffer: Vec<u8> = vec![0; 1];
        let mut buffer = &mut buffer[..];
        let key = test_key(id);
        assert_eq!(
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &key, &mut buffer)
                .unwrap_err(),
            FragmentReassemblyError::MissingFragments,
        );
    }

    #[ip_test(I)]
    fn test_ip_reassemble_after_timer<I: TestIpExt>() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;
        let key = test_key::<I>(id);

        // Make sure no timers in the dispatcher yet.
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);

        // Test that we properly reset fragment cache on timer.

        // Process fragment #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size: 1, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Make sure a timer got added.
        core_ctx.state.cache.timers.assert_timers([(
            key,
            (),
            FakeInstant::from(I::REASSEMBLY_TIMEOUT),
        )]);
        validate_size(&core_ctx.state.cache);

        // Process fragment #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 1, size: 1, m_flag: true },
            ExpectedResult::NeedMore,
        );
        // Make sure no new timers got added or fired.
        core_ctx.state.cache.timers.assert_timers([(
            key,
            (),
            FakeInstant::from(I::REASSEMBLY_TIMEOUT),
        )]);
        validate_size(&core_ctx.state.cache);

        // Process fragment #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 2, size: 1, m_flag: false },
            ExpectedResult::Ready { body_fragment_blocks: 3, key: test_key(id) },
        );
        // Make sure no new timers got added or fired.
        core_ctx.state.cache.timers.assert_timers([(
            key,
            (),
            FakeInstant::from(I::REASSEMBLY_TIMEOUT),
        )]);
        validate_size(&core_ctx.state.cache);

        // Trigger the timer (simulate a timer for the fragmented packet).
        assert_eq!(
            bindings_ctx.trigger_next_timer(&mut core_ctx),
            Some(FragmentTimerId::<I>::default())
        );

        // Make sure no other times exist..
        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);

        // Attempt to reassemble the packet but get an error since the fragment
        // data would have been reset/cleared.
        let key = test_key(id);
        let packet_len = 44;
        let mut buffer: Vec<u8> = vec![0; packet_len];
        let mut buffer = &mut buffer[..];
        assert_eq!(
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &key, &mut buffer)
                .unwrap_err(),
            FragmentReassemblyError::InvalidKey,
        );
    }

    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_ip_fragment_cache_max_data_size<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let mut id = 0;
        const THRESHOLD: usize = 8196usize;

        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
        core_ctx.state.cache.capacity.max_data_bytes = THRESHOLD;

        // Test that when cache.data_size exceeds the threshold,
        // process_fragment returns OOM.
        while core_ctx.state.cache.data_size + usize::from(size) <= THRESHOLD {
            I::process_ip_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id, offset: 0, size, m_flag: true },
                ExpectedResult::NeedMore,
            );
            validate_size(&core_ctx.state.cache);
            id += 1;
        }

        // Now that the cache is at or above the threshold, observe OOM.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::OutOfMemory,
        );
        validate_size(&core_ctx.state.cache);

        // Trigger the timers, which clears the cache.
        let _timers = bindings_ctx
            .trigger_timers_for(I::REASSEMBLY_TIMEOUT + Duration::from_secs(1), &mut core_ctx);
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
        validate_size(&core_ctx.state.cache);

        // Can process fragments again.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::NeedMore,
        );
    }

    #[ip_test(I)]
    fn test_ip_fragment_cache_max_keys<I: TestIpExt>() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        const MAX_KEYS: u16 = 3;
        core_ctx.state.cache.capacity.max_keys = MAX_KEYS.into();

        // Test that when cache.cache.len() exceeds the threshold,
        // process_fragment returns OOM.
        for id in 0..MAX_KEYS {
            I::process_ip_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id, offset: 0, size: 1, m_flag: true },
                ExpectedResult::NeedMore,
            );
        }

        // Now that the cache is at or above the threshold, observe OOM.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: MAX_KEYS, offset: 0, size: 1, m_flag: true },
            ExpectedResult::OutOfMemory,
        );

        // Trigger the timers, which clears the cache.
        let _timers = bindings_ctx
            .trigger_timers_for(I::REASSEMBLY_TIMEOUT + Duration::from_secs(1), &mut core_ctx);
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
        validate_size(&core_ctx.state.cache);

        // Can process fragments again.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: MAX_KEYS, offset: 0, size: 1, m_flag: true },
            ExpectedResult::NeedMore,
        );
    }

    #[ip_test(I)]
    fn test_ip_fragment_cache_max_fragments<I: TestIpExt>() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        const MAX_FRAGMENTS: u16 = 3;
        core_ctx.state.cache.capacity.max_fragments = MAX_FRAGMENTS.into();

        // Test that when cache.num_fragments exceeds the threshold,
        // process_fragment returns OOM.
        for offset in 0..MAX_FRAGMENTS {
            I::process_ip_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id: 0, offset, size: 1, m_flag: true },
                ExpectedResult::NeedMore,
            );
        }

        // Now that the cache is at or above the threshold, observe OOM.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: 0, offset: MAX_FRAGMENTS, size: 1, m_flag: true },
            ExpectedResult::OutOfMemory,
        );

        // Trigger the timers, which clears the cache.
        let _timers = bindings_ctx
            .trigger_timers_for(I::REASSEMBLY_TIMEOUT + Duration::from_secs(1), &mut core_ctx);
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
        validate_size(&core_ctx.state.cache);

        // Can process fragments again.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: 0, offset: MAX_FRAGMENTS, size: 1, m_flag: true },
            ExpectedResult::NeedMore,
        );
    }

    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_unordered_fragments<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Process fragment #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 2 * size, size, m_flag: false },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: size, size, m_flag: true },
            ExpectedResult::Ready { body_fragment_blocks: 3 * size, key: test_key(id) },
        );
    }

    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_ip_duplicate_fragment<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Process fragment #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process the exact same fragment over again. It should be ignored.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Verify that the fragment's cache is intact by sending the remaining
        // fragment.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: size, size, m_flag: false },
            ExpectedResult::Ready { body_fragment_blocks: 2 * size, key: test_key(id) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id,
            2 * size,
            expected_max_fragment_len::<I>(size),
        );
    }

    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_ip_out_of_bounds_fragment<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Process fragment #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: size, size, m_flag: false },
            ExpectedResult::NeedMore,
        );

        // Process a fragment after fragment #1. It should be deemed invalid
        // because fragment #1 was the end.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 2 * size, size, m_flag: false },
            ExpectedResult::Invalid,
        );
    }

    #[ip_test(I)]
    #[test_case(50, 100; "overlaps_front")]
    #[test_case(150, 100; "overlaps_back")]
    #[test_case(50, 200; "overlaps_both")]
    fn test_ip_overlapping_fragment<I: TestIpExt>(offset: u16, size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Process fragment #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 100, size: 100, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process a fragment that overlaps with fragment 0. It should be deemed
        // invalid.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset, size, m_flag: true },
            ExpectedResult::Invalid,
        );
    }

    #[test]
    fn test_ipv4_fragment_not_multiple_of_offset_unit() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv4>();
        let id = 0;

        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
        // Test that fragment bodies must be a multiple of
        // `FRAGMENT_BLOCK_SIZE`, except for the last fragment.

        // Process fragment #0
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size: 1, m_flag: true },
            get_ipv4_builder(),
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 (body size is not a multiple of
        // `FRAGMENT_BLOCK_SIZE` and more flag is `true`).
        let mut builder = get_ipv4_builder();
        builder.id(id);
        builder.fragment_offset(FragmentOffset::new(1).unwrap());
        builder.mf_flag(true);
        // Body with 1 byte less than `FRAGMENT_BLOCK_SIZE` so it is not a
        // multiple of `FRAGMENT_BLOCK_SIZE`.
        let mut body: Vec<u8> = Vec::new();
        body.extend(FRAGMENT_BLOCK_SIZE..FRAGMENT_BLOCK_SIZE * 2 - 1);
        let mut buffer = builder
            .wrap_body(Buf::new(body, ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv4Packet<_>>().unwrap();
        assert_matches!(
            FragmentHandler::process_fragment::<&[u8]>(&mut core_ctx, &mut bindings_ctx, packet),
            FragmentProcessingState::InvalidFragment
        );

        // Process fragment #1 (body size is not a multiple of
        // `FRAGMENT_BLOCK_SIZE` but more flag is `false`). The last fragment is
        // allowed to not be a multiple of `FRAGMENT_BLOCK_SIZE`.
        let mut builder = get_ipv4_builder();
        builder.id(id);
        builder.fragment_offset(FragmentOffset::new(1).unwrap());
        builder.mf_flag(false);
        // Body with 1 byte less than `FRAGMENT_BLOCK_SIZE` so it is not a
        // multiple of `FRAGMENT_BLOCK_SIZE`.
        let mut body: Vec<u8> = Vec::new();
        body.extend(FRAGMENT_BLOCK_SIZE..FRAGMENT_BLOCK_SIZE * 2 - 1);
        let mut buffer = builder
            .wrap_body(Buf::new(body, ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv4Packet<_>>().unwrap();
        let (key, packet_len) = assert_matches!(
            FragmentHandler::process_fragment::<&[u8]>(&mut core_ctx, &mut bindings_ctx, packet),
            FragmentProcessingState::Ready {key, packet_len} => (key, packet_len)
        );
        assert_eq!(key, test_key(id));
        assert_eq!(packet_len, 35);
        validate_size(&core_ctx.state.cache);
        let mut buffer: Vec<u8> = vec![0; packet_len];
        let mut buffer = &mut buffer[..];
        let max_fragment_len =
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &key, &mut buffer)
                .unwrap();
        assert_eq!(max_fragment_len, expected_max_fragment_len::<Ipv4>(1));
        let packet = Ipv4Packet::parse_mut(&mut buffer, ()).unwrap();
        let mut expected_body: Vec<u8> = Vec::new();
        expected_body.extend(0..15);
        assert_eq!(packet.body(), &expected_body[..]);
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
    }

    #[test]
    fn test_ipv6_fragment_not_multiple_of_offset_unit() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv6>();
        let id = 0;

        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
        // Test that fragment bodies must be a multiple of
        // `FRAGMENT_BLOCK_SIZE`, except for the last fragment.

        // Process fragment #0
        process_ipv6_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size: 1, m_flag: true },
            get_ipv6_builder(),
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 (body size is not a multiple of
        // `FRAGMENT_BLOCK_SIZE` and more flag is `true`).
        let offset = 1;
        let body_size: usize = (FRAGMENT_BLOCK_SIZE - 1).into();
        let builder = Ipv6PacketBuilderWithFragmentHeader::new(
            get_ipv6_builder(),
            FragmentOffset::new(offset).unwrap(),
            true,
            id.into(),
        );
        let body = generate_body_fragment(id, offset, body_size);
        let mut buffer = builder
            .wrap_body(Buf::new(body, ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv6Packet<_>>().unwrap();
        assert_matches!(
            FragmentHandler::process_fragment::<&[u8]>(&mut core_ctx, &mut bindings_ctx, packet),
            FragmentProcessingState::InvalidFragment
        );

        // Process fragment #1 (body size is not a multiple of
        // `FRAGMENT_BLOCK_SIZE` but more flag is `false`). The last fragment is
        // allowed to not be a multiple of `FRAGMENT_BLOCK_SIZE`.
        let builder = Ipv6PacketBuilderWithFragmentHeader::new(
            get_ipv6_builder(),
            FragmentOffset::new(offset).unwrap(),
            false,
            id.into(),
        );
        let body = generate_body_fragment(id, offset, body_size);
        let mut buffer = builder
            .wrap_body(Buf::new(body, ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv6Packet<_>>().unwrap();
        let (key, packet_len) = assert_matches!(
            FragmentHandler::process_fragment::<&[u8]>(&mut core_ctx, &mut bindings_ctx, packet),
            FragmentProcessingState::Ready {key, packet_len} => (key, packet_len)
        );
        assert_eq!(key, test_key(id));
        assert_eq!(packet_len, 55);

        validate_size(&core_ctx.state.cache);
        let mut buffer: Vec<u8> = vec![0; packet_len];
        let mut buffer = &mut buffer[..];
        let max_fragment_len =
            FragmentHandler::reassemble_packet(&mut core_ctx, &mut bindings_ctx, &key, &mut buffer)
                .unwrap();
        assert_eq!(max_fragment_len, expected_max_fragment_len::<Ipv6>(1));
        let packet = Ipv6Packet::parse_mut(&mut buffer, ()).unwrap();
        let mut expected_body: Vec<u8> = Vec::new();
        expected_body.extend(0..15);
        assert_eq!(packet.body(), &expected_body[..]);
        assert_eq!(core_ctx.state.cache.data_size, 0);
        assert_eq!(core_ctx.state.cache.num_fragments, 0);
    }

    #[ip_test(I)]
    fn test_ip_reassembly_with_multiple_intertwined_packets<I: TestIpExt>() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        const SIZE: u16 = 1;
        let id_0 = 5;
        let id_1 = 10;

        // Test that we properly reassemble fragmented packets when they arrive
        // intertwined with other packets' fragments.

        // Process fragment #0 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_0, offset: 0, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #0 for packet #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_1, offset: 0, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_0, offset: 1, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_1, offset: 1, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #2 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_0, offset: 2, size: SIZE, m_flag: false },
            ExpectedResult::Ready { body_fragment_blocks: 3, key: test_key(id_0) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id_0,
            3,
            expected_max_fragment_len::<I>(SIZE),
        );

        // Process fragment #2 for packet #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_1, offset: 2, size: SIZE, m_flag: false },
            ExpectedResult::Ready { body_fragment_blocks: 3, key: test_key(id_1) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id_1,
            3,
            expected_max_fragment_len::<I>(SIZE),
        );
    }

    #[ip_test(I)]
    fn test_ip_reassembly_timer_with_multiple_intertwined_packets<I: TestIpExt>() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        const SIZE: u16 = 1;
        let id_0 = 5;
        let id_1 = 10;
        let id_2 = 15;

        // Test that we properly timer with multiple intertwined packets that
        // all arrive out of order. We expect packet 1 and 3 to succeed, and
        // packet 1 to fail due to the reassembly timer.
        //
        // The flow of events:
        //   T=0:
        //     - Packet #0, Fragment #0 arrives (timer scheduled for T=60s).
        //     - Packet #1, Fragment #2 arrives (timer scheduled for T=60s).
        //     - Packet #2, Fragment #2 arrives (timer scheduled for T=60s).
        //   T=BEFORE_TIMEOUT1:
        //     - Packet #0, Fragment #2 arrives.
        //   T=BEFORE_TIMEOUT2:
        //     - Packet #2, Fragment #1 arrives.
        //     - Packet #0, Fragment #1 arrives (timer cancelled since all
        //       fragments arrived).
        //   T=BEFORE_TIMEOUT3:
        //     - Packet #1, Fragment #0 arrives.
        //     - Packet #2, Fragment #0 arrives (timer cancelled since all
        //       fragments arrived).
        //   T=TIMEOUT:
        //     - Timeout for reassembly of Packet #1.
        //     - Packet #1, Fragment #1 arrives (final fragment but timer
        //       already triggered so fragment not complete).

        const BEFORE_TIMEOUT1: Duration = Duration::from_secs(1);
        const BEFORE_TIMEOUT2: Duration = Duration::from_secs(2);
        const BEFORE_TIMEOUT3: Duration = Duration::from_secs(3);
        assert!(BEFORE_TIMEOUT1 < I::REASSEMBLY_TIMEOUT);
        assert!(BEFORE_TIMEOUT2 < I::REASSEMBLY_TIMEOUT);
        assert!(BEFORE_TIMEOUT3 < I::REASSEMBLY_TIMEOUT);

        // Process fragment #0 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_0, offset: 0, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 for packet #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_1, offset: 2, size: SIZE, m_flag: false },
            ExpectedResult::NeedMore,
        );

        // Process fragment #2 for packet #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_2, offset: 2, size: SIZE, m_flag: false },
            ExpectedResult::NeedMore,
        );

        // Advance time.
        assert_empty(
            bindings_ctx
                .trigger_timers_until_instant(FakeInstant::from(BEFORE_TIMEOUT1), &mut core_ctx),
        );

        // Process fragment #2 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_0, offset: 2, size: SIZE, m_flag: false },
            ExpectedResult::NeedMore,
        );

        // Advance time.
        assert_empty(
            bindings_ctx
                .trigger_timers_until_instant(FakeInstant::from(BEFORE_TIMEOUT2), &mut core_ctx),
        );

        // Process fragment #1 for packet #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_2, offset: 1, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 for packet #0
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_0, offset: 1, size: SIZE, m_flag: true },
            ExpectedResult::Ready { body_fragment_blocks: 3, key: test_key(id_0) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id_0,
            3,
            expected_max_fragment_len::<I>(SIZE),
        );

        // Advance time.
        assert_empty(
            bindings_ctx
                .trigger_timers_until_instant(FakeInstant::from(BEFORE_TIMEOUT3), &mut core_ctx),
        );

        // Process fragment #0 for packet #1
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_1, offset: 0, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #0 for packet #2
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_2, offset: 0, size: SIZE, m_flag: true },
            ExpectedResult::Ready { body_fragment_blocks: 3, key: test_key(id_2) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id_2,
            3,
            expected_max_fragment_len::<I>(SIZE),
        );

        // Advance time to the timeout, triggering the timer for the reassembly
        // of packet #1
        bindings_ctx.trigger_timers_until_and_expect_unordered(
            FakeInstant::from(I::REASSEMBLY_TIMEOUT),
            [FragmentTimerId::<I>::default()],
            &mut core_ctx,
        );

        // Make sure no other times exist.
        bindings_ctx.timers.assert_no_timers_installed();

        // Process fragment #2 for packet #1 Should get a need more return value
        // since even though we technically received all the fragments, the last
        // fragment didn't arrive until after the reassembly timer.
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: id_1, offset: 2, size: SIZE, m_flag: true },
            ExpectedResult::NeedMore,
        );
    }

    #[test]
    fn test_no_more_fragments_in_middle_of_block() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv4>();
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: 0, offset: 100, size: 1, m_flag: false },
            get_ipv4_builder(),
            ExpectedResult::NeedMore,
        );

        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id: 0, offset: 50, size: 1, m_flag: false },
            get_ipv4_builder(),
            ExpectedResult::Invalid,
        );
    }

    #[ip_test(I)]
    fn test_cancel_timer_on_overlap<I: TestIpExt>() {
        const FRAGMENT_ID: u16 = 1;

        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();

        let key = test_key(FRAGMENT_ID);

        // Do this a couple times to make sure that new packets matching the
        // invalid packet's fragment cache key create a new entry.
        for _ in 0..=2 {
            I::process_ip_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id: FRAGMENT_ID, offset: 0, size: 10, m_flag: true },
                ExpectedResult::NeedMore,
            );
            core_ctx
                .state
                .cache
                .timers
                .assert_timers_after(&mut bindings_ctx, [(key, (), I::REASSEMBLY_TIMEOUT)]);

            I::process_ip_fragment(
                &mut core_ctx,
                &mut bindings_ctx,
                FragmentSpec { id: FRAGMENT_ID, offset: 5, size: 10, m_flag: true },
                ExpectedResult::Invalid,
            );
            assert_eq!(bindings_ctx.timers.timers(), [],);
        }
    }

    // Regression test for https://fxbug.dev/515396407
    #[test]
    fn test_fragment_reassembly_evasion_cache_corruption() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv4>();
        let id = 5;

        // 1. Send fragment at offset 0, length 10, `m_flag = true`.
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size: 10, m_flag: true },
            get_ipv4_builder(),
            ExpectedResult::NeedMore,
        );

        // 2. Send fragment at offset 30, length 10, `m_flag = true`.
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 30, size: 10, m_flag: true },
            get_ipv4_builder(),
            ExpectedResult::NeedMore,
        );

        // 3. Send fragment at offset 10, length 10, `m_flag = false`.
        // This is invalid because we have fragment at 30.
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 10, size: 10, m_flag: false },
            get_ipv4_builder(),
            ExpectedResult::Invalid,
        );

        // 4. Send fragment at offset 40, length 10, `m_flag = false`.
        // If cache was corrupted, this might trigger Ready because gap [10, 29] was lost.
        // It should return NeedMore because we are still missing [20, 29].
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 40, size: 10, m_flag: false },
            get_ipv4_builder(),
            ExpectedResult::NeedMore,
        );
    }

    // Regression test for https://fxbug.dev/517292898
    #[test]
    fn test_multiple_last_fragments_evasion() {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<Ipv4>();
        let id = 6;

        // 1. Send fragment at offset 2, length 1, `m_flag = false`.
        // This defines the packet end at block 2. Gaps: [0, 1].
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 2, size: 1, m_flag: false },
            get_ipv4_builder(),
            ExpectedResult::NeedMore,
        );

        // 2. Send fragment at offset 1, length 1, `m_flag = false`.
        // This also claims to be last, but we already have block 2.
        // This must be invalid.
        process_ipv4_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 1, size: 1, m_flag: false },
            get_ipv4_builder(),
            ExpectedResult::Invalid,
        );
    }

    // Verify that the largest fragment size is tracked and returned.
    #[ip_test(I)]
    #[test_case(1)]
    #[test_case(10)]
    #[test_case(100)]
    fn test_max_fragment_len<I: TestIpExt>(size: u16) {
        let FakeCtxImpl { mut core_ctx, mut bindings_ctx } = new_context::<I>();
        let id = 5;

        // Process fragment #0 (small)
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 0, size: 1, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #1 (large)
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: 1, size, m_flag: true },
            ExpectedResult::NeedMore,
        );

        // Process fragment #2 (small)
        I::process_ip_fragment(
            &mut core_ctx,
            &mut bindings_ctx,
            FragmentSpec { id, offset: size + 1, size: 1, m_flag: false },
            ExpectedResult::Ready { body_fragment_blocks: size + 2, key: test_key(id) },
        );

        try_reassemble_ip_packet(
            &mut core_ctx,
            &mut bindings_ctx,
            id,
            size + 2,
            expected_max_fragment_len::<I>(size),
        );
    }
}
