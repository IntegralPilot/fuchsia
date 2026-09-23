// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Device layer api.

use alloc::fmt::Debug;
use core::marker::PhantomData;

use log::debug;
use net_types::ip::{Ipv4, Ipv6};
use netstack3_base::{
    AnyDevice, ContextPair, CoreTimerContext, Device, DeviceIdAnyCompatContext, DeviceIdContext,
    Inspector, RecvFrameContext, ReferenceNotifiers, ReferenceNotifiersExt as _,
    RemoveResourceResultWithContext, ResourceCounterContext, TimerContext,
};
use netstack3_ip::device::{
    IpDeviceBindingsContext, IpDeviceConfigurationContext, IpDeviceTimerId,
    Ipv6DeviceConfigurationContext,
};
use netstack3_ip::gmp::{IgmpCounters, MldCounters};
use netstack3_ip::{self as ip, IpCounters, RawMetric};
use packet::BufferMut;
use ref_cast::RefCast;

use crate::internal::base::{
    DeviceCollectionContext, DeviceCounters, DeviceLayerStateTypes, DeviceLayerTypes,
    DeviceReceiveFrameSpec, OriginTrackerContext,
};
use crate::internal::blackhole::BlackholeDevice;
use crate::internal::config::{
    ArpConfiguration, ArpConfigurationUpdate, DeviceConfiguration, DeviceConfigurationContext,
    DeviceConfigurationUpdate, DeviceConfigurationUpdateError, NdpConfiguration,
    NdpConfigurationUpdate,
};
use crate::internal::ethernet::EthernetLinkDevice;
use crate::internal::id::{
    BaseDeviceId, BasePrimaryDeviceId, BaseWeakDeviceId, DeviceId, DeviceProvider,
    for_any_device_id,
};
use crate::internal::loopback::LoopbackDevice;
use crate::internal::pure_ip::PureIpDevice;
use crate::internal::queue::DeviceBufferSpec;
use crate::internal::state::{
    BaseDeviceState, DeviceStateSpec, DeviceTxOffloadSpecContext, IpLinkDeviceStateInner,
};

/// Pending device configuration update.
///
/// This type is a witness for a valid [`DeviceConfigurationUpdate`] for some
/// device ID `D` and is obtained through
/// [`DeviceApi::new_configuration_update`].
///
/// The configuration is only applied when [`DeviceApi::apply_configuration`] is
/// called.
pub struct PendingDeviceConfigurationUpdate<'a, D>(DeviceConfigurationUpdate, &'a D);

/// The device API.
#[derive(RefCast)]
#[repr(transparent)]
pub struct DeviceApi<D, C>(C, PhantomData<D>);

impl<D, C> DeviceApi<D, C> {
    /// Creates a new [`DeviceApi`] from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx, PhantomData)
    }
}

