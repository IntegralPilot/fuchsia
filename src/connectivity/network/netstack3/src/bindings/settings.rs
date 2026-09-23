// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides workers and data structures for netstack-wide settings.

use std::num::{NonZeroUsize, TryFromIntError};
use std::ops::Deref;

use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_settings as fnet_settings;
use futures::TryStreamExt as _;
use log::warn;
use netstack3_core::tcp::TcpSettings;
use netstack3_core::types::{BufferSizeSettings, PositiveIsize};
use netstack3_core::{MapDerefExt as _, SettingsContext};
use once_cell::sync::Lazy;

use crate::bindings::interface_config::{
    DeviceNeighborConfig, FidlInterfaceConfig, InterfaceConfig, InterfaceConfigDefaults,
};
use crate::bindings::util::rcu::{self, SynchronizedWriterRcu};
use crate::bindings::util::{self, ErrorLogExt, IllegalZeroValueError, IntoFidl, ResultExt as _};
use crate::bindings::{BindingsCtx, Ctx};

pub(crate) async fn serve_control(
    ctx: Ctx,
    mut rs: fnet_settings::ControlRequestStream,
) -> Result<(), fidl::Error> {
    while let Some(req) = rs.try_next().await? {
        match req {
            fnet_settings::ControlRequest::UpdateInterfaceDefaults { payload, responder } => {
                let r = update_interface_defaults(&ctx.bindings_ctx().settings, payload);
                r.log_if_err("settings::UpdateInterfaceDefaults");
                responder.send(r.borrowed_ok())?;
            }
            fnet_settings::ControlRequest::UpdateTcp { payload, responder } => {
                let mut write = ctx.bindings_ctx().settings.tcp.write();
                let r = update_tcp(payload, &mut write);
                discard_changes_on_err("settings::UpdateTcp", &r, write);
                responder.send(r.borrowed_ok())?;
            }
            fnet_settings::ControlRequest::UpdateUdp { payload, responder } => {
                let mut write = ctx.bindings_ctx().settings.udp.write();
                let r = update_udp(payload, &mut write);
                discard_changes_on_err("settings::UpdateUdp", &r, write);
                responder.send(r.borrowed_ok())?;
            }
            fnet_settings::ControlRequest::UpdateIcmp { payload, responder } => {
                let mut write = ctx.bindings_ctx().settings.icmp.write();
                let r = update_icmp(payload, &mut write);
                discard_changes_on_err("settings::UpdateIcmp", &r, write);
                responder.send(r.borrowed_ok())?;
            }
            fnet_settings::ControlRequest::UpdateIp { payload, responder } => {
                let mut write = ctx.bindings_ctx().settings.ip.write();
                let r = update_ip(payload, &mut write);
                discard_changes_on_err("settings::UpdateIp", &r, write);
                responder.send(r.borrowed_ok())?;
            }
            fnet_settings::ControlRequest::UpdateDevice { payload, responder } => {
                let mut write = ctx.bindings_ctx().settings.device.write();
                let r = update_device(payload, &mut write);
                discard_changes_on_err("settings::UpdateDevice", &r, write);
                responder.send(r.borrowed_ok())?;
            }
        }
    }
    Ok(())
}

impl ErrorLogExt for fnet_settings::UpdateError {
    fn log_level(&self) -> log::Level {
        log::Level::Warn
    }
}

impl From<IllegalZeroValueError> for fnet_settings::UpdateError {
    fn from(IllegalZeroValueError: IllegalZeroValueError) -> Self {
        Self::IllegalZeroValue
    }
}

#[inline]
#[track_caller]
fn discard_changes_on_err<'a, T, O>(
    msg: &'static str,
    result: &Result<O, fnet_settings::UpdateError>,
    guard: rcu::WriteGuard<'a, T>,
) {
    match result {
        Ok(_) => {}
        Err(e) => {
            guard.discard();
            e.log(msg);
        }
    }
}

fn update_interface_defaults(
    settings: &Settings,
    interfaces: fnet_interfaces_admin::Configuration,
) -> Result<fnet_interfaces_admin::Configuration, fnet_settings::UpdateError> {
    let update = FidlInterfaceConfig::from(interfaces).try_into_update()?;
    let changed = settings.interface_defaults.write().update(update);
    Ok(FidlInterfaceConfig::new_update(changed).into())
}

