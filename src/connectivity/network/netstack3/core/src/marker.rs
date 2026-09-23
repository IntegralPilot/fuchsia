// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Marker traits with blanket implementations.
//!
//! Traits in this module exist to be exported as markers to bindings without
//! exposing the internal traits directly.

use net_types::ip::{Ipv4, Ipv6};
use netstack3_base::{
    AnyDevice, DeviceIdContext, InstantBindingsTypes, ReferenceNotifiers, RngContext,
    TimerBindingsTypes, TxMetadataBindingsTypes,
};
use netstack3_datagram as datagram;
use netstack3_device::ethernet::{
    EthernetDeviceEventBindingsContext, EthernetDeviceId, EthernetLinkDevice, EthernetWeakDeviceId,
};
use netstack3_device::{self as device, DeviceId, DeviceLayerTypes, WeakDeviceId};
use netstack3_filter::{FilterBindingsContext, FilterBindingsTypes, SocketOpsFilterBindingContext};
use netstack3_icmp_echo::{IcmpEchoBindingsContext, IcmpEchoBindingsTypes, IcmpEchoStateContext};
use netstack3_ip::device::{
    IpDeviceBindingsContext, IpDeviceConfigurationContext, IpDeviceConfigurationHandler,
    IpDeviceIpExt,
};
use netstack3_ip::icmp::{IcmpBindingsContext, IcmpBindingsTypes};
use netstack3_ip::multicast_forwarding::{
    MulticastForwardingBindingsContext, MulticastForwardingBindingsTypes,
    MulticastForwardingStateContext,
};
use netstack3_ip::nud::{NudBindingsContext, NudContext};
use netstack3_ip::raw::{
    RawIpSocketMapContext, RawIpSocketStateContext, RawIpSocketsBindingsContext,
    RawIpSocketsBindingsTypes,
};
use netstack3_ip::socket::IpSocketContext;
use netstack3_ip::{
    self as ip, IpLayerBindingsContext, IpLayerContext, IpLayerIpExt, IpRoutingBindingsTypes,
    NdpBindingsContext,
};
use netstack3_tcp::{self as tcp, TcpBindingsContext, TcpBindingsTypes, TcpContext};
use netstack3_udp::{self as udp, UdpBindingsContext, UdpBindingsTypes, UdpCounterContext};

use crate::TimerId;
use crate::transport::CoreTxMetadata;

/// A marker for extensions to IP types.
///
/// This trait acts as a marker for [`BaseIpExt`] for both `Self` and
/// `Self::OtherVersion`.
pub trait IpExt: BaseIpExt + datagram::DualStackIpExt<OtherVersion: BaseIpExt> {}

impl<I> IpExt for I where I: BaseIpExt + datagram::DualStackIpExt<OtherVersion: BaseIpExt> {}

/// A marker for extensions to IP types.
pub trait BaseIpExt:
    IpLayerIpExt
    + IpDeviceIpExt
    + netstack3_base::IcmpIpExt
    + ip::device::IpDeviceIpExt
    + tcp::DualStackIpExt
    + datagram::DualStackIpExt
{
}

impl<I> BaseIpExt for I where
    I: ip::IpLayerIpExt
        + IpDeviceIpExt
        + netstack3_base::IcmpIpExt
        + ip::device::IpDeviceIpExt
        + tcp::DualStackIpExt
        + datagram::DualStackIpExt
{
}

/// A marker trait for core context implementations.
///
/// This trait allows bindings to express trait bounds on routines that have IP
/// type parameters. It is an umbrella of all the core contexts that must be
/// implemented by [`crate::context::UnlockedCoreCtx`] to satisfy all the API
/// objects vended by [`crate::api::CoreApi`].
pub trait CoreContext<I, BC>:
    udp::StateContext<I, BC>
    + UdpCounterContext<I, WeakDeviceId<BC>, BC>
    + TcpContext<I, BC>
    + IcmpEchoStateContext<I, BC>
    + ip::icmp::IcmpStateContext
    + IpLayerContext<I, BC>
    + NudContext<I, EthernetLinkDevice, BC>
    + IpDeviceConfigurationContext<I, BC>
    + IpDeviceConfigurationHandler<I, BC>
    + IpSocketContext<I, BC>
    + DeviceIdContext<AnyDevice, DeviceId = DeviceId<BC>, WeakDeviceId = WeakDeviceId<BC>>
    + DeviceIdContext<
        EthernetLinkDevice,
        DeviceId = EthernetDeviceId<BC>,
        WeakDeviceId = EthernetWeakDeviceId<BC>,
    > + RawIpSocketMapContext<I, BC>
    + RawIpSocketStateContext<I, BC>
    + MulticastForwardingStateContext<I, BC>
where
    I: IpExt,
    BC: IpBindingsContext<I>,
{
}

