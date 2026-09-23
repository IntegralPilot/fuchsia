// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Constructs that support Generic Receive Offload (GRO) at the device layer.

use alloc::vec::Vec;
use core::num::NonZeroU16;

use assert_matches::assert_matches;
use derivative::Derivative;
use net_types::ethernet::Mac;
use net_types::for_any_ip_version;
use net_types::ip::{IpAddress, IpVersion, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use packet::ParsablePacket;
use packet_formats::ethernet::{EtherType, EthernetFrame, EthernetFrameLengthCheck};
use packet_formats::ip::{IpExt, IpPacket as _, IpProto, Ipv4Proto, Ipv6Proto};
use packet_formats::ipv4::Ipv4Packet;
use packet_formats::ipv6::Ipv6Packet;
use packet_formats::tcp::{TcpParseArgs, TcpSegment};

use netstack3_base::{ChecksumRxOffloading, NetworkParsingContext};

/// A slice view of a buffer, which is either a contiguous slice or linearized
/// into scratch storage.
#[derive(Debug)]
pub enum BufferSlice<'a, 'b> {
    /// A slice view directly into a contiguous buffer.
    Contiguous(&'a mut [u8]),
    /// A slice view into scratch storage after linearizing a non-contiguous
    /// buffer.
    Linearized(&'b mut [u8]),
}

impl BufferSlice<'_, '_> {
    /// Returns an immutable slice view of the buffer.
    pub fn as_slice(&self) -> &[u8] {
        match self {
            Self::Contiguous(s) => s,
            Self::Linearized(s) => s,
        }
    }

    /// Returns a mutable slice view of the buffer.
    pub fn as_slice_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Contiguous(s) => s,
            Self::Linearized(s) => s,
        }
    }
}

/// A buffer that may be backed by a contiguous memory slice.
pub trait MaybeContiguousBuffer {
    /// Obtains a slice view into the buffer, linearizing into `storage` if
    /// necessary, or returning `None` if linearization was required and
    /// `storage` was `None`.
    fn linearized<'a, 'b>(
        &'a mut self,
        storage: Option<&'b mut Vec<u8>>,
    ) -> Option<BufferSlice<'a, 'b>>;

    /// Obtains a mutable slice into the buffer without linearizing. Panics if
    /// called on a buffer that's not contiguous.
    fn unwrap_contiguous<'a>(&'a mut self) -> &'a mut [u8] {
        assert_matches!(
            self.linearized(None).expect("must be `Some` if contiguous"),
            BufferSlice::Contiguous(slice) => slice
        )
    }
}

/// Frame type for GRO packet parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroFrameType {
    /// Ethernet frame.
    Ethernet,
    /// Pure IP frame (IPv4 or IPv6).
    PureIp(IpVersion),
}

/// The destination for a buffer involved in GRO (e.g., a device ID). Buffers
/// with different destinations are not coalesced.
pub trait GroBufferDestination: Eq {
    /// Returns the frame type for this destination.
    fn frame_type(&self) -> GroFrameType;
}

/// Ethernet flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EthernetFlowId {
    src_mac: Mac,
    dst_mac: Mac,
    tag: Option<u32>,
}

/// Link layer flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkLayerFlowId {
    Ethernet(EthernetFlowId),
    PureIp,
}

/// Link-layer framing information used to derive GRO eligibility and flow ID.
enum LinkLayerFrame {
    Ethernet { flow_id: EthernetFlowId, ethertype: Option<EtherType> },
    PureIp(IpVersion),
}

impl LinkLayerFrame {
    fn parse<T: GroBufferDestination>(slice: &mut &[u8], target: &T) -> Option<LinkLayerFrame> {
        match target.frame_type() {
            GroFrameType::Ethernet => {
                let frame = EthernetFrame::parse(slice, EthernetFrameLengthCheck::NoCheck).ok()?;
                let flow_id = EthernetFlowId {
                    src_mac: frame.src_mac(),
                    dst_mac: frame.dst_mac(),
                    tag: frame.tag(),
                };
                Some(LinkLayerFrame::Ethernet { flow_id, ethertype: frame.ethertype() })
            }
            GroFrameType::PureIp(ip_version) => Some(LinkLayerFrame::PureIp(ip_version)),
        }
    }

    fn is_eligible_for_gro(&self) -> bool {
        match self {
            Self::Ethernet { .. } => true,
            Self::PureIp(_) => true,
        }
    }

    fn ethertype(&self) -> Option<EtherType> {
        match self {
            Self::Ethernet { ethertype, .. } => *ethertype,
            Self::PureIp(ip_version) => Some(EtherType::from_ip_version(*ip_version)),
        }
    }

    fn flow_id(&self) -> LinkLayerFlowId {
        match self {
            Self::Ethernet { flow_id, .. } => LinkLayerFlowId::Ethernet(*flow_id),
            Self::PureIp(_) => LinkLayerFlowId::PureIp,
        }
    }
}

/// IPv4 flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv4FlowId {
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
}

/// IPv6 flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv6FlowId {
    src_ip: Ipv6Addr,
    dst_ip: Ipv6Addr,
    flowlabel: u32,
}

/// IP layer flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpFlowId {
    Ipv4(Ipv4FlowId),
    Ipv6(Ipv6FlowId),
}

