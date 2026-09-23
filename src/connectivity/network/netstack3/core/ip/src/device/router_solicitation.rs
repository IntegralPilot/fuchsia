// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! IPv6 Router Solicitation as defined by [RFC 4861 section 6.3.7].
//!
//! [RFC 4861 section 6.3.7]: https://datatracker.ietf.org/doc/html/rfc4861#section-6.3.7

use core::num::NonZeroU8;
use core::time::Duration;

use assert_matches::assert_matches;
use derivative::Derivative;
use net_types::UnicastAddr;
use net_types::ip::Ipv6Addr;
use netstack3_base::{
    AnyDevice, CoreTimerContext, DeviceIdContext, HandleableTimer, NetworkPartialSerializer,
    NetworkSerializer, RngContext, StrongDeviceIdentifier as _, TimerBindingsTypes, TimerContext,
    TimerHandler, WeakDeviceIdentifier,
};
use packet::{EitherSerializer, EmptyBuf, InnerPacketBuilder as _};
use packet_formats::icmp::ndp::options::NdpOptionBuilder;
use packet_formats::icmp::ndp::{OptionSequenceBuilder, RouterSolicitation};
use rand::RngExt as _;

use crate::internal::base::IpSendFrameError;
use crate::internal::device::Ipv6LinkLayerAddr;

/// Amount of time to wait after sending `MAX_RTR_SOLICITATIONS` Router
/// Solicitation messages before determining that there are no routers on the
/// link for the purpose of IPv6 Stateless Address Autoconfiguration if no
/// Router Advertisement messages have been received as defined in [RFC 4861
/// section 10].
///
/// This parameter is also used when a host sends its initial Router
/// Solicitation message, as per [RFC 4861 section 6.3.7]. Before a node sends
/// an initial solicitation, it SHOULD delay the transmission for a random
/// amount of time between 0 and `MAX_RTR_SOLICITATION_DELAY`. This serves to
/// alleviate congestion when many hosts start up on a link at the same time.
///
/// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
/// [RFC 4861 section 6.3.7]: https://tools.ietf.org/html/rfc4861#section-6.3.7
pub const MAX_RTR_SOLICITATION_DELAY: Duration = Duration::from_secs(1);

/// Minimum duration between router solicitation messages as defined in [RFC
/// 4861 section 10].
///
/// [RFC 4861 section 10]: https://tools.ietf.org/html/rfc4861#section-10
pub const RTR_SOLICITATION_INTERVAL: Duration = Duration::from_secs(4);

/// A router solicitation timer.
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub struct RsTimerId<D: WeakDeviceIdentifier> {
    device_id: D,
}

impl<D: WeakDeviceIdentifier> RsTimerId<D> {
    pub(super) fn device_id(&self) -> &D {
        &self.device_id
    }

    /// Create a new [`RsTimerId`] for `device_id`.
    #[cfg(any(test, feature = "testutils"))]
    pub fn new(device_id: D) -> Self {
        Self { device_id }
    }
}

/// A device's router solicitation state.
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
pub struct RsState<BT: RsBindingsTypes> {
    solicitations_sent: u8,
    timer: Option<BT::Timer>,
}

