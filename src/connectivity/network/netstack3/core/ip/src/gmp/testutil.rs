// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides test utilities for generic multicast protocol implementations. The
//! utilities here allow testing GMP without regard for MLD/IGMP differences.

use alloc::vec::Vec;
use core::borrow::Borrow;
use core::time::Duration;
use net_declare::{net_ip_v4, net_ip_v6};
use net_types::MulticastAddr;
use net_types::ip::{Ip, Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
use netstack3_base::testutil::{FakeBindingsCtx, FakeDeviceId, FakeWeakDeviceId};
use netstack3_base::{
    AnyDevice, CtxPair, DeviceIdContext, HandleableTimer, InspectableValue, Inspector,
    IntoCoreTimerCtx, TimerBindingsTypes,
};
use packet_formats::gmp::{GmpReportGroupRecord, GroupRecordType};
use packet_formats::utils::NonZeroDuration;

use crate::internal::gmp::{
    self, GmpContext, GmpContextInner, GmpEnabledGroup, GmpGroupState, GmpMode, GmpState,
    GmpStateRef, GmpTimerId, GmpTypeLayout, IpExt, MulticastGroupSet,
};

pub(super) struct FakeGmpContext<I: IpExt> {
    pub inner: FakeGmpContextInner<I>,
    pub enabled: bool,
    pub groups: MulticastGroupSet<I::Addr, GmpGroupState<I, FakeGmpBindingsContext<I>>>,
    pub gmp: GmpState<I, FakeGmpTypeLayout, FakeGmpBindingsContext<I>>,
    pub config: FakeGmpConfig,
}

impl<I: IpExt> FakeGmpContext<I> {}

impl<I: IpExt> DeviceIdContext<AnyDevice> for FakeGmpContext<I> {
    type DeviceId = FakeDeviceId;
    type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
}

pub(super) type FakeGmpBindingsContext<I> =
    FakeBindingsCtx<GmpTimerId<I, FakeWeakDeviceId<FakeDeviceId>>, (), (), ()>;

pub(super) enum FakeGmpTypeLayout {}

impl<I: IpExt> GmpTypeLayout<I, FakeGmpBindingsContext<I>> for FakeGmpTypeLayout {
    type Config = FakeGmpConfig;
    type ProtoMode = GmpMode;
}

impl<I: IpExt> GmpContext<I, FakeGmpBindingsContext<I>> for FakeGmpContext<I> {
    type TypeLayout = FakeGmpTypeLayout;
    type Inner<'a> = &'a mut FakeGmpContextInner<I>;

    fn with_gmp_state_mut_and_ctx<
        O,
        F: FnOnce(
            Self::Inner<'_>,
            GmpStateRef<'_, I, FakeGmpTypeLayout, FakeGmpBindingsContext<I>>,
        ) -> O,
    >(
        &mut self,
        _device: &Self::DeviceId,
        cb: F,
    ) -> O {
        let Self { inner, enabled, groups, gmp, config } = self;
        cb(inner, GmpStateRef { enabled: *enabled, groups, gmp, config })
    }
}

impl<I: IpExt> HandleableTimer<FakeGmpContext<I>, FakeGmpBindingsContext<I>>
    for GmpTimerId<I, FakeWeakDeviceId<FakeDeviceId>>
{
    fn handle(
        self,
        core_ctx: &mut FakeGmpContext<I>,
        bindings_ctx: &mut FakeGmpBindingsContext<I>,
        _timer: <FakeGmpBindingsContext<I> as TimerBindingsTypes>::UniqueTimerId,
    ) {
        gmp::handle_timer(core_ctx, bindings_ctx, self);
    }
}

#[derive(Default)]
pub(super) struct FakeGmpContextInner<I: IpExt> {
    pub v1_messages: Vec<(MulticastAddr<I::Addr>, gmp::v1::GmpMessageType)>,
    pub v2_messages: Vec<Vec<(MulticastAddr<I::Addr>, GroupRecordType, Vec<I::Addr>)>>,
}

impl<I: IpExt> DeviceIdContext<AnyDevice> for &'_ mut FakeGmpContextInner<I> {
    type DeviceId = FakeDeviceId;
    type WeakDeviceId = FakeWeakDeviceId<FakeDeviceId>;
}