enum IpPacket<'a> {
    V4(Ipv4Packet<&'a [u8]>),
    V6(Ipv6Packet<&'a [u8]>),
}

impl<'a> From<Ipv4Packet<&'a [u8]>> for IpPacket<'a> {
    fn from(packet: Ipv4Packet<&'a [u8]>) -> IpPacket<'a> {
        IpPacket::V4(packet)
    }
}

impl<'a> From<Ipv6Packet<&'a [u8]>> for IpPacket<'a> {
    fn from(packet: Ipv6Packet<&'a [u8]>) -> IpPacket<'a> {
        IpPacket::V6(packet)
    }
}

trait GroIpPacket<I: IpExt> {
    fn is_eligible_for_gro(&self) -> bool;
    fn ip_proto(&self) -> Option<IpProto>;
    fn flow_id(&self) -> IpFlowId;
}

impl GroIpPacket<Ipv4> for Ipv4Packet<&[u8]> {
    fn is_eligible_for_gro(&self) -> bool {
        use packet_formats::ipv4::Ipv4Header as _;

        // Must not be fragmented: the transport headers are only
        // available for flow matching in the first fragment.
        self.fragment_offset() == packet_formats::ip::FragmentOffset::ZERO
            && !self.mf_flag()
            // Must not have options: this lets us avoid the need to
            // copy them out of the buffer or arbitrarily index into the
            // coalescing buffer to find them.
            && self.header_len() == packet_formats::ipv4::HDR_PREFIX_LEN
    }

    fn ip_proto(&self) -> Option<IpProto> {
        match self.proto() {
            Ipv4Proto::Proto(proto) => Some(proto),
            Ipv4Proto::Icmp | Ipv4Proto::Igmp | Ipv4Proto::Other(_) => None,
        }
    }

    fn flow_id(&self) -> IpFlowId {
        IpFlowId::Ipv4(Ipv4FlowId { src_ip: self.src_ip(), dst_ip: self.dst_ip() })
    }
}

impl GroIpPacket<Ipv6> for Ipv6Packet<&[u8]> {
    fn is_eligible_for_gro(&self) -> bool {
        // Must not have extension headers. Note that this differs from
        // Linux, which allows extension headers as long as they're
        // equal. We omit these for the same reason as the IPv4 case
        // above: to avoid needing to copy them or arbitrarily index
        // into the coalescing buffer.
        self.iter_extension_hdrs().next().is_none()
    }

    fn ip_proto(&self) -> Option<IpProto> {
        match self.proto() {
            Ipv6Proto::Proto(proto) => Some(proto),
            Ipv6Proto::Icmpv6 | Ipv6Proto::NoNextHeader | Ipv6Proto::Other(_) => None,
        }
    }

    fn flow_id(&self) -> IpFlowId {
        IpFlowId::Ipv6(Ipv6FlowId {
            src_ip: self.src_ip(),
            dst_ip: self.dst_ip(),
            flowlabel: self.flowlabel(),
        })
    }
}

/// TCP flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TcpFlowId {
    src_port: NonZeroU16,
    dst_port: NonZeroU16,
}

/// Transport layer flow identifier for GRO matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportFlowId {
    Tcp(TcpFlowId),
}

enum TransportPacket<'a> {
    Tcp(TcpSegment<&'a [u8]>),
}

impl<'a> TransportPacket<'a> {
    fn parse<A: IpAddress>(
        transport_view: &mut &'a [u8],
        proto: IpProto,
        src_ip: A,
        dst_ip: A,
        context: &mut NetworkParsingContext,
    ) -> Option<TransportPacket<'a>> {
        match proto {
            IpProto::Tcp => TcpSegment::parse(
                transport_view,
                TcpParseArgs::with_context(src_ip, dst_ip, context),
            )
            .ok()
            .map(TransportPacket::Tcp),
            // TODO(https://fxbug.dev/555942793): Implement GRO for UDP.
            IpProto::Udp | IpProto::Reserved => None,
        }
    }

    fn is_eligible_for_gro(&self) -> bool {
        match self {
            Self::Tcp(_) => true,
        }
    }

    fn flow_id(&self) -> TransportFlowId {
        match self {
            Self::Tcp(tcp) => TransportFlowId::Tcp(TcpFlowId {
                src_port: tcp.src_port(),
                dst_port: tcp.dst_port(),
            }),
        }
    }

    /// Checks if this packet must be flushed to the stack given that it was not
    /// merged into a flow, either because it didn't match one or because it
    /// failed the merge checks.
    ///
    /// If it need not be flushed, it's eligible to become a new flow.
    fn flush_if_not_merged(&self) -> bool {
        match self {
            Self::Tcp(_tcp) => {
                // True if `SYN`, `FIN`, `RST`, `URG`, or `PSH` is set, or if
                // the payload is empty.

                // TODO(https://fxbug.dev/452980285): Implement.
                false
            }
        }
    }
}

/// Flow identifier for GRO matching.
///
/// Note: if two packets have matching flow identifiers, it's not necessarily
/// true that they'll be coalesced, but it is true that their fates (coalesced,
/// flushed, or both) are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GroFlowId {
    link_layer: LinkLayerFlowId,
    ip: IpFlowId,
    transport: TransportFlowId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeaderOffsets {
    pub ip_offset: usize,
    pub transport_offset: usize,
}

/// Constructs the checksum offloading indication to include in the GRO output
/// given the input offloading indication and the parsing context used to parse
/// the GRO packet.
fn build_csum_offload_output(
    input: ChecksumRxOffloading,
    context_post_parse: &NetworkParsingContext,
) -> ChecksumRxOffloading {
    let verified = context_post_parse.verified_checksum_count();
    match input {
        ChecksumRxOffloading::FullyOffloaded => ChecksumRxOffloading::FullyOffloaded,
        ChecksumRxOffloading::Offloaded(Some(n)) => {
            ChecksumRxOffloading::Offloaded(Some(n.saturating_add(verified)))
        }
        ChecksumRxOffloading::Offloaded(None) => {
            ChecksumRxOffloading::Offloaded(NonZeroU16::new(verified))
        }
    }
}

/// Parsed packet containing all metadata needed for GRO matching and
/// accumulation.
struct GroPacket<'a> {
    pub flow_id: GroFlowId,
    pub offsets: HeaderOffsets,
    pub ip: IpPacket<'a>,
    pub transport: TransportPacket<'a>,
}

impl<'a> GroPacket<'a> {
    fn parse<T: GroBufferDestination>(
        slice: &'a [u8],
        target: &T,
        context: &mut NetworkParsingContext,
    ) -> Option<GroPacket<'a>> {
        let total_len = slice.len();
        let mut view = slice;

        let ll_frame =
            LinkLayerFrame::parse(&mut view, target).filter(|f| f.is_eligible_for_gro())?;
        let ethertype = ll_frame.ethertype()?;

        let ip_offset = total_len - view.len();
        let ip_version = ethertype.to_ip_version()?;
        let (ip, ip_flow_id, transport_offset, transport) = for_any_ip_version!(ip_version, I, {
            let ip = <I as IpExt>::Packet::parse(&mut view, ())
                .ok()
                .filter(|p| p.is_eligible_for_gro())?;
            let flow_id = ip.flow_id();
            let proto = ip.ip_proto()?;
            let src = ip.src_ip();
            let dst = ip.dst_ip();

            // NB: the IP parser trims any bytes past the end of the IP payload
            // (e.g. link layer padding) off of `view`, so `total_len -
            // view.len()` would overshoot the start of the transport header by
            // the number of trailing bytes. Derive the offset from the IP
            // header length instead.
            let transport_offset = ip_offset + ip.header_len();
            let transport = TransportPacket::parse(&mut view, proto, src, dst, context)
                .filter(|p| p.is_eligible_for_gro())?;

            (ip.into(), flow_id, transport_offset, transport)
        });

        let offsets = HeaderOffsets { ip_offset, transport_offset };
        let flow_id = GroFlowId {
            link_layer: ll_frame.flow_id(),
            ip: ip_flow_id,
            transport: transport.flow_id(),
        };

