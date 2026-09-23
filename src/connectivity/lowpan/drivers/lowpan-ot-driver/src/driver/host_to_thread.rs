// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use lowpan_driver_common::spinel::*;
use std::net::Ipv6Addr;
use std::num::NonZeroU16;

use anyhow::{Context as _, Error};

use futures::prelude::*;
use lowpan_driver_common::net::{Ipv6PacketMatcherRule, NetworkInterfaceEvent};

use packet::ParsablePacket;
use packet_formats::icmp::mld::MldPacket;
use packet_formats::icmp::ndp::NdpPacket;
use packet_formats::icmp::{IcmpParseArgs, Icmpv6Packet};
use packet_formats::ip::{IpPacket, IpProto, Ipv6Proto};
use packet_formats::ipv6::Ipv6Packet;

enum IpFormat {
    Invalid,
    Ipv4,
    Ipv6,
}

impl IpFormat {
    pub fn for_packet(packet: &[u8]) -> IpFormat {
        match packet.first().map(|x| x >> 4) {
            Some(4) => IpFormat::Ipv4,
            Some(6) => IpFormat::Ipv6,
            _ => IpFormat::Invalid,
        }
    }
}

/// Callbacks from netstack and other on-host systems.
impl<OT, NI, BI> OtDriver<OT, NI, BI>
where
    OT: ot::InstanceInterface + Send,
    NI: NetworkInterface,
    BI: BackboneInterface,
{
    pub(crate) async fn on_regulatory_region_changed(&self, region: String) -> Result<(), Error> {
        info!("Got region code {:?}", region);

        Ok(self.driver_state.lock().ot_instance.set_region(region.try_into()?)?)
    }

    pub(crate) async fn on_network_interface_event(
        &self,
        event: NetworkInterfaceEvent,
    ) -> Result<(), Error> {
        match event {
            NetworkInterfaceEvent::InterfaceEnabledChanged(enabled) => {
                let mut driver_state = self.driver_state.lock();

                let new_connectivity_state = if enabled {
                    driver_state.updated_connectivity_state().activated()
                } else {
                    driver_state.updated_connectivity_state().deactivated()
                };

                if new_connectivity_state != driver_state.connectivity_state {
                    let old_connectivity_state = driver_state.connectivity_state;
                    driver_state.connectivity_state = new_connectivity_state;
                    std::mem::drop(driver_state);
                    self.driver_state_change.trigger();
                    self.on_connectivity_state_change(
                        new_connectivity_state,
                        old_connectivity_state,
                    );
                }
            }
            NetworkInterfaceEvent::AddressWasAdded(x) => self.on_netstack_added_address(x).await?,
            NetworkInterfaceEvent::AddressWasRemoved(x) => {
                self.on_netstack_removed_address(x).await?
            }
            NetworkInterfaceEvent::RouteToSubnetProvided(x) => {
                self.on_netstack_added_route(x).await?
            }
            NetworkInterfaceEvent::RouteToSubnetRevoked(x) => {
                self.on_netstack_removed_route(x).await?
            }
            _ => {}
        };
        Ok(())
    }

    pub(crate) fn on_netstack_joined_multicast_group(
        &self,
        group: std::net::Ipv6Addr,
    ) -> Result<(), anyhow::Error> {
        let driver_state = self.driver_state.lock();

        if let Err(err) =
            driver_state.ot_instance.ip6_join_multicast_group(&group).ignore_already_exists()
        {
            warn!("Unable to join multicast group {} on OpenThread: {:?}", group, err);
        }

        Ok(())
    }

    pub(crate) fn on_netstack_left_multicast_group(
        &self,
        group: std::net::Ipv6Addr,
    ) -> Result<(), anyhow::Error> {
        let driver_state = self.driver_state.lock();

        if let Err(err) = driver_state
            .ot_instance
            .ip6_leave_multicast_group(&group)
            .ignore_already_exists()
            .ignore_not_found()
        {
            warn!("Unable to leave multicast group {} on OpenThread: {:?}", group, err);
        }

        Ok(())
    }

    pub(crate) async fn on_netstack_added_address(&self, subnet: Subnet) -> Result<(), Error> {
        debug!("Netstack added address: {:?}", subnet);
        let subnet_addr = subnet.addr;

        let should_skip = {
            let driver_state = self.driver_state.lock();
            !driver_state.is_active_and_ready()
                || driver_state
                    .ot_instance
                    .ip6_get_unicast_addresses()
                    .any(|entry| *entry.addr() == subnet.addr)
        };

        if !should_skip {
            let netif_addr = ot::NetifAddress::new(subnet.addr, subnet.prefix_len);
            let driver_state = self.driver_state.lock();

            driver_state
                .ot_instance
                .ip6_add_unicast_address(&netif_addr)
                .ignore_already_exists()
                .or_else(move |err| {
                    warn!(
                        "OpenThread refused to add unicast address {} with prefix length {}, will remove from netstack. (Error: {:?})",
                        std::net::Ipv6Addr::from(subnet.addr),
                        subnet.prefix_len,
                        err
                    );
                    self.net_if.remove_address_from_spinel_subnet(&subnet).ignore_not_found()
                })
                .context("on_netstack_added_address")?;
        }

        let driver_state = self.driver_state.lock();
        if let Some(srp_discovery_proxy) = &driver_state.srp_discovery_proxy {
            if driver_state
                .ot_instance
                .ip6_get_unicast_addresses()
                .any(|entry| *entry.addr() == subnet_addr)
            {
                srp_discovery_proxy.add_local_address(subnet_addr);
            }
        }

        Ok(())
    }

    pub(crate) async fn on_netstack_removed_address(&self, subnet: Subnet) -> Result<(), Error> {
        debug!("Netstack removed address: {:?}", subnet);

        let driver_state = self.driver_state.lock();

        if let Some(srp_discovery_proxy) = &driver_state.srp_discovery_proxy {
            srp_discovery_proxy.remove_local_address(subnet.addr);
        }

        driver_state
            .ot_instance
            .ip6_remove_unicast_address(&subnet.addr)
            .ignore_not_found()
            .ignore_rejected()
            .context("on_netstack_removed_address")
    }

    pub(crate) async fn on_netstack_added_route(&self, subnet: Subnet) -> Result<(), Error> {
        debug!("Netstack added route: {:?} (ignored)", subnet);

        let erc = ot::ExternalRouteConfig::from_prefix(ot::Ip6Prefix::new(
            subnet.addr,
            subnet.prefix_len,
        ));

        let driver_state = self.driver_state.lock();

        driver_state
            .ot_instance
            .add_external_route(&erc)
            .ignore_already_exists()
            .context("on_netstack_added_route")
    }

    pub(crate) async fn on_netstack_removed_route(&self, subnet: Subnet) -> Result<(), Error> {
        debug!("Netstack removed route: {:?}", subnet);

        let driver_state = self.driver_state.lock();

        driver_state
            .ot_instance
            .remove_external_route(&ot::Ip6Prefix::new(subnet.addr, subnet.prefix_len))
            .ignore_not_found()
            .context("on_netstack_removed_route")
    }

    async fn on_netstack_packet_for_thread(&self, packet: Vec<u8>) -> Result<(), Error> {
        match IpFormat::for_packet(&packet) {
            IpFormat::Ipv4 => {}
            _ => {
                if !self.intercept_from_host(packet.as_slice()).await {
                    trace!("Outbound packet handled internally, dropping.");
                    return Ok(());
                }
            }
        };

        let driver_state = self.driver_state.lock();

        if driver_state.ot_instance.is_active_scan_in_progress() {
            info!("OpenThread Send Failed: Active scan in progress.");
        } else if driver_state.ot_instance.is_energy_scan_in_progress() {
            info!("OpenThread Send Failed: Energy scan in progress.");
        } else {
            let mut result = Ok(());
            match IpFormat::for_packet(&packet) {
                IpFormat::Ipv6 => {
                    result = driver_state.ot_instance.ip6_send_data(packet.as_slice());
                }
                IpFormat::Ipv4 => {
                    result = driver_state.ot_instance.nat64_send_data_slice(packet.as_slice());
                }
                _ => {
                    warn!("received unkonwn version of IP packet from netstack to thread");
                }
            }
            if let Err(err) = result {
                match err {
                    ot::Error::MessageDropped => {
                        info!("OpenThread dropped a packet due to packet processing rules.");
                    }
                    ot::Error::NoRoute => {
                        info!("OpenThread dropped a packet because there was no route to host.");
                    }
                    x => {
                        warn!("Send packet to OpenThread failed: \"{:?}\"", x);
                        debug!(
                            "Message Buffer Info: {:?}",
                            driver_state.ot_instance.get_buffer_info()
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

/// Outbound (Host-to-Thread) Traffic Handling.
impl<OT, NI, BI> OtDriver<OT, NI, BI>
where
    OT: ot::InstanceInterface + Send,
    NI: NetworkInterface,
    BI: BackboneInterface,
{
    /// Packet pump stream that pulls packets from the network interface,
    /// processes them, and then sends them to OpenThread.
    pub(crate) fn outbound_packet_pump(
        &self,
    ) -> impl TryStream<Ok = (), Error = Error> + Send + '_ {
        futures::stream::try_unfold((), move |()| {
            self.net_if
                .outbound_packet_from_stack()
                .map(|x: Result<Vec<u8>, Error>| x.map(|x| Some((x, ()))))
        })
        .and_then(move |packet| self.on_netstack_packet_for_thread(packet))
    }

    /// Processes the given IPv6 packet from the host and determines if it should
    /// be forwarded or not.
    ///
    /// Returns true if the packet should be forwarded to the thread network, false otherwise.
    async fn intercept_from_host(&self, mut packet_bytes: &[u8]) -> bool {
        const LINK_LOCAL_MDNS_MULTICAST_GROUP: Ipv6Addr =
            Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);
        const MDNS_PORT: u16 = 5353;
        const MDNS_MATCHER: Ipv6PacketMatcherRule = Ipv6PacketMatcherRule {
            proto: Some(Ipv6Proto::Proto(IpProto::Udp)),
            local_port: NonZeroU16::new(MDNS_PORT),
            local_address: Subnet { addr: Ipv6Addr::UNSPECIFIED, prefix_len: 0 },
            remote_port: NonZeroU16::new(MDNS_PORT),
            remote_address: Subnet { addr: LINK_LOCAL_MDNS_MULTICAST_GROUP, prefix_len: 128 },
        };

        // TODO(https://fxbug.dev/42175057): Once bug 93289 is addressed, we can remove this filter.
        if MDNS_MATCHER.match_outbound_packet(packet_bytes) {
            fn ascii_dump(data: &[u8]) -> String {
                let vec = data
                    .iter()
                    .map(|&x| if x.is_ascii() && x > 31 { x } else { b'.' })
                    .flat_map(std::ascii::escape_default)
                    .collect::<Vec<_>>();
                std::str::from_utf8(&vec).unwrap().to_string()
            }

            trace!(
                "Dropping unwanted mDNS traffic incorrectly destined for thread network: {}",
                ascii_dump(packet_bytes)
            );
            return false;
        }

        match Ipv6Packet::parse(&mut packet_bytes, ()) {
            #[allow(clippy::single_match)]
            Ok(packet) => match packet.proto() {
                Ipv6Proto::Icmpv6 => {
                    let args = IcmpParseArgs::new(packet.src_ip(), packet.dst_ip());
                    match Icmpv6Packet::parse(&mut packet_bytes, args) {
                        Ok(Icmpv6Packet::Mld(MldPacket::MulticastListenerReport(msg))) => {
                            let group =
                                std::net::Ipv6Addr::from(msg.body().group_addr.ipv6_bytes());

                            if let Err(err) = self.on_netstack_joined_multicast_group(group) {
                                warn!(
                                    "Netstack refused to join multicast group {:?}: {:?}",
                                    group, err
                                );
                            }

                            // Drop the packet
                            return false;
                        }
                        Ok(Icmpv6Packet::Mld(MldPacket::MulticastListenerDone(msg))) => {
                            let group =
                                std::net::Ipv6Addr::from(msg.body().group_addr.ipv6_bytes());

                            if let Err(err) = self.on_netstack_left_multicast_group(group) {
                                warn!(
                                    "Netstack refused to leave multicast group {:?}: {:?}",
                                    group, err
                                );
                            }

                            // Drop the packet
                            return false;
                        }
                        Ok(Icmpv6Packet::Ndp(NdpPacket::RouterAdvertisement(ra)))
                            if packet.dst_ip().segments()[0] == 0xFF02 =>
                        {
                            info!("Received ICMPv6 RA: {ra:?}");

                            let driver_state = self.driver_state.lock();

                            if let Err(ot::WrongSize) = driver_state
                                .ot_instance
                                .border_routing_process_icmp6_ra(packet.body())
                            {
                                warn!("ICMPv6 RA way too large")
                            }

                            // Drop the packet
                            return false;
                        }
                        Ok(Icmpv6Packet::Ndp(NdpPacket::RouterAdvertisement(ra))) => {
                            info!("Ignoring suspicious non-ll-multicast RA: {ra:?}");
                        }
                        Ok(_) => {}
                        Err(err) => {
                            warn!("Unable to parse ICMPv6 packet from host: {:?}", err);
                        }
                    }
                }
                _ => {}
            },

            Err(err_ipv6) => {
                debug!("Unable to parse IPv6 packet from host: {:?}", err_ipv6);

                // Drop the packet
                return false;
            }
        }

        // Pass the packet along to OpenThread.
        true
    }
}