impl<I: IpExt> GmpContextInner<I, FakeGmpBindingsContext<I>> for &'_ mut FakeGmpContextInner<I> {
    type TypeLayout = FakeGmpTypeLayout;

    fn send_message_v1(
        &mut self,
        _bindings_ctx: &mut FakeGmpBindingsContext<I>,
        _device: &Self::DeviceId,
        _cur_mode: &GmpMode,
        group_addr: GmpEnabledGroup<I::Addr>,
        msg_type: gmp::v1::GmpMessageType,
    ) {
        self.v1_messages.push((group_addr.into_multicast_addr(), msg_type));
    }

    fn send_report_v2(
        &mut self,
        _bindings_ctx: &mut FakeGmpBindingsContext<I>,
        _device: &Self::DeviceId,
        groups: impl Iterator<Item: GmpReportGroupRecord<I::Addr> + Clone> + Clone,
    ) {
        // NB: We sort the message so we can perform stable equality operations
        // in tests.
        let mut groups = groups
            .map(|g| {
                let mut sources = g.sources().map(|i| i.borrow().clone()).collect::<Vec<_>>();
                sources.sort();
                (g.group(), g.record_type(), sources)
            })
            .collect::<Vec<_>>();
        groups.sort();
        self.v2_messages.push(groups)
    }

    fn mode_update_from_v1_query<Q: gmp::v1::QueryMessage<I>>(
        &mut self,
        _bindings_ctx: &mut FakeGmpBindingsContext<I>,
        _query: &Q,
        gmp_state: &GmpState<I, Self::TypeLayout, FakeGmpBindingsContext<I>>,
        _config: &FakeGmpConfig,
    ) -> GmpMode {
        gmp_state.mode.maybe_enter_v1_compat()
    }

    fn mode_to_config(
        _mode: &<Self::TypeLayout as GmpTypeLayout<I, FakeGmpBindingsContext<I>>>::ProtoMode,
    ) -> I::GmpProtoConfigMode {
        unimplemented!()
    }

    fn config_to_mode(
        _cur_mode: &<Self::TypeLayout as GmpTypeLayout<I, FakeGmpBindingsContext<I>>>::ProtoMode,
        _config: I::GmpProtoConfigMode,
    ) -> GmpMode {
        unimplemented!()
    }

    fn mode_on_disable(cur_mode: &GmpMode) -> GmpMode {
        cur_mode.maybe_exit_v1_compat()
    }

    fn mode_on_exit_compat() -> GmpMode {
        GmpMode::V2
    }
}

#[derive(Debug, Default)]
pub(super) struct FakeGmpConfig {}

impl gmp::v1::ProtocolConfig for FakeGmpConfig {
    fn unsolicited_report_interval(&self) -> Duration {
        Duration::from_secs(10)
    }

    fn send_leave_anyway(&self) -> bool {
        false
    }

    fn get_max_resp_time(&self, resp_time: Duration) -> Option<NonZeroDuration> {
        NonZeroDuration::new(resp_time)
    }
}

impl gmp::v2::ProtocolConfig for FakeGmpConfig {
    fn query_response_interval(&self) -> NonZeroDuration {
        gmp::v2::DEFAULT_QUERY_RESPONSE_INTERVAL
    }

    fn unsolicited_report_interval(&self) -> NonZeroDuration {
        gmp::v2::DEFAULT_UNSOLICITED_REPORT_INTERVAL
    }
}

pub(super) type FakeCtx<I> = CtxPair<FakeGmpContext<I>, FakeGmpBindingsContext<I>>;