impl<D, C> DeviceApi<D, C>
where
    D: Device + DeviceStateSpec + DeviceReceiveFrameSpec + DeviceBufferSpec<C::BindingsContext>,
    C: ContextPair,
    C::CoreContext: DeviceApiCoreContext<D, C::BindingsContext>,
    C::BindingsContext: DeviceApiBindingsContext,
{
    pub(crate) fn contexts(&mut self) -> (&mut C::CoreContext, &mut C::BindingsContext) {
        let Self(pair, PhantomData) = self;
        pair.contexts()
    }

    pub(crate) fn core_ctx(&mut self) -> &mut C::CoreContext {
        let Self(pair, PhantomData) = self;
        pair.core_ctx()
    }

    /// Adds a new device to the stack and returns its identifier.
    ///
    /// # Panics
    ///
    /// Panics if more than 1 loopback device is added to the stack.
    pub fn add_device(
        &mut self,
        bindings_id: <C::BindingsContext as DeviceLayerStateTypes>::DeviceIdentifier,
        properties: D::CreationProperties,
        metric: RawMetric,
        external_state: D::External<C::BindingsContext>,
        tx_allocator: D::TxAllocator,
    ) -> <C::CoreContext as DeviceIdContext<D>>::DeviceId
    where
        C::CoreContext: DeviceApiIpLayerCoreContext<D, C::BindingsContext>,
    {
        debug!("adding {} device with {:?} metric:{metric}", D::DEBUG_TYPE, properties);
        let (core_ctx, bindings_ctx) = self.contexts();
        let origin = core_ctx.origin_tracker();
        let primary = BasePrimaryDeviceId::new(
            |weak_ref| {
                let link = D::new_device_state::<C::CoreContext, _>(
                    bindings_ctx,
                    weak_ref.clone(),
                    properties,
                    tx_allocator,
                );
                IpLinkDeviceStateInner::new::<_, C::CoreContext>(
                    bindings_ctx,
                    weak_ref.into(),
                    link,
                    metric,
                    origin,
                )
            },
            external_state,
            bindings_id,
        );
        let id = primary.clone_strong();
        core_ctx.insert(primary);
        id
    }

    /// Like [`DeviceApi::add_device`] but using default values for
    /// `bindings_id` and `external_state`.
    ///
    /// This is provided as a convenience method for tests with faked bindings
    /// contexts that have simple implementations for bindings state.
    #[cfg(any(test, feature = "testutils"))]
    pub fn add_device_with_default_state(
        &mut self,
        properties: D::CreationProperties,
        metric: RawMetric,
    ) -> <C::CoreContext as DeviceIdContext<D>>::DeviceId
    where
        <C::BindingsContext as DeviceLayerStateTypes>::DeviceIdentifier: Default,
        D::External<C::BindingsContext>: Default,
        C::CoreContext: DeviceApiIpLayerCoreContext<D, C::BindingsContext>,
        D::TxAllocator: Default,
    {
        self.add_device(
            Default::default(),
            properties,
            metric,
            Default::default(),
            Default::default(),
        )
    }

    /// Removes `device` from the stack.
    ///
    /// If the return value is `RemoveDeviceResult::Removed` the device is
    /// immediately removed from the stack, otherwise
    /// `RemoveDeviceResult::Deferred` indicates that the device was marked for
    /// destruction but there are still references to it. It carries a
    /// `ReferenceReceiver` from the bindings context that can be awaited on
    /// until removal is complete.
    ///
    /// # Panics
    ///
    /// Panics if the device is not currently in the stack.
    pub fn remove_device(
        &mut self,
        device: BaseDeviceId<D, C::BindingsContext>,
    ) -> RemoveResourceResultWithContext<D::External<C::BindingsContext>, C::BindingsContext>
    where
        // Required to call into IP layer for cleanup on removal:
        BaseDeviceId<D, C::BindingsContext>: Into<DeviceId<C::BindingsContext>>,
        C::CoreContext: IpDeviceConfigurationContext<Ipv4, C::BindingsContext>
            + Ipv6DeviceConfigurationContext<C::BindingsContext>
            + DeviceIdContext<AnyDevice, DeviceId = DeviceId<C::BindingsContext>>,
        C::BindingsContext: IpDeviceBindingsContext<Ipv4, <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>
            + IpDeviceBindingsContext<Ipv6, <C::CoreContext as DeviceIdContext<AnyDevice>>::DeviceId>,
    {
        // Start cleaning up the device by disabling IP state. This removes timers
        // for the device that would otherwise hold references to defunct device
        // state.
        let (core_ctx, bindings_ctx) = self.contexts();
        {
            let device = device.clone().into();
            ip::device::clear_ipv4_device_state(core_ctx, bindings_ctx, &device);
            ip::device::clear_ipv6_device_state(core_ctx, bindings_ctx, &device);
        };

        debug!("removing {device:?}");
        let primary = core_ctx.remove(&device).expect("tried to remove device not in stack");
        assert_eq!(device, primary);
        core::mem::drop(device);
        C::BindingsContext::unwrap_or_notify_with_new_reference_notifier(
            primary.into_inner(),
            |state: BaseDeviceState<_, _>| state.external_state,
        )
    }

    /// Receive a device layer frame from the network.
    pub fn receive_frame<B: BufferMut + Debug>(
        &mut self,
        meta: D::FrameMetadata<BaseDeviceId<D, C::BindingsContext>>,
        frame: B,
    ) {
        let (core_ctx, bindings_ctx) = self.contexts();
        core_ctx.receive_frame(bindings_ctx, meta, frame)
    }

    /// Applies the configuration and returns a [`DeviceConfigurationUpdate`]
    /// with the previous values for all configurations for all `Some` fields.
    ///
    /// Note that even if the previous value matched the requested value, it is
    /// still populated in the returned `DeviceConfigurationUpdate`.
    pub fn apply_configuration(
        &mut self,
        pending: PendingDeviceConfigurationUpdate<'_, BaseDeviceId<D, C::BindingsContext>>,
    ) -> DeviceConfigurationUpdate {
        let PendingDeviceConfigurationUpdate(DeviceConfigurationUpdate { arp, ndp }, device_id) =
            pending;
        let core_ctx = self.core_ctx();
        let arp = core_ctx.with_nud_config_mut::<Ipv4, _, _>(device_id, move |device_config| {
            let device_config = match device_config {
                Some(c) => c,
                None => {
                    // Can't set ARP configuration if device doesn't support it,
                    // this is validated when creating the
                    // `PendingDeviceConfigurationUpdate`.
                    assert!(arp.is_none());
                    return None;
                }
            };
            arp.map(|ArpConfigurationUpdate { nud }| {
                let nud = nud.map(|config| config.apply_and_take_previous(device_config));
                ArpConfigurationUpdate { nud }
            })
        });
        let ndp = core_ctx.with_nud_config_mut::<Ipv6, _, _>(device_id, move |device_config| {
            let device_config = match device_config {
                Some(c) => c,
                None => {
                    // Can't set NDP configuration if device doesn't support it,
                    // this is validated when creating the
                    // `PendingDeviceConfigurationUpdate`.
                    assert!(ndp.is_none());
                    return None;
                }
            };
            ndp.map(|NdpConfigurationUpdate { nud }| {
                let nud = nud.map(|config| config.apply_and_take_previous(device_config));
                NdpConfigurationUpdate { nud }
            })
        });
        DeviceConfigurationUpdate { arp, ndp }
    }

    /// Creates a new device configuration update for the given device.
    ///
    /// This method only validates that `config` is valid for `device`.
    /// [`DeviceApi::apply`] must be called to apply the configuration.
    pub fn new_configuration_update<'a>(
        &mut self,
        device: &'a BaseDeviceId<D, C::BindingsContext>,
        config: DeviceConfigurationUpdate,
    ) -> Result<
        PendingDeviceConfigurationUpdate<'a, BaseDeviceId<D, C::BindingsContext>>,
        DeviceConfigurationUpdateError,
    > {
        let core_ctx = self.core_ctx();
        let DeviceConfigurationUpdate { arp, ndp } = &config;
        if arp.is_some() && core_ctx.with_nud_config::<Ipv4, _, _>(device, |c| c.is_none()) {
            return Err(DeviceConfigurationUpdateError::ArpNotSupported);
        }
        if ndp.is_some() && core_ctx.with_nud_config::<Ipv6, _, _>(device, |c| c.is_none()) {
            return Err(DeviceConfigurationUpdateError::NdpNotSupported);
        }
        Ok(PendingDeviceConfigurationUpdate(config, device))
    }

    /// Returns a snapshot of the given device's configuration.
    pub fn get_configuration(
        &mut self,
        device: &BaseDeviceId<D, C::BindingsContext>,
    ) -> DeviceConfiguration {
        let core_ctx = self.core_ctx();
        let arp = core_ctx
            .with_nud_config::<Ipv4, _, _>(device, |config| config.cloned())
            .map(|nud| ArpConfiguration { nud });
        let ndp = core_ctx
            .with_nud_config::<Ipv6, _, _>(device, |config| config.cloned())
            .map(|nud| NdpConfiguration { nud });
        DeviceConfiguration { arp, ndp }
    }

    /// Returns a borrow to the [`DeviceCounters`] structure for `device`.
    pub fn get_counters<'a>(
        &'a mut self,
        device: &'a BaseDeviceId<D, C::BindingsContext>,
    ) -> &'a DeviceCounters {
        ResourceCounterContext::<_, DeviceCounters>::per_resource_counters(self.core_ctx(), device)
    }

    /// Exports state for `device` into `inspector`.
    pub fn inspect<N: Inspector>(
        &mut self,
        device: &BaseDeviceId<D, C::BindingsContext>,
        inspector: &mut N,
    ) {
        if let Some(tx_offload_spec) = self.core_ctx().tx_offload_spec(device) {
            inspector.record_child("TxOffloadSpec", |inspector| {
                inspector.delegate_inspectable(&tx_offload_spec);
            });
        }

        inspector.record_child("Counters", |inspector| {
            inspector.delegate_inspectable(
                ResourceCounterContext::<_, DeviceCounters>::per_resource_counters(
                    self.core_ctx(),
                    device,
                ),
            );
            inspector.delegate_inspectable(
                ResourceCounterContext::<_, D::Counters>::per_resource_counters(
                    self.core_ctx(),
                    device,
                ),
            );
            inspector.record_child("IPv4", |inspector| {
                inspector.delegate_inspectable(
                    ResourceCounterContext::<_, IpCounters<Ipv4>>::per_resource_counters(
                        self.core_ctx(),
                        device,
                    ),
                )
            });
            inspector.record_child("IPv6", |inspector| {
                inspector.delegate_inspectable(
                    ResourceCounterContext::<_, IpCounters<Ipv6>>::per_resource_counters(
                        self.core_ctx(),
                        device,
                    ),
                )
            });
            inspector.record_child("IGMP", |inspector| {
                inspector.delegate_inspectable(
                    ResourceCounterContext::<_, IgmpCounters>::per_resource_counters(
                        self.core_ctx(),
                        device,
                    ),
                );
            });
            inspector.record_child("MLD", |inspector| {
                inspector.delegate_inspectable(
                    ResourceCounterContext::<_, MldCounters>::per_resource_counters(
                        self.core_ctx(),
                        device,
                    ),
                );
            });
        });
    }
}

