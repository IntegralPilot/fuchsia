// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::hash_map::{self, HashMap};
use std::convert::Infallible as Never;
use std::fmt::{self, Debug, Display};
use std::num::NonZeroU64;
use std::ops::{Deref as _, DerefMut as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::DeviceIdExt;
use super::util::NeedsDataWatcher;
use assert_matches::assert_matches;
use derivative::Derivative;
use fuchsia_async as fasync;
use futures::{FutureExt as _, StreamExt as _};
use itertools::Itertools as _;
use net_types::ethernet::Mac;
use net_types::ip::{IpAddr, Mtu};
use net_types::{SpecifiedAddr, UnicastAddr};
use netstack3_core::device::{
    BatchSize, DeviceBufferBindingsTypes, DeviceClassMatcher, DeviceId, DeviceIdAndNameMatcher,
    DeviceSendFrameError, EthernetLinkDevice, LoopbackDeviceId, PureIpDevice, TxBufferAllocator,
};
use netstack3_core::sync::RwLock as CoreRwLock;
use netstack3_core::trace::trace_duration;
use netstack3_core::types::WorkQueueReport;
use thiserror::Error;

use fidl_fuchsia_net_interfaces as fnet_interfaces;

use crate::bindings::power::TransmitSuspensionHandler;
use crate::bindings::routes::interface_local::LocalRouteTables;
use crate::bindings::stats_sampler::InterfaceStatusSampler;
use crate::bindings::util::NeedsDataNotifier;
use crate::bindings::{
    BindingsCtx, Ctx, InterfaceEventProducer, interfaces_admin, neighbor_worker, netdevice_worker,
};
use netdevice_worker::LinkMulticastEvent;

pub(crate) const LOOPBACK_MAC: Mac = Mac::new([0, 0, 0, 0, 0, 0]);

pub(crate) type BindingId = NonZeroU64;

/// A witness that a given binding ID has been allocated and its name reserved.
/// This type will cause a panic if dropped, so it should be allocated only after any
/// necessary fallible operations are performed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BindingIdAllocation(BindingId);

impl Drop for BindingIdAllocation {
    fn drop(&mut self) {
        unreachable!("BindingIdAllocation should never be dropped")
    }
}

impl BindingIdAllocation {
    fn into_inner(self) -> BindingId {
        let id = self.0;
        std::mem::forget(self);
        id
    }
}

/// Keeps tabs on devices.
///
/// `Devices` keeps a list of devices that are installed in the netstack with
/// an associated netstack core ID `C` used to reference the device.
///
/// The type parameter `C` is for the extra information associated with the
/// device. The type parameters are there to allow testing without dependencies
/// on `core`.
pub(crate) struct Devices<C> {
    inner: CoreRwLock<DevicesInner<C>>,
}

#[derive(PartialEq, Eq, Debug)]
struct DevicesInner<C> {
    id_map: HashMap<BindingId, IdMapEntry<C>>,
    last_id: BindingId,
}

impl<C> DevicesInner<C> {
    fn reserve_name_and_alloc_id(&mut self, name: String) -> (BindingId, BindingIdAllocation) {
        let DevicesInner { id_map, last_id } = self;

        let id = *last_id;
        *last_id = last_id.checked_add(1).expect("exhausted binding device IDs");

        assert!(id_map.insert(id, IdMapEntry::ReservedName(name)).is_none());
        (id, BindingIdAllocation(id))
    }
}

impl<C: HasDeviceName + Clone + Debug + PartialEq> DevicesInner<C> {
    fn iter_device_names(&self) -> impl Iterator<Item = &String> + '_ {
        self.id_map.values().map(|entry| match entry {
            IdMapEntry::ReservedName(s) => s,
            IdMapEntry::CoreId(c) => c.device_name(),
        })
    }
}

pub(crate) struct IdMapIterator<'a, C> {
    inner: hash_map::Values<'a, BindingId, IdMapEntry<C>>,
}

impl<'a, C> Iterator for IdMapIterator<'a, C> {
    type Item = &'a C;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .by_ref()
            .filter_map(|entry| match entry {
                IdMapEntry::ReservedName(_) => None,
                IdMapEntry::CoreId(c) => Some(c),
            })
            .next()
    }
}