fn update_tcp(
    tcp: fnet_settings::Tcp,
    settings: &mut TcpSettings,
) -> Result<fnet_settings::Tcp, fnet_settings::UpdateError> {
    let fnet_settings::Tcp { buffer_sizes, __source_breaking } = tcp;
    let mut prev = fnet_settings::Tcp::default();
    if let Some(buffer_sizes) = buffer_sizes {
        let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } = buffer_sizes;
        let mut buffer_sizes = fnet_settings::SocketBufferSizes::default();
        if let Some(send) = send {
            buffer_sizes.send = Some(update_buffer_sizes(&mut settings.send_buffer, send)?);
        }
        if let Some(receive) = receive {
            buffer_sizes.receive =
                Some(update_buffer_sizes(&mut settings.receive_buffer, receive)?);
        }

        prev.buffer_sizes = util::some_if_not_default(buffer_sizes);
    }
    Ok(prev)
}

fn update_udp(
    udp: fnet_settings::Udp,
    settings: &mut UdpSettings,
) -> Result<fnet_settings::Udp, fnet_settings::UpdateError> {
    let fnet_settings::Udp { buffer_sizes, __source_breaking } = udp;
    let mut prev = fnet_settings::Udp::default();
    if let Some(buffer_sizes) = buffer_sizes {
        let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } = buffer_sizes;
        let mut buffer_sizes = fnet_settings::SocketBufferSizes::default();
        if let Some(send) = send {
            buffer_sizes.send =
                Some(update_buffer_sizes(&mut settings.core.datagram.send_buffer, send)?);
        }
        if let Some(receive) = receive {
            buffer_sizes.receive =
                Some(update_buffer_sizes(&mut settings.receive_buffer, receive)?);
        }
        prev.buffer_sizes = util::some_if_not_default(buffer_sizes);
    }
    Ok(prev)
}

fn update_icmp(
    icmp: fnet_settings::Icmp,
    settings: &mut IcmpSettings,
) -> Result<fnet_settings::Icmp, fnet_settings::UpdateError> {
    let fnet_settings::Icmp { echo_buffer_sizes, icmpv4, icmpv6, __source_breaking } = icmp;
    let mut prev = fnet_settings::Icmp::default();
    if let Some(buffer_sizes) = echo_buffer_sizes {
        let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } = buffer_sizes;
        let mut buffer_sizes = fnet_settings::SocketBufferSizes::default();
        if let Some(send) = send {
            buffer_sizes.send =
                Some(update_buffer_sizes(&mut settings.echo_core.datagram.send_buffer, send)?);
        }
        if let Some(receive) = receive {
            buffer_sizes.receive =
                Some(update_buffer_sizes(&mut settings.echo_receive_buffer, receive)?);
        }
        prev.echo_buffer_sizes = util::some_if_not_default(buffer_sizes);
    }
    if let Some(fnet_settings::Icmpv4 { __source_breaking }) = icmpv4 {}
    if let Some(fnet_settings::Icmpv6 { __source_breaking }) = icmpv6 {}
    Ok(prev)
}

fn update_ip(
    ip: fnet_settings::Ip,
    settings: &mut IpLayerSettings,
) -> Result<fnet_settings::Ip, fnet_settings::UpdateError> {
    let fnet_settings::Ip { raw_buffer_sizes, ipv4, ipv6, __source_breaking } = ip;
    let mut prev = fnet_settings::Ip::default();
    if let Some(buffer_sizes) = raw_buffer_sizes {
        let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } = buffer_sizes;
        let mut buffer_sizes = fnet_settings::SocketBufferSizes::default();
        if let Some(_send) = send {
            warn!("TODO(https://fxbug/dev/392111277): Support SNDBUF for raw IP sockets");
        }
        if let Some(receive) = receive {
            buffer_sizes.receive =
                Some(update_buffer_sizes(&mut settings.raw_receive_buffer, receive)?);
        }
        prev.raw_buffer_sizes = util::some_if_not_default(buffer_sizes);
    }
    if let Some(fnet_settings::Ipv4 { __source_breaking }) = ipv4 {}
    if let Some(fnet_settings::Ipv6 { __source_breaking }) = ipv6 {}
    Ok(prev)
}

fn update_device(
    device: fnet_settings::Device,
    settings: &mut DeviceLayerSettings,
) -> Result<fnet_settings::Device, fnet_settings::UpdateError> {
    let fnet_settings::Device { packet_buffer_sizes, __source_breaking } = device;
    let mut prev = fnet_settings::Device::default();
    if let Some(buffer_sizes) = packet_buffer_sizes {
        let fnet_settings::SocketBufferSizes { send, receive, __source_breaking } = buffer_sizes;
        let mut buffer_sizes = fnet_settings::SocketBufferSizes::default();
        if let Some(_send) = send {
            warn!("TODO(https://fxbug/dev/391946195): Support SNDBUF for packet sockets");
        }
        if let Some(receive) = receive {
            buffer_sizes.receive =
                Some(update_buffer_sizes(&mut settings.packet_receive_buffer, receive)?);
        }
        prev.packet_buffer_sizes = util::some_if_not_default(buffer_sizes);
    }
    Ok(prev)
}

