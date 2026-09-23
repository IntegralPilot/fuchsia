// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Netstack3 bindings.
//!
//! This module provides Fuchsia bindings for the [`netstack3_core`] crate.

#![warn(
    missing_docs,
    unreachable_patterns,
    clippy::useless_conversion,
    clippy::redundant_clone,
    clippy::precedence
)]

#[cfg(test)]
mod integration_tests;

mod bpf;
mod counters;
mod debug_fidl_worker;
mod devices;
mod errno;
mod error;
mod filter;
mod health_check_worker;
mod inspect;
mod interface_config;
mod interfaces_admin;
mod interfaces_watcher;
mod matchers;
mod multicast_admin;
mod name_worker;
mod ndp_watcher;
mod neighbor_worker;
mod netdevice_worker;
mod packet_capture_worker;
mod persistence;
mod power;
mod reference_notifier;
mod resource_removal;
mod root_fidl_worker;
mod routes;
mod settings;
mod socket;
mod sockets_fidl;
mod stack_fidl_worker;
mod stats_sampler;
mod time;
mod timers;
mod util;
mod waker;

use std::fmt::Debug;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use assert_matches::assert_matches;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_hardware_network as fhardware_network;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_multicast_admin as fnet_multicast_admin;
use fidl_fuchsia_net_multicast_ext::FidlMulticastAdminIpExt;
use fidl_fuchsia_net_routes as fnet_routes;
use fidl_fuchsia_net_routes_admin as fnet_routes_admin;
use fuchsia_async as fasync;
use fuchsia_inspect::health::Reporter as _;
use futures::channel::{mpsc, oneshot};
use futures::{FutureExt as _, StreamExt as _};
use log::{debug, error, info, warn};
use netstack3_core::ChecksumOffloadResult;
use packet::BufferMut;
use rand::rngs::SysRng;
use util::{ConversionContext, IntoFidl as _};

use devices::{
    BindingId, BlackholeDeviceInfo, DeviceIdAndName, DeviceSpecificInfo, Devices,
    DynamicCommonInfo, DynamicEthernetInfo, DynamicNetdeviceInfo, EthernetInfo, LoopbackInfo,
    PureIpDeviceInfo, StaticCommonInfo, StaticNetdeviceInfo, TxTaskState,
};
use interfaces_watcher::{InterfaceEventProducer, InterfaceProperties, InterfaceUpdate};
use multicast_admin::{MulticastAdminEventSinks, MulticastAdminWorkers};
use ndp_watcher::RouterAdvertisementSinkError;
use netdevice_worker::LinkMulticastEvent;
use power::{PowerWorker, PowerWorkerSink};
use resource_removal::{ResourceRemovalSink, ResourceRemovalWorker};

use crate::bindings::bpf::{EbpfManager, SocketFilterProgram};
use crate::bindings::counters::BindingsCounters;
use crate::bindings::devices::TxBuffer;
pub use crate::bindings::interface_config::InterfaceConfigDefaults;
use crate::bindings::interface_config::InterfaceConfigType;
use crate::bindings::interfaces_watcher::AddressPropertiesUpdate;
use crate::bindings::packet_capture_worker::PacketCaptures;
use crate::bindings::settings::Settings;
use crate::bindings::socket::queue::NoSpace;

use crate::bindings::sockets_fidl::SocketDestructionDispatcher;
use crate::bindings::stats_sampler::StatsSampler;
use crate::bindings::time::{AtomicStackTime, StackTime};
use crate::bindings::util::{ScopeExt as _, fidl_mark_domain_to_core};
use net_types::SpecifiedAddr;
use net_types::ethernet::Mac;
use net_types::ip::{
    AddrSubnet, AddrSubnetEither, Ip, IpAddr, IpAddress, IpVersion, Ipv4, Ipv6, Mtu,
};
use netstack3_core::device::{
    DeviceConfigurationUpdate, DeviceId, DeviceLayerEventDispatcher, DeviceLayerStateTypes,
    DeviceSendFrameError, EthernetDeviceEvent, EthernetDeviceId, LoopbackCreationProperties,
    LoopbackDevice, LoopbackDeviceId, PureIpDeviceId, ReceiveQueueBindingsContext,
    TransmitQueueBindingsContext, WeakDeviceId,
};
use netstack3_core::error::ExistsError;
use netstack3_core::filter::{SocketOpsFilter, SocketOpsFilterBindingContext};
use netstack3_core::icmp::{
    IcmpEchoBindingsContext, IcmpEchoBindingsTypes, IcmpSocketId, ReceiveIcmpEchoError,
};
use netstack3_core::inspect::{InspectableValue, Inspector};
use netstack3_core::ip::{
    AddIpAddrSubnetError, AddressRemovedReason, IpDeviceEvent, IpLayerEvent,
    IpRoutingBindingsTypes, Ipv4DeviceConfigurationUpdate, Ipv6DeviceConfigurationUpdate, Lifetime,
    MarkDomain, MarksBindingsContext, RouterAdvertisementEvent,
};
use netstack3_core::routes::RawMetric;
use netstack3_core::sync::RwLock as CoreRwLock;
use netstack3_core::udp::{
    ReceiveUdpError, UdpBindingsTypes, UdpPacketMeta, UdpReceiveBindingsContext, UdpSocketId,
};
use netstack3_core::{
    CoreTxMetadata, DeferredResourceRemovalContext, EventContext, InstantBindingsTypes,
    InstantContext, IpExt, MatcherBindingsTypes, RngContext, StackState, StackStateBuilder,
    TimerBindingsTypes, TimerContext, TimerId, TxMetadataBindingsTypes, neighbor,
};

pub(crate) use inspect::InspectPublisher;

mod ctx {
    use crate::bindings::interface_config::InterfaceConfigDefaults;

    use super::*;
    use thiserror::Error;

    /// Provides an implementation of [`BindingsContext`].
    #[derive(Clone)]
    pub(crate) struct BindingsCtx(Arc<BindingsCtxInner>);

    impl Deref for BindingsCtx {
        type Target = BindingsCtxInner;

        fn deref(&self) -> &BindingsCtxInner {
            let Self(this) = self;
            this.deref()
        }
    }

    pub(crate) struct Ctx {
        // `bindings_ctx` is the first member so all strongly-held references are
        // dropped before primary references held in `core_ctx` are dropped. Note
        // that dropping a primary reference while holding onto strong references
        // will cause a panic. See `netstack3_core::sync::PrimaryRc` for more
        // details.
        bindings_ctx: BindingsCtx,
        core_ctx: Arc<StackState<BindingsCtx>>,
    }

    /// Error observed while attempting to destroy the last remaining clone of `Ctx`.
    #[derive(Debug, Error)]
    pub enum DestructionError {
        /// Another reference of `BindingsCtx` still exists.
        #[error("bindings ctx still has {0} references")]
        BindingsCtxStillCloned(usize),
        /// Another reference of `CoreCtx` still exists.
        #[error("core ctx still has {0} references")]
        CoreCtxStillCloned(usize),
    }

    impl Ctx {
        fn new(
            config: GlobalConfig,
            interface_config: &InterfaceConfigDefaults,
            routes_change_sink: routes::ChangeSink,
            resource_removal: ResourceRemovalSink,
            multicast_admin: MulticastAdminEventSinks,
            ndp_ra_sink: ndp_watcher::WorkerRouterAdvertisementSink,
            power: PowerWorkerSink,
            destruction_dispatcher: SocketDestructionDispatcher,
        ) -> Self {
            let mut bindings_ctx = BindingsCtx(Arc::new(BindingsCtxInner::new(
                config,
                interface_config,
                routes_change_sink,
                resource_removal,
                multicast_admin,
                ndp_ra_sink,
                power,
                destruction_dispatcher,
            )));
            let persistence::State { opaque_iid_secret_key } =
                persistence::State::load_or_create(&mut bindings_ctx.rng());
            let mut state = StackStateBuilder::default();
            let _: &mut _ = state.ipv6_builder().slaac_stable_secret_key(opaque_iid_secret_key);
            let core_ctx = Arc::new(state.build_with_ctx(&mut bindings_ctx));

            Self { bindings_ctx, core_ctx }
        }

        pub(crate) fn bindings_ctx(&self) -> &BindingsCtx {
            &self.bindings_ctx
        }

