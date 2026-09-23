// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Handles sending neighbor discovery user option (nduseropt) messages to
//! clients that have joined the corresponding multicast group.
//!
//! The messages are derived from watching for NDP options received via router
//! advertisements using fuchsia.net.ndp.

use std::pin::Pin;

use fidl_fuchsia_net_ndp_ext::{OptionWatchStreamError, OptionWatchStreamItem};
use futures::{Stream, StreamExt};
use netlink_packet_core::NetlinkMessage;
use netlink_packet_route::RouteNetlinkMessage;
use netlink_packet_route::neighbour_discovery_user_option::{
    NeighbourDiscoveryIcmpType, NeighbourDiscoveryIcmpV6Type, NeighbourDiscoveryUserOptionHeader,
    NeighbourDiscoveryUserOptionMessage, Nla,
};
use packet::records::options::OptionLayout;
use packet_formats::icmp::ndp;
use packet_formats::icmp::ndp::options::option_types as ndp_option_types;

use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_ndp_ext as fnet_ndp_ext;

use crate::logging::{log_error, log_info, log_warn};

pub(crate) struct NduseroptWorker {
    clients_count: usize,
    provider: fnet_ndp::RouterAdvertisementOptionWatcherProviderProxy,
    stream: Option<
        Pin<Box<dyn Stream<Item = Result<OptionWatchStreamItem, OptionWatchStreamError>> + Send>>,
    >,
}

impl NduseroptWorker {
    pub(crate) fn new(provider: fnet_ndp::RouterAdvertisementOptionWatcherProviderProxy) -> Self {
        Self { clients_count: 0, provider, stream: None }
    }
}

/// These are the same NDP options that Linux forwards to userspace from router
/// advertisements.
const USEROPTS: [fnet_ndp::OptionType; 6] = [
    ndp_option_types::PREFIX_INFORMATION,
    ndp_option_types::RECURSIVE_DNS_SERVER,
    ndp_option_types::DNS_SEARCH_LIST,
    ndp_option_types::SIXLOWPAN_CONTEXT,
    ndp_option_types::CAPTIVE_PORTAL,
    ndp_option_types::PREF64,
];

impl NduseroptWorker {
    /// Behaves like [`futures::StreamExt::select_next_some`] in that it only
    /// yields once a [`NetlinkMessage<RouteNetlinkMessage>`] has been observed,
    /// and otherwise waits forever (even if no hanging get is currently hooked
    /// up).
    pub(crate) async fn select_next_message(&mut self) -> NetlinkMessage<RouteNetlinkMessage> {
        let stream = match self.stream.as_mut() {
            None => futures::future::pending().await,
            Some(stream) => stream,
        };
        while let Some(item) = stream.next().await {
            let item = match item {
                Ok(item) => item,
                Err(e) => {
                    log_error!("observed error in NDP option watcher stream: {e:?}");
                    continue;
                }
            };
            let entry = match item {
                OptionWatchStreamItem::Entry(option_watch_entry) => option_watch_entry,
                OptionWatchStreamItem::Dropped(non_zero) => {
                    log_warn!("dropped {non_zero} NDP option watch entries due to falling behind");
                    continue;
                }
            };
            return build_nduseropt_message(entry);
        }
        log_error!("NDP option watcher stream unexpectedly ended");
        self.stream = None;
        futures::future::pending().await
    }

    pub(crate) async fn increment_clients_count(&mut self) {
        let next_count = self.clients_count.checked_add(1).expect("should not overflow");
        let prev_count = std::mem::replace(&mut self.clients_count, next_count);

        if prev_count != 0 {
            assert!(self.stream.is_some());
            return;
        }

        log_info!(
            "NduseroptWorker initializing NDP watcher due to going from \
            0 to 1 nduseropt subscriber"
        );

        assert!(self.stream.is_none());
        let stream = fnet_ndp_ext::create_watcher_stream(
            &self.provider,
            &fnet_ndp::RouterAdvertisementOptionWatcherParams {
                interest_types: Some(USEROPTS.to_vec()),
                interest_interface_id: None,
                ..Default::default()
            },
        )
        .await
        .expect("protocol should be present")
        .expect("watcher creation should succeed")
        .fuse()
        .boxed();
        self.stream = Some(stream);
    }

    pub(crate) fn decrement_clients_count(&mut self) {
        let next_count = self.clients_count.checked_sub(1).unwrap_or_else(|| {
            log_error!(
                "NduseroptWorker tried to decrement client count below 0, \
                there must be a bug in tracking multicast group membership."
            );
            0
        });
        let prev_count = std::mem::replace(&mut self.clients_count, next_count);
        if self.clients_count == 0 {
            self.stream = None;
            if prev_count != 0 {
                log_info!(
                    "NduseroptWorker stopping NDP watcher due to going from \
                     1 to 0 nduseropt subscriber"
                );
            }
        }
    }
}

fn build_nduseropt_message(
    entry: fnet_ndp_ext::OptionWatchEntry,
) -> NetlinkMessage<RouteNetlinkMessage> {
    let fnet_ndp_ext::OptionWatchEntry { interface_id, source_address, option_type, body } = entry;

    // [`fnet_ndp_ext::OptionWatchEntry`] guarantees that `body`'s length is
    // already padded as needed to reach a multiple of 8, which allows it to be
    // encoded properly as an NDP option body.
    let body = body.into_inner();

    let mut body_with_kind_and_length_bytes = vec![0u8; body.len() + 2];
    body_with_kind_and_length_bytes[0] = option_type;
    body_with_kind_and_length_bytes[1] = ndp::options::NdpOptionsImpl::LENGTH_ENCODING
        .encode_length(body.len())
        .expect("should successfully encode length");
    body_with_kind_and_length_bytes[2..].copy_from_slice(&body);

    let source_lla_nla = {
        if !source_address.is_unicast_link_local() {
            log_warn!(
                "Router Advertisement source address is \
                unexpectedly not link local unicast: {source_address}"
            );
        }
        Nla::SourceLinkLocalAddress(source_address.ipv6_bytes().to_vec())
    };

    let mut message: NetlinkMessage<RouteNetlinkMessage> =
        RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(
            NeighbourDiscoveryUserOptionMessage::new(
                NeighbourDiscoveryUserOptionHeader::new(
                    u32::try_from(interface_id.get()).expect("should fit in u32"),
                    NeighbourDiscoveryIcmpType::Inet6(
                        NeighbourDiscoveryIcmpV6Type::RouterAdvertisement,
                    ),
                ),
                body_with_kind_and_length_bytes,
                vec![source_lla_nla],
            ),
        )
        .into();
    message.finalize();
    message
}
