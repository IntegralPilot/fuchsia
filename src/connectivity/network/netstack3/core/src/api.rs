// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! Defines the main API entry objects for the exposed API from core.
use lock_order::Unlocked;
use net_types::ip::Ip;
use netstack3_base::{
    ContextPair as _, ContextProvider, CtxPair, TimerBindingsTypes, TimerHandler as _,
};
use netstack3_device::queue::{ReceiveQueueApi, TransmitQueueApi};
use netstack3_device::socket::DeviceSocketApi;
use netstack3_device::{DeviceAnyApi, DeviceApi};
use netstack3_filter::FilterApi;
use netstack3_icmp_echo::IcmpEchoSocketApi;
use netstack3_ip::device::{DeviceIpAnyApi, DeviceIpApi};
use netstack3_ip::multicast_forwarding::MulticastForwardingApi;
use netstack3_ip::nud::NeighborApi;
use netstack3_ip::raw::RawIpSocketApi;
use netstack3_ip::{RoutesAnyApi, RoutesApi};
use netstack3_tcp::TcpApi;
use netstack3_udp::UdpApi;

use crate::BindingsTypes;
use crate::context::CoreCtx;
use crate::counters::CountersApi;
use crate::time::TimerId;

type CoreApiCtxPair<'a, BP> = CtxPair<CoreCtx<'a, <BP as ContextProvider>::Context, Unlocked>, BP>;

/// The single entry point for function calls into netstack3 core.
pub struct CoreApi<'a, BP>(CoreApiCtxPair<'a, BP>)
where
    BP: ContextProvider,
    BP::Context: BindingsTypes;

impl<'a, BP> CoreApi<'a, BP>
where
    BP: ContextProvider,
    BP::Context: BindingsTypes,
{
    pub(crate) fn new(ctx_pair: CoreApiCtxPair<'a, BP>) -> Self {
        Self(ctx_pair)
    }

    /// Gets access to the UDP API for IP version `I`.
    pub fn udp<I: Ip>(self) -> UdpApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        UdpApi::new(ctx)
    }

    /// Gets access to the ICMP socket API for IP version `I`.
    pub fn icmp_echo<I: Ip>(self) -> IcmpEchoSocketApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        IcmpEchoSocketApi::new(ctx)
    }

    /// Gets access to the TCP API for IP version `I`.
    pub fn tcp<I: Ip>(self) -> TcpApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        TcpApi::new(ctx)
    }

    /// Gets access to the raw IP socket API.
    pub fn raw_ip_socket<I: Ip>(self) -> RawIpSocketApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        RawIpSocketApi::new(ctx)
    }

    /// Gets access to the device socket API.
    pub fn device_socket(self) -> DeviceSocketApi<CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        DeviceSocketApi::new(ctx)
    }

    /// Gets access to the filtering API.
    pub fn filter(self) -> FilterApi<CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        FilterApi::new(ctx)
    }

    /// Gets access to the routes API for IP version `I`.
    pub fn routes<I: Ip>(self) -> RoutesApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        RoutesApi::new(ctx)
    }

    /// Gets access to the routes API for all IP versions.
    pub fn routes_any(self) -> RoutesAnyApi<CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        RoutesAnyApi::new(ctx)
    }

    /// Gets access to the multicast forwarding API for IP version `I`.
    pub fn multicast_forwarding<I: Ip>(self) -> MulticastForwardingApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        MulticastForwardingApi::new(ctx)
    }

    /// Gets access to the neighbor API for IP version `I` and device `D`.
    pub fn neighbor<I: Ip, D>(self) -> NeighborApi<I, D, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        NeighborApi::new(ctx)
    }

    /// Gets access to the device API for device type `D`.
    pub fn device<D>(self) -> DeviceApi<D, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        DeviceApi::new(ctx)
    }

    /// Gets access to the device API for all device types.
    pub fn device_any(self) -> DeviceAnyApi<CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        DeviceAnyApi::new(ctx)
    }

    /// Gets access to the device IP API for IP version `I``.
    pub fn device_ip<I: Ip>(self) -> DeviceIpApi<I, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        DeviceIpApi::new(ctx)
    }

    /// Gets access to the device IP API for all IP versions.
    pub fn device_ip_any(self) -> DeviceIpAnyApi<CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        DeviceIpAnyApi::new(ctx)
    }

    /// Gets access to the transmit queue API for device type `D`.
    pub fn transmit_queue<D>(self) -> TransmitQueueApi<D, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        TransmitQueueApi::new(ctx)
    }

    /// Gets access to the receive queue API for device type `D`.
    pub fn receive_queue<D>(self) -> ReceiveQueueApi<D, CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        ReceiveQueueApi::new(ctx)
    }

    /// Gets access to the counters API.
    pub fn counters(self) -> CountersApi<CoreApiCtxPair<'a, BP>> {
        let Self(ctx) = self;
        CountersApi::new(ctx)
    }

    /// Handles a timer.
    pub fn handle_timer(
        &mut self,
        dispatch: TimerId<BP::Context>,
        timer: <BP::Context as TimerBindingsTypes>::UniqueTimerId,
    ) where
        BP::Context: crate::BindingsContext,
    {
        let Self(ctx) = self;
        let (core_ctx, bindings_ctx) = ctx.contexts();

        core_ctx.handle_timer(bindings_ctx, dispatch, timer)
    }
}