        /// Destroys the last standing clone of [`Ctx`].
        pub(crate) fn try_destroy_last(self) -> Result<(), DestructionError> {
            let Self { bindings_ctx: BindingsCtx(bindings_ctx), core_ctx } = self;

            fn unwrap_and_drop_or_get_count<T>(arc: Arc<T>) -> Result<(), usize> {
                match Arc::try_unwrap(arc) {
                    Ok(t) => Ok(std::mem::drop(t)),
                    Err(arc) => Err(Arc::strong_count(&arc)),
                }
            }

            // Always destroy bindings ctx first.
            unwrap_and_drop_or_get_count(bindings_ctx)
                .map_err(DestructionError::BindingsCtxStillCloned)?;
            unwrap_and_drop_or_get_count(core_ctx).map_err(DestructionError::CoreCtxStillCloned)
        }

        pub(crate) fn api(&mut self) -> netstack3_core::CoreApi<'_, &mut BindingsCtx> {
            let Ctx { bindings_ctx, core_ctx } = self;
            core_ctx.api(bindings_ctx)
        }
    }

    impl Clone for Ctx {
        fn clone(&self) -> Self {
            let Self { bindings_ctx: BindingsCtx(inner), core_ctx } = self;
            Self { bindings_ctx: BindingsCtx(inner.clone()), core_ctx: core_ctx.clone() }
        }
    }

    /// Contains the information needed to start serving a network stack over FIDL.
    pub(crate) struct NetstackSeed {
        pub(crate) netstack: Netstack,
        pub(crate) interfaces_worker: interfaces_watcher::Worker,
        pub(crate) interfaces_watcher_sink: interfaces_watcher::WorkerWatcherSink,
        pub(crate) routes_change_runner: routes::ChangeRunner,
        pub(crate) ndp_watcher_worker: ndp_watcher::Worker,
        pub(crate) ndp_watcher_sink: ndp_watcher::WorkerWatcherSink,
        pub(crate) neighbor_worker: neighbor_worker::Worker,
        pub(crate) neighbor_watcher_sink: mpsc::Sender<neighbor_worker::NewWatcher>,
        pub(crate) resource_removal_worker: ResourceRemovalWorker,
        pub(crate) multicast_admin_workers: MulticastAdminWorkers,
        pub(crate) power_worker: PowerWorker,
    }

    impl NetstackSeed {
        pub(crate) fn new(
            config: GlobalConfig,
            interface_config: &InterfaceConfigDefaults,
        ) -> Self {
            let (interfaces_worker, interfaces_watcher_sink, interfaces_event_sink) =
                interfaces_watcher::Worker::new();
            let (routes_change_sink, routes_change_runner) = routes::create_sink_and_runner();
            let (resource_removal_worker, resource_removal_sink) = ResourceRemovalWorker::new();
            let (multicast_admin_workers, multicast_admin_sinks) =
                multicast_admin::new_workers_and_sinks();
            let (ndp_watcher_worker, ndp_watcher_sink, ndp_ra_sink) = ndp_watcher::Worker::new();
            let (power_worker, power_sink) = PowerWorker::new();
            let destruction_dispatcher = SocketDestructionDispatcher::default();

            let ctx = Ctx::new(
                config,
                &interface_config,
                routes_change_sink,
                resource_removal_sink,
                multicast_admin_sinks,
                ndp_ra_sink,
                power_sink,
                destruction_dispatcher,
            );
            let (neighbor_worker, neighbor_watcher_sink, neighbor_event_sink) =
                neighbor_worker::new_worker();
            Self {
                netstack: Netstack { ctx, interfaces_event_sink, neighbor_event_sink },
                interfaces_worker,
                interfaces_watcher_sink,
                routes_change_runner,
                ndp_watcher_worker,
                ndp_watcher_sink,
                neighbor_worker,
                neighbor_watcher_sink,
                resource_removal_worker,
                multicast_admin_workers,
                power_worker,
            }
        }
    }

    impl Default for NetstackSeed {
        fn default() -> Self {
            Self::new(Default::default(), &Default::default())
        }
    }
}

pub(crate) use ctx::{BindingsCtx, Ctx, NetstackSeed};

/// Extends the methods available to [`DeviceId`].
pub(crate) trait DeviceIdExt {
    /// Returns the state associated with devices.
    fn external_state(&self) -> DeviceSpecificInfo<'_>;
}

impl DeviceIdExt for DeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        match self {
            DeviceId::Ethernet(d) => DeviceSpecificInfo::Ethernet(d.external_state()),
            DeviceId::Loopback(d) => DeviceSpecificInfo::Loopback(d.external_state()),
            DeviceId::Blackhole(d) => DeviceSpecificInfo::Blackhole(d.external_state()),
            DeviceId::PureIp(d) => DeviceSpecificInfo::PureIp(d.external_state()),
        }
    }
}

impl DeviceIdExt for EthernetDeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        DeviceSpecificInfo::Ethernet(self.external_state())
    }
}

impl DeviceIdExt for LoopbackDeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        DeviceSpecificInfo::Loopback(self.external_state())
    }
}

impl DeviceIdExt for PureIpDeviceId<BindingsCtx> {
    fn external_state(&self) -> DeviceSpecificInfo<'_> {
        DeviceSpecificInfo::PureIp(self.external_state())
    }
}

/// Extends the methods available to [`Lifetime`].
trait LifetimeExt {
    /// Converts `self` to `zx::MonotonicInstant`.
    fn into_zx_time(self) -> zx::MonotonicInstant;
    /// Converts from `zx::MonotonicInstant` to `Self`.
    fn from_zx_time(t: zx::MonotonicInstant) -> Self;
}

impl LifetimeExt for Lifetime<StackTime> {
    fn into_zx_time(self) -> zx::MonotonicInstant {
        self.map_instant(|i| i.into_zx()).into_zx_time()
    }

    fn from_zx_time(t: zx::MonotonicInstant) -> Self {
        Lifetime::<zx::MonotonicInstant>::from_zx_time(t).map_instant(StackTime::from_zx)
    }
}

impl LifetimeExt for Lifetime<zx::MonotonicInstant> {
    fn into_zx_time(self) -> zx::MonotonicInstant {
        match self {
            Lifetime::Finite(time) => time,
            Lifetime::Infinite => zx::MonotonicInstant::INFINITE,
        }
    }

    fn from_zx_time(t: zx::MonotonicInstant) -> Self {
        if t == zx::MonotonicInstant::INFINITE { Self::Infinite } else { Self::Finite(t) }
    }
}

const LOOPBACK_NAME: &'static str = "lo";

/// Default MTU for loopback.
///
/// This value is also the default value used on Linux. As of writing:
///
/// ```shell
/// $ ip link show dev lo
/// 1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN mode DEFAULT group default qlen 1000
///     link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
/// ```
const DEFAULT_LOOPBACK_MTU: Mtu = Mtu::new(65536);

/// Default routing metric for newly created interfaces, if unspecified.
///
/// The value is currently kept in sync with the Netstack2 implementation.
const DEFAULT_INTERFACE_METRIC: u32 = 100;

/// Default routing metric for the loopback interface.
///
/// This value is larger than [`DEFAULT_INTERFACE_METRIC`] to avoid choosing
/// loopback if a route conflicts with another interface.
const LOOPBACK_INTERFACE_METRIC: u32 = 1000;

/// Global stack configuration.
#[derive(Debug)]
pub(crate) struct GlobalConfig {
    pub(crate) suspend_enabled: bool,
    pub(crate) sampled_stats_enabled: bool,
    pub(crate) multi_vmo: bool,
    pub(crate) max_rolling_capture_buffer_size: u32,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            suspend_enabled: false,
            sampled_stats_enabled: false,
            multi_vmo: false,
            max_rolling_capture_buffer_size: 16777216,
        }
    }
}

pub(crate) struct BindingsCtxInner {
    timers: timers::TimerDispatcher<TimerId<BindingsCtx>>,
    devices: Devices<DeviceId<BindingsCtx>>,
    routes: routes::ChangeSink,
    ndp_ra_sink: ndp_watcher::WorkerRouterAdvertisementSink,
    resource_removal: ResourceRemovalSink,
    multicast_admin: MulticastAdminEventSinks,
    config: GlobalConfig,
    counters: BindingsCounters,
    ebpf_manager: EbpfManager,
    packet_captures: PacketCaptures,
    power: PowerWorkerSink,
    wake_groups: waker::WakeGroups,
    settings: Settings,
    destruction_dispatcher: SocketDestructionDispatcher,
}

impl BindingsCtxInner {
    fn new(
        config: GlobalConfig,
        interface_config: &InterfaceConfigDefaults,
        routes_change_sink: routes::ChangeSink,
        resource_removal: ResourceRemovalSink,
        multicast_admin: MulticastAdminEventSinks,
        ndp_ra_sink: ndp_watcher::WorkerRouterAdvertisementSink,
        power: PowerWorkerSink,
        destruction_dispatcher: SocketDestructionDispatcher,
    ) -> Self {
        Self {
            timers: Default::default(),
            devices: Default::default(),
            routes: routes_change_sink,
            ndp_ra_sink,
            resource_removal,
            multicast_admin,
            config,
            counters: Default::default(),
            ebpf_manager: Default::default(),
            packet_captures: Default::default(),
            power,
            wake_groups: waker::WakeGroups::default(),
            settings: Settings::new(interface_config),
            destruction_dispatcher,
        }
    }
}

