// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::prelude_internal::*;
use crate::spinel::Subnet;
use anyhow::Error;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::num::NonZeroU64;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum NetworkInterfaceEvent {
    InterfaceEnabledChanged(bool),
    AddressWasAdded(Subnet),
    AddressWasRemoved(Subnet),
    RouteToSubnetProvided(Subnet),
    RouteToSubnetRevoked(Subnet),
    InterfaceRemoved(),
    InterfaceAdded(),
}

#[async_trait()]
pub trait NetworkInterface: Send + Sync {
    /// Returns the network interface index.
    fn get_index(&self) -> u64;

    /// Returns the network interface index.
    fn get_nicid(&self) -> NonZeroU64;

    /// Blocks until the network stack has a packet to send.
    async fn outbound_packet_from_stack(&self) -> Result<Vec<u8>, Error>;

    /// Provides the given IPv6 packet to the network stack.
    async fn inbound_packet_to_stack(
        &self,
        packet_in: &[u8],
        frame_type: fidl_fuchsia_hardware_network::FrameType,
    ) -> Result<(), Error>;

    /// Changes the online status of the network interface. If an interface is
    /// enabled, the stack won't start handling packets until it is marked as online.
    /// This is controlled by the driver.
    async fn set_online(&self, online: bool) -> Result<(), Error>;

    /// Changes the enabled state of the network interface. An interface must
    /// be both online and enabled in order for packets to be handled.
    /// This is generally controlled by the administrator.
    async fn set_enabled(&self, enabled: bool) -> Result<(), Error>;

    /// Adds the given IPv6 address of Spinel::Subnet type to this interface.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn add_address_from_spinel_subnet(&self, addr: &Subnet) -> Result<(), Error>;

    /// Adds the given address to this interface.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn add_address(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error>;

    /// Removes the given address of Spinel::Subnet type from this interface.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn remove_address_from_spinel_subnet(&self, addr: &Subnet) -> Result<(), Error>;

    /// Removes the given address from this interface.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn remove_address(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error>;

    /// Adds the forward entry to the interface per subnet info.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn add_forwarding_entry(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error>;

    /// Removes the forward entry to the interface per subnet info.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn remove_forwarding_entry(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error>;

    /// Indicates to the net stack that this subnet is accessible through this interface.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn add_external_route(&self, addr: &Subnet) -> Result<(), Error>;

    /// Removes the given subnet from being considered routable over this interface.
    // TODO(https://fxbug.dev/42143339): Consider making this method async. This method is
    //       currently synchronous so that it can be used directly from
    //       `Driver::on_prop_value_is`, which is also synchronous.
    fn remove_external_route(&self, addr: &Subnet) -> Result<(), Error>;

    /// Has the interface join the given multicast group.
    fn join_mcast_group(&self, addr: &std::net::Ipv6Addr) -> Result<(), Error>;

    /// Has the interface leave the given multicast group.
    fn leave_mcast_group(&self, addr: &std::net::Ipv6Addr) -> Result<(), Error>;

    /// Gets the event stream for this network interface.
    ///
    /// Calling this method more than once will cause a panic.
    fn take_event_stream(&self) -> BoxStream<'_, Result<NetworkInterfaceEvent, Error>>;

    /// Set the ipv6 packet forwarding for lowpan interface
    async fn set_ipv6_forwarding_enabled(&self, enabled: bool) -> Result<(), Error>;

    /// Set the ipv4 packet forwarding for lowpan interface
    async fn set_ipv4_forwarding_enabled(&self, enabled: bool) -> Result<(), Error>;
}

use fuchsia_sync::Mutex;
use futures::channel::mpsc;

pub struct DummyNetworkInterface {
    event_receiver: Mutex<Option<mpsc::Receiver<Result<NetworkInterfaceEvent, Error>>>>,
    event_sender: Mutex<mpsc::Sender<Result<NetworkInterfaceEvent, Error>>>,
}

impl std::fmt::Debug for DummyNetworkInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.write_str("DummyNetworkInterface")
    }
}

impl Default for DummyNetworkInterface {
    fn default() -> Self {
        let (event_sender, event_receiver) = mpsc::channel(10);
        DummyNetworkInterface {
            event_receiver: Mutex::new(Some(event_receiver)),
            event_sender: Mutex::new(event_sender),
        }
    }
}

#[async_trait]
impl NetworkInterface for DummyNetworkInterface {
    fn get_index(&self) -> u64 {
        3
    }

    fn get_nicid(&self) -> NonZeroU64 {
        NonZeroU64::new(3).unwrap()
    }

    async fn outbound_packet_from_stack(&self) -> Result<Vec<u8>, Error> {
        futures::future::pending().await
    }

    async fn inbound_packet_to_stack(
        &self,
        packet: &[u8],
        frame_type: fidl_fuchsia_hardware_network::FrameType,
    ) -> Result<(), Error> {
        info!("Packet to Stack: {}, frame_type: {:?}", hex::encode(packet), frame_type);
        Ok(())
    }

    async fn set_online(&self, online: bool) -> Result<(), Error> {
        info!("Interface online: {:?}", online);
        Ok(())
    }

    async fn set_enabled(&self, enabled: bool) -> Result<(), Error> {
        info!("Interface enabled: {:?}", enabled);
        self.event_sender
            .lock()
            .try_send(Ok(NetworkInterfaceEvent::InterfaceEnabledChanged(enabled)))?;
        Ok(())
    }

    fn add_address_from_spinel_subnet(&self, addr: &Subnet) -> Result<(), Error> {
        info!("Address Added: {:?}", addr);
        Ok(())
    }

    fn add_address(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        info!("Address Added: {:?}", addr);
        Ok(())
    }

    fn remove_address_from_spinel_subnet(&self, addr: &Subnet) -> Result<(), Error> {
        info!("Address Removed: {:?}", addr);
        Ok(())
    }

    fn remove_address(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        info!("Address Removed: {:?}", addr);
        Ok(())
    }

    fn add_forwarding_entry(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        info!("Forward Entry Added: {:?}", addr);
        Ok(())
    }

    fn remove_forwarding_entry(&self, addr: fidl_fuchsia_net::Subnet) -> Result<(), Error> {
        info!("Forward Entry Removed: {:?}", addr);
        Ok(())
    }

    fn add_external_route(&self, addr: &Subnet) -> Result<(), Error> {
        info!("External Route Added: {:?}", addr);
        Ok(())
    }

    fn remove_external_route(&self, addr: &Subnet) -> Result<(), Error> {
        info!("External Route Removed: {:?}", addr);
        Ok(())
    }

    fn join_mcast_group(&self, addr: &std::net::Ipv6Addr) -> Result<(), Error> {
        info!("Joining multicast group: {:?}", addr);
        Ok(())
    }

    fn leave_mcast_group(&self, addr: &std::net::Ipv6Addr) -> Result<(), Error> {
        info!("Leaving multicast group: {:?}", addr);
        Ok(())
    }

    fn take_event_stream(&self) -> BoxStream<'_, Result<NetworkInterfaceEvent, Error>> {
        self.event_receiver.lock().take().expect("take_event_stream called twice").boxed()
    }

    async fn set_ipv6_forwarding_enabled(&self, _enabled: bool) -> Result<(), Error> {
        Ok(())
    }

    async fn set_ipv4_forwarding_enabled(&self, _enabled: bool) -> Result<(), Error> {
        Ok(())
    }
}
