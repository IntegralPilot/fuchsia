// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::convert::{TryFrom as _, TryInto as _};
use std::num::TryFromIntError;
use std::sync::Arc;

use assert_matches::assert_matches;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{FutureExt as _, StreamExt, TryStreamExt as _};
use log::{debug, error, info, warn};
use net_types::ethernet::Mac;
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6, Ipv6Addr, Mtu, Subnet};
use net_types::{MulticastAddr, UnicastAddr};
use netstack3_core::device::{
    BufferSlice, EthernetCreationProperties, EthernetDeviceId, EthernetLinkDevice,
    EthernetWeakDeviceId, GroBufferDestination, GroBufferStorage, GroFrameType, GroInputItem,
    GroOutputItem, MaxEthernetFrameSize, MaybeContiguousBuffer, PureIpDevice,
    PureIpDeviceCreationProperties, PureIpDeviceId, PureIpDeviceReceiveFrameMetadata,
    PureIpWeakDeviceId, RecvEthernetFrameMeta,
};
use netstack3_core::routes::RawMetric;
use netstack3_core::sync::RwLock as CoreRwLock;
use netstack3_core::trace::trace_duration;
use netstack3_core::{
    ChecksumOffloadResult, ChecksumOffloadSpec, ChecksumRxOffloading, NetworkParsingContext,
};
use thiserror::Error;

use crate::bindings::devices::{NetdeviceAllocator, StaticCommonInfo, TxTask};
use crate::bindings::interfaces_admin::{InterfaceOptions, maybe_create_local_route_tables};
use crate::bindings::stats_sampler::{InterfaceStatusBufferedState, InterfaceStatusSampler};
use crate::bindings::util::{IntoFidl, NeedsDataNotifier, ScopeExt as _};
use crate::bindings::{
    BindingId, BindingsCtx, Ctx, DEFAULT_INTERFACE_METRIC, DeviceId, GlobalConfig, Netstack,
    devices, interfaces_admin, routes,
};

/// Like [`DeviceId`], but restricted to netdevice devices.
enum NetdeviceId {
    Ethernet(EthernetDeviceId<BindingsCtx>),
    PureIp(PureIpDeviceId<BindingsCtx>),
}

#[derive(Debug)]
struct RxBuffer(netdevice_client::Buffer<netdevice_client::Rx>);

impl MaybeContiguousBuffer for RxBuffer {
    fn linearized<'a, 'b>(
        &'a mut self,
        vec: Option<&'b mut Vec<u8>>,
    ) -> Option<BufferSlice<'a, 'b>> {
        let Self(buf) = self;
        if let Some(slice) = buf.as_slice_mut() {
            Some(BufferSlice::Contiguous(slice))
        } else {
            let vec = vec?;
            let frame_length = buf.len();
            if vec.len() < frame_length {
                vec.resize(frame_length, 0);
            }
            let slice = &mut vec[..frame_length];
            let read_len = buf.io().read_at(0, slice);
            assert_eq!(read_len, frame_length);
            Some(BufferSlice::Linearized(slice))
        }
    }
}

/// Like [`WeakDeviceId`], but restricted to netdevice devices.
#[derive(Clone, Debug)]
enum WeakNetdeviceId {
    Ethernet(EthernetWeakDeviceId<BindingsCtx>),
    PureIp(PureIpWeakDeviceId<BindingsCtx>),
}

impl WeakNetdeviceId {
    fn upgrade(&self) -> Option<NetdeviceId> {
        match self {
            WeakNetdeviceId::Ethernet(eth) => eth.upgrade().map(NetdeviceId::Ethernet),
            WeakNetdeviceId::PureIp(ip) => ip.upgrade().map(NetdeviceId::PureIp),
        }
    }
}

#[derive(Clone)]
struct Inner {
    device: netdevice_client::Client,
    session: netdevice_client::Session,
    state: Arc<Mutex<netdevice_client::PortSlab<WeakNetdeviceId>>>,
}