impl AsRef<Devices<DeviceId<BindingsCtx>>> for BindingsCtx {
    fn as_ref(&self) -> &Devices<DeviceId<BindingsCtx>> {
        &self.devices
    }
}

impl<D> ConversionContext for D
where
    D: AsRef<Devices<DeviceId<BindingsCtx>>>,
{
    fn get_core_id(&self, binding_id: BindingId) -> Option<DeviceId<BindingsCtx>> {
        self.as_ref().get_core_id(binding_id)
    }

    fn get_binding_id(&self, core_id: DeviceId<BindingsCtx>) -> BindingId {
        core_id.bindings_id().id
    }
}

impl InstantBindingsTypes for BindingsCtx {
    type Instant = StackTime;
    type AtomicInstant = AtomicStackTime;
}

impl InstantContext for BindingsCtx {
    fn now(&self) -> StackTime {
        StackTime::now()
    }
}

impl MatcherBindingsTypes for BindingsCtx {
    type DeviceClass = fidl_fuchsia_net_interfaces::PortClass;
    type BindingsPacketMatcher = Arc<SocketFilterProgram>;
}

impl SocketOpsFilterBindingContext<DeviceId<BindingsCtx>> for BindingsCtx {
    fn socket_ops_filter(&self) -> impl SocketOpsFilter<DeviceId<BindingsCtx>> {
        &self.ebpf_manager
    }
}

#[derive(Default)]
pub(crate) struct RngImpl;

impl RngImpl {
    fn new() -> Self {
        // A change detector in case SysRng is no longer a ZST and we should keep
        // state for it inside RngImpl.
        let SysRng {} = SysRng::default();
        RngImpl {}
    }
}

/// [`Rng`] for `RngImpl` relies entirely on the operating system to
/// generate random numbers and it needs not keep any state itself.
///
/// [`SysRng`] is a zero-sized type that provides randomness from the OS.
impl rand::TryRng for RngImpl {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(SysRng.try_next_u32().unwrap())
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(SysRng.try_next_u64().unwrap())
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
        Ok(SysRng.try_fill_bytes(dest).unwrap())
    }
}

impl rand::TryCryptoRng for RngImpl {}

impl RngContext for BindingsCtx {
    type Rng<'a> = RngImpl;

    fn rng(&mut self) -> RngImpl {
        RngImpl::new()
    }
}

impl TimerBindingsTypes for BindingsCtx {
    type Timer = timers::Timer<TimerId<BindingsCtx>>;
    type DispatchId = TimerId<BindingsCtx>;
    type UniqueTimerId = timers::UniqueTimerId<TimerId<BindingsCtx>>;
}

impl TimerContext for BindingsCtx {
    fn new_timer(&mut self, id: Self::DispatchId) -> Self::Timer {
        self.timers.new_timer(id)
    }

    fn schedule_timer_instant(
        &mut self,
        time: Self::Instant,
        timer: &mut Self::Timer,
    ) -> Option<Self::Instant> {
        timer.schedule(time.into_fuchsia_time()).map(Into::into)
    }

    fn cancel_timer(&mut self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        timer.cancel().map(Into::into)
    }

    fn scheduled_instant(&self, timer: &mut Self::Timer) -> Option<Self::Instant> {
        timer.scheduled_time().map(Into::into)
    }

    fn unique_timer_id(&self, timer: &Self::Timer) -> Self::UniqueTimerId {
        timer.unique_id()
    }
}

impl DeviceLayerStateTypes for BindingsCtx {
    type LoopbackDeviceState = LoopbackInfo;
    type EthernetDeviceState = EthernetInfo;
    type PureIpDeviceState = PureIpDeviceInfo;
    type DeviceIdentifier = DeviceIdAndName;
    type BlackholeDeviceState = BlackholeDeviceInfo;
}

impl TxMetadataBindingsTypes for BindingsCtx {
    type TxMetadata = CoreTxMetadata<Self>;
}

impl ReceiveQueueBindingsContext<LoopbackDeviceId<Self>> for BindingsCtx {
    fn wake_rx_task(&mut self, device: &LoopbackDeviceId<Self>) {
        let LoopbackInfo { static_common_info: _, dynamic_common_info: _, rx_notifier } =
            device.external_state();
        rx_notifier.schedule()
    }
}

impl<D: DeviceIdExt> TransmitQueueBindingsContext<D> for BindingsCtx {
    fn wake_tx_task(&mut self, device: &D) {
        let netdevice = match device.external_state() {
            DeviceSpecificInfo::Loopback(_) => panic!("loopback does not support tx tasks"),
            DeviceSpecificInfo::Blackhole(_) => panic!("blackhole does not support tx tasks"),
            DeviceSpecificInfo::Ethernet(EthernetInfo { netdevice, .. }) => netdevice,
            DeviceSpecificInfo::PureIp(PureIpDeviceInfo { netdevice, .. }) => netdevice,
        };
        netdevice.tx_notifier.schedule();
    }
}

impl DeviceLayerEventDispatcher for BindingsCtx {
    type DequeueContext = TxTaskState;

    fn send_ethernet_frame(
        &mut self,
        device: &EthernetDeviceId<Self>,
        frame: TxBuffer,
        dequeue_context: Option<&mut Self::DequeueContext>,
        csum_offload: Option<ChecksumOffloadResult>,
    ) -> Result<(), DeviceSendFrameError> {
        let EthernetInfo {
            mac: _,
            multicast_event_sink: _,
            common_info: _,
            netdevice,
            dynamic_info,
            status_sampler: _,
        } = device.external_state();
        let dynamic_info = dynamic_info.read();
        send_netdevice_frame(
            netdevice,
            &dynamic_info.netdevice,
            frame,
            fhardware_network::FrameType::Ethernet,
            dequeue_context,
            csum_offload,
        )
    }

    fn send_ip_packet(
        &mut self,
        device: &PureIpDeviceId<Self>,
        packet: TxBuffer,
        ip_version: IpVersion,
        dequeue_context: Option<&mut Self::DequeueContext>,
        csum_offload: Option<ChecksumOffloadResult>,
    ) -> Result<(), DeviceSendFrameError> {
        let frame_type = match ip_version {
            IpVersion::V4 => fhardware_network::FrameType::Ipv4,
            IpVersion::V6 => fhardware_network::FrameType::Ipv6,
        };
        let PureIpDeviceInfo { common_info: _, netdevice, dynamic_info, status_sampler: _ } =
            device.external_state();
        let dynamic_info = dynamic_info.read();
        send_netdevice_frame(
            netdevice,
            &dynamic_info,
            packet,
            frame_type,
            dequeue_context,
            csum_offload,
        )
    }
}

/// Send a frame on a Netdevice backed device.
fn send_netdevice_frame(
    netdevice: &StaticNetdeviceInfo,
    DynamicNetdeviceInfo {
        phy_up,
        common_info:
            DynamicCommonInfo {
                admin_enabled,
                ipv4_enabled: _,
                ipv6_enabled: _,
                mtu: _,
                events: _,
                control_hook: _,
                addresses: _,
            },
    }: &DynamicNetdeviceInfo,
    frame: TxBuffer,
    frame_type: fhardware_network::FrameType,
    dequeue_context: Option<&mut TxTaskState>,
    csum_offload: Option<ChecksumOffloadResult>,
) -> Result<(), DeviceSendFrameError> {
    let StaticNetdeviceInfo { handler, .. } = netdevice;
    if !(*phy_up && *admin_enabled) {
        debug!("dropped frame to {handler:?}, device offline");
        return Ok(());
    }

    let tx_buffer = match frame {
        packet::Either::A(netdev_tx_buffer) => {
            let tx_buffer = netdev_tx_buffer.into_inner();
            if let Some(TxTaskState { alloc_state, .. }) = dequeue_context {
                let _ = alloc_state.device_buffers_in_queue.fetch_sub(1, Ordering::Relaxed);
            }
            tx_buffer
        }
        packet::Either::B(buf_vec) => {
            let mut tx_buffer =
                match dequeue_context.and_then(|TxTaskState { tx_buffers, .. }| tx_buffers.pop()) {
                    Some(b) => b,
                    None => {
                        match handler.alloc_tx_buffer() {
                            Ok(Some(b)) => b,
                            Ok(None) => {
                                return Err(DeviceSendFrameError::NoBuffers);
                            }
                            Err(e) => {
                                error!("failed to allocate frame to {handler:?}: {e:?}");
                                // There's nothing core can do with this error, pretend like
                                // everything's okay.

                                // TODO(https://fxbug.dev/353718697): Consider signalling
                                // back through the handler that we want to shutdown this
                                // interface.
                                return Ok(());
                            }
                        }
                    }
                };
            let buf_len = buf_vec.as_ref().len();
            tx_buffer.shrink_to(buf_len).expect("failed to shrink buffer");
            let written = tx_buffer.io_mut().write_at(0, buf_vec.as_ref());
            debug_assert_eq!(written, buf_len);
            tx_buffer
        }
    };
    handler.send(frame_type, tx_buffer, csum_offload);
    Ok(())
}