        Some(GroPacket { flow_id, offsets, ip, transport })
    }

    fn flush_if_not_merged(&self) -> bool {
        self.transport.flush_if_not_merged()
    }
}

enum CoalesceResult {
    // TODO(https://fxbug.dev/452980285): Remove once used.
    #[expect(dead_code)]
    Flush,
    Continue,
}

/// TCP flow matching criteria and accumulation state.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TcpFlow {
    // TODO(https://fxbug.dev/452980285): Add flow state tracking.
}

impl TcpFlow {
    fn new(_segment: &TcpSegment<&'_ [u8]>) -> Self {
        Self {}
    }

    /// Determines whether a TCP segment already matched to this flow is
    /// permitted to coalesce with it.
    fn can_coalesce(&self, _current_frame_len: usize, _segment: &TcpSegment<&'_ [u8]>) -> bool {
        // Requires that:
        // - payload is nonempty and no larger than `gso_size`
        // - new frame length would not exceed the maximum
        // - flags match in all bits but `PSH` and `FIN`
        // - ACK number, window size, and options match
        // - sequence numbers are contiguous

        // TODO(https://fxbug.dev/452980285): Implement.
        false
    }

    /// Coalesces a TCP segment into this flow.
    ///
    /// Note that this operation does not modify the header of the coalesced TCP
    /// segment. Instead it performs internal bookkeeping and performs the
    /// modifications once in `finalize`.
    fn coalesce(
        &mut self,
        _ip_flow: &IpFlowId,
        _segment: &TcpSegment<&'_ [u8]>,
        _coalesce_into: &mut Vec<u8>,
    ) -> CoalesceResult {
        // Flow matching is permitted to continue unless `SYN`, `FIN`, `RST`,
        // `URG`, or `PSH` is set on `segment`, or its length is less than
        // `gso_size` (all GRO'd segments must be the same length, save for the
        // last one).

        // TODO(https://fxbug.dev/452980285): Implement.
        CoalesceResult::Continue
    }

    /// Finalizes the coalesced TCP segment by updating the header with the
    /// correct checksum and the merged flags (which may only set `FIN` and/or
    /// `PSH`).
    fn finalize(&self, mut _transport_view: &mut [u8]) {
        // TODO(https://fxbug.dev/452980285): Implement.
    }
}

/// Transport-specific flow state.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportFlow {
    Tcp(TcpFlow),
}

impl TransportFlow {
    fn new(packet: &TransportPacket<'_>) -> Self {
        match packet {
            TransportPacket::Tcp(tcp) => Self::Tcp(TcpFlow::new(tcp)),
        }
    }

    fn can_coalesce(&self, current_frame_len: usize, packet: &TransportPacket<'_>) -> bool {
        match (self, packet) {
            (Self::Tcp(flow), TransportPacket::Tcp(tcp)) => {
                flow.can_coalesce(current_frame_len, tcp)
            }
        }
    }

    fn coalesce(
        &mut self,
        ip_flow: &IpFlowId,
        packet: &TransportPacket<'_>,
        coalesce_into: &mut Vec<u8>,
    ) -> CoalesceResult {
        match (self, packet) {
            (Self::Tcp(flow), TransportPacket::Tcp(tcp)) => {
                flow.coalesce(ip_flow, tcp, coalesce_into)
            }
        }
    }

    fn finalize(&self, transport_view: &mut [u8]) {
        match self {
            Self::Tcp(tcp) => tcp.finalize(transport_view),
        }
    }
}

/// IPv4 flow post-match fields and header finalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv4Flow {
    // TODO(https://fxbug.dev/452980285): Add flow state tracking.
}

impl Ipv4Flow {
    fn new(_packet: &Ipv4Packet<&'_ [u8]>) -> Ipv4Flow {
        Self {}
    }

    /// Determines whether an IPv4 packet already matched to this flow is
    /// permitted to coalesce with it.
    fn can_coalesce(&self, _packet: &Ipv4Packet<&'_ [u8]>) -> bool {
        // Requires that;
        // - TTLs match
        // - DSCP, ECN, and DF bit match
        // - ID matches the pattern established for the flow (may be constant or
        //   increasing by one with each packet)

        // TODO(https://fxbug.dev/452980285): Implement.
        false
    }

    /// Finalizes the coalesced IPv4 packet by updating the header with the new
    /// length and checksum.
    fn finalize(&self, mut _ip_view: &mut [u8]) {
        // TODO(https://fxbug.dev/452980285): Implement.
    }
}

/// IPv6 flow post-match fields and header finalization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ipv6Flow {
    // TODO(https://fxbug.dev/452980285): Add flow state tracking.
}

impl Ipv6Flow {
    fn new(_packet: &Ipv6Packet<&'_ [u8]>) -> Ipv6Flow {
        Self {}
    }

    /// Determines whether an IPv6 packet already matched to this flow is
    /// permitted to coalesce with it.
    fn can_coalesce(&self, _packet: &Ipv6Packet<&'_ [u8]>) -> bool {
        // Requires that:
        // - Hop limits match
        // - DSCP and ECN match

        // TODO(https://fxbug.dev/452980285): Implement.
        false
    }

    /// Finalizes the coalesced IPv6 packet by updating the header with the new
    /// payload length.
    fn finalize(&self, mut _ip_view: &mut [u8]) {
        // TODO(https://fxbug.dev/452980285): Implement.
    }
}

/// IP-specific flow state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpFlow {
    Ipv4(Ipv4Flow),
    Ipv6(Ipv6Flow),
}

impl IpFlow {
    fn new(ip: &IpPacket<'_>) -> Self {
        match ip {
            IpPacket::V4(packet) => IpFlow::Ipv4(Ipv4Flow::new(packet)),
            IpPacket::V6(packet) => IpFlow::Ipv6(Ipv6Flow::new(packet)),
        }
    }

    fn can_coalesce(&self, ip: &IpPacket<'_>) -> bool {
        match (self, ip) {
            (Self::Ipv4(flow), IpPacket::V4(packet)) => flow.can_coalesce(packet),
            (Self::Ipv6(flow), IpPacket::V6(packet)) => flow.can_coalesce(packet),
            (Self::Ipv4(_), IpPacket::V6(_)) | (Self::Ipv6(_), IpPacket::V4(_)) => false,
        }
    }

    fn finalize(&self, ip_view: &mut [u8]) {
        match self {
            Self::Ipv4(flow) => flow.finalize(ip_view),
            Self::Ipv6(flow) => flow.finalize(ip_view),
        }
    }
}

struct GroFlow<T> {
    target: T,
    flow_id: GroFlowId,
    offsets: HeaderOffsets,
    ip: IpFlow,
    transport: TransportFlow,
    checksum_offload: ChecksumRxOffloading,
    num_coalesced: usize,
}