/// The worker that receives messages from the ethernet device, and passes them
/// on to the main event loop.
pub(crate) struct NetdeviceWorker {
    ctx: Ctx,
    task: netdevice_client::Task,
    inner: Inner,
    watch_rx_leases: bool,
}

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("client error: {0}")]
    Client(#[from] netdevice_client::Error),
    #[error("port {0:?} already installed")]
    AlreadyInstalled(netdevice_client::Port),
    #[error("fidl error: {0}")]
    Fidl(#[from] fidl::Error),
    #[error("mac addressing error: {0}")]
    MacAddressing(zx::Status),
    #[error("port closed")]
    PortClosed,
    #[error("invalid port info: {0}")]
    InvalidPortInfo(netdevice_client::client::PortInfoValidationError),
    #[error("unsupported configuration")]
    ConfigurationNotSupported,
    #[error("mac {mac} on port {port:?} is not a valid unicast address")]
    MacNotUnicast { mac: net_types::ethernet::Mac, port: netdevice_client::Port },
    #[error("interface named {0} already exists")]
    DuplicateName(String),
    #[error("{port_class:?} port received unexpected frame type: {frame_type:?}")]
    MismatchedRxFrameType { port_class: PortWireFormat, frame_type: FrameType },
    #[error("invalid port class: {0}")]
    InvalidPortClass(fnet_interfaces_ext::UnknownHardwareNetworkPortClassError),
    #[error("received unsupported frame type: {0:?}")]
    UnsupportedFrameType(fhardware_network::FrameType),
    #[error("scope finished")]
    ScopeFinished,
    #[error("cannot create the local route tables")]
    CantCreateLocalRouteTables,
}

const DEFAULT_BUFFER_LENGTH: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrameType {
    Ethernet,
    Ipv4,
    Ipv6,
}

impl TryFrom<fhardware_network::FrameType> for FrameType {
    type Error = Error;
    fn try_from(value: fhardware_network::FrameType) -> Result<Self, Self::Error> {
        match value {
            fhardware_network::FrameType::Ethernet => Ok(Self::Ethernet),
            fhardware_network::FrameType::Ipv4 => Ok(Self::Ipv4),
            fhardware_network::FrameType::Ipv6 => Ok(Self::Ipv6),
            x @ fhardware_network::FrameType::__SourceBreaking { .. } => {
                Err(Error::UnsupportedFrameType(x))
            }
        }
    }
}

impl NetdeviceWorker {
    pub(crate) async fn new(
        ctx: Ctx,
        device: fidl::endpoints::ClientEnd<fhardware_network::DeviceMarker>,
    ) -> Result<Self, Error> {
        let device = netdevice_client::Client::new(device.into_proxy());
        // Enable rx lease watching when suspension is enabled.
        let GlobalConfig {
            suspend_enabled,
            multi_vmo,
            sampled_stats_enabled: _,
            max_rolling_capture_buffer_size: _,
        } = &ctx.bindings_ctx().config;
        let watch_rx_leases = *suspend_enabled;
        let (session, task) = device
            .new_session_with_derivable_config(
                "netstack3",
                netdevice_client::DerivableConfig {
                    default_buffer_length: DEFAULT_BUFFER_LENGTH,
                    watch_rx_leases,
                    multi_vmo: *multi_vmo,
                    ..Default::default()
                },
            )
            .await
            .map_err(Error::Client)?;
        Ok(Self {
            ctx,
            inner: Inner { device, session, state: Default::default() },
            task,
            watch_rx_leases,
        })
    }

    pub(crate) fn new_handler(&self) -> DeviceHandler {
        DeviceHandler { inner: self.inner.clone() }
    }

    pub(crate) async fn run(self) -> Result<std::convert::Infallible, Error> {
        let Self { mut ctx, inner: Inner { device: _, session, state }, task, watch_rx_leases } =
            self;
        // Allow buffer shuttling to happen in other threads.
        let mut task = fasync::Scope::current().compute(task).fuse();

        // Watch rx leases in a separate task if configured to do so.
        //
        // We need not poll this task to observe its completion, it suffices
        // that it goes away when we stop serving the device (executor polls the
        // future for us). Any leases held in the stream will be safely dropped,
        // but before netstack executes any logic on them.
        if watch_rx_leases {
            let ctx = ctx.clone();
            let fut = session
                .watch_rx_leases()
                .map(move |r| {
                    let lease = match r {
                        Ok(l) => l,
                        Err(e) => {
                            error!("error watching rx leases: {e:?}");
                            return;
                        }
                    };
                    // NB: Increment the counter before dropping the lease so
                    // the side effects are synchronized. Useful for tests to
                    // synchronize on lease drop
                    debug!("dropping delegated rx power lease");
                    ctx.bindings_ctx().counters.power.dropped_rx_leases.increment();
                    std::mem::drop(lease);
                })
                .collect::<()>();
            let _: fasync::JoinHandle<()> = fasync::Scope::current().spawn(fut);
        };

        let mut rx_ready_storage = session.new_rx_ready_storage();
        // Maintain persistent GRO buffer storage to avoid repeated allocations.
        let mut gro_storage = GroBufferStorage::new();
        loop {
            let rx_buffers = futures::select! {
                r = session.recv(&mut rx_ready_storage).fuse() => r.map_err(Error::Client)?,
                r = task => match r {
                    Ok(()) => panic!("task should never end cleanly"),
                    Err(e) => return Err(Error::Client(e))
                }
            };
            // Here we intentionally lock the port slab once for the entire
            // batch rather than once per buffer. This should not present a
            // significant problem since ports are seldom added or removed.
            let state = state.lock().await;
            let mut rx_buffers = ShortCircuit::new(rx_buffers.map(build_gro_input));
            let mut gro = gro_storage.coalesce(&mut rx_buffers);
            while let Some(item) = gro.next() {
                let GroOutputItem {
                    target: GroPortTarget { port, frame_type },
                    checksum_offload,
                    mut buffers,
                } = item;
                let slice = buffers.slice_mut();

                let Some(id) = state.get(&port) else {
                    debug!("dropping frame for port {:?}, no device mapping available", port);
                    continue;
                };

                trace_duration!("netdevice::recv");

                let Some(id) = id.upgrade() else {
                    // This is okay because we hold a weak reference; the device
                    // may be removed under us. Note that when the device
                    // removal has completed, the interface's `PortHandler` will
                    // be uninstalled from the port slab (table of ports for
                    // this network device).
                    debug!("received frame for device after it has been removed; device_id={id:?}");
                    // We continue because even though we got frames for a
                    // removed device, this network device may have other ports
                    // that will receive and handle frames.
                    continue;
                };

                let parsing_context = NetworkParsingContext::new(checksum_offload);
                let buf = packet::Buf::new(slice, ..);
                receive_frame(&mut ctx, id, frame_type, parsing_context, buf)?;
            }
            std::mem::drop(gro);
            rx_buffers.check_error()?;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GroPortTarget {
    port: netdevice_client::Port,
    frame_type: FrameType,
}

impl GroBufferDestination for GroPortTarget {
    fn frame_type(&self) -> GroFrameType {
        match self.frame_type {
            FrameType::Ethernet => GroFrameType::Ethernet,
            FrameType::Ipv4 => GroFrameType::PureIp(IpVersion::V4),
            FrameType::Ipv6 => GroFrameType::PureIp(IpVersion::V6),
        }
    }
}

struct ShortCircuit<I, E> {
    iter: I,
    error: Option<E>,
}

impl<I, E> ShortCircuit<I, E> {
    fn new(iter: I) -> Self {
        Self { iter, error: None }
    }

    fn check_error(&mut self) -> Result<(), E> {
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl<I, R, E> Iterator for ShortCircuit<I, E>
where
    I: Iterator<Item = Result<R, E>>,
{
    type Item = R;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next() {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                self.error = Some(e);
                None
            }
            None => None,
        }
    }
}

/// Converts an rx buffer into a format suitable for processing by GRO.
///
/// `Err` indicates that a critical error was encountered and the worker should
/// be terminated.
fn build_gro_input(
    rx_result: Result<netdevice_client::Buffer<netdevice_client::Rx>, netdevice_client::Error>,
) -> Result<GroInputItem<RxBuffer, GroPortTarget>, Error> {
    let rx = rx_result.map_err(Error::Client)?;
    let rx_meta = rx.meta();
    let port = rx_meta.port();
    let frame_type = rx_meta.frame_type().map_err(Error::Client)?.try_into()?;
    let checksum_offloading = match rx_meta.rx_checksum_offloading() {
        Some(netdevice_client::ChecksumRxOffloading::Offloaded(n)) => {
            ChecksumRxOffloading::Offloaded(Some(n))
        }
        None => ChecksumRxOffloading::Offloaded(None),
    };
    std::mem::drop(rx_meta);

    Ok(GroInputItem {
        buffer: RxBuffer(rx),
        target: GroPortTarget { port, frame_type },
        checksum_offload: checksum_offloading,
    })
}

fn receive_frame<B: packet::BufferMut + std::fmt::Debug>(
    ctx: &mut Ctx,
    id: NetdeviceId,
    frame_type: FrameType,
    parsing_context: NetworkParsingContext,
    buf: B,
) -> Result<(), Error> {
    match id {
        NetdeviceId::Ethernet(id) => {
            match frame_type {
                FrameType::Ethernet => {}
                f @ FrameType::Ipv4 | f @ FrameType::Ipv6 => {
                    // NB: When the port was attached, `Ethernet` was
                    // the only permitted frame type; anything else here
                    // indicates a bug in `netdevice_client` or the core
                    // netdevice driver.
                    return Err(Error::MismatchedRxFrameType {
                        port_class: PortWireFormat::Ethernet,
                        frame_type: f,
                    });
                }
            }
            ctx.api()
                .device::<EthernetLinkDevice>()
                .receive_frame(RecvEthernetFrameMeta { device_id: id, parsing_context }, buf);
        }
        NetdeviceId::PureIp(id) => {
            let ip_version = match frame_type {
                FrameType::Ipv4 => IpVersion::V4,
                FrameType::Ipv6 => IpVersion::V6,
                f @ FrameType::Ethernet => {
                    // NB: When the port was attached, `IPv4` & `Ipv6`
                    // were the only permitted frame types; anything
                    // else here indicates a bug in `netdevice_client` or
                    // the core netdevice driver.
                    return Err(Error::MismatchedRxFrameType {
                        port_class: PortWireFormat::Ip,
                        frame_type: f,
                    });
                }
            };
            ctx.api().device::<PureIpDevice>().receive_frame(
                PureIpDeviceReceiveFrameMetadata { device_id: id, ip_version, parsing_context },
                buf,
            );
        }
    }
    Ok(())
}

pub(crate) struct DeviceHandler {
    inner: Inner,
}

/// The wire format for packets sent to and received on a port.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PortWireFormat {
    /// The port supports sending/receiving Ethernet frames.
    Ethernet,
    /// The port supports sending/receiving IPv4 and IPv6 packets.
    Ip,
}

impl PortWireFormat {
    fn frame_types(&self) -> &[fhardware_network::FrameType] {
        const ETHERNET_FRAMES: [fhardware_network::FrameType; 1] =
            [fhardware_network::FrameType::Ethernet];
        const IP_FRAMES: [fhardware_network::FrameType; 2] =
            [fhardware_network::FrameType::Ipv4, fhardware_network::FrameType::Ipv6];
        match self {
            Self::Ethernet => &ETHERNET_FRAMES,
            Self::Ip => &IP_FRAMES,
        }
    }
}

/// Error returned for ports with unsupported wire formats.
#[derive(Debug)]
pub(crate) enum PortWireFormatError<'a> {
    InvalidRxFrameTypes { _frame_types: Vec<&'a fhardware_network::FrameType> },
    InvalidTxFrameTypes { _frame_types: Vec<&'a fhardware_network::FrameType> },
    MismatchedRxTx { _rx: PortWireFormat, _tx: PortWireFormat },
}

impl PortWireFormat {
    fn new_from_port_info(
        info: &netdevice_client::client::PortBaseInfo,
    ) -> Result<PortWireFormat, PortWireFormatError<'_>> {
        let netdevice_client::client::PortBaseInfo { port_class: _, rx_types, tx_types } = info;

        // Verify the wire format in a single direction (tx/rx).
        fn wire_format_from_frame_types<'a>(
            frame_types: impl Iterator<Item = &'a fhardware_network::FrameType> + Clone,
        ) -> Result<PortWireFormat, impl Iterator<Item = &'a fhardware_network::FrameType>>
        {
            struct SupportedFormats {
                ethernet: bool,
                ipv4: bool,
                ipv6: bool,
            }
            let SupportedFormats { ethernet, ipv4, ipv6 } = frame_types.clone().fold(
                SupportedFormats { ethernet: false, ipv4: false, ipv6: false },
                |mut sf, frame_type| {
                    match frame_type {
                        fhardware_network::FrameType::Ethernet => sf.ethernet = true,
                        fhardware_network::FrameType::Ipv4 => sf.ipv4 = true,
                        fhardware_network::FrameType::Ipv6 => sf.ipv6 = true,
                        fhardware_network::FrameType::__SourceBreaking { .. } => (),
                    }
                    sf
                },
            );
            // Disallow devices with mixed frame types, and require that IP
            // Devices support both IPv4 and IPv6.
            if ethernet && !ipv4 && !ipv6 {
                Ok(PortWireFormat::Ethernet)
            } else if !ethernet && ipv4 && ipv6 {
                Ok(PortWireFormat::Ip)
            } else {
                Err(frame_types)
            }
        }

        // Ignore the superfluous information included with the tx frame types.
        let tx_iterator = || {
            tx_types.iter().map(
                |fhardware_network::FrameTypeSupport {
                     type_: frame_type,
                     features: _,
                     supported_flags: _,
                 }| { frame_type },
            )
        };

        // Verify each direction independently, and then ensure the port is
        // symmetrical.
        let rx_wire_format = wire_format_from_frame_types(rx_types.iter()).map_err(|rx_types| {
            PortWireFormatError::InvalidRxFrameTypes { _frame_types: rx_types.collect() }
        })?;
        let tx_wire_format = wire_format_from_frame_types(tx_iterator()).map_err(|tx_types| {
            PortWireFormatError::InvalidTxFrameTypes { _frame_types: tx_types.collect() }
        })?;
        if rx_wire_format == tx_wire_format {
            Ok(rx_wire_format)
        } else {
            Err(PortWireFormatError::MismatchedRxTx { _rx: rx_wire_format, _tx: tx_wire_format })
        }
    }
}

/// Calculates the tx checksum offload spec based on the supported frame types'
/// supported flags.
///
/// Note that this function is more restrictive than the shape of the API allows
/// in order to support a simplified internal representation. See inline
/// comments for details.
fn tx_offload_spec_from_port_info(
    info: &netdevice_client::client::PortBaseInfo,
) -> ChecksumOffloadSpec {
    let netdevice_client::client::PortBaseInfo { port_class: _, rx_types: _, tx_types } = info;
    if tx_types.is_empty() {
        return ChecksumOffloadSpec::none();
    }
    let mut tx_flags = tx_types.iter().map(
        |fhardware_network::FrameTypeSupport { type_: _, features: _, supported_flags }| {
            supported_flags
        },
    );
    // Generic checksum offloading is only supported if all frame types support
    // it.
    if tx_flags.clone().all(|f| f.contains(fhardware_network::TxFlags::COMPUTE_GENERIC_CHECKSUM)) {
        return ChecksumOffloadSpec::generic();
    } else if tx_flags.any(|f| f.contains(fhardware_network::TxFlags::COMPUTE_GENERIC_CHECKSUM)) {
        warn!("ignoring tx feature COMPUTE_GENERIC_CHECKSUM enabled on only some frame types");
    }
    // Protocol-specific offloading, when supported, may be enabled on a
    // per-protocol basis (e.g. TCP/UDP-over-IPv4 and TCP/UDP-over-IPv6 may be
    // enabled independently).
    ChecksumOffloadSpec::none()
}

impl DeviceHandler {
    pub(crate) async fn add_port(
        &self,
        ns: &mut Netstack,
        scope: &fasync::ScopeHandle,
        InterfaceOptions { name, metric, netstack_managed_routes_designation }: InterfaceOptions,
        port: fhardware_network::PortId,
        control_hook: mpsc::Sender<interfaces_admin::OwnedControlHandle>,
    ) -> Result<
        (
            BindingId,
            impl futures::Stream<Item = netdevice_client::Result<netdevice_client::PortStatus>> + use<>,
            fasync::scope::ScopeActiveGuard,
            TxTask,
            Option<LinkMulticastWorker>,
        ),
        Error,
    > {
        let port = netdevice_client::Port::try_from(port)?;

        let DeviceHandler { inner: Inner { state, device, session } } = self;
        let port_proxy = device.connect_port(port)?;
        let netdevice_client::client::PortInfo { id: _, base_info } =
            port_proxy.get_info().await?.try_into().map_err(Error::InvalidPortInfo)?;
        let port_identity = port_proxy.get_identity().await?;
        // FIDL bindings protect us from getting an invalid handle, getting the
        // KOID from a handle that is open always succeeds.
        let port_identity_koid = Some(port_identity.koid().expect("extract port id event"));

        let mut status_stream =
            netdevice_client::client::new_port_status_stream(&port_proxy, None)?;

        let wire_format = PortWireFormat::new_from_port_info(&base_info).map_err(
            |e: PortWireFormatError<'_>| {
                warn!("not installing port with invalid wire format: {:?}", e);
                Error::ConfigurationNotSupported
            },
        )?;

        let tx_offload_spec = tx_offload_spec_from_port_info(&base_info);

        let netdevice_client::client::PortStatus { flags, mtu } =
            status_stream.try_next().await?.ok_or_else(|| Error::PortClosed)?;
        let phy_up = flags.contains(fhardware_network::StatusFlags::ONLINE);
        let netdevice_client::client::PortBaseInfo {
            port_class: hw_port_class,
            rx_types: _,
            tx_types: _,
        } = base_info;

        enum DeviceProperties {
            Ethernet {
                max_frame_size: MaxEthernetFrameSize,
                mac: UnicastAddr<Mac>,
                mac_proxy: fhardware_network::MacAddressingProxy,
            },
            Ip {
                max_frame_size: Mtu,
            },
        }
        let properties = match wire_format {
            PortWireFormat::Ethernet => {
                let max_frame_size =
                    MaxEthernetFrameSize::new(mtu).ok_or(Error::ConfigurationNotSupported)?;
                let (mac, mac_proxy) = get_mac(&port_proxy, &port).await?;
                DeviceProperties::Ethernet { max_frame_size, mac, mac_proxy }
            }
            PortWireFormat::Ip => DeviceProperties::Ip { max_frame_size: Mtu::new(mtu) },
        };

        let mut state = state.lock().await;
        let state_entry = match state.entry(port) {
            netdevice_client::port_slab::Entry::Occupied(occupied) => {
                warn!(
                    "attempted to install port {:?} which is already installed for {:?}",
                    port,
                    occupied.get()
                );
                return Err(Error::AlreadyInstalled(port));
            }
            netdevice_client::port_slab::Entry::SaltMismatch(stale) => {
                warn!(
                    "attempted to install port {:?} which is already has a stale entry: {:?}",
                    port, stale
                );
                return Err(Error::AlreadyInstalled(port));
            }
            netdevice_client::port_slab::Entry::Vacant(e) => e,
        };
        let Netstack { interfaces_event_sink, neighbor_event_sink, ctx } = ns;

        let max_frame_size =
            mtu.try_into().map_err(|TryFromIntError { .. }| Error::ConfigurationNotSupported)?;

        let port_class = fnet_interfaces_ext::PortClass::try_from(hw_port_class)
            .map_err(Error::InvalidPortClass)?;

        // Ensure we're not going to get stopped. Holding a guard on a child
        // scope ensures a guard on the parent.
        //
        // Unfortunately, the guard must be acquired before we know the device
        // name. Create a temporary guard for now, and replace it later with
        // a properly named guard.
        let temp_guard = scope
            .new_detached_child("add_port_temporary_guard")
            .active_guard()
            .ok_or(Error::ScopeFinished)?;

        // Note: `binding_id_alloc` will panic if dropped. Care must be taken
        // to cancel the device reservation should an error occur after this
        // point.
        let (binding_id, binding_id_alloc, name) = match name {
            None => ctx.bindings_ctx().devices.generate_and_reserve_name_and_alloc_new_id(
                match wire_format {
                    PortWireFormat::Ethernet => "eth",
                    PortWireFormat::Ip => "ip",
                },
            ),
            Some(name) => {
                match ctx.bindings_ctx().devices.try_reserve_name_and_alloc_new_id(name.clone()) {
                    Err(devices::NameNotAvailableError) => return Err(Error::DuplicateName(name)),
                    Ok((id, alloc)) => (id, alloc, name),
                }
            }
        };

        // Now that we know the device name, try to acquire a permanent guard.
        // This operation is fallible, so cancel the device ID reservation on
        // failure.
        let guard = match scope.new_detached_child(format!("{binding_id}({name})")).active_guard() {
            Some(guard) => guard,
            None => {
                ctx.bindings_ctx().devices.cancel_device_reservation(binding_id_alloc);
                return Err(Error::ScopeFinished);
            }
        };
        std::mem::drop(temp_guard);

        // Try to create local route tables for the device. This operation is
        // fallible, so cancel the device ID reservation on failure.
        let local_route_tables = match maybe_create_local_route_tables(
            &*ctx,
            &name,
            netstack_managed_routes_designation,
        )
        .await
        {
            Ok(tables) => tables,
            Err(err) => {
                log::error!("failed to create local route tables for {name}: {err:?}");
                ctx.bindings_ctx().devices.cancel_device_reservation(binding_id_alloc);
                return Err(Error::CantCreateLocalRouteTables);
            }
        };

        // Do the rest of the work in a closure so we don't accidentally add
        // errors after the binding ID is already allocated. This part of the
        // function should be infallible.
        let finalize = move || async move {
            let tx_notifier = NeedsDataNotifier::default();
            let tx_watcher = tx_notifier.watcher();
            let static_netdevice_info = devices::StaticNetdeviceInfo {
                handler: PortHandler {
                    id: binding_id,
                    port_id: port,
                    inner: self.inner.clone(),
                    port_class: hw_port_class,
                    wire_format,
                    max_frame_size,
                },
                tx_notifier,
            };
            let dynamic_netdevice_info_builder = |mtu: Mtu| devices::DynamicNetdeviceInfo {
                phy_up,
                common_info: devices::DynamicCommonInfo::new(
                    mtu,
                    crate::bindings::create_interface_event_producer(
                        interfaces_event_sink,
                        binding_id,
                        crate::bindings::InterfaceProperties {
                            name: name.clone(),
                            port_class,
                            port_identity_koid,
                        },
                    ),
                    control_hook,
                ),
            };

            let (tx_allocator, tx_task_state) = NetdeviceAllocator::new(session.clone());

            let (core_id, link_multicast_task) = match properties {
                DeviceProperties::Ethernet { max_frame_size, mac, mac_proxy } => {
                    let (link_multicast_task, link_multicast_sink) =
                        LinkMulticastWorker::new(mac_proxy);

                    let info = devices::EthernetInfo {
                        mac,
                        multicast_event_sink: link_multicast_sink,
                        netdevice: static_netdevice_info,
                        common_info: StaticCommonInfo::new(local_route_tables),
                        dynamic_info: CoreRwLock::new(devices::DynamicEthernetInfo {
                            netdevice: dynamic_netdevice_info_builder(max_frame_size.as_mtu()),
                            neighbor_event_sink: neighbor_event_sink.clone(),
                        }),
                        status_sampler: InterfaceStatusSampler::new(InterfaceStatusBufferedState {
                            link_up: phy_up,
                            admin_up: devices::DynamicCommonInfo::DEFAULT_ADMIN_ENABLED,
                        }),
                    };
                    let core_ethernet_id = ctx.api().device::<EthernetLinkDevice>().add_device(
                        devices::DeviceIdAndName { id: binding_id, name: name.clone() },
                        EthernetCreationProperties { mac, max_frame_size, tx_offload_spec },
                        RawMetric(metric.unwrap_or(DEFAULT_INTERFACE_METRIC)),
                        info,
                        tx_allocator,
                    );
                    state_entry.insert(WeakNetdeviceId::Ethernet(core_ethernet_id.downgrade()));

                    (DeviceId::from(core_ethernet_id), Some(link_multicast_task))
                }
                DeviceProperties::Ip { max_frame_size } => {
                    let info = devices::PureIpDeviceInfo {
                        common_info: StaticCommonInfo {
                            authorization_token: zx::Event::create(),
                            local_route_tables,
                        },
                        netdevice: static_netdevice_info,
                        dynamic_info: CoreRwLock::new(dynamic_netdevice_info_builder(
                            max_frame_size,
                        )),
                        status_sampler: InterfaceStatusSampler::new(InterfaceStatusBufferedState {
                            link_up: phy_up,
                            admin_up: devices::DynamicCommonInfo::DEFAULT_ADMIN_ENABLED,
                        }),
                    };
                    let core_pure_ip_id = ctx.api().device::<PureIpDevice>().add_device(
                        devices::DeviceIdAndName { id: binding_id, name: name.clone() },
                        PureIpDeviceCreationProperties { mtu: max_frame_size, tx_offload_spec },
                        RawMetric(metric.unwrap_or(DEFAULT_INTERFACE_METRIC)),
                        info,
                        tx_allocator,
                    );
                    state_entry.insert(WeakNetdeviceId::PureIp(core_pure_ip_id.downgrade()));
                    (DeviceId::from(core_pure_ip_id), None)
                }
            };

            let tx_task = TxTask::new(ctx.clone(), core_id.clone(), tx_watcher, tx_task_state);
            match &core_id {
                DeviceId::PureIp(device) => {
                    ctx.api().transmit_queue::<PureIpDevice>().set_configuration(
                        device,
                        netstack3_core::device::TransmitQueueConfiguration::Fifo,
                    );
                }
                DeviceId::Ethernet(device) => {
                    ctx.api().transmit_queue::<EthernetLinkDevice>().set_configuration(
                        device,
                        netstack3_core::device::TransmitQueueConfiguration::Fifo,
                    );
                }
                DeviceId::Loopback(device) => {
                    unreachable!("loopback device should not be backed by netdevice: {device:?}")
                }
                DeviceId::Blackhole(device) => {
                    unreachable!("blackhole device should not be backed by netdevice: {device:?}")
                }
            }
            add_initial_routes(ctx.bindings_ctx(), &core_id).await;

            ctx.apply_interface_defaults(&core_id);

            // LINT.IfChange(netstack_created_interface_tefmo)
            info!("created interface {:?}", core_id);
            // LINT.ThenChange(//tools/testing/tefmocheck/cdc_ethernet_state_check.go:netstack_created_interface_tefmo)
            ctx.bindings_ctx().devices.add_device_and_start_publishing(binding_id_alloc, core_id);

            (binding_id, tx_task, link_multicast_task)
        };
        let (binding_id, tx_task, link_multicast_task) = finalize().await;

        Ok((binding_id, status_stream, guard, tx_task, link_multicast_task))
    }
}

/// Connect to the Port's `MacAddressingProxy`, and fetch the MAC address.
async fn get_mac(
    port_proxy: &fhardware_network::PortProxy,
    port: &netdevice_client::Port,
) -> Result<(UnicastAddr<Mac>, fhardware_network::MacAddressingProxy), Error> {
    let (mac_proxy, mac_server) =
        fidl::endpoints::create_proxy::<fhardware_network::MacAddressingMarker>();
    port_proxy.get_mac(mac_server)?;

    let mac_addr = {
        let fnet::MacAddress { octets } = mac_proxy.get_unicast_address().await.map_err(|e| {
            warn!("failed to get unicast address, sending not supported: {:?}", e);
            Error::ConfigurationNotSupported
        })?;
        let mac = net_types::ethernet::Mac::new(octets);
        net_types::UnicastAddr::new(mac).ok_or_else(|| {
            warn!("{} is not a valid unicast address", mac);
            Error::MacNotUnicast { mac, port: *port }
        })?
    };

    Ok((mac_addr, mac_proxy))
}

/// Adds the IPv4 and IPv6 multicast subnet routes and the IPv6 link-local
/// subnet route.
///
/// Note that if an error is encountered while installing a route, any routes
/// that were successfully installed prior to the error will not be removed.
async fn add_initial_routes(bindings_ctx: &BindingsCtx, device: &DeviceId<BindingsCtx>) {
    use netstack3_core::routes::{AddableEntry, AddableMetric};
    const LINK_LOCAL_SUBNET: Subnet<Ipv6Addr> = net_declare::net_subnet_v6!("fe80::/64");

    let v4_changes = std::iter::once(AddableEntry::without_gateway(
        Ipv4::MULTICAST_SUBNET,
        device.downgrade(),
        AddableMetric::MetricTracksInterface,
    ))
    .map(|entry| {
        routes::Change::<Ipv4>::RouteOp(
            routes::RouteOp::Add(entry),
            routes::SetMembership::InitialDeviceRoutes,
        )
    })
    .map(Into::into);

    let v6_changes = [
        AddableEntry::without_gateway(
            LINK_LOCAL_SUBNET,
            device.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
        AddableEntry::without_gateway(
            Ipv6::MULTICAST_SUBNET,
            device.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
    ]
    .into_iter()
    .map(|entry| {
        routes::Change::<Ipv6>::RouteOp(
            routes::RouteOp::Add(entry),
            routes::SetMembership::InitialDeviceRoutes,
        )
    })
    .map(Into::into);

    for change in v4_changes.chain(v6_changes) {
        bindings_ctx
            .apply_route_change_either(change)
            .await
            .map(|outcome| assert_matches!(outcome, routes::ChangeOutcome::Changed))
            .expect("adding initial routes should succeed");
    }
}

pub(crate) struct PortHandler {
    id: BindingId,
    port_id: netdevice_client::Port,
    inner: Inner,
    port_class: fhardware_network::PortClass,
    wire_format: PortWireFormat,
    max_frame_size: usize,
}

impl PortHandler {
    pub(crate) fn port_class(&self) -> fhardware_network::PortClass {
        self.port_class
    }

    pub(crate) async fn attach(&self) -> Result<(), netdevice_client::Error> {
        let Self { port_id, inner: Inner { session, .. }, wire_format, .. } = self;
        session.attach(*port_id, wire_format.frame_types()).await
    }

    pub(crate) async fn detach(&self) -> Result<(), netdevice_client::Error> {
        let Self { port_id, inner: Inner { session, .. }, .. } = self;
        session.detach(*port_id).await
    }

    /// Waits for at least one buffer be available and returns an iterator of
    /// available tx buffers.
    ///
    /// Buffers are allocated with the ports's maximum frame size.
    pub(crate) async fn alloc_tx_buffers(
        &self,
    ) -> Result<
        impl Iterator<Item = Result<netdevice_client::TxBuffer, netdevice_client::Error>> + '_,
        netdevice_client::Error,
    > {
        let Self { inner: Inner { session, .. }, max_frame_size, .. } = self;
        session.alloc_tx_buffers(*max_frame_size).await
    }

    /// Attempts to allocate a [`netdevice::TxBuffer`] synchronously.
    ///
    /// Returns `Ok(None)` if no buffers are available.
    ///
    /// The buffer is always allocated with the ports's maximum frame size.
    pub(crate) fn alloc_tx_buffer(
        &self,
    ) -> Result<Option<netdevice_client::TxBuffer>, netdevice_client::Error> {
        let Self { inner: Inner { session, .. }, max_frame_size, .. } = self;
        session.alloc_tx_buffer(*max_frame_size).now_or_never().transpose()
    }

    pub(crate) fn send(
        &self,
        frame_type: fhardware_network::FrameType,
        mut tx: netdevice_client::TxBuffer,
        csum_offload: Option<ChecksumOffloadResult>,
    ) {
        trace_duration!("netdevice::send");
        let Self { port_id, inner: Inner { session, .. }, .. } = self;
        {
            let mut meta = tx.meta_mut();
            meta.set_port(*port_id);
            meta.set_frame_type(frame_type);
            match csum_offload {
                Some(ChecksumOffloadResult::Generic(partial)) => {
                    meta.set_generic_csum_offload(partial.start, partial.offset);
                }
                Some(ChecksumOffloadResult::ProtocolSpecific(_)) => {
                    // TODO(https://fxbug.dev/527139703): Expose protocol-specific
                    // TX checksum offloading in the netdevice API.
                    todo!("protocol-specific TX checksum offloading not yet supported");
                }
                None => {}
            }
        }
        session.send(tx);
    }

    pub(crate) async fn uninstall(self) -> Result<(), netdevice_client::Error> {
        let Self { port_id, inner: Inner { session, state, .. }, .. } = self;
        let _: WeakNetdeviceId = assert_matches!(
            state.lock().await.remove(&port_id),
            netdevice_client::port_slab::RemoveOutcome::Removed(core_id) => core_id
        );
        session.detach(port_id).await
    }

    pub(crate) fn connect_port(
        &self,
        port: fidl::endpoints::ServerEnd<fhardware_network::PortMarker>,
    ) -> Result<(), netdevice_client::Error> {
        let Self { port_id, inner: Inner { device, .. }, .. } = self;
        device.connect_port_server_end(*port_id, port)
    }

    pub(crate) async fn wait_tx_done(&self) {
        let Self { inner: Inner { session, .. }, .. } = self;
        session.wait_tx_idle().await;
    }

    pub(crate) async fn close_session(&self) -> Result<(), netdevice_client::Error> {
        let Self { inner: Inner { session, .. }, .. } = self;
        session.close().await
    }
}

impl std::fmt::Debug for PortHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { id, port_id, inner: _, port_class, wire_format, max_frame_size } = self;
        f.debug_struct("PortHandler")
            .field("id", id)
            .field("port_id", port_id)
            .field("port_class", port_class)
            .field("wire_format", wire_format)
            .field("max_frame_size", max_frame_size)
            .finish()
    }
}

pub(crate) enum LinkMulticastEvent {
    Join(MulticastAddr<Mac>),
    Leave(MulticastAddr<Mac>),
}

pub(crate) struct LinkMulticastWorker {
    receiver: mpsc::UnboundedReceiver<LinkMulticastEvent>,
    mac_proxy: fhardware_network::MacAddressingProxy,
}

impl LinkMulticastWorker {
    fn new(
        mac_proxy: fhardware_network::MacAddressingProxy,
    ) -> (Self, mpsc::UnboundedSender<LinkMulticastEvent>) {
        let (sender, receiver) = mpsc::unbounded();
        (Self { receiver, mac_proxy }, sender)
    }

    pub(crate) async fn run(&mut self) -> Result<(), Error> {
        while let Some(event) = self.receiver.next().await {
            let result = match event {
                LinkMulticastEvent::Join(addr) => {
                    self.mac_proxy.add_multicast_address(&addr.into_fidl()).await
                }
                LinkMulticastEvent::Leave(addr) => {
                    self.mac_proxy.remove_multicast_address(&addr.into_fidl()).await
                }
            }
            .map(zx::Status::ok)?;

            match result {
                Ok(()) => {}
                Err(e @ zx::Status::NO_RESOURCES) => {
                    warn!(
                        "too many multicast groups, switching device to multicast promiscuous: {e}"
                    );
                    // If we go over MAX_MAC_FILTER addresses, the netdevice driver
                    // can't track the diffs anymore, so stick this device in promiscuous mode.
                    // TODO(https://fxbug.dev/435532334): send a snapshot update to recover
                    // instead of having a sticky error.
                    zx::Status::ok(
                        self.mac_proxy
                            .set_mode(fhardware_network::MacFilterMode::MulticastPromiscuous)
                            .await?,
                    )
                    .map_err(Error::MacAddressing)?;
                }
                Err(err) => return Err(Error::MacAddressing(err)),
            };
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(vec![], ChecksumOffloadSpec::none(); "empty")]
    #[test_case(
        vec![
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ethernet,
                features: 0,
                supported_flags: fhardware_network::TxFlags::empty(),
            },
        ],
        ChecksumOffloadSpec::none();
        "single frame type, no generic support"
    )]
    #[test_case(
        vec![
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ethernet,
                features: 0,
                supported_flags: fhardware_network::TxFlags::COMPUTE_GENERIC_CHECKSUM,
            },
        ],
        ChecksumOffloadSpec::generic();
        "single frame type, generic support"
    )]
    #[test_case(
        vec![
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv6,
                features: 0,
                supported_flags: fhardware_network::TxFlags::empty(),
            },
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv4,
                features: 0,
                supported_flags: fhardware_network::TxFlags::empty(),
            },
        ],
        ChecksumOffloadSpec::none();
        "multiple frame types, no generic support"
    )]
    #[test_case(
        vec![
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv6,
                features: 0,
                supported_flags: fhardware_network::TxFlags::COMPUTE_GENERIC_CHECKSUM,
            },
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv4,
                features: 0,
                supported_flags: fhardware_network::TxFlags::COMPUTE_GENERIC_CHECKSUM,
            },
        ],
        ChecksumOffloadSpec::generic();
        "multiple frame types, generic support"
    )]
    #[test_case(
        vec![
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv6,
                features: 0,
                supported_flags: fhardware_network::TxFlags::COMPUTE_GENERIC_CHECKSUM,
            },
            fhardware_network::FrameTypeSupport {
                type_: fhardware_network::FrameType::Ipv4,
                features: 0,
                supported_flags: fhardware_network::TxFlags::empty(),
            },
        ],
        ChecksumOffloadSpec::none();
        "multiple frame types, no unanimous generic support"
    )]
    fn build_tx_offload_spec_from_port_info(
        tx_types: Vec<fhardware_network::FrameTypeSupport>,
        expected: ChecksumOffloadSpec,
    ) {
        let info = netdevice_client::client::PortBaseInfo {
            port_class: fhardware_network::PortClass::Ethernet,
            rx_types: vec![],
            tx_types,
        };
        assert_eq!(tx_offload_spec_from_port_info(&info), expected);
    }

    #[test]
    fn short_circuit_on_error() {
        let items: Vec<Result<i32, ()>> = vec![Ok(1), Err(()), Ok(2)];
        let mut iter = ShortCircuit::new(items.into_iter());
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.check_error(), Err(()));
    }

    #[test]
    fn short_circuit_empty_ok() {
        let items: Vec<Result<(), ()>> = vec![];
        let mut iter = ShortCircuit::new(items.into_iter());
        assert_eq!(iter.next(), None);
        assert_eq!(iter.check_error(), Ok(()));
    }
}