impl<I: IpExt> IcmpEchoBindingsContext<I, DeviceId<BindingsCtx>> for BindingsCtx {
    fn receive_icmp_echo_reply<B: BufferMut>(
        &mut self,
        conn: &IcmpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>,
        device: &DeviceId<BindingsCtx>,
        src_ip: I::Addr,
        dst_ip: I::Addr,
        id: u16,
        data: B,
    ) -> Result<(), ReceiveIcmpEchoError> {
        conn.external_data().receive_icmp_echo_reply(device, src_ip, dst_ip, id, data)
    }
}

impl IcmpEchoBindingsTypes for BindingsCtx {
    type ExternalData<I: Ip> = socket::datagram::DatagramSocketExternalData<I>;
    type SocketWritableListener = socket::event_pair::SocketEventPair;
    type SendToken = ();
}

impl<I: IpExt> UdpReceiveBindingsContext<I, DeviceId<BindingsCtx>> for BindingsCtx {
    fn receive_udp(
        &mut self,
        id: &UdpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>,
        device_id: &DeviceId<BindingsCtx>,
        meta: UdpPacketMeta<I>,
        body: &[u8],
    ) -> Result<(), ReceiveUdpError> {
        id.external_data()
            .receive_udp(device_id, meta, body)
            .map_err(|NoSpace {}| ReceiveUdpError::QueueFull)
    }

    fn on_socket_error(
        &mut self,
        id: &UdpSocketId<I, WeakDeviceId<BindingsCtx>, BindingsCtx>,
        err: netstack3_core::PendingDatagramSocketError,
    ) {
        id.external_data().on_socket_error(err)
    }
}

impl UdpBindingsTypes for BindingsCtx {
    type ExternalData<I: Ip> = socket::datagram::DatagramSocketExternalData<I>;
    type SocketWritableListener = socket::event_pair::SocketEventPair;
    type SendToken = ();
}

impl<I: Ip> EventContext<IpDeviceEvent<DeviceId<BindingsCtx>, I, StackTime>> for BindingsCtx {
    fn on_event(&mut self, event: IpDeviceEvent<DeviceId<BindingsCtx>, I, StackTime>) {
        match event {
            IpDeviceEvent::AddressAdded {
                device,
                addr,
                state,
                valid_until,
                preferred_lifetime,
            } => {
                let valid_until = valid_until.into_zx_time();

                self.notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressAdded {
                        addr: addr.into(),
                        assignment_state: state,
                        valid_until,
                        preferred_lifetime: preferred_lifetime.map_instant(|i| i.into_zx()),
                    },
                );
                self.notify_address_update(&device, addr.addr().into(), state);
            }
            IpDeviceEvent::AddressRemoved { device, addr, reason } => {
                self.notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressRemoved(addr.to_ip_addr()),
                );
                match reason {
                    AddressRemovedReason::Manual => (),
                    AddressRemovedReason::DadFailed => self.notify_address_removed(
                        &device,
                        addr.into(),
                        interfaces_admin::AddressStateProviderCancellationReason::DadFailed,
                    ),
                    AddressRemovedReason::Forfeited => self.notify_address_removed(
                        &device,
                        addr.into(),
                        interfaces_admin::AddressStateProviderCancellationReason::Forfeited,
                    ),
                }
            }
            IpDeviceEvent::AddressStateChanged { device, addr, state } => {
                self.notify_interface_update(
                    &device,
                    InterfaceUpdate::AddressAssignmentStateChanged {
                        addr: addr.to_ip_addr(),
                        new_state: state,
                    },
                );
                self.notify_address_update(&device, addr.into(), state);
            }
            IpDeviceEvent::EnabledChanged { device, ip_enabled } => self.notify_interface_update(
                &device,
                InterfaceUpdate::IpEnabledChanged { version: I::VERSION, enabled: ip_enabled },
            ),
            IpDeviceEvent::AddressPropertiesChanged {
                device,
                addr,
                valid_until,
                preferred_lifetime,
            } => self.notify_interface_update(
                &device,
                InterfaceUpdate::AddressPropertiesChanged {
                    addr: addr.to_ip_addr(),
                    update: AddressPropertiesUpdate {
                        valid_until: valid_until.into_zx_time(),
                        preferred_lifetime: preferred_lifetime.map_instant(|i| i.into_zx()),
                    },
                },
            ),
        };
    }
}

impl<I: IpExt + FidlMulticastAdminIpExt> EventContext<IpLayerEvent<DeviceId<BindingsCtx>, I>>
    for BindingsCtx
{
    fn on_event(&mut self, event: IpLayerEvent<DeviceId<BindingsCtx>, I>) {
        // NB: Downgrade the device ID immediately because we're about to stash
        // the event in a channel.
        let event = event.map_device(|d| d.downgrade());
        match event {
            IpLayerEvent::AddRoute(entry) => {
                self.routes.fire_main_table_route_change_and_forget::<I>(routes::Change::RouteOp(
                    routes::RouteOp::Add(entry),
                    routes::SetMembership::CoreNdp,
                ))
            }
            IpLayerEvent::RemoveRoutes { subnet, device, gateway } => {
                self.routes.fire_main_table_route_change_and_forget::<I>(routes::Change::RouteOp(
                    routes::RouteOp::RemoveMatching {
                        subnet,
                        device,
                        gateway: routes::Matcher::Exact(gateway),
                        metric: routes::Matcher::Any,
                    },
                    routes::SetMembership::CoreNdp,
                ))
            }
            IpLayerEvent::MulticastForwarding(event) => {
                self.multicast_admin.sink::<I>().dispatch_multicast_forwarding_event(event);
            }
        };
    }
}

impl<I: Ip> EventContext<neighbor::Event<Mac, EthernetDeviceId<Self>, I, StackTime>>
    for BindingsCtx
{
    fn on_event(
        &mut self,
        neighbor::Event { device, kind, addr, at }: neighbor::Event<
            Mac,
            EthernetDeviceId<Self>,
            I,
            StackTime,
        >,
    ) {
        device.external_state().with_dynamic_info(|i| {
            i.neighbor_event_sink
                .unbounded_send(neighbor_worker::Event {
                    id: device.downgrade(),
                    kind,
                    addr: addr.into(),
                    at,
                })
                .expect("should be able to send neighbor event")
        })
    }
}

impl EventContext<RouterAdvertisementEvent<DeviceId<BindingsCtx>>> for BindingsCtx {
    fn on_event(&mut self, event: RouterAdvertisementEvent<DeviceId<BindingsCtx>>) {
        let RouterAdvertisementEvent { options_bytes, source, device } = event;
        // NOTE: one could imagine sending a WeakDeviceId to the NDP watcher
        // worker instead of a plain scalar ID.
        // However, the worker itself does some queuing of watch entries that
        // themselves will keep the device ID as a scalar rather than using the
        // weak/strong reference-counted IDs.
        // For this reason, we downgrade straight to a scalar here to avoid
        // giving off the impression that we are promising anything to do with
        // device removal.
        let interface_id: BindingId = device.bindings_id().id;
        match self.ndp_ra_sink.try_send_router_advertisement(options_bytes, source, interface_id) {
            Ok(()) => (),
            Err(err) => match err {
                RouterAdvertisementSinkError::WorkerClosed(ndp_watcher::WorkerClosedError) => {
                    warn!("could not send RA to NDP watcher worker: worker closed");
                }
                RouterAdvertisementSinkError::SinkFull => {
                    warn!("could not send RA to NDP watcher worker: sink full");
                }
            },
        }
    }
}