/// The device API interacting with any kind of supported device.
#[repr(transparent)]
pub struct DeviceAnyApi<C>(C);

impl<C> DeviceAnyApi<C> {
    /// Creates a new [`DeviceAnyApi`] from `ctx`.
    pub fn new(ctx: C) -> Self {
        Self(ctx)
    }
}

impl<C> DeviceAnyApi<C>
where
    C: ContextPair,
    C::CoreContext: DeviceApiCoreContext<EthernetLinkDevice, C::BindingsContext>
        + DeviceApiCoreContext<LoopbackDevice, C::BindingsContext>
        + DeviceApiCoreContext<PureIpDevice, C::BindingsContext>
        + DeviceApiCoreContext<BlackholeDevice, C::BindingsContext>,
    C::BindingsContext: DeviceApiBindingsContext,
{
    fn device<D>(&mut self) -> &mut DeviceApi<D, C> {
        let Self(ctx) = self;
        DeviceApi::ref_cast_mut(ctx)
    }

    /// Like [`DeviceApi::apply_configuration`] but for any device types.
    pub fn apply_configuration(
        &mut self,
        pending: PendingDeviceConfigurationUpdate<'_, DeviceId<C::BindingsContext>>,
    ) -> DeviceConfigurationUpdate {
        let PendingDeviceConfigurationUpdate(config, device) = pending;
        for_any_device_id!(DeviceId, device,
            device => {
                self.device().apply_configuration(PendingDeviceConfigurationUpdate(config, device))
            }
        )
    }

    /// Like [`DeviceApi::new_configuration_update`] but for any device
    /// types.
    pub fn new_configuration_update<'a>(
        &mut self,
        device: &'a DeviceId<C::BindingsContext>,
        config: DeviceConfigurationUpdate,
    ) -> Result<
        PendingDeviceConfigurationUpdate<'a, DeviceId<C::BindingsContext>>,
        DeviceConfigurationUpdateError,
    > {
        for_any_device_id!(DeviceId, device,
            inner => {
                self.device()
                .new_configuration_update(inner, config)
                .map(|PendingDeviceConfigurationUpdate(config, _)| {
                    PendingDeviceConfigurationUpdate(config, device)
                })
            }
        )
    }

    /// A shortcut for [`DeviceAnyApi::new_configuration_update`] followed by
    /// [`DeviceAnyApi::apply_configuration`].
    pub fn update_configuration(
        &mut self,
        device: &DeviceId<C::BindingsContext>,
        config: DeviceConfigurationUpdate,
    ) -> Result<DeviceConfigurationUpdate, DeviceConfigurationUpdateError> {
        let pending = self.new_configuration_update(device, config)?;
        Ok(self.apply_configuration(pending))
    }

    /// Like [`DeviceApi::get_configuration`] but for any device types.
    pub fn get_configuration(
        &mut self,
        device: &DeviceId<C::BindingsContext>,
    ) -> DeviceConfiguration {
        for_any_device_id!(DeviceId, device,
            device => self.device().get_configuration(device))
    }

    /// Like [`DeviceApi::get_counters`] but for any device types.
    pub fn get_counters<'a>(
        &'a mut self,
        device: &'a DeviceId<C::BindingsContext>,
    ) -> &'a DeviceCounters {
        for_any_device_id!(DeviceId, device,
            device => self.device().get_counters(device))
    }

    /// Like [`DeviceApi::inspect`] but for any device type.
    pub fn inspect<N: Inspector>(
        &mut self,
        device: &DeviceId<C::BindingsContext>,
        inspector: &mut N,
    ) {
        for_any_device_id!(DeviceId, DeviceProvider, D, device,
            device => self.device::<D>().inspect(device, inspector))
    }
}

