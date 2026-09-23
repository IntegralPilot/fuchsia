// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(test)]

use std::convert::From as _;
use std::num::NonZeroU64;

use fidl_fuchsia_net_ndp as fnet_ndp;
use fidl_fuchsia_net_ndp_ext as fnet_ndp_ext;

use futures::StreamExt as _;
use net_declare::fidl_mac;
use netemul::TestSandbox;
use netstack_testing_common::realms::{Netstack3, TestSandboxExt as _};
use netstack_testing_macros::netstack_test;
use packet_formats::icmp::ndp as packet_formats_ndp;

const MAC: fidl_fuchsia_net::MacAddress = fidl_mac!("02:00:01:02:03:04");
const SOURCE_ADDR: net_types::ip::Ipv6Addr = net_declare::net_ip_v6!("fe80::1");

fn nonce_option_builder_and_watch_entry(
    interface_id: NonZeroU64,
) -> (packet_formats_ndp::options::NdpOptionBuilder<'static>, fnet_ndp_ext::OptionWatchEntry) {
    const NONCE: [u8; 6] = [1, 2, 3, 4, 5, 6];

    let builder = packet_formats_ndp::options::NdpOptionBuilder::Nonce(
        packet_formats_ndp::options::NdpNonce::from(&NONCE),
    );
    let len = packet::records::options::OptionBuilder::serialized_len(&builder);
    let mut data = vec![0u8; len];
    packet::records::options::OptionBuilder::serialize_into(&builder, &mut data);

    (
        builder,
        fnet_ndp_ext::OptionWatchEntry {
            interface_id,
            source_address: SOURCE_ADDR,
            option_type: packet_formats::icmp::ndp::options::NdpOptionType::Nonce.into(),
            body: fnet_ndp_ext::OptionBody::new(data).expect("should be valid option body"),
        },
    )
}

#[netstack_test]
async fn watch_ndp_option(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network(name).await.expect("failed to create network");

    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let iface = realm
        .join_network_with(
            &network,
            name,
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(MAC)),
            Default::default(),
        )
        .await
        .expect("failed to join network");

    let watcher_provider = realm
        .connect_to_protocol::<fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker>()
        .expect("should successfully connect to provider");

    let mut watcher = std::pin::pin!(
        fnet_ndp_ext::create_watcher_stream(
            &watcher_provider,
            &fnet_ndp::RouterAdvertisementOptionWatcherParams {
                interest_types: None,
                interest_interface_id: Some(iface.id()),
                ..Default::default()
            }
        )
        .await
        .expect("protocol should be present")
        .expect("creating watcher should succeed")
    );

    let (option_builder, watch_entry) =
        nonce_option_builder_and_watch_entry(NonZeroU64::new(iface.id()).unwrap());

    netstack_testing_common::ndp::send_ra_with_router_lifetime(
        &fake_ep,
        u16::MAX,
        &[option_builder],
        SOURCE_ADDR,
    )
    .await
    .expect("should succeed");

    let item = watcher.next().await.expect("should not have ended").expect("should not get error");
    let entry = item.try_into_entry().expect("should not have dropped any entries");
    assert_eq!(entry, watch_entry);
}

fn rdnss_option_builder_and_watch_entry(
    interface_id: NonZeroU64,
) -> (packet_formats_ndp::options::NdpOptionBuilder<'static>, fnet_ndp_ext::OptionWatchEntry) {
    const ADDRESSES: [net_types::ip::Ipv6Addr; 2] =
        [net_declare::net_ip_v6!("2001:db8::1"), net_declare::net_ip_v6!("2001:db8::2")];
    let option = packet_formats::icmp::ndp::options::RecursiveDnsServer::new(u32::MAX, &ADDRESSES);
    let builder = packet_formats::icmp::ndp::options::NdpOptionBuilder::RecursiveDnsServer(option);
    let len = packet::records::options::OptionBuilder::serialized_len(&builder);
    let mut body = vec![0u8; len];
    packet::records::options::OptionBuilder::serialize_into(&builder, &mut body);
    (
        builder,
        fnet_ndp_ext::OptionWatchEntry {
            interface_id,
            source_address: SOURCE_ADDR,
            option_type: packet_formats_ndp::options::NdpOptionType::RecursiveDnsServer.into(),
            body: fnet_ndp_ext::OptionBody::new(body).expect("should be valid option body"),
        },
    )
}

#[netstack_test]
async fn filters_for_rdnss(name: &str) {
    let sandbox = TestSandbox::new().expect("failed to create sandbox");
    let network = sandbox.create_network(name).await.expect("failed to create network");

    let fake_ep = network.create_fake_endpoint().expect("failed to create fake endpoint");
    let realm =
        sandbox.create_netstack_realm::<Netstack3, _>(name).expect("failed to create realm");
    let iface = realm
        .join_network_with(
            &network,
            name,
            netemul::new_endpoint_config(netemul::DEFAULT_MTU, Some(MAC)),
            Default::default(),
        )
        .await
        .expect("failed to join network");

    let watcher_provider = realm
        .connect_to_protocol::<fnet_ndp::RouterAdvertisementOptionWatcherProviderMarker>()
        .expect("should successfully connect to provider");
    let mut watcher = std::pin::pin!(
        fnet_ndp_ext::create_watcher_stream(
            &watcher_provider,
            &fnet_ndp::RouterAdvertisementOptionWatcherParams {
                interest_types: Some(vec![
                    packet_formats_ndp::options::NdpOptionType::RecursiveDnsServer.into()
                ]),
                interest_interface_id: None,
                ..Default::default()
            }
        )
        .await
        .expect("protocol should be present")
        .expect("should successfully create watcher stream")
    );

    let (nonce_option_builder, _nonce_watch_entry) =
        nonce_option_builder_and_watch_entry(NonZeroU64::new(iface.id()).unwrap());
    let (rdnss_option_builder, rdnss_watch_entry) =
        rdnss_option_builder_and_watch_entry(NonZeroU64::new(iface.id()).unwrap());

    netstack_testing_common::ndp::send_ra_with_router_lifetime(
        &fake_ep,
        u16::MAX,
        &[
            // Include an option we don't care about in order to exercise
            // filtering.
            nonce_option_builder,
            rdnss_option_builder,
        ],
        SOURCE_ADDR,
    )
    .await
    .expect("should succeed");
    let item = watcher.next().await.expect("should not have ended").expect("should not get error");
    let entry = item.try_into_entry().expect("should not have dropped any entries");
    assert_eq!(entry, rdnss_watch_entry);
}