impl EventContext<EthernetDeviceEvent<EthernetDeviceId<BindingsCtx>>> for BindingsCtx {
    fn on_event(&mut self, event: EthernetDeviceEvent<EthernetDeviceId<BindingsCtx>>) {
        let (device, event) = match event {
            EthernetDeviceEvent::MulticastJoin { device, addr } => {
                (device, LinkMulticastEvent::Join(addr))
            }
            EthernetDeviceEvent::MulticastLeave { device, addr } => {
                (device, LinkMulticastEvent::Leave(addr))
            }
        };

        device
            .external_state()
            .multicast_event_sink
            .unbounded_send(event)
            .expect("sender was orphaned unexpectedly");
    }
}

impl DeferredResourceRemovalContext for BindingsCtx {
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn defer_removal<T: Send + 'static>(&mut self, receiver: Self::ReferenceReceiver<T>) {
        self.resource_removal.defer_removal_with_receiver(receiver);
    }
}

impl IpRoutingBindingsTypes for BindingsCtx {
    type RoutingTableId = u32;
}

impl MarksBindingsContext for BindingsCtx {
    fn marks_to_keep_on_egress() -> &'static [MarkDomain] {
        // When a packet traverses the loopback device, it should keep the
        // first mark (initially set by the socket) and drop any other marks.
        // Particularly socket UID is invalidated. This matches Linux behavior.
        const MARKS: [MarkDomain; 1] = [fidl_mark_domain_to_core(fnet::MARK_DOMAIN_SO_MARK)];
        &MARKS
    }

    fn marks_to_set_on_ingress() -> &'static [MarkDomain] {
        // Set socket UID in the ingress packets, from the socket. This matches
        // Linux behavior and ensures that `bpf_get_socket_uid()` returns the
        // correct value when executing LOCAL_INGRESS filters.
        const MARKS: [MarkDomain; 1] = [fidl_mark_domain_to_core(fnet::MARK_DOMAIN_SOCKET_UID)];
        &MARKS
    }
}

impl Ctx {
    pub(crate) fn apply_interface_defaults(&mut self, device_id: &DeviceId<BindingsCtx>) {
        let config_type = match device_id {
            DeviceId::Ethernet(_) => InterfaceConfigType::Ethernet,
            DeviceId::PureIp(_) => InterfaceConfigType::PureIp,
            DeviceId::Blackhole(_) => InterfaceConfigType::Blackhole,
            DeviceId::Loopback(_) => InterfaceConfigType::Loopback,
        };
        let defaults = self.bindings_ctx().settings.interface_defaults.read();
        let ipv6_config = defaults.to_core_ipv6_update(config_type);
        let ipv4_config = defaults.to_core_ipv4_update(config_type);
        let device_config = defaults.to_core_device_update(config_type);
        defaults.bindings_config.apply_interface_defaults(device_id);

        let _: Ipv6DeviceConfigurationUpdate =
            self.api().device_ip::<Ipv6>().update_configuration(&device_id, ipv6_config).unwrap();
        let _: Ipv4DeviceConfigurationUpdate =
            self.api().device_ip::<Ipv4>().update_configuration(&device_id, ipv4_config).unwrap();
        let _: DeviceConfigurationUpdate =
            self.api().device_any().update_configuration(&device_id, device_config).unwrap();
    }
}

impl BindingsCtx {
    fn notify_interface_update(&self, device: &DeviceId<BindingsCtx>, event: InterfaceUpdate) {
        device
            .external_state()
            .with_common_info(|i| i.events.notify(event).expect("interfaces worker closed"));
    }

    /// Notify `AddressStateProvider.WatchAddressAssignmentState` watchers.
    fn notify_address_update(
        &self,
        device: &DeviceId<BindingsCtx>,
        address: SpecifiedAddr<IpAddr>,
        state: netstack3_core::ip::IpAddressState,
    ) {
        // Note that not all addresses have an associated watcher (e.g. loopback
        // address & autoconfigured SLAAC addresses).
        device.external_state().with_common_info(|i| {
            if let Some(address_info) = i.addresses.get(&address) {
                address_info
                    .assignment_state_sender
                    .unbounded_send(state.into_fidl())
                    .expect("assignment state receiver unexpectedly disconnected");
            }
        })
    }

    fn notify_address_removed(
        &mut self,
        device: &DeviceId<BindingsCtx>,
        address: SpecifiedAddr<IpAddr>,
        reason: interfaces_admin::AddressStateProviderCancellationReason,
    ) {
        device.external_state().with_common_info_mut(|i| {
            if let Some(address_info) = i.addresses.get_mut(&address) {
                let devices::FidlWorkerInfo { worker: _, cancelation_sender } =
                    &mut address_info.address_state_provider;
                if let Some(sender) = cancelation_sender.take() {
                    sender
                        .send(reason)
                        .expect("assignment state receiver unexpectedly disconnected");
                }
            }
        })
    }

    pub(crate) async fn apply_route_change<I: Ip>(
        &self,
        change: routes::Change<I>,
    ) -> Result<routes::ChangeOutcome, routes::ChangeError> {
        self.routes.send_main_table_route_change(change).await
    }

    pub(crate) async fn apply_route_change_either(
        &self,
        change: routes::ChangeEither,
    ) -> Result<routes::ChangeOutcome, routes::ChangeError> {
        match change {
            routes::ChangeEither::V4(change) => self.apply_route_change::<Ipv4>(change).await,
            routes::ChangeEither::V6(change) => self.apply_route_change::<Ipv6>(change).await,
        }
    }

    pub(crate) async fn remove_routes_on_device(
        &self,
        device: &netstack3_core::device::WeakDeviceId<Self>,
    ) {
        match self
            .apply_route_change::<Ipv4>(routes::Change::RemoveMatchingDevice(device.clone()))
            .await
            .expect("deleting routes on device during removal should succeed")
        {
            routes::ChangeOutcome::Changed | routes::ChangeOutcome::NoChange => {
                // We don't care whether there were any routes on the device or not.
            }
        }
        match self
            .apply_route_change::<Ipv6>(routes::Change::RemoveMatchingDevice(device.clone()))
            .await
            .expect("deleting routes on device during removal should succeed")
        {
            routes::ChangeOutcome::Changed | routes::ChangeOutcome::NoChange => {
                // We don't care whether there were any routes on the device or not.
            }
        }
    }

    pub(crate) fn get_route_table_name<I: Ip>(
        &self,
        table_id: routes::TableId<I>,
        responder: fnet_routes::StateGetRouteTableNameResponder,
    ) {
        self.routes.get_route_table_name(table_id, responder)
    }
}

fn add_loopback_ip_addrs(
    ctx: &mut Ctx,
    loopback: &DeviceId<BindingsCtx>,
) -> Result<(), ExistsError> {
    for addr_subnet in [
        AddrSubnetEither::V4(
            AddrSubnet::from_witness(Ipv4::LOOPBACK_ADDRESS, Ipv4::LOOPBACK_SUBNET.prefix())
                .expect("error creating IPv4 loopback AddrSub"),
        ),
        AddrSubnetEither::V6(
            AddrSubnet::from_witness(Ipv6::LOOPBACK_ADDRESS, Ipv6::LOOPBACK_SUBNET.prefix())
                .expect("error creating IPv6 loopback AddrSub"),
        ),
    ] {
        ctx.api().device_ip_any().add_ip_addr_subnet(loopback, addr_subnet).map_err(
            |e| match e {
                AddIpAddrSubnetError::Exists => ExistsError,
                AddIpAddrSubnetError::InvalidAddr => {
                    panic!("loopback address should not be invalid")
                }
            },
        )?
    }
    Ok(())
}