pub(crate) async fn serve_state(
    ctx: Ctx,
    mut rs: fnet_settings::StateRequestStream,
) -> Result<(), fidl::Error> {
    while let Some(req) = rs.try_next().await? {
        match req {
            fnet_settings::StateRequest::GetInterfaceDefaults { responder } => {
                responder.send(&get_interface_defaults(&ctx.bindings_ctx().settings))?;
            }
            fnet_settings::StateRequest::GetTcp { responder } => {
                responder.send(&get_tcp(&ctx.bindings_ctx().settings.tcp.read()))?;
            }
            fnet_settings::StateRequest::GetUdp { responder } => {
                responder.send(&get_udp(&ctx.bindings_ctx().settings.udp.read()))?;
            }
            fnet_settings::StateRequest::GetIcmp { responder } => {
                responder.send(&get_icmp(&ctx.bindings_ctx().settings.icmp.read()))?;
            }
            fnet_settings::StateRequest::GetIp { responder } => {
                responder.send(&get_ip(&ctx.bindings_ctx().settings.ip.read()))?;
            }
            fnet_settings::StateRequest::GetDevice { responder } => {
                responder.send(&get_device(&ctx.bindings_ctx().settings.device.read()))?;
            }
        }
    }
    Ok(())
}

fn get_interface_defaults(settings: &Settings) -> fnet_interfaces_admin::Configuration {
    FidlInterfaceConfig::new_complete(settings.interface_defaults.read().clone()).into()
}