/// The execution context for router solicitation.
pub trait RsContext<BC: RsBindingsTypes>:
    DeviceIdContext<AnyDevice> + CoreTimerContext<RsTimerId<Self::WeakDeviceId>, BC>
{
    /// A link-layer address.
    type LinkLayerAddr: Ipv6LinkLayerAddr;

    /// Calls the callback with a mutable reference to the router solicitation
    /// state and the maximum number of router solications to send.
    fn with_rs_state_mut_and_max<O, F: FnOnce(&mut RsState<BC>, Option<NonZeroU8>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O;

    /// Calls the callback with a mutable reference to the router solicitation
    /// state.
    fn with_rs_state_mut<O, F: FnOnce(&mut RsState<BC>) -> O>(
        &mut self,
        device_id: &Self::DeviceId,
        cb: F,
    ) -> O {
        self.with_rs_state_mut_and_max(device_id, |state, _max| cb(state))
    }

    /// Gets the device's link-layer address, if the device supports link-layer
    /// addressing.
    fn get_link_layer_addr(&mut self, device_id: &Self::DeviceId) -> Option<Self::LinkLayerAddr>;

    /// Sends an NDP Router Solicitation to the local-link.
    ///
    /// The callback is called with a source address suitable for an outgoing
    /// router solicitation message and returns the message body.
    fn send_rs_packet<
        S: NetworkSerializer<Buffer = EmptyBuf> + NetworkPartialSerializer,
        F: FnOnce(Option<UnicastAddr<Ipv6Addr>>) -> S,
    >(
        &mut self,
        bindings_ctx: &mut BC,
        device_id: &Self::DeviceId,
        message: RouterSolicitation,
        body: F,
    ) -> Result<(), IpSendFrameError<S>>;
}

/// The bindings types for router solicitation.
pub trait RsBindingsTypes: TimerBindingsTypes {}
impl<BT> RsBindingsTypes for BT where BT: TimerBindingsTypes {}

/// The bindings execution context for router solicitation.
pub trait RsBindingsContext: RngContext + TimerContext {}
impl<BC> RsBindingsContext for BC where BC: RngContext + TimerContext {}

/// An implementation of Router Solicitation.
pub trait RsHandler<BC: RsBindingsTypes>:
    DeviceIdContext<AnyDevice> + TimerHandler<BC, RsTimerId<Self::WeakDeviceId>>
{
    /// Starts router solicitation.
    fn start_router_solicitation(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId);

    /// Stops router solicitation.
    ///
    /// Does nothing if router solicitaiton is not being performed
    fn stop_router_solicitation(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId);
}

impl<BC: RsBindingsContext, CC: RsContext<BC>> RsHandler<BC> for CC {
    fn start_router_solicitation(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId) {
        self.with_rs_state_mut_and_max(device_id, |state, max| {
            let RsState { solicitations_sent, timer } = state;
            *solicitations_sent = 0;

            // The caller *must call* `stop_router_solicitation` before starting
            // a new one.
            assert_matches!(timer, None);

            match max {
                None => {}
                Some(_) => {
                    // As per RFC 4861 section 6.3.7, delay the first transmission for a
                    // random amount of time between 0 and `MAX_RTR_SOLICITATION_DELAY` to
                    // alleviate congestion when many hosts start up on a link at the same
                    // time.
                    let delay =
                        bindings_ctx.rng().random_range(Duration::ZERO..MAX_RTR_SOLICITATION_DELAY);

                    let timer = timer.insert(CC::new_timer(
                        bindings_ctx,
                        RsTimerId { device_id: device_id.downgrade() },
                    ));
                    assert_eq!(bindings_ctx.schedule_timer(delay, timer), None);
                }
            }
        });
    }

    fn stop_router_solicitation(&mut self, bindings_ctx: &mut BC, device_id: &Self::DeviceId) {
        self.with_rs_state_mut(device_id, |state| {
            // Get rid of the installed timer if we're stopping solicitations.
            if let Some(mut timer) = state.timer.take() {
                let _: Option<BC::Instant> = bindings_ctx.cancel_timer(&mut timer);
            }
        });
    }
}

impl<BC: RsBindingsContext, CC: RsContext<BC>> HandleableTimer<CC, BC>
    for RsTimerId<CC::WeakDeviceId>
{
    fn handle(self, core_ctx: &mut CC, bindings_ctx: &mut BC, timer: BC::UniqueTimerId) {
        let Self { device_id } = self;
        if let Some(device_id) = device_id.upgrade() {
            do_router_solicitation(core_ctx, bindings_ctx, &device_id, timer)
        }
    }
}

/// Solicit routers once and schedule next message.
fn do_router_solicitation<BC: RsBindingsContext, CC: RsContext<BC>>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device_id: &CC::DeviceId,
    timer_id: BC::UniqueTimerId,
) {
    let send_rs = core_ctx.with_rs_state_mut_and_max(
        device_id,
        |RsState { solicitations_sent, timer }, max| {
            let Some(timer) = timer.as_mut() else {
                // If we don't have a timer then RS has been stopped.
                return false;
            };
            if bindings_ctx.unique_timer_id(timer) != timer_id {
                // This is an errant request from a previous round of RS enablement,
                // just ignore it and don't send a solicitation.
                return false;
            }

            let max_solicitations = max.map_or(0, NonZeroU8::get);
            if *solicitations_sent >= max_solicitations {
                return false;
            }

            *solicitations_sent = solicitations_sent.saturating_add(1);

            // Schedule the next timer if more solicitations are needed.
            if *solicitations_sent < max_solicitations {
                assert_eq!(bindings_ctx.schedule_timer(RTR_SOLICITATION_INTERVAL, timer), None);
            }

            true
        },
    );

    if !send_rs {
        return;
    }

    let src_ll = core_ctx.get_link_layer_addr(device_id);

    // TODO(https://fxbug.dev/42165912): Either panic or guarantee that this error
    // can't happen statically.
    let _: Result<(), _> =
        core_ctx.send_rs_packet(bindings_ctx, device_id, RouterSolicitation::default(), |src_ip| {
            // As per RFC 4861 section 4.1,
            //
            //   Valid Options:
            //
            //      Source link-layer address The link-layer address of the
            //                     sender, if known. MUST NOT be included if the
            //                     Source Address is the unspecified address.
            //                     Otherwise, it SHOULD be included on link
            //                     layers that have addresses.
            src_ip.map_or(EitherSerializer::A(EmptyBuf), |UnicastAddr { .. }| {
                EitherSerializer::B(
                    OptionSequenceBuilder::new(
                        src_ll
                            .as_ref()
                            .map(Ipv6LinkLayerAddr::as_bytes)
                            .into_iter()
                            .map(NdpOptionBuilder::SourceLinkLayerAddress),
                    )
                    .into_serializer(),
                )
            })
        });
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use net_declare::net_ip_v6;
    use netstack3_base::testutil::{
        FakeBindingsCtx, FakeCoreCtx, FakeDeviceId, FakeTimerCtxExt as _, FakeWeakDeviceId,
    };
    use netstack3_base::{CtxPair, InstantContext as _, SendFrameContext as _};
    use packet_formats::icmp::ndp::Options;
    use packet_formats::icmp::ndp::options::NdpOption;
    use test_case::test_case;

    use super::*;

    struct FakeRsContext {
        max_router_solicitations: Option<NonZeroU8>,
        rs_state: RsState<FakeBindingsCtxImpl>,
        source_address: Option<UnicastAddr<Ipv6Addr>>,
        link_layer_bytes: Option<Vec<u8>>,
    }

    #[derive(Debug, PartialEq)]
    struct RsMessageMeta {
        message: RouterSolicitation,
    }

    type FakeCoreCtxImpl = FakeCoreCtx<FakeRsContext, RsMessageMeta, FakeDeviceId>;
    type FakeBindingsCtxImpl =
        FakeBindingsCtx<RsTimerId<FakeWeakDeviceId<FakeDeviceId>>, (), (), ()>;

    impl CoreTimerContext<RsTimerId<FakeWeakDeviceId<FakeDeviceId>>, FakeBindingsCtxImpl>
        for FakeCoreCtxImpl
    {
        fn convert_timer(
            dispatch_id: RsTimerId<FakeWeakDeviceId<FakeDeviceId>>,
        ) -> <FakeBindingsCtxImpl as TimerBindingsTypes>::DispatchId {
            dispatch_id
        }
    }

    impl Ipv6LinkLayerAddr for Vec<u8> {
        fn as_bytes(&self) -> &[u8] {
            &self
        }

        fn eui64_iid(&self) -> [u8; 8] {
            unimplemented!()
        }
    }

    impl RsContext<FakeBindingsCtxImpl> for FakeCoreCtxImpl {
        type LinkLayerAddr = Vec<u8>;

        fn with_rs_state_mut_and_max<
            O,
            F: FnOnce(&mut RsState<FakeBindingsCtxImpl>, Option<NonZeroU8>) -> O,
        >(
            &mut self,
            &FakeDeviceId: &FakeDeviceId,
            cb: F,
        ) -> O {
            let FakeRsContext { max_router_solicitations, rs_state, .. } = &mut self.state;
            cb(rs_state, *max_router_solicitations)
        }

        fn get_link_layer_addr(&mut self, &FakeDeviceId: &FakeDeviceId) -> Option<Vec<u8>> {
            let FakeRsContext { link_layer_bytes, .. } = &self.state;
            link_layer_bytes.clone()
        }

        fn send_rs_packet<
            S: NetworkSerializer<Buffer = EmptyBuf>,
            F: FnOnce(Option<UnicastAddr<Ipv6Addr>>) -> S,
        >(
            &mut self,
            bindings_ctx: &mut FakeBindingsCtxImpl,
            &FakeDeviceId: &FakeDeviceId,
            message: RouterSolicitation,
            body: F,
        ) -> Result<(), IpSendFrameError<S>> {
            let FakeRsContext { source_address, .. } = &self.state;
            self.send_frame(bindings_ctx, RsMessageMeta { message }, body(*source_address))
                .map_err(|e| e.err_into())
        }
    }

    const RS_TIMER_ID: RsTimerId<FakeWeakDeviceId<FakeDeviceId>> =
        RsTimerId { device_id: FakeWeakDeviceId(FakeDeviceId) };

    #[test]
    fn stop_router_solicitation() {
        let CtxPair { mut core_ctx, mut bindings_ctx } =
            CtxPair::with_core_ctx(FakeCoreCtxImpl::with_state(FakeRsContext {
                max_router_solicitations: NonZeroU8::new(1),
                rs_state: Default::default(),
                source_address: None,
                link_layer_bytes: None,
            }));
        RsHandler::start_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);

        let now = bindings_ctx.now();
        bindings_ctx
            .timers
            .assert_timers_installed_range([(RS_TIMER_ID, now..=now + MAX_RTR_SOLICITATION_DELAY)]);

        RsHandler::stop_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);
        bindings_ctx.timers.assert_no_timers_installed();

        assert_eq!(core_ctx.frames(), &[][..]);
    }

    const SOURCE_ADDRESS: UnicastAddr<Ipv6Addr> =
        unsafe { UnicastAddr::new_unchecked(net_ip_v6!("fe80::1")) };

    #[test_case(0, None, None, None; "disabled")]
    #[test_case(1, None, None, None; "once_without_source_address_or_link_layer_option")]
    #[test_case(
        1,
        Some(SOURCE_ADDRESS),
        None,
        None; "once_with_source_address_and_without_link_layer_option")]
    #[test_case(
        1,
        None,
        Some(vec![1, 2, 3, 4, 5, 6]),
        None; "once_without_source_address_and_with_mac_address_source_link_layer_option")]
    #[test_case(
        1,
        Some(SOURCE_ADDRESS),
        Some(vec![1, 2, 3, 4, 5, 6]),
        Some(&[1, 2, 3, 4, 5, 6]); "once_with_source_address_and_mac_address_source_link_layer_option")]
    #[test_case(
        1,
        Some(SOURCE_ADDRESS),
        Some(vec![1, 2, 3, 4, 5]),
        Some(&[1, 2, 3, 4, 5, 0]); "once_with_source_address_and_short_address_source_link_layer_option")]
    #[test_case(
        1,
        Some(SOURCE_ADDRESS),
        Some(vec![1, 2, 3, 4, 5, 6, 7]),
        Some(&[
            1, 2, 3, 4, 5, 6, 7,
            0, 0, 0, 0, 0, 0, 0,
        ]); "once_with_source_address_and_long_address_source_link_layer_option")]
    fn perform_router_solicitation(
        max_router_solicitations: u8,
        source_address: Option<UnicastAddr<Ipv6Addr>>,
        link_layer_bytes: Option<Vec<u8>>,
        expected_sll_bytes: Option<&[u8]>,
    ) {
        let CtxPair { mut core_ctx, mut bindings_ctx } =
            CtxPair::with_core_ctx(FakeCoreCtxImpl::with_state(FakeRsContext {
                max_router_solicitations: NonZeroU8::new(max_router_solicitations),
                rs_state: Default::default(),
                source_address,
                link_layer_bytes,
            }));
        RsHandler::start_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);

        assert_eq!(core_ctx.frames(), &[][..]);

        let mut duration = MAX_RTR_SOLICITATION_DELAY;
        for i in 0..max_router_solicitations {
            assert_eq!(core_ctx.state.rs_state.solicitations_sent, i);
            let now = bindings_ctx.now();
            bindings_ctx
                .timers
                .assert_timers_installed_range([(RS_TIMER_ID, now..=now + duration)]);

            assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(RS_TIMER_ID));
            let frames = core_ctx.frames();
            assert_eq!(frames.len(), usize::from(i + 1), "frames = {:?}", frames);
            let (RsMessageMeta { message }, frame) =
                frames.last().expect("should have transmitted a frame");
            assert_eq!(*message, RouterSolicitation::default());
            let options = Options::parse(&frame[..]).expect("parse NDP options");
            let sll_bytes = options.iter().find_map(|o| match o {
                NdpOption::SourceLinkLayerAddress(a) => Some(a),
                o => panic!("unexpected NDP option = {:?}", o),
            });

            assert_eq!(sll_bytes, expected_sll_bytes);
            duration = RTR_SOLICITATION_INTERVAL;
        }

        bindings_ctx.timers.assert_no_timers_installed();
        assert_eq!(core_ctx.state.rs_state.solicitations_sent, max_router_solicitations);
        let frames = core_ctx.frames();
        assert_eq!(frames.len(), usize::from(max_router_solicitations), "frames = {:?}", frames);
    }

    #[test]
    fn max_router_solicitations_updated_after_start() {
        let CtxPair { mut core_ctx, mut bindings_ctx } =
            CtxPair::with_core_ctx(FakeCoreCtxImpl::with_state(FakeRsContext {
                max_router_solicitations: NonZeroU8::new(3),
                rs_state: Default::default(),
                source_address: None,
                link_layer_bytes: None,
            }));
        RsHandler::start_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);

        // Trigger first timer: 1 RS sent.
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(RS_TIMER_ID));
        assert_eq!(core_ctx.frames().len(), 1);

        // Reduce max_router_solicitations to 1 while solicitation is ongoing.
        core_ctx.state.max_router_solicitations = NonZeroU8::new(1);

        // Trigger second timer: 0 RS sent because of the new limit.
        assert_eq!(bindings_ctx.trigger_next_timer(&mut core_ctx), Some(RS_TIMER_ID));
        assert_eq!(core_ctx.frames().len(), 1);
        bindings_ctx.timers.assert_no_timers_installed();
    }

    // Regression guard for when router solicitations would consider timers for
    // the previously enabled start request if they were raced against dispatch.
    #[test]
    fn previous_cycle_timers_ignored() {
        let CtxPair { mut core_ctx, mut bindings_ctx } =
            CtxPair::with_core_ctx(FakeCoreCtxImpl::with_state(FakeRsContext {
                max_router_solicitations: NonZeroU8::new(1),
                rs_state: Default::default(),
                source_address: None,
                link_layer_bytes: None,
            }));
        RsHandler::start_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);
        // Get the timer ID for the first instantiation.
        let timer_id = core_ctx.state.rs_state.timer.as_ref().unwrap().timer_id();
        RsHandler::stop_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);
        // Attempt to run router solicitation after stopping, nothing is sent out.
        do_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, timer_id);
        assert_eq!(core_ctx.frames(), &[][..]);
        // Start again and if the same timer_id fires it should not cause an RS
        // to be sent out.
        RsHandler::start_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId);
        do_router_solicitation(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, timer_id);
        assert_eq!(core_ctx.frames(), &[][..]);
    }
}