/// Adds the IPv4 and IPv6 Loopback and multicast subnet routes, and the IPv4
/// limited broadcast subnet route.
async fn add_loopback_routes(bindings_ctx: &BindingsCtx, loopback: &DeviceId<BindingsCtx>) {
    use netstack3_core::routes::{AddableEntry, AddableMetric};

    let v4_changes = [
        AddableEntry::without_gateway(
            Ipv4::LOOPBACK_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
        AddableEntry::without_gateway(
            Ipv4::MULTICAST_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
    ]
    .into_iter()
    .map(|entry| {
        routes::Change::<Ipv4>::RouteOp(
            routes::RouteOp::Add(entry),
            routes::SetMembership::Loopback,
        )
    })
    .map(Into::into);

    let v6_changes = [
        AddableEntry::without_gateway(
            Ipv6::LOOPBACK_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
        AddableEntry::without_gateway(
            Ipv6::MULTICAST_SUBNET,
            loopback.downgrade(),
            AddableMetric::MetricTracksInterface,
        ),
    ]
    .into_iter()
    .map(|entry| {
        routes::Change::<Ipv6>::RouteOp(
            routes::RouteOp::Add(entry),
            routes::SetMembership::Loopback,
        )
    })
    .map(Into::into);

    for change in v4_changes.chain(v6_changes) {
        bindings_ctx
            .apply_route_change_either(change)
            .await
            .map(|outcome| assert_matches!(outcome, routes::ChangeOutcome::Changed))
            .expect("adding loopback routes should succeed");
    }
}

/// The netstack.
///
/// Provides the entry point for creating a netstack to be served as a
/// component.
#[derive(Clone)]
pub(crate) struct Netstack {
    ctx: Ctx,
    interfaces_event_sink: interfaces_watcher::WorkerInterfaceSink,
    neighbor_event_sink: mpsc::UnboundedSender<neighbor_worker::Event>,
}

fn create_interface_event_producer(
    interfaces_event_sink: &crate::bindings::interfaces_watcher::WorkerInterfaceSink,
    id: BindingId,
    properties: InterfaceProperties,
) -> InterfaceEventProducer {
    interfaces_event_sink.reserve_interface(id, properties).expect("interface worker not running")
}

impl Netstack {
    fn create_interface_event_producer(
        &self,
        id: BindingId,
        properties: InterfaceProperties,
    ) -> InterfaceEventProducer {
        create_interface_event_producer(&self.interfaces_event_sink, id, properties)
    }

    async fn add_default_rule<I: Ip>(&self) {
        self.ctx.bindings_ctx().routes.add_default_rule::<I>().await
    }

    async fn add_loopback(
        &mut self,
        scope: &fasync::ScopeHandle,
    ) -> (oneshot::Sender<fnet_interfaces_admin::InterfaceRemovedReason>, BindingId) {
        let guard = scope.active_guard().expect("scope should be active");
        let inner_scope = scope.new_child_with_name("loopback_inner");

        // Add and initialize the loopback interface with the IPv4 and IPv6
        // loopback addresses and on-link routes to the loopback subnets.
        let devices: &Devices<_> = self.ctx.bindings_ctx().as_ref();
        let (control_sender, control_receiver) =
            interfaces_admin::OwnedControlHandle::new_channel();
        let loopback_rx_notifier = Default::default();

        let (binding_id, binding_id_alloc) = devices
            .try_reserve_name_and_alloc_new_id(LOOPBACK_NAME.to_string())
            .expect("loopback device should only be added once");
        let events = self.create_interface_event_producer(
            binding_id,
            InterfaceProperties {
                name: LOOPBACK_NAME.to_string(),
                port_class: fidl_fuchsia_net_interfaces_ext::PortClass::Loopback,
                // Loopback is not backed by a netdevice port.
                port_identity_koid: None,
            },
        );

        let loopback_info = LoopbackInfo {
            static_common_info: StaticCommonInfo {
                authorization_token: zx::Event::create(),
                local_route_tables: None,
            },
            dynamic_common_info: CoreRwLock::new(DynamicCommonInfo::new_for_loopback(
                DEFAULT_LOOPBACK_MTU,
                events,
                control_sender,
            )),
            rx_notifier: loopback_rx_notifier,
        };

        let loopback = self.ctx.api().device::<LoopbackDevice>().add_device(
            DeviceIdAndName { id: binding_id, name: LOOPBACK_NAME.to_string() },
            LoopbackCreationProperties { mtu: DEFAULT_LOOPBACK_MTU },
            RawMetric(LOOPBACK_INTERFACE_METRIC),
            loopback_info,
            Default::default(),
        );

        let LoopbackInfo { static_common_info: _, dynamic_common_info: _, rx_notifier } =
            loopback.external_state();
        let _: fasync::JoinHandle<()> = inner_scope.spawn_guarded_assert_cancelled(
            guard.clone(),
            crate::bindings::devices::rx_task(
                self.ctx.clone(),
                rx_notifier.watcher(),
                loopback.clone(),
            ),
        );
        let loopback: DeviceId<_> = loopback.into();
        self.ctx
            .bindings_ctx()
            .devices
            .add_device_and_start_publishing(binding_id_alloc, loopback.clone());

        self.ctx.apply_interface_defaults(&loopback);
        add_loopback_ip_addrs(&mut self.ctx, &loopback).expect("error adding loopback addresses");
        add_loopback_routes(self.ctx.bindings_ctx(), &loopback).await;

        let (stop_sender, stop_receiver) = oneshot::channel();

        // Loopback interface can't be removed.
        let removable = false;
        // Loopback doesn't have a defined state stream, provide a stream that
        // never yields anything.
        let state_stream = futures::stream::pending();
        let _: fasync::JoinHandle<()> = scope.spawn_guarded_assert_cancelled(
            guard,
            interfaces_admin::run_interface_control(
                self.ctx.clone(),
                inner_scope,
                binding_id,
                stop_receiver,
                control_receiver,
                removable,
                state_stream,
            ),
        );
        (stop_sender, binding_id)
    }
}

pub(crate) enum Service {
    DnsServerWatcher(fidl_fuchsia_net_name::DnsServerWatcherRequestStream),
    DebugDiagnostics(fidl::endpoints::ServerEnd<fidl_fuchsia_net_debug::DiagnosticsMarker>),
    DebugInterfaces(fidl_fuchsia_net_debug::InterfacesRequestStream),
    FilterControl(fidl_fuchsia_net_filter::ControlRequestStream),
    FilterState(fidl_fuchsia_net_filter::StateRequestStream),
    HealthCheck(fidl_fuchsia_update_verify::ComponentOtaHealthCheckRequestStream),
    Interfaces(fidl_fuchsia_net_interfaces::StateRequestStream),
    InterfacesAdmin(fidl_fuchsia_net_interfaces_admin::InstallerRequestStream),
    MulticastAdminV4(fidl_fuchsia_net_multicast_admin::Ipv4RoutingTableControllerRequestStream),
    MulticastAdminV6(fidl_fuchsia_net_multicast_admin::Ipv6RoutingTableControllerRequestStream),
    NdpWatcher(fidl_fuchsia_net_ndp::RouterAdvertisementOptionWatcherProviderRequestStream),
    NeighborController(fidl_fuchsia_net_neighbor::ControllerRequestStream),
    Neighbor(fidl_fuchsia_net_neighbor::ViewRequestStream),
    PacketSocket(fidl_fuchsia_posix_socket_packet::ProviderRequestStream),
    PacketCapture(fidl_fuchsia_net_debug::PacketCaptureProviderRequestStream),
    RawSocket(fidl_fuchsia_posix_socket_raw::ProviderRequestStream),
    RootFilter(fidl_fuchsia_net_root::FilterRequestStream),
    RootInterfaces(fidl_fuchsia_net_root::InterfacesRequestStream),
    RootRoutesV4(fidl_fuchsia_net_root::RoutesV4RequestStream),
    RootRoutesV6(fidl_fuchsia_net_root::RoutesV6RequestStream),
    RoutesState(fidl_fuchsia_net_routes::StateRequestStream),
    RoutesStateV4(fidl_fuchsia_net_routes::StateV4RequestStream),
    RoutesStateV6(fidl_fuchsia_net_routes::StateV6RequestStream),
    RoutesAdminV4(fnet_routes_admin::RouteTableV4RequestStream),
    RoutesAdminV6(fnet_routes_admin::RouteTableV6RequestStream),
    RouteTableProviderV4(fnet_routes_admin::RouteTableProviderV4RequestStream),
    RouteTableProviderV6(fnet_routes_admin::RouteTableProviderV6RequestStream),
    RuleTableV4(fnet_routes_admin::RuleTableV4RequestStream),
    RuleTableV6(fnet_routes_admin::RuleTableV6RequestStream),
    Socket(fidl_fuchsia_posix_socket::ProviderRequestStream),
    SocketControl(fidl_fuchsia_net_filter::SocketControlRequestStream),
    Control(fidl_fuchsia_net_sockets::ControlRequestStream),
    SocketDiagnostics(fidl_fuchsia_net_sockets::DiagnosticsRequestStream),
    Stack(fidl_fuchsia_net_stack::StackRequestStream),
    SettingsControl(fidl_fuchsia_net_settings::ControlRequestStream),
    SettingsState(fidl_fuchsia_net_settings::StateRequestStream),
    WakeGroupProvider(fidl_fuchsia_net_power::WakeGroupProviderRequestStream),
}

impl NetstackSeed {
    /// Consumes the netstack and starts serving all the FIDL services it
    /// implements to the outgoing service directory.
    pub(crate) async fn serve<S: futures::Stream<Item = Service>>(
        self,
        services: S,
        inspect_publisher: InspectPublisher<'_>,
    ) {
        let Self {
            mut netstack,
            interfaces_worker,
            interfaces_watcher_sink,
            mut routes_change_runner,
            ndp_watcher_worker,
            ndp_watcher_sink,
            neighbor_worker,
            neighbor_watcher_sink,
            resource_removal_worker,
            mut multicast_admin_workers,
            power_worker,
        } = self;

        // Declare distinct levels of workers, organized by shutdown order
        // requirements.
        //
        // - Level 1 workers can be cancelled and terminated as soon as service
        //   is finished. They may have dependencies on Level 2 workers being
        //   running.
        // - Level 2 workers are only cancelled after all level 1 workers are
        //   done.
        let level1_workers = fasync::Scope::new_with_name("workers/1");
        let level2_workers = fasync::Scope::new_with_name("workers/2");

        // Start servicing timers.
        let mut timer_handler_ctx = netstack.ctx.clone();
        let _: fasync::JoinHandle<()> = netstack.ctx.bindings_ctx().timers.spawn(
            level1_workers.as_handle(),
            move |dispatch, timer| {
                timer_handler_ctx.api().handle_timer(dispatch, timer);
            },
        );

        let (dispatchers_v4, dispatchers_v6) = routes_change_runner.update_dispatchers();

        // Start executing routes changes.
        let routes_change_task = level2_workers
            .spawn_new_guard_assert_cancelled({
                let ctx = netstack.ctx.clone();
                async move { routes_change_runner.run(ctx).await }
            })
            .expect("scope cancelled");

        // Start running the multicast admin worker.
        let multicast_admin_task = level2_workers
            .spawn_new_guard_assert_cancelled({
                let ctx = netstack.ctx.clone();
                async move { multicast_admin_workers.run(ctx).await }
            })
            .expect("scope cancelled");

        // Start executing delayed resource removal.
        let resource_removal_task = level2_workers
            .spawn_new_guard_assert_cancelled(resource_removal_worker.run())
            .expect("scope cancelled");

        // We don't need to join the power worker task, it should be resilient
        // to being dropped alongside the other level2 workers.
        let _: fasync::JoinHandle<()> = level2_workers
            .spawn(power_worker.run(netstack.ctx.bindings_ctx().config.suspend_enabled));

        netstack.add_default_rule::<Ipv4>().await;
        netstack.add_default_rule::<Ipv6>().await;

        let loopback_scope = level1_workers.new_detached_child("loopback");
        let (loopback_stopper, _): (_, BindingId) = netstack.add_loopback(&loopback_scope).await;

        let _: fasync::JoinHandle<()> = level1_workers
            .spawn_new_guard_assert_cancelled(async move {
                let result = interfaces_worker.run().await;
                let watchers = result.expect("interfaces worker ended with an error");
                if !watchers.is_empty() {
                    warn!("interfaces worker shut down, dropped {} watchers", watchers.len());
                }
            })
            .expect("scope cancelled");

        let _: fasync::JoinHandle<()> = level1_workers
            .spawn_new_guard_assert_cancelled(ndp_watcher_worker.run())
            .expect("scope cancelled");

        let _: fasync::JoinHandle<()> = level1_workers
            .spawn_new_guard_assert_cancelled({
                let ctx = netstack.ctx.clone();
                neighbor_worker.run(ctx)
            })
            .expect("scope cancelled");

        let inspector = inspect_publisher.inspector();
        let inspect_nodes = {
            // The presence of the health check node is useful even though the
            // status will always be OK because the same node exists
            // in NS2 and this helps for test assertions to guard against
            // issues such as https://fxbug.dev/326510415.
            let mut health = fuchsia_inspect::health::Node::new(inspector.root());
            health.set_ok();
            let socket_ctx = netstack.ctx.clone();
            let sockets = inspector.root().create_lazy_child("Sockets", move || {
                futures::future::ok(inspect::sockets(&mut socket_ctx.clone())).boxed()
            });
            let routes_ctx = netstack.ctx.clone();
            let routes = inspector.root().create_lazy_child("Routes", move || {
                futures::future::ok(inspect::routes(&mut routes_ctx.clone())).boxed()
            });
            let multicast_forwarding_ctx = netstack.ctx.clone();
            let multicast_forwarding =
                inspector.root().create_lazy_child("MulticastForwarding", move || {
                    futures::future::ok(inspect::multicast_forwarding(
                        &mut multicast_forwarding_ctx.clone(),
                    ))
                    .boxed()
                });
            let devices_ctx = netstack.ctx.clone();
            let devices = inspector.root().create_lazy_child("Devices", move || {
                futures::future::ok(inspect::devices(&mut devices_ctx.clone())).boxed()
            });
            let neighbors_ctx = netstack.ctx.clone();
            let neighbors = inspector.root().create_lazy_child("Neighbors", move || {
                futures::future::ok(inspect::neighbors(neighbors_ctx.clone())).boxed()
            });
            let counters_ctx = netstack.ctx.clone();
            let counters = inspector.root().create_lazy_child("Counters", move || {
                futures::future::ok(inspect::counters(&mut counters_ctx.clone())).boxed()
            });
            let filter_ctx = netstack.ctx.clone();
            let filtering_state =
                inspector.root().create_lazy_child("Filtering State", move || {
                    futures::future::ok(inspect::filtering_state(&mut filter_ctx.clone())).boxed()
                });
            (
                health,
                sockets,
                routes,
                multicast_forwarding,
                devices,
                neighbors,
                counters,
                filtering_state,
            )
        };

        if netstack.ctx.bindings_ctx().config.sampled_stats_enabled {
            let _: fasync::JoinHandle<()> = level1_workers
                .spawn(StatsSampler::new(netstack.ctx.clone(), inspector.root()).run());
        }

        let diagnostics_handler = debug_fidl_worker::DiagnosticsHandler::default();

        // Keep a clone of Ctx around for teardown before moving it to the
        // services future.
        let teardown_ctx = netstack.ctx.clone();

        // Use a reference to the watcher sink in the services loop.
        let interfaces_watcher_sink_ref = &interfaces_watcher_sink;
        let ndp_watcher_sink_ref = &ndp_watcher_sink;
        let neighbor_watcher_sink_ref = &neighbor_watcher_sink;

        let services_scope = fasync::Scope::new_with_name("services");
        let sockets_scope = services_scope.new_detached_child("sockets");

        let filter_update_dispatcher = filter::UpdateDispatcher::default();
        let services_handle = services_scope.to_handle();

        let services_fut = services
            // NB: Move here is load bearing to ensure things that are moved in
            // do not outlive the services stream.
            .map(move |s| match s {
                Service::Stack(stack) => services_handle
                    .spawn_request_stream_handler(stack, |rs| {
                        stack_fidl_worker::StackFidlWorker::serve(netstack.clone(), rs)
                    }),
                Service::Socket(socket) => sockets_scope
                    .spawn_request_stream_handler(socket, |rs| {
                        socket::serve(netstack.ctx.clone(), rs)
                    }),
                Service::PacketSocket(socket) => sockets_scope
                    .spawn_request_stream_handler(socket, |rs| {
                        socket::packet::serve(netstack.ctx.clone(), rs)
                    }),
                Service::RawSocket(socket) => sockets_scope
                    .spawn_request_stream_handler(socket, |rs| {
                        socket::raw::serve(netstack.ctx.clone(), rs)
                    }),
                Service::SocketDiagnostics(stream) => services_handle
                    .spawn_request_stream_handler(stream, |stream| {
                        sockets_fidl::serve_diagnostics(stream, netstack.ctx.clone())
                    }),
                Service::RootInterfaces(root_interfaces) => services_handle
                    .spawn_request_stream_handler(root_interfaces, |rs| {
                        root_fidl_worker::serve_interfaces(netstack.clone(), rs)
                    }),
                Service::RootFilter(root_filter) => {
                    services_handle.spawn_request_stream_handler(root_filter, |rs| {
                        filter::serve_root(
                            rs,
                            filter_update_dispatcher.clone(),
                            netstack.ctx.clone(),
                        )
                    })
                }
                Service::SocketControl(rs) => services_handle
                    .spawn_request_stream_handler(rs, |rs| {
                        filter::socket_filters::serve_socket_control(rs, netstack.ctx.clone())
                    }),
                Service::Control(stream) => services_handle
                    .spawn_request_stream_handler(stream, |stream| {
                        sockets_fidl::serve_control(stream, netstack.ctx.clone())
                    }),
                Service::RoutesState(rs) => services_handle
                    .spawn_request_stream_handler(rs, |rs| {
                        routes::state::serve_state(rs, netstack.ctx.clone())
                    }),
                Service::RoutesStateV4(rs) => services_handle
                    .spawn_request_stream_handler(rs, |rs| {
                        routes::state::serve_state_v4(rs, dispatchers_v4.clone())
                    }),
                Service::RoutesStateV6(rs) => services_handle
                    .spawn_request_stream_handler(rs, |rs| {
                        routes::state::serve_state_v6(rs, dispatchers_v6.clone())
                    }),
                Service::RoutesAdminV4(rs) => {
                    let ctx = netstack.ctx.clone();
                    services_handle.spawn_request_stream_handler(rs, |rs| async move {
                        routes::admin::serve_route_table::<Ipv4, routes::admin::MainRouteTable>(
                            rs,
                            routes::admin::MainRouteTable::new::<Ipv4>(&ctx),
                            &ctx,
                        )
                        .await
                    })
                }
                Service::RoutesAdminV6(rs) => {
                    let ctx = netstack.ctx.clone();
                    services_handle.spawn_request_stream_handler(rs, |rs| async move {
                        routes::admin::serve_route_table::<Ipv6, routes::admin::MainRouteTable>(
                            rs,
                            routes::admin::MainRouteTable::new::<Ipv6>(&ctx),
                            &ctx,
                        )
                        .await
                    })
                }
                Service::RouteTableProviderV4(stream) => services_handle
                    .spawn_request_stream_handler(stream, |stream| {
                        routes::admin::serve_route_table_provider::<Ipv4>(
                            stream,
                            netstack.ctx.clone(),
                        )
                    }),
                Service::RouteTableProviderV6(stream) => services_handle
                    .spawn_request_stream_handler(stream, |stream| {
                        routes::admin::serve_route_table_provider::<Ipv6>(
                            stream,
                            netstack.ctx.clone(),
                        )
                    }),
                Service::RuleTableV4(rule_table) => services_handle
                    .spawn_request_stream_handler(rule_table, |rs| {
                        routes::admin::serve_rule_table::<Ipv4>(rs, netstack.ctx.clone())
                    }),
                Service::RuleTableV6(rule_table) => services_handle
                    .spawn_request_stream_handler(rule_table, |rs| {
                        routes::admin::serve_rule_table::<Ipv6>(rs, netstack.ctx.clone())
                    }),
                Service::RootRoutesV4(rs) => services_handle
                    .spawn_request_stream_handler(rs, |rs| {
                        root_fidl_worker::serve_routes_v4(rs, netstack.ctx.clone())
                    }),
                Service::RootRoutesV6(rs) => services_handle
                    .spawn_request_stream_handler(rs, |rs| {
                        root_fidl_worker::serve_routes_v6(rs, netstack.ctx.clone())
                    }),
                Service::Interfaces(interfaces) => services_handle
                    .spawn_request_stream_handler(interfaces, |rs| {
                        interfaces_watcher::serve(rs, interfaces_watcher_sink_ref.clone())
                    }),
                Service::NdpWatcher(stream) => services_handle
                    .spawn_request_stream_handler(stream, |rs| {
                        ndp_watcher::serve(rs, ndp_watcher_sink_ref.clone())
                    }),
                Service::InterfacesAdmin(installer) => services_handle
                    .spawn_request_stream_handler(installer, |installer| {
                        interfaces_admin::serve(netstack.clone(), installer)
                    }),
                Service::MulticastAdminV4(controller) => {
                    debug!(
                        "serving {}",
                        fnet_multicast_admin::Ipv4RoutingTableControllerMarker::PROTOCOL_NAME
                    );
                    netstack
                        .ctx
                        .bindings_ctx()
                        .multicast_admin
                        .sink::<Ipv4>()
                        .serve_multicast_admin_client(controller);
                }
                Service::MulticastAdminV6(controller) => {
                    debug!(
                        "serving {}",
                        fnet_multicast_admin::Ipv6RoutingTableControllerMarker::PROTOCOL_NAME
                    );
                    netstack
                        .ctx
                        .bindings_ctx()
                        .multicast_admin
                        .sink::<Ipv6>()
                        .serve_multicast_admin_client(controller);
                }
                Service::DebugInterfaces(debug_interfaces) => services_handle
                    .spawn_request_stream_handler(debug_interfaces, |rs| {
                        debug_fidl_worker::serve_interfaces(netstack.ctx.clone(), rs)
                    }),
                Service::DebugDiagnostics(debug_diagnostics) => {
                    diagnostics_handler.serve_diagnostics(debug_diagnostics)
                }
                Service::PacketCapture(packet_capture) => services_handle
                    .spawn_request_stream_handler(packet_capture, |rs| {
                        packet_capture_worker::serve_packet_captures(netstack.ctx.clone(), rs)
                    }),
                Service::DnsServerWatcher(dns) => services_handle
                    .spawn_request_stream_handler(dns, |rs| {
                        name_worker::serve(netstack.clone(), rs)
                    }),
                Service::FilterState(filter) => services_handle
                    .spawn_request_stream_handler(filter, |rs| {
                        filter::serve_state(rs, filter_update_dispatcher.clone())
                    }),
                Service::FilterControl(filter) => {
                    services_handle.spawn_request_stream_handler(filter, |rs| {
                        filter::serve_control(
                            rs,
                            filter_update_dispatcher.clone(),
                            netstack.ctx.clone(),
                        )
                    })
                }
                Service::Neighbor(neighbor) => services_handle
                    .spawn_request_stream_handler(neighbor, |rs| {
                        neighbor_worker::serve_view(rs, neighbor_watcher_sink_ref.clone())
                    }),
                Service::NeighborController(neighbor_controller) => services_handle
                    .spawn_request_stream_handler(neighbor_controller, |rs| {
                        neighbor_worker::serve_controller(netstack.ctx.clone(), rs)
                    }),
                Service::HealthCheck(health_check) => services_handle
                    .spawn_request_stream_handler(health_check, |rs| {
                        health_check_worker::serve(rs)
                    }),
                Service::SettingsControl(state) => services_handle
                    .spawn_request_stream_handler(state, |rs| {
                        settings::serve_control(netstack.ctx.clone(), rs)
                    }),
                Service::SettingsState(control) => services_handle
                    .spawn_request_stream_handler(control, |rs| {
                        settings::serve_state(netstack.ctx.clone(), rs)
                    }),
                Service::WakeGroupProvider(waker) => services_handle
                    .spawn_request_stream_handler(waker, |rs| {
                        netstack.ctx.bindings_ctx().wake_groups.clone().serve_provider(rs)
                    }),
            })
            .collect::<()>();

        // We just let this be destroyed on drop because it's effectively tied
        // to the lifecycle of the entire component.
        let inspect_task = inspect_publisher.publish();

        // Wait for services to stop.
        services_fut.await;
        info!("services stream terminated, starting shutdown");

        // Cancel all services and wait for them to join.
        services_scope.cancel().await;

        info!("stopping level 1 workers");
        let level1_workers = level1_workers.cancel();

        let ctx = teardown_ctx;
        // Stop the loopback interface.
        loopback_stopper
            .send(fnet_interfaces_admin::InterfaceRemovedReason::PortClosed)
            .expect("loopback task must still be running");
        // Stop the timer dispatcher.
        ctx.bindings_ctx().timers.stop();
        // Stop the interfaces watcher worker.
        std::mem::drop(interfaces_watcher_sink);
        // Stop the ndp watcher worker.
        std::mem::drop(ndp_watcher_sink);
        // Stop the neighbor watcher worker.
        std::mem::drop(neighbor_watcher_sink);

        // We've signalled all the level 1 workers. Wait for them to finish.
        level1_workers.await;

        // Now get rid of level2 workers that must exit after level1. Here we
        // carefully end each task separately.
        info!("stopping level 2 workers");
        let level2_workers = level2_workers.cancel();

        // Stop the routes change runner.
        // NB: All devices must be removed before stopping the routes change
        // runner, otherwise device removal will fail when purging references
        // from the routing table.
        ctx.bindings_ctx().routes.close_senders();
        routes_change_task.await;

        // Stop the multicast admin worker.
        // NB: All devices must be removed before stopping the multicast admin
        // worker, otherwise device removal will fail when purging references
        // from the multicast routing table.
        ctx.bindings_ctx().multicast_admin.close();
        multicast_admin_task.await;

        // Stop the resource removal worker.
        ctx.bindings_ctx().resource_removal.close();
        resource_removal_task.await;

        // All level2 workers should've finished.
        level2_workers.await;

        // Drop all inspector data, it holds ctx clones.
        std::mem::drop(inspect_nodes);
        inspector.root().clear_recorded();
        if let Some(inspect_task) = inspect_task {
            inspect_task.cancel().await;
        }

        info!("shutdown complete");

        // Last thing to happen is dropping the context.
        ctx.try_destroy_last().expect("all Ctx references must have been dropped")
    }
}