impl<T: Eq> GroFlow<T> {
    fn new(target: T, parsed: GroPacket<'_>, checksum_offload: ChecksumRxOffloading) -> Self {
        let GroPacket { flow_id, offsets, ip, transport } = parsed;
        Self {
            target,
            flow_id,
            offsets,
            ip: IpFlow::new(&ip),
            transport: TransportFlow::new(&transport),
            checksum_offload,
            num_coalesced: 1,
        }
    }

    fn matches(
        &self,
        target: &T,
        flow_id: &GroFlowId,
        checksum_offload: ChecksumRxOffloading,
    ) -> bool {
        self.target == *target
            && self.flow_id == *flow_id
            && self.checksum_offload == checksum_offload
    }

    fn can_coalesce(&self, current_frame_len: usize, parsed: &GroPacket<'_>) -> bool {
        self.ip.can_coalesce(&parsed.ip)
            && self.transport.can_coalesce(current_frame_len, &parsed.transport)
    }

    fn coalesce(&mut self, parsed: GroPacket<'_>, coalesce_into: &mut Vec<u8>) -> CoalesceResult {
        self.num_coalesced += 1;
        self.transport.coalesce(&self.flow_id.ip, &parsed.transport, coalesce_into)
    }

    fn finalize(&self, view: &mut [u8]) {
        let HeaderOffsets { ip_offset, transport_offset } = self.offsets;
        if self.num_coalesced > 1 {
            self.ip.finalize(&mut view[ip_offset..]);
            self.transport.finalize(&mut view[transport_offset..]);
        }
    }
}

/// An input buffer item for GRO processing.
#[derive(Debug, PartialEq, Eq)]
pub struct GroInputItem<B, T> {
    /// The buffer.
    pub buffer: B,
    /// Target for the incoming frame.
    pub target: T,
    /// Checksum offload state for the incoming frame.
    pub checksum_offload: ChecksumRxOffloading,
}

/// Buffers associated with a GRO output item.
#[derive(Debug)]
pub enum GroOutputBuffers<'a, B, O> {
    /// A single contiguous buffer.
    Contiguous(B),
    /// A single buffer that was linearized into temporary scratch space.
    Linearized {
        /// The linearized slice view into scratch storage.
        slice: &'a mut [u8],
        /// The original buffer.
        buffer: B,
    },
    /// A coalesced set of buffers.
    Coalesced {
        /// The coalesced slice view into coalescing storage.
        slice: &'a mut [u8],
        /// The original buffers that formed this frame.
        buffers: O,
    },
}

impl<'a, B: MaybeContiguousBuffer, O> GroOutputBuffers<'a, B, O> {
    /// Returns a mutable slice view of the frame buffer.
    pub fn slice_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Contiguous(b) => b.unwrap_contiguous(),
            Self::Linearized { slice, .. } | Self::Coalesced { slice, .. } => slice,
        }
    }
}

/// An item yielded by GRO processing.
#[derive(Debug)]
pub struct GroOutputItem<'a, B, T, O> {
    /// Target for the frame.
    pub target: T,
    /// Checksum offload state for the frame.
    pub checksum_offload: ChecksumRxOffloading,
    /// The buffer(s) associated with this frame.
    pub buffers: GroOutputBuffers<'a, B, O>,
}

/// Persistent reusable buffer storage for GRO to save on per-batch allocations.
#[derive(Debug, Derivative)]
#[derivative(Default(bound = ""))]
pub struct GroBufferStorage<B> {
    /// Buffer for GRO coalescing.
    coalescing_vec: Vec<u8>,
    /// Holds onto the original buffers while building a coalesced frame before
    /// it's passed to the stack.
    coalesced_buffers: Vec<B>,
    /// Buffer for linearization of fragmented buffers.
    linearization_vec: Vec<u8>,
}

impl<B> GroBufferStorage<B> {
    /// Creates a new `GroBufferStorage`.
    pub fn new() -> Self {
        Self::default()
    }

    fn clear(&mut self) {
        self.coalescing_vec.clear();
        self.linearization_vec.clear();
        self.coalesced_buffers.clear();
    }
}

impl<B: MaybeContiguousBuffer> GroBufferStorage<B> {
    /// Adapts the provided iterator of packet buffers into a GRO iterator.
    pub fn coalesce<I, T>(&mut self, iter: I) -> GroIter<'_, I, B, T>
    where
        I: Iterator<Item = GroInputItem<B, T>>,
        T: GroBufferDestination,
    {
        // TODO(https://fxbug.dev/452980285): Create a setting to control whether
        // GRO is enabled and implement TCP coalescing.
        GroIter::new(iter, self, false)
    }

    fn hold(&mut self, buffer: B) {
        self.coalesced_buffers.push(buffer);
    }

    fn build_output<'a, T: Eq>(
        &'a mut self,
        flow: GroFlow<T>,
    ) -> GroOutputItem<'a, B, T, alloc::vec::Drain<'a, B>> {
        let GroBufferStorage { coalescing_vec, coalesced_buffers, .. } = self;

        flow.finalize(&mut coalescing_vec[..]);

        let GroFlow { target, checksum_offload, num_coalesced, .. } = flow;
        let buffers = GroOutputBuffers::Coalesced {
            slice: &mut coalescing_vec[..],
            buffers: coalesced_buffers.drain(..num_coalesced),
        };
        GroOutputItem { target, checksum_offload, buffers }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BufferLinearization {
    Contiguous,
    Linearized,
}

/// The subsequent call to `GroIter::next()` must establish this as the active
/// flow.
///
/// To avoid double-linearizing a fragmented buffer, `linearization` indicates
/// whether `buffer` is contiguous or its linearization is still held in the
/// linearization buffer.
struct PendingFlow<B, T> {
    buffer: B,
    flow: GroFlow<T>,
    linearization: BufferLinearization,
}

/// The subsequent call to `GroIter::next()` must flush this buffer.
///
/// To avoid double-linearizing a fragmented buffer, `linearization` indicates
/// whether `buffer` is contiguous or its linearization is still held in the
/// linearization buffer.
struct PendingFlush<B, T> {
    buffer: B,
    target: T,
    linearization: BufferLinearization,
    checksum_offload: ChecksumRxOffloading,
}

enum PendingItem<B, T> {
    NewFlow(PendingFlow<B, T>),
    Flush(PendingFlush<B, T>),
}

/// An iterator adapter for GRO processing.
pub struct GroIter<'a, I, B, T> {
    iter: I,
    storage: &'a mut GroBufferStorage<B>,
    /// The active GRO flow that GRO is attempting to match.
    active_flow: Option<GroFlow<T>>,
    /// Item pending processing on next iteration.
    pending_item: Option<PendingItem<B, T>>,
    enable_tcp_gro: bool,
}