impl<I, BC, O> CoreContext<I, BC> for O
where
    I: IpExt,
    BC: IpBindingsContext<I>,
    O: udp::StateContext<I, BC>
        + UdpCounterContext<I, WeakDeviceId<BC>, BC>
        + TcpContext<I, BC>
        + IcmpEchoStateContext<I, BC>
        + ip::icmp::IcmpStateContext
        + IpLayerContext<I, BC>
        + NudContext<I, EthernetLinkDevice, BC>
        + IpDeviceConfigurationContext<I, BC>
        + IpDeviceConfigurationHandler<I, BC>
        + IpSocketContext<I, BC>
        + DeviceIdContext<AnyDevice, DeviceId = DeviceId<BC>, WeakDeviceId = WeakDeviceId<BC>>
        + DeviceIdContext<
            EthernetLinkDevice,
            DeviceId = EthernetDeviceId<BC>,
            WeakDeviceId = EthernetWeakDeviceId<BC>,
        > + RawIpSocketMapContext<I, BC>
        + RawIpSocketStateContext<I, BC>
        + MulticastForwardingStateContext<I, BC>,
{
}

/// A marker trait for all the types stored in core objects that are specified
/// by bindings.
pub trait BindingsTypes:
    InstantBindingsTypes
    + DeviceLayerTypes
    + TcpBindingsTypes
    + FilterBindingsTypes
    + IcmpEchoBindingsTypes
    + IcmpBindingsTypes
    + MulticastForwardingBindingsTypes
    + RawIpSocketsBindingsTypes
    + UdpBindingsTypes
    + TimerBindingsTypes<DispatchId = TimerId<Self>>
    + TxMetadataBindingsTypes<TxMetadata = CoreTxMetadata<Self>>
    + IpRoutingBindingsTypes
{
}

impl<O> BindingsTypes for O where
    O: InstantBindingsTypes
        + DeviceLayerTypes
        + TcpBindingsTypes
        + FilterBindingsTypes
        + IcmpEchoBindingsTypes
        + IcmpBindingsTypes
        + MulticastForwardingBindingsTypes
        + RawIpSocketsBindingsTypes
        + UdpBindingsTypes
        + TimerBindingsTypes<DispatchId = TimerId<Self>>
        + TxMetadataBindingsTypes<TxMetadata = CoreTxMetadata<Self>>
        + IpRoutingBindingsTypes
{
}

/// The execution context provided by bindings for a given IP version.
pub trait IpBindingsContext<I: IpExt>:
    BindingsTypes
    + RngContext
    + UdpBindingsContext<I, DeviceId<Self>>
    + TcpBindingsContext<DeviceId<Self>>
    + FilterBindingsContext<DeviceId<Self>>
    + SocketOpsFilterBindingContext<DeviceId<Self>>
    + IcmpBindingsContext
    + IcmpEchoBindingsContext<I, DeviceId<Self>>
    + MulticastForwardingBindingsContext<I, DeviceId<Self>>
    + RawIpSocketsBindingsContext<I, DeviceId<Self>>
    + IpDeviceBindingsContext<I, DeviceId<Self>>
    + IpLayerBindingsContext<I, DeviceId<Self>>
    + NudBindingsContext<I, EthernetLinkDevice, EthernetDeviceId<Self>>
    + device::DeviceLayerEventDispatcher
    + device::socket::DeviceSocketBindingsContext<DeviceId<Self>>
    + ReferenceNotifiers
    + 'static
{
}

impl<I, BC> IpBindingsContext<I> for BC
where
    I: IpExt,
    BC: BindingsTypes
        + RngContext
        + UdpBindingsContext<I, DeviceId<Self>>
        + TcpBindingsContext<DeviceId<Self>>
        + FilterBindingsContext<DeviceId<Self>>
        + SocketOpsFilterBindingContext<DeviceId<Self>>
        + IcmpBindingsContext
        + IcmpEchoBindingsContext<I, DeviceId<Self>>
        + MulticastForwardingBindingsContext<I, DeviceId<Self>>
        + RawIpSocketsBindingsContext<I, DeviceId<Self>>
        + IpDeviceBindingsContext<I, DeviceId<Self>>
        + IpLayerBindingsContext<I, DeviceId<Self>>
        + NudBindingsContext<I, EthernetLinkDevice, EthernetDeviceId<Self>>
        + device::DeviceLayerEventDispatcher
        + device::socket::DeviceSocketBindingsContext<DeviceId<Self>>
        + ReferenceNotifiers
        + 'static,
{
}

/// The execution context provided by bindings.
pub trait BindingsContext:
    IpBindingsContext<Ipv4>
    + IpBindingsContext<Ipv6>
    + NdpBindingsContext<DeviceId<Self>>
    + EthernetDeviceEventBindingsContext<EthernetDeviceId<Self>>
{
}
impl<BC> BindingsContext for BC where
    BC: IpBindingsContext<Ipv4>
        + IpBindingsContext<Ipv6>
        + NdpBindingsContext<DeviceId<Self>>
        + EthernetDeviceEventBindingsContext<EthernetDeviceId<Self>>
{
}