/// A marker trait for all the core context traits required to fulfill the
/// [`DeviceApi`].
pub trait DeviceApiCoreContext<
    D: Device + DeviceStateSpec + DeviceReceiveFrameSpec,
    BC: DeviceApiBindingsContext,
>:
    DeviceIdContext<D, DeviceId = BaseDeviceId<D, BC>, WeakDeviceId = BaseWeakDeviceId<D, BC>>
    + OriginTrackerContext
    + DeviceCollectionContext<D, BC>
    + DeviceConfigurationContext<D>
    + RecvFrameContext<D::FrameMetadata<BaseDeviceId<D, BC>>, BC>
    + ResourceCounterContext<Self::DeviceId, DeviceCounters>
    + ResourceCounterContext<Self::DeviceId, D::Counters>
    + ResourceCounterContext<Self::DeviceId, IpCounters<Ipv4>>
    + ResourceCounterContext<Self::DeviceId, IpCounters<Ipv6>>
    + ResourceCounterContext<Self::DeviceId, IgmpCounters>
    + ResourceCounterContext<Self::DeviceId, MldCounters>
    + CoreTimerContext<D::TimerId<Self::WeakDeviceId>, BC>
    + DeviceTxOffloadSpecContext<D, BC>
{
}

impl<CC, D, BC> DeviceApiCoreContext<D, BC> for CC
where
    D: Device + DeviceStateSpec + DeviceReceiveFrameSpec,
    BC: DeviceApiBindingsContext,
    CC: DeviceIdContext<D, DeviceId = BaseDeviceId<D, BC>, WeakDeviceId = BaseWeakDeviceId<D, BC>>
        + OriginTrackerContext
        + DeviceCollectionContext<D, BC>
        + DeviceConfigurationContext<D>
        + RecvFrameContext<D::FrameMetadata<BaseDeviceId<D, BC>>, BC>
        + ResourceCounterContext<Self::DeviceId, DeviceCounters>
        + ResourceCounterContext<Self::DeviceId, D::Counters>
        + ResourceCounterContext<Self::DeviceId, IpCounters<Ipv4>>
        + ResourceCounterContext<Self::DeviceId, IpCounters<Ipv6>>
        + ResourceCounterContext<Self::DeviceId, IgmpCounters>
        + ResourceCounterContext<Self::DeviceId, MldCounters>
        + CoreTimerContext<D::TimerId<Self::WeakDeviceId>, BC>
        + DeviceTxOffloadSpecContext<D, BC>,
{
}