impl<'a, I, B, T> GroIter<'a, I, B, T>
where
    B: MaybeContiguousBuffer,
{
    fn new(iter: I, storage: &'a mut GroBufferStorage<B>, enable_tcp_gro: bool) -> Self {
        Self { iter, storage, active_flow: None, pending_item: None, enable_tcp_gro }
    }
}

impl<'a, I, B, T> Drop for GroIter<'a, I, B, T> {
    fn drop(&mut self) {
        self.storage.clear();
    }
}

enum ProcessingResult<'a, B, T, O> {
    Continue,
    Return(GroOutputItem<'a, B, T, O>),
}

impl<'a, I, B, T> GroIter<'a, I, B, T>
where
    B: MaybeContiguousBuffer,
    T: GroBufferDestination,
    I: Iterator<Item = GroInputItem<B, T>>,
{
    /// Advances the iterator and returns the next GRO output item.
    pub fn next<'b>(&'b mut self) -> Option<GroOutputItem<'b, B, T, alloc::vec::Drain<'b, B>>> {
        loop {
            if let Some(i) = self.pending_item.take() {
                match self.process_pending(i) {
                    ProcessingResult::Continue => continue,
                    ProcessingResult::Return(out) => return Some(out),
                };
            }
            if let Some(i) = self.iter.next() {
                match self.process_input(i) {
                    ProcessingResult::Continue => continue,
                    ProcessingResult::Return(out) => return Some(out),
                };
            }
            if let Some(f) = self.active_flow.take() {
                return Some(self.storage.build_output(f));
            }
            return None;
        }
    }

    fn process_pending<'b>(
        &'b mut self,
        pending: PendingItem<B, T>,
    ) -> ProcessingResult<'b, B, T, alloc::vec::Drain<'b, B>> {
        let Self { storage, active_flow, .. } = self;
        match pending {
            PendingItem::NewFlow(flow) => {
                let PendingFlow { mut buffer, flow, linearization } = flow;
                let slice = match linearization {
                    BufferLinearization::Contiguous => buffer.unwrap_contiguous(),
                    BufferLinearization::Linearized => &mut storage.linearization_vec,
                };

                storage.coalescing_vec.clear();
                storage.coalescing_vec.extend_from_slice(slice);

                storage.hold(buffer);
                *active_flow = Some(flow);
                ProcessingResult::Continue
            }
            PendingItem::Flush(pending) => {
                let PendingFlush { buffer, target, linearization, checksum_offload } = pending;

                let buffers = match linearization {
                    BufferLinearization::Contiguous => GroOutputBuffers::Contiguous(buffer),
                    BufferLinearization::Linearized => GroOutputBuffers::Linearized {
                        buffer,
                        slice: &mut storage.linearization_vec,
                    },
                };
                ProcessingResult::Return(GroOutputItem { target, checksum_offload, buffers })
            }
        }
    }

    /// Processes a GRO input item and returns the action to be taken as a
    /// result of the processing.
    fn process_input<'b>(
        &'b mut self,
        item: GroInputItem<B, T>,
    ) -> ProcessingResult<'b, B, T, alloc::vec::Drain<'b, B>> {
        let Self { storage, active_flow, pending_item, enable_tcp_gro, .. } = self;
        let GroInputItem { mut buffer, target, checksum_offload } = item;

        storage.linearization_vec.clear();
        let buffer_slice = buffer
            .linearized(Some(&mut storage.linearization_vec))
            .expect("must be `Some` if linearization vec is provided");
        let linearization = match &buffer_slice {
            BufferSlice::Contiguous(_) => BufferLinearization::Contiguous,
            BufferSlice::Linearized(_) => BufferLinearization::Linearized,
        };

        // Implemented as a macro rather than a function or closure because
        // passing `buffer` and `buffer_slice` across a call boundary causes the
        // borrow checker to reject moving `buffer` while it is borrowed by
        // `buffer_slice`. Expanding the pattern match inline allows the borrow
        // to be dropped before moving `buffer` in the `Contiguous` arm.
        macro_rules! return_single_buffer {
            ($csum_offload:expr) => {{
                let buffers = match buffer_slice {
                    BufferSlice::Contiguous(_) => GroOutputBuffers::Contiguous(buffer),
                    BufferSlice::Linearized(slice) => {
                        GroOutputBuffers::Linearized { buffer, slice }
                    }
                };
                return ProcessingResult::Return(GroOutputItem {
                    target,
                    checksum_offload: $csum_offload,
                    buffers,
                });
            }};
        }

        if !*enable_tcp_gro {
            return_single_buffer!(checksum_offload);
        }

        let mut context = NetworkParsingContext::new(checksum_offload);
        let parsed = GroPacket::parse(buffer_slice.as_slice(), &target, &mut context);
        // Note: even if `parse` failed to produce a GRO-eligible packet, it may
        // still have verified the transport checksum so we build the output
        // offloading indication prior to checking the parse result.
        let checksum_offload = build_csum_offload_output(checksum_offload, &context);
        let parsed = match parsed {
            Some(p) => p,
            None => return_single_buffer!(checksum_offload),
        };

        let flush_if_not_merged = parsed.flush_if_not_merged();
        let Some(mut active) = active_flow.take() else {
            // There's no active flow. Establish the active flow unless the
            // buffer needs to be flushed immediately.

            if flush_if_not_merged {
                return_single_buffer!(checksum_offload);
            }

            storage.coalescing_vec.clear();
            storage.coalescing_vec.extend_from_slice(buffer_slice.as_slice());

            let flow = GroFlow::new(target, parsed, checksum_offload);
            storage.hold(buffer);
            *active_flow = Some(flow);
            return ProcessingResult::Continue;
        };

        if !active.matches(&target, &parsed.flow_id, checksum_offload) {
            // The buffer didn't match the active flow, so it was not merged and
            // the active flow is not flushed. Replace the active flow with a
            // new one unless the buffer must be flushed immediately, in which
            // case we just keep the active flow as-is.
            //
            // Note that while this may result in packet re-ordering across
            // separate flows, it will still preserve ordering within any given
            // flow. This is consistent with Linux's implementation of GRO
            // (albeit with a single tracked flow, where Linux tracks up to 8).

            if flush_if_not_merged {
                *active_flow = Some(active);
                return_single_buffer!(checksum_offload);
            }

            let flow = GroFlow::new(target, parsed, checksum_offload);
            let pending = PendingItem::NewFlow(PendingFlow { buffer, flow, linearization });
            *pending_item = Some(pending);
            return ProcessingResult::Return(storage.build_output(active));
        }

        if active.can_coalesce(storage.coalescing_vec.len(), &parsed) {
            let coalesce_result = active.coalesce(parsed, &mut storage.coalescing_vec);
            storage.hold(buffer);

            match coalesce_result {
                CoalesceResult::Flush => ProcessingResult::Return(storage.build_output(active)),
                CoalesceResult::Continue => {
                    *active_flow = Some(active);
                    ProcessingResult::Continue
                }
            }
        } else {
            // The buffer matched the active flow but could not be merged into
            // it, so the active flow is flushed. The buffer is flushed as well
            // if it must be; otherwise it becomes the new active flow.

            let pending = if flush_if_not_merged {
                PendingItem::Flush(PendingFlush { buffer, target, linearization, checksum_offload })
            } else {
                let flow = GroFlow::new(target, parsed, checksum_offload);
                PendingItem::NewFlow(PendingFlow { buffer, flow, linearization })
            };
            *pending_item = Some(pending);
            ProcessingResult::Return(storage.build_output(active))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use alloc::vec;
    use core::sync::atomic::{AtomicBool, Ordering};

    use net_declare::{net_ip_v4, net_ip_v6, net_mac};
    use net_types::ip::{Ipv4, Ipv6};
    use netstack3_base::NetworkSerializationContext;
    use packet::{Buf, InnerPacketBuilder, NestableSerializer as _, Serializer};
    use packet_formats::arp::{ArpOp, ArpPacketBuilder};
    use packet_formats::ethernet::{EtherType, EthernetFrameBuilder};
    use packet_formats::ip::{FragmentOffset, IpExt, IpProto, Ipv4Proto};
    use packet_formats::ipv4::options::Ipv4Option;
    use packet_formats::ipv4::{Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions};
    use packet_formats::ipv6::ext_hdrs::{
        ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData,
    };
    use packet_formats::ipv6::{Ipv6PacketBuilder, Ipv6PacketBuilderWithHbhOptions};
    use packet_formats::tcp::TcpSegmentBuilder;
    use packet_formats::udp::UdpPacketBuilder;
    use test_case::test_case;

    impl GroBufferDestination for GroFrameType {
        fn frame_type(&self) -> GroFrameType {
            *self
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct TestBuffer {
        buf: Vec<u8>,
        contiguous: bool,
    }

    impl MaybeContiguousBuffer for TestBuffer {
        fn linearized<'a, 'b>(
            &'a mut self,
            storage: Option<&'b mut Vec<u8>>,
        ) -> Option<BufferSlice<'a, 'b>> {
            if self.contiguous {
                Some(BufferSlice::Contiguous(&mut self.buf[..]))
            } else {
                let storage = storage?;
                let frame_length = self.buf.len();
                if storage.len() < frame_length {
                    storage.resize(frame_length, 0);
                }
                let slice = &mut storage[..frame_length];
                slice.copy_from_slice(&self.buf);
                Some(BufferSlice::Linearized(slice))
            }
        }
    }

    #[test]
    fn process_gro_handles_fragmented() {
        let items: Vec<GroInputItem<TestBuffer, GroFrameType>> = vec![
            GroInputItem {
                buffer: TestBuffer { buf: vec![1, 2, 3], contiguous: true },
                target: GroFrameType::Ethernet,
                checksum_offload: ChecksumRxOffloading::FullyOffloaded,
            },
            GroInputItem {
                buffer: TestBuffer { buf: vec![4, 5, 6], contiguous: false },
                target: GroFrameType::Ethernet,
                checksum_offload: ChecksumRxOffloading::FullyOffloaded,
            },
        ];

        let mut storage = GroBufferStorage::new();
        let mut output = Vec::new();
        let mut gro = storage.coalesce(items.into_iter());
        while let Some(mut item) = gro.next() {
            output.push(item.buffers.slice_mut().to_vec());
        }

        assert_eq!(output, vec![vec![1, 2, 3], vec![4, 5, 6]]);
    }

    #[derive(Debug)]
    struct TrackedBuffer {
        buf: Vec<u8>,
        contiguous: bool,
        dropped: alloc::sync::Arc<core::sync::atomic::AtomicBool>,
    }

    impl Drop for TrackedBuffer {
        fn drop(&mut self) {
            self.dropped.store(true, core::sync::atomic::Ordering::SeqCst);
        }
    }

    impl MaybeContiguousBuffer for TrackedBuffer {
        fn linearized<'a, 'b>(
            &'a mut self,
            storage: Option<&'b mut Vec<u8>>,
        ) -> Option<BufferSlice<'a, 'b>> {
            if self.contiguous {
                Some(BufferSlice::Contiguous(&mut self.buf[..]))
            } else {
                let storage = storage?;
                let frame_length = self.buf.len();
                if storage.len() < frame_length {
                    storage.resize(frame_length, 0);
                }
                let slice = &mut storage[..frame_length];
                slice.copy_from_slice(&self.buf);
                Some(BufferSlice::Linearized(slice))
            }
        }
    }

    #[test]
    fn gro_buffers_dropped_when_item_dropped() {
        let dropped1 = Arc::new(AtomicBool::new(false));
        let dropped2 = Arc::new(AtomicBool::new(false));

        let items: Vec<GroInputItem<TrackedBuffer, GroFrameType>> = vec![
            GroInputItem {
                buffer: TrackedBuffer {
                    buf: vec![1, 2, 3],
                    contiguous: true,
                    dropped: dropped1.clone(),
                },
                target: GroFrameType::Ethernet,
                checksum_offload: ChecksumRxOffloading::FullyOffloaded,
            },
            GroInputItem {
                buffer: TrackedBuffer {
                    buf: vec![4, 5, 6],
                    contiguous: false,
                    dropped: dropped2.clone(),
                },
                target: GroFrameType::Ethernet,
                checksum_offload: ChecksumRxOffloading::FullyOffloaded,
            },
        ];

        let mut storage = GroBufferStorage::new();
        let mut gro = storage.coalesce(items.into_iter());

        let mut item1 = gro.next().unwrap();
        assert_eq!(item1.buffers.slice_mut(), &[1, 2, 3]);
        assert!(!dropped1.load(Ordering::SeqCst));
        assert!(!dropped2.load(Ordering::SeqCst));
        drop(item1);
        assert!(dropped1.load(Ordering::SeqCst));
        assert!(!dropped2.load(Ordering::SeqCst));

        let mut item2 = gro.next().unwrap();
        assert_eq!(item2.buffers.slice_mut(), &[4, 5, 6]);
        assert!(dropped1.load(Ordering::SeqCst));
        assert!(!dropped2.load(Ordering::SeqCst));
        drop(item2);
        assert!(dropped2.load(Ordering::SeqCst));
    }

    const TEST_SRC_MAC: Mac = net_mac!("00:11:22:33:44:55");
    const TEST_DST_MAC: Mac = net_mac!("66:77:88:99:aa:bb");
    const TEST_SRC_PORT: NonZeroU16 = NonZeroU16::new(1234).unwrap();
    const TEST_DST_PORT: NonZeroU16 = NonZeroU16::new(5678).unwrap();
    const TEST_FLOWLABEL: u32 = 0x12345;
    const TEST_PAYLOAD: [u8; 12] = *b"hello world!";

    trait TestIpExt: IpExt {
        const SRC_IP: Self::Addr;
        const DST_IP: Self::Addr;
        fn ip_builder(proto: IpProto) -> Self::PacketBuilder<NetworkSerializationContext>;
    }

    impl TestIpExt for Ipv4 {
        const SRC_IP: Ipv4Addr = net_ip_v4!("192.168.0.1");
        const DST_IP: Ipv4Addr = net_ip_v4!("192.168.0.2");
        fn ip_builder(proto: IpProto) -> Ipv4PacketBuilder {
            Ipv4PacketBuilder::new(Self::SRC_IP, Self::DST_IP, 64, Ipv4Proto::Proto(proto))
        }
    }

    impl TestIpExt for Ipv6 {
        const SRC_IP: Ipv6Addr = net_ip_v6!("2001:db8::1");
        const DST_IP: Ipv6Addr = net_ip_v6!("2001:db8::2");
        fn ip_builder(proto: IpProto) -> Ipv6PacketBuilder {
            let mut ip = Ipv6PacketBuilder::new(Self::SRC_IP, Self::DST_IP, 64, proto.into());
            ip.flowlabel(TEST_FLOWLABEL);
            ip
        }
    }

    fn ethernet_builder<I: TestIpExt>() -> EthernetFrameBuilder {
        EthernetFrameBuilder::new(TEST_SRC_MAC, TEST_DST_MAC, I::ETHER_TYPE, 0)
    }

    fn tcp_builder<I: TestIpExt>() -> TcpSegmentBuilder<I::Addr> {
        TcpSegmentBuilder::new(
            I::SRC_IP,
            I::DST_IP,
            TEST_SRC_PORT,
            TEST_DST_PORT,
            100,
            Some(200),
            1024,
        )
    }

    fn build_ethernet_tcp_packet<I: TestIpExt>() -> Vec<u8> {
        Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<I>())
            .wrap_in(I::ip_builder(IpProto::Tcp))
            .wrap_in(ethernet_builder::<I>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec()
    }

    fn build_pure_ip_tcp_packet<I: TestIpExt>() -> Vec<u8> {
        Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<I>())
            .wrap_in(I::ip_builder(IpProto::Tcp))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec()
    }

    #[test_case(
        build_ethernet_tcp_packet::<Ipv4>(),
        GroFrameType::Ethernet,
        HeaderOffsets { ip_offset: 14, transport_offset: 34 },
        GroFlowId {
            link_layer: LinkLayerFlowId::Ethernet(EthernetFlowId {
                src_mac: TEST_SRC_MAC,
                dst_mac: TEST_DST_MAC,
                tag: None,
            }),
            ip: IpFlowId::Ipv4(Ipv4FlowId { src_ip: Ipv4::SRC_IP, dst_ip: Ipv4::DST_IP }),
            transport: TransportFlowId::Tcp(TcpFlowId {
                src_port: TEST_SRC_PORT,
                dst_port: TEST_DST_PORT,
            }),
        };
        "ethernet_ipv4_tcp"
    )]
    #[test_case(
        build_ethernet_tcp_packet::<Ipv6>(),
        GroFrameType::Ethernet,
        HeaderOffsets { ip_offset: 14, transport_offset: 54 },
        GroFlowId {
            link_layer: LinkLayerFlowId::Ethernet(EthernetFlowId {
                src_mac: TEST_SRC_MAC,
                dst_mac: TEST_DST_MAC,
                tag: None,
            }),
            ip: IpFlowId::Ipv6(Ipv6FlowId {
                src_ip: Ipv6::SRC_IP,
                dst_ip: Ipv6::DST_IP,
                flowlabel: TEST_FLOWLABEL,
            }),
            transport: TransportFlowId::Tcp(TcpFlowId {
                src_port: TEST_SRC_PORT,
                dst_port: TEST_DST_PORT,
            }),
        };
        "ethernet_ipv6_tcp"
    )]
    #[test_case(
        build_pure_ip_tcp_packet::<Ipv4>(),
        GroFrameType::PureIp(IpVersion::V4),
        HeaderOffsets { ip_offset: 0, transport_offset: 20 },
        GroFlowId {
            link_layer: LinkLayerFlowId::PureIp,
            ip: IpFlowId::Ipv4(Ipv4FlowId { src_ip: Ipv4::SRC_IP, dst_ip: Ipv4::DST_IP }),
            transport: TransportFlowId::Tcp(TcpFlowId {
                src_port: TEST_SRC_PORT,
                dst_port: TEST_DST_PORT,
            }),
        };
        "pure_ip_v4_tcp"
    )]
    #[test_case(
        build_pure_ip_tcp_packet::<Ipv6>(),
        GroFrameType::PureIp(IpVersion::V6),
        HeaderOffsets { ip_offset: 0, transport_offset: 40 },
        GroFlowId {
            link_layer: LinkLayerFlowId::PureIp,
            ip: IpFlowId::Ipv6(Ipv6FlowId {
                src_ip: Ipv6::SRC_IP,
                dst_ip: Ipv6::DST_IP,
                flowlabel: TEST_FLOWLABEL,
            }),
            transport: TransportFlowId::Tcp(TcpFlowId {
                src_port: TEST_SRC_PORT,
                dst_port: TEST_DST_PORT,
            }),
        };
        "pure_ip_v6_tcp"
    )]
    fn gro_packet_parse_success(
        packet_bytes: Vec<u8>,
        target: GroFrameType,
        expected_offsets: HeaderOffsets,
        expected_flow_id: GroFlowId,
    ) {
        let mut context = NetworkParsingContext::default();
        let parsed = GroPacket::parse(&packet_bytes, &target, &mut context)
            .expect("GroPacket::parse should succeed");
        assert_eq!(parsed.offsets, expected_offsets);
        assert_eq!(parsed.flow_id, expected_flow_id);
    }

    #[test]
    fn gro_ineligible_non_ip_ethertype() {
        let arp = ArpPacketBuilder::new(
            ArpOp::Request,
            TEST_SRC_MAC,
            Ipv4::SRC_IP,
            TEST_DST_MAC,
            Ipv4::DST_IP,
        );
        let arp_bytes = arp
            .into_serializer()
            .wrap_in(EthernetFrameBuilder::new(TEST_SRC_MAC, TEST_DST_MAC, EtherType::Arp, 0))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .unwrap_b()
            .as_ref()
            .to_vec();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&arp_bytes, &GroFrameType::Ethernet, &mut context).is_none());
    }

    #[test]
    fn gro_ineligible_ipv4_options() {
        let ip = Ipv4PacketBuilderWithOptions::new(
            Ipv4::ip_builder(IpProto::Tcp),
            [Ipv4Option::RouterAlert { data: 0 }],
        )
        .unwrap();
        let packet = Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<Ipv4>())
            .wrap_in(ip)
            .wrap_in(ethernet_builder::<Ipv4>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_none());
    }

    #[test]
    fn gro_ineligible_ipv4_mf_flag() {
        let mut ip = Ipv4::ip_builder(IpProto::Tcp);
        ip.mf_flag(true);
        let packet = Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<Ipv4>())
            .wrap_in(ip)
            .wrap_in(ethernet_builder::<Ipv4>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_none());
    }

    #[test]
    fn gro_ineligible_ipv4_fragment_offset() {
        let mut ip = Ipv4::ip_builder(IpProto::Tcp);
        ip.fragment_offset(FragmentOffset::new(1).unwrap());
        let packet = Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<Ipv4>())
            .wrap_in(ip)
            .wrap_in(ethernet_builder::<Ipv4>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_none());
    }

    #[test]
    fn gro_ineligible_ipv6_extension_headers() {
        let hbh_opt = [HopByHopOption {
            action: ExtensionHeaderOptionAction::SkipAndContinue,
            mutable: false,
            data: HopByHopOptionData::RouterAlert { data: 0 },
        }];
        let ip =
            Ipv6PacketBuilderWithHbhOptions::new(Ipv6::ip_builder(IpProto::Tcp), &hbh_opt).unwrap();
        let packet = Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<Ipv6>())
            .wrap_in(ip)
            .wrap_in(ethernet_builder::<Ipv6>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_none());
    }

    #[test]
    fn gro_ineligible_non_tcp_transport_proto() {
        let udp =
            UdpPacketBuilder::new(Ipv4::SRC_IP, Ipv4::DST_IP, Some(TEST_SRC_PORT), TEST_DST_PORT);
        let packet = Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(udp)
            .wrap_in(Ipv4::ip_builder(IpProto::Udp))
            .wrap_in(ethernet_builder::<Ipv4>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_none());
    }

    #[test]
    fn gro_corrupt_tcp_checksum() {
        let mut packet = build_ethernet_tcp_packet::<Ipv4>();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        let parsed = GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context)
            .expect("should parse valid packet");
        let checksum_offset =
            parsed.offsets.transport_offset + packet_formats::tcp::CHECKSUM_OFFSET;
        packet[checksum_offset] ^= 0xff;

        // Fails when checksum verification is not offloaded.
        let mut context = NetworkParsingContext::default();
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_none());

        // Succeeds when checksum verification is offloaded.
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context).is_some());
    }

    #[test]
    fn gro_ineligible_pure_ip_version_mismatch() {
        let pure_v4 = build_pure_ip_tcp_packet::<Ipv4>();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(
            GroPacket::parse(&pure_v4, &GroFrameType::PureIp(IpVersion::V6), &mut context)
                .is_none()
        );

        let pure_v6 = build_pure_ip_tcp_packet::<Ipv6>();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        assert!(
            GroPacket::parse(&pure_v6, &GroFrameType::PureIp(IpVersion::V4), &mut context)
                .is_none()
        );
    }

    #[test]
    fn upgrades_csum_offload_on_verified_tcp() {
        let packet = build_ethernet_tcp_packet::<Ipv4>();
        let items: Vec<GroInputItem<TestBuffer, GroFrameType>> = vec![GroInputItem {
            buffer: TestBuffer { buf: packet, contiguous: true },
            target: GroFrameType::Ethernet,
            checksum_offload: ChecksumRxOffloading::default(),
        }];

        let mut storage = GroBufferStorage::new();
        let mut gro = GroIter::new(items.into_iter(), &mut storage, true);
        let item = gro.next().unwrap();
        assert_eq!(
            item.checksum_offload,
            ChecksumRxOffloading::Offloaded(Some(NonZeroU16::new(1).unwrap()))
        );
    }

    #[test]
    fn preserves_input_csum_offload_on_unparsed_tcp() {
        // Build a packet with IPv4 options: ineligible for GRO, so transport
        // checksum verification is never reached.
        let ip = Ipv4PacketBuilderWithOptions::new(
            Ipv4::ip_builder(IpProto::Tcp),
            [Ipv4Option::RouterAlert { data: 0 }],
        )
        .unwrap();
        let packet = Buf::new(TEST_PAYLOAD.to_vec(), ..)
            .wrap_in(tcp_builder::<Ipv4>())
            .wrap_in(ip)
            .wrap_in(ethernet_builder::<Ipv4>())
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap()
            .into_inner()
            .as_ref()
            .to_vec();

        let items: Vec<GroInputItem<TestBuffer, GroFrameType>> = vec![GroInputItem {
            buffer: TestBuffer { buf: packet, contiguous: true },
            target: GroFrameType::Ethernet,
            checksum_offload: ChecksumRxOffloading::default(),
        }];

        let mut storage = GroBufferStorage::new();
        let mut gro = GroIter::new(items.into_iter(), &mut storage, true);
        let item = gro.next().unwrap();
        assert_eq!(item.checksum_offload, ChecksumRxOffloading::default());
    }

    #[test]
    fn preserves_input_csum_offload_on_corrupt_csum() {
        let mut packet = build_ethernet_tcp_packet::<Ipv4>();
        let mut context = NetworkParsingContext::new(ChecksumRxOffloading::FullyOffloaded);
        let parsed = GroPacket::parse(&packet, &GroFrameType::Ethernet, &mut context)
            .expect("should parse valid packet");
        let checksum_offset =
            parsed.offsets.transport_offset + packet_formats::tcp::CHECKSUM_OFFSET;
        packet[checksum_offset] ^= 0xff;

        let items: Vec<GroInputItem<TestBuffer, GroFrameType>> = vec![GroInputItem {
            buffer: TestBuffer { buf: packet, contiguous: true },
            target: GroFrameType::Ethernet,
            checksum_offload: ChecksumRxOffloading::default(),
        }];

        let mut storage = GroBufferStorage::new();
        let mut gro = GroIter::new(items.into_iter(), &mut storage, true);
        let item = gro.next().unwrap();
        assert_eq!(item.checksum_offload, ChecksumRxOffloading::default());
    }
}
