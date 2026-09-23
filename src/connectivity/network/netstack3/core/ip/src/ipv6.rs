// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IPv6 specific functionality.

use netstack3_base::{AnyDevice, DeviceIdContext, LocalFrameDestination};
use packet_formats::ipv6::Ipv6Packet;
use packet_formats::ipv6::ext_hdrs::{
    DestinationOptionData, ExtensionHeaderOption, FragmentData, HopByHopOptionData,
    Ipv6ExtensionHeader, RoutingData, RoutingTypeParseError,
};
use zerocopy::SplitByteSlice;

/// What to do with an IPv6 packet after parsing an extension header.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Ipv6PacketAction {
    /// Discard the packet.
    _Discard,

    /// Continue processing the next extension header (if any are
    /// available and the processing node is the destination node)
    /// or continue processing the packet (if the extension headers
    /// have been exhausted or the processing node is not the
    /// destination node).
    Continue,

    /// Stop processing extension headers and consider the
    /// packet fragmented. The node must attempt to handle
    /// the fragmented packet (attempt reassembly).
    ProcessFragment,
}

/// Handle IPv6 extension headers.
///
/// What this function does depends on whether or not the `at_destination` flag
/// is set. If it is `true`, then we will attempt to process all the extension
/// headers in `packet`. Otherwise, we will only attempt to process the
/// hop-by-hop extension header (which MUST be the first extension header if
/// present) as per RFC 8200 section 4.
pub(crate) fn handle_extension_headers<CC: DeviceIdContext<AnyDevice>, B: SplitByteSlice>(
    core_ctx: &mut CC,
    device: &CC::DeviceId,
    frame_dst: Option<LocalFrameDestination>,
    packet: &Ipv6Packet<B>,
    at_destination: bool,
) -> Ipv6PacketAction {
    // The next action we need to do after processing an extension header.
    //
    // Initialized to `Ipv6PacketAction::Continue` so we start off processing
    // extension headers.
    let mut action = Ipv6PacketAction::Continue;
    let mut iter = packet.iter_extension_hdrs();

    if at_destination {
        // Keep looping while we are okay to just continue parsing extension
        // headers.
        while action == Ipv6PacketAction::Continue {
            let ext_hdr = match iter.next() {
                None => break,
                Some(x) => x,
            };

            match ext_hdr {
                Ipv6ExtensionHeader::HopByHopOptions { options } => {
                    action = handle_hop_by_hop_options_ext_hdr(
                        core_ctx,
                        device,
                        frame_dst,
                        packet,
                        options.iter(),
                    );
                }
                Ipv6ExtensionHeader::Routing { routing_data } => {
                    action =
                        handle_routing_ext_hdr(core_ctx, device, frame_dst, packet, &routing_data);
                }
                Ipv6ExtensionHeader::Fragment { fragment_data } => {
                    action =
                        handle_fragment_ext_hdr(core_ctx, device, frame_dst, packet, fragment_data);
                }
                Ipv6ExtensionHeader::DestinationOptions { options } => {
                    action = handle_destination_options_ext_hdr(
                        core_ctx,
                        device,
                        frame_dst,
                        packet,
                        options.iter(),
                    );
                }
            }
        }
    } else {
        // Packet is not yet at the destination, so only process the hop-by-hop
        // options extension header (which MUST be the first extension header if
        // it is present) as per RFC 8200 section 4.
        if let Some(ext_hdr) = iter.next() {
            if let Ipv6ExtensionHeader::HopByHopOptions { options } = ext_hdr {
                action = handle_hop_by_hop_options_ext_hdr(
                    core_ctx,
                    device,
                    frame_dst,
                    packet,
                    options.iter(),
                );
            }
        }
    }

    action
}

/// Handles a Hop By Hop extension header for a `packet`.
// For now, we do not support any options. If parsing succeeds we are guaranteed
// that the options present are safely skippable. If they aren't safely
// skippable, we must have resulted in a parsing error when parsing the packet,
// and so this function will never be called.
fn handle_hop_by_hop_options_ext_hdr<
    'a,
    CC: DeviceIdContext<AnyDevice>,
    B: SplitByteSlice,
    I: Iterator<Item = ExtensionHeaderOption<HopByHopOptionData<'a>>>,