impl<C> Default for Devices<C> {
    fn default() -> Self {
        Self {
            inner: CoreRwLock::new(DevicesInner {
                id_map: Default::default(),
                last_id: BindingId::MIN,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum IdMapEntry<C> {
    ReservedName(String),
    CoreId(C),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NameNotAvailableError;

impl<C> Devices<C>
where
    C: Clone + Debug + PartialEq,
{
    /// Adds a new device with the id and start publishing events to FIDL.
    pub(crate) fn add_device_and_start_publishing(&self, id: BindingIdAllocation, core_id: C)
    where
        C: DeviceIdExt,
    {
        self.add_device(id, core_id.clone());
        core_id
            .external_state()
            .with_common_info(|info| info.events.start_publishing())
            .expect("failed to start publishing events to FIDL");
    }

    /// Adds a new device with the id corresponding to the given [`BindingIdAllocation`].
    ///
    /// Consumes the [`BindingIdAllocation`] witness.
    ///
    /// Note: This method only adds the core ID into the mapping and it does
    /// not start publishing events to FIDL, which might not be the behavior
    /// you want. See [`add_device_and_start_publishing`] for a more common
    /// use case.
    fn add_device(&self, id: BindingIdAllocation, core_id: C) {
        let mut inner = self.inner.write();
        let DevicesInner { id_map, last_id: _ } = inner.deref_mut();

        let id = id.into_inner();
        assert_matches!(
            id_map.insert(id, IdMapEntry::CoreId(core_id)),
            Some(IdMapEntry::ReservedName(_))
        );
    }

    /// Cancel a [`BindingIdAllocation`].
    ///
    /// Use to safely consume the [`BindingIdAllocation`] in the event adding
    /// the device failed and the binding ID reservation should be freed.
    pub(crate) fn cancel_device_reservation(&self, id: BindingIdAllocation) {
        let id = id.into_inner();
        let core_id = self.remove_device(id);
        // NB: A device that has an allocation but hasn't been finalized
        // shouldn't have a core ID.
        debug_assert_eq!(core_id, None);
    }

    /// Removes a device from the internal list.
    ///
    /// Removes a device from the internal [`Devices`] list and returns the
    /// associated [`DeviceInfo`] if `id` is found or `None` otherwise.
    pub(crate) fn remove_device(&self, id: BindingId) -> Option<C> {
        let mut inner = self.inner.write();
        let DevicesInner { id_map, last_id: _ } = inner.deref_mut();
        id_map.remove(&id).and_then(|entry| match entry {
            IdMapEntry::ReservedName(_) => None,
            IdMapEntry::CoreId(c) => Some(c),
        })
    }

    /// Retrieve associated `core_id` for [`BindingId`].
    pub(crate) fn get_core_id(&self, id: BindingId) -> Option<C> {
        self.inner.read().id_map.get(&id).and_then(|entry| match entry {
            IdMapEntry::ReservedName(_) => None,
            IdMapEntry::CoreId(c) => Some(c.clone()),
        })
    }

    /// Call the provided callback with an iterator over the devices.
    pub(crate) fn with_devices<R>(&self, f: impl FnOnce(IdMapIterator<'_, C>) -> R) -> R {
        let inner = self.inner.read();
        let DevicesInner { id_map, last_id: _ } = &*inner;
        f(IdMapIterator { inner: id_map.values() })
    }
}

pub(crate) trait HasDeviceName {
    fn device_name(&self) -> &String;
}

impl HasDeviceName for DeviceId<BindingsCtx> {
    fn device_name(&self) -> &String {
        &self.bindings_id().name
    }
}

impl<C: HasDeviceName + Clone + Debug + PartialEq> Devices<C> {
    /// Reserves the given name and allocates a new [`BindingId`].
    /// If the name is already taken, returns an error.
    ///
    /// Returns both a [`BindingId`] and a [`BindingIdAllocation`] witnessing the same
    /// [`BindingId`]. The `BindingIdAllocation` must be passed to [`Devices::add_device`]; if
    /// dropped it will cause a panic.
    pub(crate) fn try_reserve_name_and_alloc_new_id(
        &self,
        name: String,
    ) -> Result<(BindingId, BindingIdAllocation), NameNotAvailableError> {
        let mut inner = self.inner.write();
        let inner = inner.deref_mut();

        if inner.iter_device_names().any(|s| s == &name) {
            return Err(NameNotAvailableError);
        }

        let id = inner.reserve_name_and_alloc_id(name);
        Ok(id)
    }

    /// Generates and reserves name with the prefix and allocates a new [`BindingId`].
    ///
    /// In addition to the generated name, returns both a [`BindingId`] and a
    /// [`BindingIdAllocation`] witnessing the same [`BindingId`]. The `BindingIdAllocation` must be
    /// passed to [`Devices::add_device`]; if dropped it will cause a panic.
    pub(crate) fn generate_and_reserve_name_and_alloc_new_id(
        &self,
        prefix: &'static str,
    ) -> (BindingId, BindingIdAllocation, String) {
        let mut inner = self.inner.write();
        let inner = inner.deref_mut();

        let name = {
            let next_id = inner.last_id;
            let name_candidates = std::iter::once(format!("{prefix}{next_id}"))
                .chain((0..).map(|n| format!("{prefix}{next_id}_{n}")));
            name_candidates
                .filter(|name| !inner.iter_device_names().contains(&name))
                .next()
                .expect("should find available name")
        };

        let (id, allocation) = inner.reserve_name_and_alloc_id(name.clone());
        (id, allocation, name)
    }

    /// Retrieves the device with the given name.
    pub(crate) fn get_device_by_name(&self, name: &str) -> Option<C> {
        self.with_devices(|mut devices| devices.find(|c| c.device_name() == name).cloned())
    }
}

/// Owned device specific information
pub(crate) enum OwnedDeviceSpecificInfo {
    Loopback(LoopbackInfo),
    Ethernet(EthernetInfo),
    PureIp(PureIpDeviceInfo),
    Blackhole(BlackholeDeviceInfo),
}

impl From<LoopbackInfo> for OwnedDeviceSpecificInfo {
    fn from(info: LoopbackInfo) -> Self {
        Self::Loopback(info)
    }
}

impl From<EthernetInfo> for OwnedDeviceSpecificInfo {
    fn from(info: EthernetInfo) -> Self {
        Self::Ethernet(info)
    }
}

impl From<PureIpDeviceInfo> for OwnedDeviceSpecificInfo {
    fn from(info: PureIpDeviceInfo) -> Self {
        Self::PureIp(info)
    }
}

impl From<BlackholeDeviceInfo> for OwnedDeviceSpecificInfo {
    fn from(info: BlackholeDeviceInfo) -> Self {
        Self::Blackhole(info)
    }
}

/// Borrowed device specific information.
#[derive(Debug)]
pub(crate) enum DeviceSpecificInfo<'a> {
    Loopback(&'a LoopbackInfo),
    Ethernet(&'a EthernetInfo),
    PureIp(&'a PureIpDeviceInfo),
    Blackhole(&'a BlackholeDeviceInfo),
}

impl DeviceSpecificInfo<'_> {
    pub(crate) fn static_common_info(&self) -> &StaticCommonInfo {
        match self {
            Self::Loopback(i) => &i.static_common_info,
            Self::Blackhole(i) => &i.common_info,
            Self::Ethernet(i) => &i.common_info,
            Self::PureIp(i) => &i.common_info,
        }
    }

    pub(crate) fn with_common_info<O, F: FnOnce(&DynamicCommonInfo) -> O>(&self, cb: F) -> O {
        match self {
            Self::Loopback(i) => i.with_dynamic_info(cb),
            Self::Blackhole(i) => i.with_dynamic_info(cb),
            Self::Ethernet(i) => i.with_dynamic_info(|dynamic| cb(&dynamic.netdevice.common_info)),
            Self::PureIp(i) => i.with_dynamic_info(|dynamic| cb(&dynamic.common_info)),
        }
    }

    pub(crate) fn with_common_info_mut<O, F: FnOnce(&mut DynamicCommonInfo) -> O>(
        &self,
        cb: F,
    ) -> O {
        match self {
            Self::Loopback(i) => i.with_dynamic_info_mut(cb),
            Self::Blackhole(i) => i.with_dynamic_info_mut(cb),
            Self::Ethernet(i) => {
                i.with_dynamic_info_mut(|dynamic| cb(&mut dynamic.netdevice.common_info))
            }
            Self::PureIp(i) => i.with_dynamic_info_mut(|dynamic| cb(&mut dynamic.common_info)),
        }
    }
}

pub(crate) async fn rx_task(
    mut ctx: Ctx,
    mut watcher: NeedsDataWatcher,
    device_id: LoopbackDeviceId<BindingsCtx>,
) {
    let mut yield_fut = futures::future::OptionFuture::default();
    loop {
        // Loop while we are woken up to handle enqueued Rx packets.
        futures::select! {
            w = watcher.next().fuse() => w.expect("watcher closed with loopback task running"),
            y = yield_fut => y.expect("OptionFuture is only selected when non-empty"),
        };

        match ctx.api().receive_queue().handle_queued_frames(&device_id) {
            WorkQueueReport::AllDone => (),
            WorkQueueReport::Pending => {
                // Yield the task to the executor once.
                yield_fut = Some(async_utils::futures::YieldToExecutorOnce::new()).into();
            }
        }
    }
}

#[derive(Error, Debug)]
pub(crate) enum TxTaskError {
    #[error("netdevice error: {0}")]
    Netdevice(#[from] netdevice_client::Error),
}

pub(crate) struct TxTask {
    ctx: Ctx,
    device_id: DeviceId<BindingsCtx>,
    watcher: NeedsDataWatcher,
    task_state: TxTaskState,
}

impl TxTask {
    pub(crate) fn new(
        ctx: Ctx,
        device_id: DeviceId<BindingsCtx>,
        watcher: NeedsDataWatcher,
        task_state: TxTaskState,
    ) -> Self {
        Self { ctx, device_id, watcher, task_state }
    }

    pub(crate) async fn run(self) -> Result<Never, TxTaskError> {
        let Self { ctx, device_id, watcher, task_state } = self;
        tx_task(ctx, device_id, watcher, task_state).await
    }
}

/// Allocator state shared with the TxTask.
#[derive(Default)]
pub(crate) struct TxAllocatorState {
    /// Count of netdevice buffers that has been allocated but not sent yet.
    /// This is used to guide how many TxBuffers are needed in the dequeue
    /// context, because netdevice buffers do not need another TxBuffer to
    /// be allocated for dequeue.
    ///
    /// Note that we can track the number of netdevice buffers but can't track
    /// the `Vec` buffers because sending those might fail with the `NoBuffer`
    /// error and get dropped. For netdevice buffers, when failed to send, we
    /// have a bad session and should stop the data plane.
    pub(super) device_buffers_in_queue: AtomicUsize,
}

pub(crate) struct NetdeviceAllocator {
    session: netdevice_client::Session,
    alloc_state: Arc<TxAllocatorState>,
    /// Track whether `Buf<Vec<u8>>` are currently in use. No new netdevice
    /// buffers will be allocated to Core before all the existing Vec-based
    /// buffers are sent out. If we allocate netdevice buffers to Core causing
    /// them to be queued after the existing Vec-based buffers, it will only
    /// delay the draining of those buffers.
    use_device_buffer: bool,
}

impl NetdeviceAllocator {
    pub(crate) fn new(session: netdevice_client::Session) -> (Self, TxTaskState) {
        let alloc_state = Arc::new(TxAllocatorState::default());
        (
            Self { session, alloc_state: alloc_state.clone(), use_device_buffer: true },
            TxTaskState { tx_buffers: Default::default(), alloc_state },
        )
    }
}

pub(crate) type TxBuffer =
    packet::Either<netdevice_client::SinglePartTxBuffer, packet::Buf<Vec<u8>>>;

impl TxBufferAllocator<TxBuffer> for NetdeviceAllocator {
    type Error = netdevice_client::Error;

    fn alloc(&mut self, len: usize, queue_len: usize) -> Result<TxBuffer, Self::Error> {
        if !self.use_device_buffer && queue_len == 0 {
            self.use_device_buffer = true;
        }

        if self.use_device_buffer {
            if let Some(buffer) = self.session.try_alloc_single_part_tx_buffer(len)? {
                let _ = self.alloc_state.device_buffers_in_queue.fetch_add(1, Ordering::Relaxed);
                return Ok(packet::Either::A(buffer));
            }
        }
        self.use_device_buffer = false;
        Ok(packet::Either::B(packet::new_buf_vec(len).unwrap_or_else(|never| match never {})))
    }
}

impl DeviceBufferBindingsTypes for BindingsCtx {
    type TxBuffer = TxBuffer;
    type TxAllocator = NetdeviceAllocator;
}

#[derive(Default)]
pub(crate) struct TxTaskState {
    pub(crate) tx_buffers: Vec<netdevice_client::TxBuffer>,
    pub(crate) alloc_state: Arc<TxAllocatorState>,
}

impl TxTaskState {
    /// Collects up to `count` tx buffers from `port_handler` into the task
    /// state.
    ///
    /// Note: `count` is `BatchSize` since the buffers are expected to be fed
    /// into the transmit queue API which has a limited batch size, so we avoid
    /// over allocating.
    async fn collect_buffers(
        &mut self,
        port_handler: &netdevice_worker::PortHandler,
        count: BatchSize,
    ) -> Result<usize, TxTaskError> {
        let Self { tx_buffers, .. } = self;
        let mut iter = port_handler.alloc_tx_buffers().await?.take(count.into());
        iter.try_fold((0, tx_buffers), |(count, b), item| {
            b.push(item?);
            Ok((count + 1, b))
        })
        .map(|(count, _)| count)
    }
}

// NB: We could write this function generically in terms of `D: Device`, which
// is the type parameter given to instantiate the transmit queue API. To do
// that, core would need to expose a marker for CoreContext that is generic on
// `D`. That is doable but not worth the extra code at this moment since
// bindings doesn't have meaningful amounts of code that is generic over the
// device type.
pub(crate) async fn tx_task(
    mut ctx: Ctx,
    device_id: DeviceId<BindingsCtx>,
    mut watcher: NeedsDataWatcher,
    mut task_state: TxTaskState,
) -> Result<Never, TxTaskError> {
    let mut yield_fut = futures::future::OptionFuture::default();
    let mut suspension_handler = TransmitSuspensionHandler::new(&ctx, device_id.downgrade()).await;

    // This control loop selects an action which may be:
    //   - suspension:
    //       * enter suspension handling, which allows the system to suspend and
    //         only finishes when the system resumes
    //   - send packets
    //       * determine packet batch size
    //       * enqueue packets and send them, waiting for the send result
    //       * if the send result is an error, yield and continue sending
    //         on the next iteration
    //   - continue sending packets after yielding in a previous batch.
    // The loop continues until `watcher` closes or the `device_id` is invalid.
    loop {
        enum Action<S> {
            TxAvailable,
            Suspension(S),
        }
        // Loop while we are woken up to handle enqueued TX packets.
        let action = futures::select_biased! {
            s = suspension_handler.wait().fuse() => Action::Suspension(s),
            w = watcher.next().fuse() => match w {
                Some(()) => Action::TxAvailable,
                // Notifying watcher is closed, interface must've been removed.
                // This shouldn't happen we're keeping the device alive.
                None => unreachable!("watcher closed unexpectedly"),
            },
            y = yield_fut => {
                y.expect("OptionFuture is only selected when non-empty");
                Action::TxAvailable
            }
        };

        match action {
            Action::TxAvailable => {}
            Action::Suspension(s) => {
                // Suspension requested, finish up in-progress work and block
                // until we are allowed to resume.
                s.handle_suspension().await;
                continue;
            }
        };

        let batch_size = match device_id.external_state() {
            DeviceSpecificInfo::Loopback(_) => {
                unimplemented!("tx task is not supported for loopback devices")
            }
            DeviceSpecificInfo::Blackhole(_) => {
                unimplemented!("tx task is not supported for blackhole devices")
            }
            DeviceSpecificInfo::Ethernet(EthernetInfo { netdevice, .. })
            | DeviceSpecificInfo::PureIp(PureIpDeviceInfo { netdevice, .. }) => {
                // Attempt to preallocate buffers to handle the queue.
                let queue_len = match &device_id {
                    DeviceId::Ethernet(id) => {
                        ctx.api().transmit_queue::<EthernetLinkDevice>().count(id)
                    }
                    DeviceId::PureIp(id) => ctx.api().transmit_queue::<PureIpDevice>().count(id),
                    DeviceId::Loopback(_) | DeviceId::Blackhole(_) => unreachable!(),
                };

                match queue_len {
                    Some(queue_len) => {
                        if queue_len != 0 {
                            let device_buffers_in_queue = task_state
                                .alloc_state
                                .device_buffers_in_queue
                                .load(Ordering::Relaxed);
                            // If we have more buffers in queue, then Vec-based buffers must be in
                            // use, we try to collect buffers to dequeue context. Note that we now
                            // can't be blocked on `collect_buffers` because it is possible that
                            // all available Tx device buffers are already in the queue and we
                            // dead lock.
                            if device_buffers_in_queue == 0 {
                                let collected = task_state
                                    .collect_buffers(
                                        &netdevice.handler,
                                        BatchSize::new_saturating(queue_len),
                                    )
                                    .await?;
                                BatchSize::new_saturating(collected)
                            } else {
                                BatchSize::new_saturating(device_buffers_in_queue)
                            }
                        } else {
                            // We got woken up to do tx work but core says we
                            // have zero buffers in the queue, go back to
                            // waiting.
                            continue;
                        }
                    }
                    None => {
                        // Queueing is not configured, go back to waiting.
                        continue;
                    }
                }
            }
        };

        trace_duration!("devices::tx_task",
            "iface" => device_id.bindings_id().id.get(),
            "batch" => usize::from(batch_size)
        );

        let r = match &device_id {
            DeviceId::Loopback(_) => {
                unimplemented!("tx task is not supported for loopback devices")
            }
            DeviceId::Blackhole(_) => {
                unimplemented!("tx task is not supported for blackhole devices")
            }
            DeviceId::Ethernet(id) => ctx
                .api()
                .transmit_queue::<EthernetLinkDevice>()
                .transmit_queued_frames(id, batch_size, &mut task_state),
            DeviceId::PureIp(id) => ctx
                .api()
                .transmit_queue::<PureIpDevice>()
                .transmit_queued_frames(id, batch_size, &mut task_state),
        }
        .unwrap_or_else(|err| {
            match err {
                // Core is already keeping track of counters for this, nothing
                // to be done in bindings.
                DeviceSendFrameError::NoBuffers => (),
            }
            // If we observe an error when sending, we're not sure if the queue
            // was drained or not, so assume we need to look at it again.
            WorkQueueReport::Pending
        });

        let TxTaskState { tx_buffers, alloc_state: _ } = &mut task_state;
        // If for some reason we have buffers left here, return them so other
        // interfaces on the same device can use the buffers.
        tx_buffers.clear();

        match r {
            WorkQueueReport::AllDone => (),
            WorkQueueReport::Pending => {
                // Yield the task to the executor once.
                yield_fut = Some(async_utils::futures::YieldToExecutorOnce::new()).into();
            }
        }
    }
}

/// Static information common to all devices.
#[derive(Derivative, Debug)]
pub(crate) struct StaticCommonInfo {
    #[derivative(Debug = "ignore")]
    pub(crate) authorization_token: zx::Event,
    pub(crate) local_route_tables: Option<LocalRouteTables>,
}

impl StaticCommonInfo {
    pub(crate) fn new(local_route_tables: Option<LocalRouteTables>) -> Self {
        Self { authorization_token: zx::Event::create(), local_route_tables }
    }
}

/// Information common to all devices.
#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct DynamicCommonInfo {
    pub(crate) mtu: Mtu,
    pub(crate) admin_enabled: bool,
    pub(crate) ipv4_enabled: bool,
    pub(crate) ipv6_enabled: bool,
    pub(crate) events: super::InterfaceEventProducer,
    // An attach point to send `fuchsia.net.interfaces.admin/Control` handles to the Interfaces
    // Admin worker.
    #[derivative(Debug = "ignore")]
    pub(crate) control_hook: futures::channel::mpsc::Sender<interfaces_admin::OwnedControlHandle>,
    pub(crate) addresses: HashMap<SpecifiedAddr<IpAddr>, AddressInfo>,
}

impl DynamicCommonInfo {
    pub(crate) const DEFAULT_ADMIN_ENABLED: bool = false;
    pub(crate) const DEFAULT_IPV4_ENABLED: bool = true;
    pub(crate) const DEFAULT_IPV6_ENABLED: bool = true;

    pub(crate) fn new(
        mtu: Mtu,
        events: InterfaceEventProducer,
        control_hook: futures::channel::mpsc::Sender<interfaces_admin::OwnedControlHandle>,
    ) -> Self {
        Self {
            mtu,
            admin_enabled: Self::DEFAULT_ADMIN_ENABLED,
            ipv4_enabled: Self::DEFAULT_IPV4_ENABLED,
            ipv6_enabled: Self::DEFAULT_IPV6_ENABLED,
            events,
            control_hook,
            addresses: HashMap::new(),
        }
    }

    /// Only loopback should start with `admin_enabled` = true.
    pub(crate) fn new_for_loopback(
        mtu: Mtu,
        events: super::InterfaceEventProducer,
        control_hook: futures::channel::mpsc::Sender<interfaces_admin::OwnedControlHandle>,
    ) -> Self {
        Self {
            mtu,
            admin_enabled: true,
            ipv4_enabled: true,
            ipv6_enabled: true,
            events,
            control_hook,
            addresses: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct AddressInfo {
    // The `AddressStateProvider` FIDL protocol worker.
    pub(crate) address_state_provider:
        FidlWorkerInfo<interfaces_admin::AddressStateProviderCancellationReason>,
    // Sender for [`AddressAssignmentState`] change events published by Core;
    // the receiver is held by the `AddressStateProvider` worker. Note that an
    // [`UnboundedSender`] is used because it exposes a synchronous send API
    // which is required since Core is no-async.
    pub(crate) assignment_state_sender:
        futures::channel::mpsc::UnboundedSender<fnet_interfaces::AddressAssignmentState>,
}

/// Information associated with FIDL Protocol workers.
#[derive(Debug)]
pub(crate) struct FidlWorkerInfo<R> {
    // The worker `JoinHandle`, wrapped in a `Shared` future so that it can be
    // awaited multiple times.
    pub(crate) worker: futures::future::Shared<fasync::JoinHandle<()>>,
    // Mechanism to cancel the worker with reason `R`. If `Some`, the worker is
    // active (and holds the `Receiver`). Otherwise, the worker has been
    // canceled.
    pub(crate) cancelation_sender: Option<futures::channel::oneshot::Sender<R>>,
}

/// Loopback device information.
#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct LoopbackInfo {
    pub(crate) static_common_info: StaticCommonInfo,
    pub(crate) dynamic_common_info: CoreRwLock<DynamicCommonInfo>,
    #[derivative(Debug = "ignore")]
    pub(crate) rx_notifier: NeedsDataNotifier,
}

impl LoopbackInfo {
    pub(crate) fn with_dynamic_info<O, F: FnOnce(&DynamicCommonInfo) -> O>(&self, cb: F) -> O {
        cb(self.dynamic_common_info.read().deref())
    }

    pub(crate) fn with_dynamic_info_mut<O, F: FnOnce(&mut DynamicCommonInfo) -> O>(
        &self,
        cb: F,
    ) -> O {
        cb(self.dynamic_common_info.write().deref_mut())
    }
}

impl DeviceClassMatcher<fidl_fuchsia_net_interfaces::PortClass> for LoopbackInfo {
    fn device_class_matches(&self, port_class: &fidl_fuchsia_net_interfaces::PortClass) -> bool {
        match port_class {
            fidl_fuchsia_net_interfaces::PortClass::Loopback(
                fidl_fuchsia_net_interfaces::Empty {},
            ) => true,
            fidl_fuchsia_net_interfaces::PortClass::Blackhole(
                fidl_fuchsia_net_interfaces::Empty {},
            )
            | fidl_fuchsia_net_interfaces::PortClass::Device(_) => false,
            fidl_fuchsia_net_interfaces::PortClass::__SourceBreaking { unknown_ordinal } => {
                panic!("unknown device class ordinal {unknown_ordinal:?}")
            }
        }
    }
}

/// Dynamic information common to all Netdevice backed devices.
#[derive(Debug)]
pub(crate) struct DynamicNetdeviceInfo {
    pub(crate) phy_up: bool,
    pub(crate) common_info: DynamicCommonInfo,
}

/// Static information common to all Netdevice backed devices.
#[derive(Debug)]
pub(crate) struct StaticNetdeviceInfo {
    pub(crate) handler: netdevice_worker::PortHandler,
    pub(crate) tx_notifier: NeedsDataNotifier,
}

impl StaticNetdeviceInfo {
    fn device_class_matches(&self, port_class: &fidl_fuchsia_net_interfaces::PortClass) -> bool {
        match port_class {
            fidl_fuchsia_net_interfaces::PortClass::Loopback(
                fidl_fuchsia_net_interfaces::Empty {},
            )
            | fidl_fuchsia_net_interfaces::PortClass::Blackhole(
                fidl_fuchsia_net_interfaces::Empty {},
            ) => false,
            fidl_fuchsia_net_interfaces::PortClass::Device(port_class) => {
                *port_class == self.handler.port_class()
            }
            fidl_fuchsia_net_interfaces::PortClass::__SourceBreaking { unknown_ordinal } => {
                panic!("unknown device class ordinal {unknown_ordinal:?}")
            }
        }
    }
}

/// Dynamic information for Ethernet devices
#[derive(Derivative)]
#[derivative(Debug)]
pub(crate) struct DynamicEthernetInfo {
    pub(crate) netdevice: DynamicNetdeviceInfo,
    #[derivative(Debug = "ignore")]
    pub(crate) neighbor_event_sink: futures::channel::mpsc::UnboundedSender<neighbor_worker::Event>,
}

/// Ethernet device information.
#[derive(Debug)]
pub(crate) struct EthernetInfo {
    pub(crate) dynamic_info: CoreRwLock<DynamicEthernetInfo>,
    pub(crate) common_info: StaticCommonInfo,
    pub(crate) netdevice: StaticNetdeviceInfo,
    pub(crate) mac: UnicastAddr<Mac>,
    pub(crate) multicast_event_sink: futures::channel::mpsc::UnboundedSender<LinkMulticastEvent>,
    pub(crate) status_sampler: InterfaceStatusSampler,
}

impl EthernetInfo {
    pub(crate) fn with_dynamic_info<O, F: FnOnce(&DynamicEthernetInfo) -> O>(&self, cb: F) -> O {
        let dynamic = self.dynamic_info.read();
        cb(dynamic.deref())
    }

    pub(crate) fn with_dynamic_info_mut<O, F: FnOnce(&mut DynamicEthernetInfo) -> O>(
        &self,
        cb: F,
    ) -> O {
        let mut dynamic = self.dynamic_info.write();
        cb(dynamic.deref_mut())
    }
}

impl DeviceClassMatcher<fidl_fuchsia_net_interfaces::PortClass> for EthernetInfo {
    fn device_class_matches(&self, device_class: &fidl_fuchsia_net_interfaces::PortClass) -> bool {
        self.netdevice.device_class_matches(device_class)
    }
}

/// Pure IP device information.
#[derive(Debug)]
pub(crate) struct PureIpDeviceInfo {
    pub(crate) common_info: StaticCommonInfo,
    pub(crate) netdevice: StaticNetdeviceInfo,
    pub(crate) dynamic_info: CoreRwLock<DynamicNetdeviceInfo>,
    pub(crate) status_sampler: InterfaceStatusSampler,
}

impl PureIpDeviceInfo {
    pub(crate) fn with_dynamic_info<O, F: FnOnce(&DynamicNetdeviceInfo) -> O>(&self, cb: F) -> O {
        let dynamic = self.dynamic_info.read();
        cb(dynamic.deref())
    }

    pub(crate) fn with_dynamic_info_mut<O, F: FnOnce(&mut DynamicNetdeviceInfo) -> O>(
        &self,
        cb: F,
    ) -> O {
        let mut dynamic = self.dynamic_info.write();
        cb(dynamic.deref_mut())
    }
}

impl DeviceClassMatcher<fidl_fuchsia_net_interfaces::PortClass> for PureIpDeviceInfo {
    fn device_class_matches(&self, device_class: &fidl_fuchsia_net_interfaces::PortClass) -> bool {
        self.netdevice.device_class_matches(device_class)
    }
}

/// Blackhole device information.
#[derive(Debug)]
pub(crate) struct BlackholeDeviceInfo {
    pub(crate) dynamic_common_info: CoreRwLock<DynamicCommonInfo>,
    pub(crate) common_info: StaticCommonInfo,
}

impl BlackholeDeviceInfo {
    pub(crate) fn with_dynamic_info<O, F: FnOnce(&DynamicCommonInfo) -> O>(&self, cb: F) -> O {
        cb(self.dynamic_common_info.read().deref())
    }

    pub(crate) fn with_dynamic_info_mut<O, F: FnOnce(&mut DynamicCommonInfo) -> O>(
        &self,
        cb: F,
    ) -> O {
        cb(self.dynamic_common_info.write().deref_mut())
    }
}

impl DeviceClassMatcher<fidl_fuchsia_net_interfaces::PortClass> for BlackholeDeviceInfo {
    fn device_class_matches(&self, port_class: &fidl_fuchsia_net_interfaces::PortClass) -> bool {
        match port_class {
            fidl_fuchsia_net_interfaces::PortClass::Blackhole(
                fidl_fuchsia_net_interfaces::Empty {},
            ) => true,
            fidl_fuchsia_net_interfaces::PortClass::Loopback(
                fidl_fuchsia_net_interfaces::Empty {},
            )
            | fidl_fuchsia_net_interfaces::PortClass::Device(_)
            | fidl_fuchsia_net_interfaces::PortClass::__SourceBreaking { unknown_ordinal: _ } => {
                false
            }
        }
    }
}

pub(crate) struct DeviceIdAndName {
    pub(crate) id: BindingId,
    pub(crate) name: String,
}

impl Debug for DeviceIdAndName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { id, name } = self;
        write!(f, "{id}=>{name}")
    }
}

impl Display for DeviceIdAndName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(self, f)
    }
}

impl DeviceIdAndNameMatcher for DeviceIdAndName {
    fn id_matches(&self, id: &NonZeroU64) -> bool {
        self.id == *id
    }

    fn name_matches(&self, name: &str) -> bool {
        self.name == name
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Debug, PartialEq, Eq, Clone)]
    struct TestCoreId(String);

    impl HasDeviceName for TestCoreId {
        fn device_name(&self) -> &String {
            &self.0
        }
    }

    fn devices_inner(
        last_id: u64,
        id_map: impl IntoIterator<Item = (u64, IdMapEntry<TestCoreId>)>,
    ) -> DevicesInner<TestCoreId> {
        DevicesInner {
            last_id: NonZeroU64::new(last_id).unwrap(),
            id_map: HashMap::from_iter(
                id_map.into_iter().map(|(id, entry)| (NonZeroU64::new(id).unwrap(), entry)),
            ),
        }
    }

    #[test]
    fn add_device() {
        let devices = Devices::<TestCoreId>::default();

        const NAME: &str = "name";
        let (id, allocation) =
            devices.try_reserve_name_and_alloc_new_id(NAME.to_string()).expect("should succeed");
        assert_eq!(id.get(), 1);

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::ReservedName(NAME.to_string()))])
            );
        }

        devices.add_device(allocation, TestCoreId(NAME.to_string()));

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::CoreId(TestCoreId(NAME.to_string())))])
            );
        }
    }

    #[test]
    fn add_device_avoids_conflict_with_existing_device() {
        let devices = Devices::<TestCoreId>::default();

        const NAME: &str = "name";
        let (_id, allocation) =
            devices.try_reserve_name_and_alloc_new_id(NAME.to_string()).expect("should succeed");
        devices.add_device(allocation, TestCoreId(NAME.to_string()));

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::CoreId(TestCoreId(NAME.to_string())))])
            );
        }

        let result = devices.try_reserve_name_and_alloc_new_id(NAME.to_string());
        assert_eq!(result, Err(NameNotAvailableError));

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::CoreId(TestCoreId(NAME.to_string())))])
            );
        }
    }

    #[test]
    fn add_device_avoids_conflict_with_reserved_name() {
        let devices = Devices::<TestCoreId>::default();

        const NAME: &str = "name";
        let (_id, allocation) =
            devices.try_reserve_name_and_alloc_new_id(NAME.to_string()).expect("should succeed");

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::ReservedName(NAME.to_string()))])
            );
        }
        // Need to do this to avoid BindingIdAllocation's panic on drop.
        let _ = allocation.into_inner();

        let result = devices.try_reserve_name_and_alloc_new_id(NAME.to_string());
        assert_eq!(result, Err(NameNotAvailableError));

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::ReservedName(NAME.to_string()))])
            );
        }
    }

    #[test]
    fn add_device_with_generated_name() {
        let devices = Devices::<TestCoreId>::default();

        const PREFIX: &str = "prefix";
        const EXPECTED_NAME: &str = "prefix1";
        let (id, allocation, name) = devices.generate_and_reserve_name_and_alloc_new_id(PREFIX);
        assert_eq!(id.get(), 1);
        assert_eq!(&name, EXPECTED_NAME);

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::ReservedName(EXPECTED_NAME.to_string()))])
            );
        }

        devices.add_device(allocation, TestCoreId(name));

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::CoreId(TestCoreId(EXPECTED_NAME.to_string())))])
            );
        }
    }

    #[test]
    fn add_device_avoids_conflict_with_existing_interface_when_generating_name() {
        let devices = Devices::<TestCoreId>::default();

        // Chosen so that it will clash with the generated name.
        const EXISTING_NAME: &str = "prefix2";
        const PREFIX: &str = "prefix";
        const EXPECTED_GENERATED_NAME: &str = "prefix2_0";

        let (_id, allocation) = devices
            .try_reserve_name_and_alloc_new_id(EXISTING_NAME.to_string())
            .expect("should succeed");
        devices.add_device(allocation, TestCoreId(EXISTING_NAME.to_string()));
        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::CoreId(TestCoreId(EXISTING_NAME.to_string())))])
            );
        }

        let (id, allocation, name) = devices.generate_and_reserve_name_and_alloc_new_id(PREFIX);
        assert_eq!(id.get(), 2);
        assert_eq!(&name, EXPECTED_GENERATED_NAME);

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(
                    3,
                    [
                        (1, IdMapEntry::CoreId(TestCoreId(EXISTING_NAME.to_string()))),
                        (2, IdMapEntry::ReservedName(EXPECTED_GENERATED_NAME.to_string()))
                    ]
                )
            );
        }

        // Need to do this to avoid BindingIdAllocation's panic on drop.
        let _ = allocation.into_inner();
    }

    #[test]
    fn add_device_avoids_conflict_with_reserved_name_when_generating_name() {
        let devices = Devices::<TestCoreId>::default();

        // Chosen so that it will clash with the generated name.
        const EXISTING_NAME: &str = "prefix2";
        const PREFIX: &str = "prefix";
        const EXPECTED_GENERATED_NAME: &str = "prefix2_0";

        let (_id, allocation) = devices
            .try_reserve_name_and_alloc_new_id(EXISTING_NAME.to_string())
            .expect("should succeed");
        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(2, [(1, IdMapEntry::ReservedName(EXISTING_NAME.to_string()))])
            );
        }

        // Need to do this to avoid BindingIdAllocation's panic on drop.
        let _ = allocation.into_inner();

        let (id, allocation, name) = devices.generate_and_reserve_name_and_alloc_new_id(PREFIX);
        assert_eq!(id.get(), 2);
        assert_eq!(&name, EXPECTED_GENERATED_NAME);

        {
            let inner = devices.inner.read();
            assert_eq!(
                inner.deref(),
                &devices_inner(
                    3,
                    [
                        (1, IdMapEntry::ReservedName(EXISTING_NAME.to_string())),
                        (2, IdMapEntry::ReservedName(EXPECTED_GENERATED_NAME.to_string()))
                    ]
                )
            );
        }

        // Need to do this to avoid BindingIdAllocation's panic on drop.
        let _ = allocation.into_inner();
    }
}