/// A marker trait for all the bindings context traits required to fulfill the
/// [`DeviceApi`].
pub trait DeviceApiBindingsContext: DeviceLayerTypes + ReferenceNotifiers + TimerContext {}

impl<O> DeviceApiBindingsContext for O where O: DeviceLayerTypes + ReferenceNotifiers + TimerContext {}

/// A marker trait with traits required to tie the device layer with the IP
/// layer to fulfill [`DeviceApi`].
pub trait DeviceApiIpLayerCoreContext<D: Device, BC: DeviceLayerTypes>:
    DeviceIdAnyCompatContext<D>
    + CoreTimerContext<
        IpDeviceTimerId<Ipv6, <Self as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        BC,
    > + CoreTimerContext<
        IpDeviceTimerId<Ipv4, <Self as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
        BC,
    >
{
}

impl<O, D, BC> DeviceApiIpLayerCoreContext<D, BC> for O
where
    D: Device,
    BC: DeviceLayerTypes,
    O: DeviceIdAnyCompatContext<D>
        + CoreTimerContext<
            IpDeviceTimerId<Ipv6, <Self as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
            BC,
        > + CoreTimerContext<
            IpDeviceTimerId<Ipv4, <Self as DeviceIdContext<AnyDevice>>::WeakDeviceId, BC>,
            BC,
        >,
{
}