fn get_tcp(settings: &TcpSettings) -> fnet_settings::Tcp {
    let buffer_sizes = fnet_settings::SocketBufferSizes {
        send: Some(settings.send_buffer.into_fidl()),
        receive: Some(settings.receive_buffer.into_fidl()),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    fnet_settings::Tcp {
        buffer_sizes: Some(buffer_sizes),
        __source_breaking: fidl::marker::SourceBreaking,
    }
}

fn get_udp(settings: &UdpSettings) -> fnet_settings::Udp {
    let buffer_sizes = fnet_settings::SocketBufferSizes {
        send: Some(settings.core.datagram.send_buffer.into_fidl()),
        receive: Some(settings.receive_buffer.into_fidl()),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    fnet_settings::Udp {
        buffer_sizes: Some(buffer_sizes),
        __source_breaking: fidl::marker::SourceBreaking,
    }
}

fn get_icmp(settings: &IcmpSettings) -> fnet_settings::Icmp {
    let echo_buffer_sizes = fnet_settings::SocketBufferSizes {
        send: Some(settings.echo_core.datagram.send_buffer.into_fidl()),
        receive: Some(settings.echo_receive_buffer.into_fidl()),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let icmpv4 = fnet_settings::Icmpv4 { __source_breaking: fidl::marker::SourceBreaking };
    let icmpv6 = fnet_settings::Icmpv6 { __source_breaking: fidl::marker::SourceBreaking };
    fnet_settings::Icmp {
        echo_buffer_sizes: Some(echo_buffer_sizes),
        icmpv4: Some(icmpv4),
        icmpv6: Some(icmpv6),
        __source_breaking: fidl::marker::SourceBreaking,
    }
}

fn get_ip(settings: &IpLayerSettings) -> fnet_settings::Ip {
    let raw_buffer_sizes = fnet_settings::SocketBufferSizes {
        // TODO(https://fxbug/dev/392111277): Support SNDBUF for raw IP sockets.
        send: None,
        receive: Some(settings.raw_receive_buffer.into_fidl()),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    let ipv4 = fnet_settings::Ipv4 { __source_breaking: fidl::marker::SourceBreaking };
    let ipv6 = fnet_settings::Ipv6 { __source_breaking: fidl::marker::SourceBreaking };
    fnet_settings::Ip {
        raw_buffer_sizes: Some(raw_buffer_sizes),
        ipv4: Some(ipv4),
        ipv6: Some(ipv6),
        __source_breaking: fidl::marker::SourceBreaking,
    }
}

fn get_device(settings: &DeviceLayerSettings) -> fnet_settings::Device {
    let packet_buffer_sizes = fnet_settings::SocketBufferSizes {
        // TODO(https://fxbug/dev/391946195): Support SNDBUF for packet sockets.
        send: None,
        receive: Some(settings.packet_receive_buffer.into_fidl()),
        __source_breaking: fidl::marker::SourceBreaking,
    };
    fnet_settings::Device {
        packet_buffer_sizes: Some(packet_buffer_sizes),
        __source_breaking: fidl::marker::SourceBreaking,
    }
}

pub(crate) struct Settings {
    pub(crate) interface_defaults: SynchronizedWriterRcu<InterfaceConfig<DeviceNeighborConfig>>,
    pub(crate) tcp: SynchronizedWriterRcu<TcpSettings>,
    pub(crate) udp: SynchronizedWriterRcu<UdpSettings>,
    pub(crate) icmp: SynchronizedWriterRcu<IcmpSettings>,
    pub(crate) device: SynchronizedWriterRcu<DeviceLayerSettings>,
    pub(crate) ip: SynchronizedWriterRcu<IpLayerSettings>,
}

impl Settings {
    pub(crate) fn new(interface: &InterfaceConfigDefaults) -> Self {
        Self {
            interface_defaults: SynchronizedWriterRcu::new(InterfaceConfig::new(interface)),
            tcp: SynchronizedWriterRcu::new(default_tcp_settings()),
            udp: Default::default(),
            icmp: Default::default(),
            device: Default::default(),
            ip: Default::default(),
        }
    }
}

fn default_tcp_settings() -> TcpSettings {
    static ZIRCON_SOCKET_BUFFER_SIZE: Lazy<NonZeroUsize> = Lazy::new(|| {
        let (local, _peer) = zx::Socket::create_stream();
        NonZeroUsize::new(local.info().unwrap().tx_buf_max).unwrap()
    });
    // Borrowed from netstack2.
    const MAX_BUFFER_SIZE: NonZeroUsize = NonZeroUsize::new(4 << 20).unwrap();

    // Borrowed from Linux: https://man7.org/linux/man-pages/man7/socket.7.html
    const RCVBUF_MIN: NonZeroUsize = NonZeroUsize::new(256).unwrap();
    let receive_buffer =
        BufferSizeSettings::new(RCVBUF_MIN, *ZIRCON_SOCKET_BUFFER_SIZE, MAX_BUFFER_SIZE).unwrap();

    // Borrowed from Linux: https://man7.org/linux/man-pages/man7/socket.7.html
    const SNDBUF_MIN: NonZeroUsize = NonZeroUsize::new(2048).unwrap();
    let send_buffer =
        BufferSizeSettings::new(SNDBUF_MIN, *ZIRCON_SOCKET_BUFFER_SIZE, MAX_BUFFER_SIZE).unwrap();

    TcpSettings { receive_buffer, send_buffer }
}

fn default_dgram_rcvbuf_sizes() -> BufferSizeSettings<NonZeroUsize> {
    // These values were picked to match Linux defaults.
    const DEFAULT_DATAGRAM_MIN_RCVBUF: NonZeroUsize = NonZeroUsize::new(256).unwrap();
    const DEFAULT_DATAGRAM_DEFAULT_RCVBUF: NonZeroUsize = NonZeroUsize::new(208 * 1024).unwrap();
    const DEFAULT_DATAGRAM_MAX_RCVBUF: NonZeroUsize = NonZeroUsize::new(4 * 1024 * 1024).unwrap();
    BufferSizeSettings::new(
        DEFAULT_DATAGRAM_MIN_RCVBUF,
        DEFAULT_DATAGRAM_DEFAULT_RCVBUF,
        DEFAULT_DATAGRAM_MAX_RCVBUF,
    )
    .unwrap()
}

#[derive(Clone)]
pub(crate) struct UdpSettings {
    core: netstack3_core::udp::UdpSettings,
    pub(crate) receive_buffer: BufferSizeSettings<NonZeroUsize>,
}

impl Default for UdpSettings {
    fn default() -> Self {
        Self { core: Default::default(), receive_buffer: default_dgram_rcvbuf_sizes() }
    }
}

#[derive(Clone)]
pub(crate) struct IcmpSettings {
    echo_core: netstack3_core::icmp::IcmpEchoSettings,
    pub(crate) echo_receive_buffer: BufferSizeSettings<NonZeroUsize>,
}

impl Default for IcmpSettings {
    fn default() -> Self {
        Self { echo_core: Default::default(), echo_receive_buffer: default_dgram_rcvbuf_sizes() }
    }
}

#[derive(Clone)]
pub(crate) struct DeviceLayerSettings {
    pub(crate) packet_receive_buffer: BufferSizeSettings<NonZeroUsize>,
}

impl Default for DeviceLayerSettings {
    fn default() -> Self {
        Self { packet_receive_buffer: default_dgram_rcvbuf_sizes() }
    }
}

#[derive(Clone)]
pub(crate) struct IpLayerSettings {
    pub(crate) raw_receive_buffer: BufferSizeSettings<NonZeroUsize>,
}

impl Default for IpLayerSettings {
    fn default() -> Self {
        Self { raw_receive_buffer: default_dgram_rcvbuf_sizes() }
    }
}

impl SettingsContext<TcpSettings> for BindingsCtx {
    fn settings(&self) -> impl Deref<Target = TcpSettings> + '_ {
        self.settings.tcp.read()
    }
}

impl SettingsContext<netstack3_core::udp::UdpSettings> for BindingsCtx {
    fn settings(&self) -> impl Deref<Target = netstack3_core::udp::UdpSettings> + '_ {
        self.settings.udp.read().map_deref(|u| &u.core)
    }
}

impl SettingsContext<netstack3_core::icmp::IcmpEchoSettings> for BindingsCtx {
    fn settings(&self) -> impl Deref<Target = netstack3_core::icmp::IcmpEchoSettings> + '_ {
        self.settings.icmp.read().map_deref(|i| &i.echo_core)
    }
}

/// This trait provides an abstraction between the conversion for the core
/// representations for buffer sizes and the single
/// [`fnet_settings::SocketBufferSizes`] representation that exists over FIDL.
///
/// Core picks representations that are more ergonomic, while FIDL is more
/// restrictive on the types to avoid enormous values.
trait BufferSizesRepr: Sized + Copy + Ord {
    fn into_fidl_repr(self) -> u32;
    fn try_from_fidl_repr(fidl: u32) -> Result<Self, fnet_settings::UpdateError>;
}

impl BufferSizesRepr for NonZeroUsize {
    fn into_fidl_repr(self) -> u32 {
        usize::from(self).try_into().unwrap_or(u32::MAX)
    }

    fn try_from_fidl_repr(fidl: u32) -> Result<Self, fnet_settings::UpdateError> {
        NonZeroUsize::new(
            fidl.try_into().map_err(|_: TryFromIntError| fnet_settings::UpdateError::OutOfRange)?,
        )
        .ok_or(fnet_settings::UpdateError::IllegalZeroValue)
    }
}

impl BufferSizesRepr for PositiveIsize {
    fn into_fidl_repr(self) -> u32 {
        usize::from(self).try_into().unwrap_or(u32::MAX)
    }

    fn try_from_fidl_repr(fidl: u32) -> Result<Self, fnet_settings::UpdateError> {
        PositiveIsize::new_nonzero_unsigned(NonZeroUsize::try_from_fidl_repr(fidl)?)
            .ok_or(fnet_settings::UpdateError::OutOfRange)
    }
}

impl<T: BufferSizesRepr> IntoFidl<fnet_settings::SocketBufferSizeRange> for BufferSizeSettings<T> {
    fn into_fidl(self) -> fnet_settings::SocketBufferSizeRange {
        fnet_settings::SocketBufferSizeRange {
            max: Some(self.max().into_fidl_repr()),
            default: Some(self.default().into_fidl_repr()),
            min: Some(self.min().into_fidl_repr()),
            __source_breaking: fidl::marker::SourceBreaking,
        }
    }
}

fn update_buffer_sizes<T>(
    target: &mut BufferSizeSettings<T>,
    update: fnet_settings::SocketBufferSizeRange,
) -> Result<fnet_settings::SocketBufferSizeRange, fnet_settings::UpdateError>
where
    T: BufferSizesRepr,
{
    let fnet_settings::SocketBufferSizeRange { max, default, min, __source_breaking } = update;

    let update_and_next = |cur: T, next| -> Result<_, fnet_settings::UpdateError> {
        Ok(match next {
            Some(v) => (Some(cur.into_fidl_repr()), T::try_from_fidl_repr(v)?),
            None => (None, cur),
        })
    };

    let (max_update, max) = update_and_next(target.max(), max)?;
    let (default_update, default) = update_and_next(target.default(), default)?;
    let (min_update, min) = update_and_next(target.min(), min)?;

    let updated =
        BufferSizeSettings::new(min, default, max).ok_or(fnet_settings::UpdateError::OutOfRange)?;
    *target = updated;

    Ok(fnet_settings::SocketBufferSizeRange {
        max: max_update,
        default: default_update,
        min: min_update,
        __source_breaking,
    })
}