>(
    _bindings_ctx: &mut CC,
    _device: &CC::DeviceId,
    _frame_dst: Option<LocalFrameDestination>,
    _packet: &Ipv6Packet<B>,
    options: I,
) -> Ipv6PacketAction {
    for option in options {
        match option.data {
            // Safely skip and continue, as we know that if we parsed an
            // unrecognized option, the option's action was set to skip and
            // continue.
            HopByHopOptionData::Unrecognized { .. } => {}
            // Also skip RouterAlert because router part of MLD is not
            // implemented.
            HopByHopOptionData::RouterAlert { .. } => {}
        }
    }

    Ipv6PacketAction::Continue
}

/// Handles a Routing extension header for a `packet`.
fn handle_routing_ext_hdr<'a, CC: DeviceIdContext<AnyDevice>, B: SplitByteSlice>(
    _bindings_ctx: &mut CC,
    _device: &CC::DeviceId,
    _frame_dst: Option<LocalFrameDestination>,
    _packet: &Ipv6Packet<B>,
    routing_data: &RoutingData<'a>,
) -> Ipv6PacketAction {
    match routing_data.routing_type() {
        // For now, we do not support any routing types. If Segments Left is
        // nonzero then packet parsing fails; if we're here then we must ignore
        // the header and continue per RFC 8200 section 4.4.
        Err(RoutingTypeParseError::UnsupportedType(_)) => Ipv6PacketAction::Continue,
    }
}

/// Handles a fragment extension header for a `packet`.
fn handle_fragment_ext_hdr<CC: DeviceIdContext<AnyDevice>, B: SplitByteSlice>(
    _bindings_ctx: &mut CC,
    _device: &CC::DeviceId,
    _frame_dst: Option<LocalFrameDestination>,
    _packet: &Ipv6Packet<B>,
    _fragment_data: FragmentData,
) -> Ipv6PacketAction {
    Ipv6PacketAction::ProcessFragment
}

/// Handles a destination extension header for a `packet`.
// For now, we do not support any options. If parsing succeeds we are guaranteed
// that the options present are safely skippable. If they aren't safely
// skippable, we must have resulted in a parsing error when parsing the packet,
// and so this function will never be called.
fn handle_destination_options_ext_hdr<
    'a,
    CC: DeviceIdContext<AnyDevice>,
    B: SplitByteSlice,
    I: Iterator<Item = ExtensionHeaderOption<DestinationOptionData<'a>>>,
>(
    _bindings_ctx: &mut CC,
    _device: &CC::DeviceId,
    _frame_dst: Option<LocalFrameDestination>,
    _packet: &Ipv6Packet<B>,
    options: I,
) -> Ipv6PacketAction {
    for option in options {
        match option.data {
            // Safely skip and continue, as we know that if we parsed an
            // unrecognized option, the option's action was set to skip and
            // continue.
            DestinationOptionData::Unrecognized { .. } => {}
        }
    }

    Ipv6PacketAction::Continue
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use netstack3_base::NetworkSerializationContext;
    use netstack3_base::testutil::{FakeDeviceId, TEST_ADDRS_V6};
    use packet::ParseBuffer;
    use packet::serialize::{Buf, NestablePacketBuilder as _, Serializer};
    use packet_formats::ip::IpProto;
    use packet_formats::ipv6::Ipv6PacketBuilder;

    use super::*;

    type FakeCoreCtx = netstack3_base::testutil::FakeCoreCtx<(), (), FakeDeviceId>;

    #[test]
    fn test_no_extension_headers() {
        // Test that if we have no extension headers, we continue.
        let mut core_ctx = FakeCoreCtx::default();
        let builder = Ipv6PacketBuilder::new(
            TEST_ADDRS_V6.remote_ip,
            TEST_ADDRS_V6.local_ip,
            10,
            IpProto::Tcp.into(),
        );
        let frame_dst = LocalFrameDestination::Individual { local: () };
        let mut buffer = builder
            .wrap_body(Buf::new(vec![1, 2, 3, 4, 5], ..))
            .serialize_vec_outer(&mut NetworkSerializationContext::default())
            .unwrap();
        let packet = buffer.parse::<Ipv6Packet<_>>().unwrap();

        assert_eq!(
            handle_extension_headers(&mut core_ctx, &FakeDeviceId, Some(frame_dst), &packet, false),
            Ipv6PacketAction::Continue
        );
    }
}