pub(super) fn new_context_with_mode<I: IpExt>(mode: GmpMode) -> FakeCtx<I> {
    FakeCtx::with_default_bindings_ctx(|bindings_ctx| {
        // Use "true" random numbers. This drives better coverage over time of
        // all the randomized delays in GMP, preventing a fixed seed from hiding
        // subtle bugs.

        // We start with enabled true to make tests easier to write.
        let enabled = true;
        bindings_ctx.rng = rand::make_rng();
        FakeGmpContext {
            inner: FakeGmpContextInner::default(),
            enabled,
            groups: Default::default(),
            gmp: GmpState::new_with_enabled_and_mode::<_, IntoCoreTimerCtx>(
                bindings_ctx,
                FakeWeakDeviceId(FakeDeviceId),
                enabled,
                mode,
            ),
            config: Default::default(),
        }
    })
}

pub(super) struct FakeV1Query<I: Ip> {
    pub group_addr: I::Addr,
    pub max_response_time: Duration,
}

impl<I: Ip> gmp::v1::QueryMessage<I> for FakeV1Query<I> {
    fn group_addr(&self) -> I::Addr {
        self.group_addr
    }

    fn max_response_time(&self) -> Duration {
        self.max_response_time
    }
}

pub(super) struct FakeV2Query<I: Ip> {
    pub group_addr: I::Addr,
    pub max_response_time: Duration,
    pub robustness_variable: u8,
    pub sources: Vec<I::Addr>,
    pub query_interval: Duration,
}
impl<I: Ip> Default for FakeV2Query<I> {
    fn default() -> Self {
        Self {
            group_addr: I::UNSPECIFIED_ADDRESS,
            max_response_time: gmp::v2::DEFAULT_QUERY_RESPONSE_INTERVAL.into(),
            robustness_variable: gmp::v2::DEFAULT_ROBUSTNESS_VARIABLE.into(),
            sources: Default::default(),
            query_interval: gmp::v2::DEFAULT_QUERY_INTERVAL.into(),
        }
    }
}

impl<I: Ip> gmp::v2::QueryMessage<I> for FakeV2Query<I> {
    fn as_v1(&self) -> impl gmp::v1::QueryMessage<I> + '_ {
        let Self { group_addr, max_response_time, .. } = self;
        FakeV1Query { group_addr: *group_addr, max_response_time: *max_response_time }
    }

    fn robustness_variable(&self) -> u8 {
        self.robustness_variable
    }

    fn query_interval(&self) -> Duration {
        self.query_interval
    }

    fn group_address(&self) -> <I as Ip>::Addr {
        self.group_addr
    }

    fn max_response_time(&self) -> Duration {
        self.max_response_time
    }

    fn sources(&self) -> impl Iterator<Item = I::Addr> + '_ {
        self.sources.iter().copied()
    }
}

// Required to satisfy trait bounds and use GmpMode directly as our protocol
// mode.
impl InspectableValue for GmpMode {
    fn record<I: Inspector>(&self, _name: &str, _inspector: &mut I) {
        unimplemented!()
    }
}

/// Extension trait so IP-independent tests can be written.
pub(super) trait TestIpExt: netstack3_base::testutil::TestIpExt + IpExt {
    const GROUP_ADDR1: MulticastAddr<Self::Addr>;
    const GROUP_ADDR2: MulticastAddr<Self::Addr>;
}

impl TestIpExt for Ipv4 {
    const GROUP_ADDR1: MulticastAddr<Ipv4Addr> =
        unsafe { MulticastAddr::new_unchecked(net_ip_v4!("224.0.0.4")) };
    const GROUP_ADDR2: MulticastAddr<Ipv4Addr> =
        unsafe { MulticastAddr::new_unchecked(net_ip_v4!("224.0.0.5")) };
}

impl TestIpExt for Ipv6 {
    const GROUP_ADDR1: MulticastAddr<Ipv6Addr> =
        unsafe { MulticastAddr::new_unchecked(net_ip_v6!("ff02::AA")) };
    const GROUP_ADDR2: MulticastAddr<Ipv6Addr> =
        unsafe { MulticastAddr::new_unchecked(net_ip_v6!("ff02::AB")) };
}
