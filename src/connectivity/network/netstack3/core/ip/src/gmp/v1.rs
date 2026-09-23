// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! GMP V1 common implementation.
//!
//! GMPv1 is the common implementation of a fictitious GMP protocol that covers
//! the common parts of MLDv1 and IGMPv1, IGMPv2.

use core::time::Duration;

use assert_matches::assert_matches;
use net_types::MulticastAddr;
use net_types::ip::Ip;
use netstack3_base::Instant;
use packet_formats::utils::NonZeroDuration;
use rand::Rng;

use crate::internal::gmp::{
    self, GmpBindingsContext, GmpContext, GmpContextInner, GmpEnabledGroup, GmpGroupState, GmpMode,
    GmpStateRef, GroupJoinResult, GroupLeaveResult, IpExt, NotAMemberErr, QueryTarget,
};

/// Timers installed by GMP v1.
///
/// The timer always refers to a delayed report.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub(super) struct DelayedReportTimerId<I: Ip>(pub(super) GmpEnabledGroup<I::Addr>);

#[cfg(test)]
impl<I: IpExt> DelayedReportTimerId<I> {
    pub(super) fn new_multicast(addr: MulticastAddr<I::Addr>) -> Self {
        Self(GmpEnabledGroup::try_new(addr).expect("not GMP enabled"))
    }
}

/// A type of GMP v1 message.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum GmpMessageType {
    Report,
    Leave,
}

/// Actions to take as a consequence of joining a group.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(super) struct JoinGroupActions {
    send_report_and_schedule_timer: Option<Duration>,
}

impl JoinGroupActions {
    const NOOP: Self = Self { send_report_and_schedule_timer: None };
}

/// Actions to take as a consequence of leaving a group.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct LeaveGroupActions {
    send_leave: bool,
    stop_timer: bool,
}

impl LeaveGroupActions {
    const NOOP: Self = Self { send_leave: false, stop_timer: false };
}

/// Actions to take as a consequence of handling a received report message.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(super) struct ReportReceivedActions {
    pub(super) stop_timer: bool,
}

impl ReportReceivedActions {
    const NOOP: Self = Self { stop_timer: false };
}

/// Actions to take as a consequence of receiving a query message.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(super) enum QueryReceivedActions {
    ScheduleTimer(Duration),
    StopTimerAndSendReport,
    None,
}

/// Actions to take as a consequence of a report timer expiring.
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(super) struct ReportTimerExpiredActions;

/// This trait is used to model configuration values used by GMP and abstracting
/// the different parts of MLDv1 and IGMPv1/v2.
pub trait ProtocolConfig {
    /// The maximum delay to wait to send an unsolicited report.
    fn unsolicited_report_interval(&self) -> Duration;

    /// Whether the host should send a leave message even if it is not the last
    /// host in the group.
    fn send_leave_anyway(&self) -> bool;

    /// Get the _real_ `MAX_RESP_TIME`
    ///
    /// `None` indicates that the maximum response time is zero and thus a
    /// response should be sent immediately.
    fn get_max_resp_time(&self, resp_time: Duration) -> Option<NonZeroDuration>;
}

/// The transition between one state and the next.
///
/// A `Transition` includes the next state to enter and any actions to take
/// while executing the transition.
struct Transition<S, Actions>(S, Actions);

/// Represents Non Member-specific state variables.
///
/// Memberships may be a non-member when joined locally but are not performing
/// GMP.
///
/// Note that the special all-nodes addresses 224.0.0.1 and ff02::1 are modelled
/// as permanently in `NonMember` state instead of `Idle` state in NS3.
#[cfg_attr(test, derive(Debug))]
pub(super) struct NonMember;

/// Represents Delaying Member-specific state variables.
#[cfg_attr(test, derive(Debug))]
pub(super) struct DelayingMember<I: Instant> {
    /// The expiration time for the current timer. Useful to check if the timer
    /// needs to be reset when a query arrives.
    timer_expiration: I,

    /// Used to indicate whether we need to send out a Leave message when we are
    /// leaving the group. This flag will become false once we heard about
    /// another reporter.
    last_reporter: bool,
}

/// Represents Idle Member-specific state variables.
#[cfg_attr(test, derive(Debug))]
pub(super) struct IdleMember {
    /// Used to indicate whether we need to send out a Leave message when we are
    /// leaving the group.
    last_reporter: bool,
}

/// The state for a multicast group membership.
///
/// The terms used here are biased towards [IGMPv2]. In [MLD], their names are
/// {Non, Delaying, Idle}-Listener instead.
///
/// [IGMPv2]: https://tools.ietf.org/html/rfc2236
/// [MLD]: https://tools.ietf.org/html/rfc2710
#[cfg_attr(test, derive(Debug))]
pub(super) enum MemberState<I: Instant> {
    NonMember(NonMember),
    Delaying(DelayingMember<I>),
    Idle(IdleMember),
}

impl<I: Instant> From<NonMember> for MemberState<I> {
    fn from(s: NonMember) -> Self {
        MemberState::NonMember(s)
    }
}

impl<I: Instant> From<DelayingMember<I>> for MemberState<I> {
    fn from(s: DelayingMember<I>) -> Self {
        MemberState::Delaying(s)
    }
}

impl<I: Instant> From<IdleMember> for MemberState<I> {
    fn from(s: IdleMember) -> Self {
        MemberState::Idle(s)
    }
}

impl<S, A> Transition<S, A> {
    fn into_state_actions<I: Instant>(self) -> (MemberState<I>, A)
    where
        MemberState<I>: From<S>,
    {
        (self.0.into(), self.1)
    }
}

/// Compute the next state and actions to take for a member state (Delaying or
/// Idle member) that has received a query message.
///
/// # Arguments
/// * `last_reporter` indicates if the last report was sent by this node.
/// * `timer_expiration` is `None` if there are currently no timers, otherwise
///   `Some(t)` where `t` is the old instant when the currently installed timer
///   should fire. That is, `None` if an Idle member and `Some` if a Delaying
///   member.
/// * `max_resp_time` is the maximum response time required by Query message.
fn member_query_received<R: Rng, I: Instant, C: ProtocolConfig>(
    rng: &mut R,
    last_reporter: bool,
    timer_expiration: Option<I>,
    max_resp_time: Duration,
    now: I,
    cfg: &C,
) -> (MemberState<I>, QueryReceivedActions) {
    let (transition, actions) = match cfg.get_max_resp_time(max_resp_time) {
        None => (IdleMember { last_reporter }.into(), QueryReceivedActions::StopTimerAndSendReport),
        Some(max_resp_time) => {
            let max_resp_time = max_resp_time.get();
            let new_deadline = now.saturating_add(max_resp_time);

            let (timer_expiration, action) = match timer_expiration {
                Some(old) if new_deadline >= old => (old, QueryReceivedActions::None),
                None | Some(_) => {
                    let delay = gmp::random_report_timeout(rng, max_resp_time);
                    (now.saturating_add(delay), QueryReceivedActions::ScheduleTimer(delay))
                }
            };

            (DelayingMember { last_reporter, timer_expiration }.into(), action)
        }
    };

    (transition, actions)
}

impl NonMember {
    fn join_group<I: Instant, R: Rng, C: ProtocolConfig>(
        self,
        rng: &mut R,
        now: I,
        cfg: &C,
    ) -> Transition<DelayingMember<I>, JoinGroupActions> {
        let duration = cfg.unsolicited_report_interval();
        let delay = gmp::random_report_timeout(rng, duration);
        let actions = JoinGroupActions { send_report_and_schedule_timer: Some(delay) };
        Transition(
            DelayingMember {
                last_reporter: true,
                timer_expiration: now.checked_add(delay).expect("timer expiration overflowed"),
            },
            actions,
        )
    }

    fn leave_group(self) -> Transition<NonMember, LeaveGroupActions> {
        Transition(NonMember, LeaveGroupActions::NOOP)
    }
}

impl<I: Instant> DelayingMember<I> {
    fn query_received<R: Rng, C: ProtocolConfig>(
        self,
        rng: &mut R,
        max_resp_time: Duration,
        now: I,
        cfg: &C,
    ) -> (MemberState<I>, QueryReceivedActions) {
        let DelayingMember { last_reporter, timer_expiration } = self;
        member_query_received(rng, last_reporter, Some(timer_expiration), max_resp_time, now, cfg)
    }

    fn leave_group<C: ProtocolConfig>(self, cfg: &C) -> Transition<NonMember, LeaveGroupActions> {
        let actions = LeaveGroupActions {
            send_leave: self.last_reporter || cfg.send_leave_anyway(),
            stop_timer: true,
        };
        Transition(NonMember, actions)
    }

    fn report_received(self) -> Transition<IdleMember, ReportReceivedActions> {
        Transition(IdleMember { last_reporter: false }, ReportReceivedActions { stop_timer: true })
    }

    fn report_timer_expired(self) -> Transition<IdleMember, ReportTimerExpiredActions> {
        Transition(IdleMember { last_reporter: true }, ReportTimerExpiredActions)
    }
}

impl IdleMember {
    fn query_received<I: Instant, R: Rng, C: ProtocolConfig>(
        self,
        rng: &mut R,
        max_resp_time: Duration,
        now: I,
        cfg: &C,
    ) -> (MemberState<I>, QueryReceivedActions) {
        let IdleMember { last_reporter } = self;
        member_query_received(rng, last_reporter, None, max_resp_time, now, cfg)
    }

    fn leave_group<C: ProtocolConfig>(self, cfg: &C) -> Transition<NonMember, LeaveGroupActions> {
        let actions = LeaveGroupActions {
            send_leave: self.last_reporter || cfg.send_leave_anyway(),
            stop_timer: false,
        };
        Transition(NonMember, actions)
    }
}

impl<I: Instant> MemberState<I> {
    /// Performs the "join group" transition, producing a new `MemberState` and
    /// set of actions to execute.
    fn join_group<R: Rng, C: ProtocolConfig>(
        cfg: &C,
        rng: &mut R,
        now: I,
        gmp_disabled: bool,
    ) -> (MemberState<I>, JoinGroupActions) {
        let non_member = NonMember;
        if gmp_disabled {
            (non_member.into(), JoinGroupActions::NOOP)
        } else {
            non_member.join_group(rng, now, cfg).into_state_actions()
        }
    }

    /// Performs the "leave group" transition, consuming the state by value, and
    /// returning the next state and a set of actions to execute.
    fn leave_group<C: ProtocolConfig>(self, cfg: &C) -> (MemberState<I>, LeaveGroupActions) {
        // Rust can infer these types, but since we're just discarding `_state`,
        // we explicitly make sure it's the state we expect in case we introduce
        // a bug.
        match self {
            MemberState::NonMember(state) => state.leave_group(),
            MemberState::Delaying(state) => state.leave_group(cfg),
            MemberState::Idle(state) => state.leave_group(cfg),
        }
        .into_state_actions()
    }

    fn query_received<R: Rng, C: ProtocolConfig>(
        self,
        rng: &mut R,
        max_resp_time: Duration,
        now: I,
        cfg: &C,
    ) -> (MemberState<I>, QueryReceivedActions) {
        match self {
            state @ MemberState::NonMember(_) => (state, QueryReceivedActions::None),
            MemberState::Delaying(state) => state.query_received(rng, max_resp_time, now, cfg),
            MemberState::Idle(state) => state.query_received(rng, max_resp_time, now, cfg),
        }
    }

    fn report_received(self) -> (MemberState<I>, ReportReceivedActions) {
        match self {
            state @ MemberState::Idle(_) | state @ MemberState::NonMember(_) => {
                (state, ReportReceivedActions::NOOP)
            }
            MemberState::Delaying(state) => state.report_received().into_state_actions(),
        }
    }

    fn report_timer_expired(self) -> (MemberState<I>, ReportTimerExpiredActions) {
        match self {
            MemberState::Idle(_) | MemberState::NonMember(_) => {
                unreachable!("got report timer in non-delaying state")
            }
            MemberState::Delaying(state) => state.report_timer_expired().into_state_actions(),
        }
    }
}

#[cfg_attr(test, derive(Debug))]
pub struct GmpStateMachine<I: Instant> {
    // Invariant: `inner` is always `Some`. It is stored as an `Option` so that
    // methods can `.take()` the `MemberState` in order to perform transitions
    // that consume `MemberState` by value. However, a new `MemberState` is
    // always put back in its place so that `inner` is `Some` by the time the
    // methods return.
    pub(super) inner: Option<MemberState<I>>,
}

impl<I: Instant> GmpStateMachine<I> {
    /// When a "join group" command is received.
    ///
    /// `join_group` initializes a new state machine in the Non-Member state and
    /// then immediately executes the "join group" transition. The new state
    /// machine is returned along with any actions to take.
    pub(super) fn join_group<R: Rng, C: ProtocolConfig>(
        rng: &mut R,
        now: I,
        gmp_disabled: bool,
        cfg: &C,
    ) -> (GmpStateMachine<I>, JoinGroupActions) {
        let (state, actions) = MemberState::join_group(cfg, rng, now, gmp_disabled);
        (GmpStateMachine { inner: Some(state) }, actions)
    }

    /// Attempts to join the group if the group is currently in the non-member
    /// state.
    ///
    /// If the group is in a member state (delaying/idle), this method does
    /// nothing.
    fn join_if_non_member<R: Rng, C: ProtocolConfig>(
        &mut self,
        rng: &mut R,
        now: I,
        cfg: &C,
    ) -> JoinGroupActions {
        self.update(|s| match s {
            MemberState::NonMember(s) => s.join_group(rng, now, cfg).into_state_actions(),
            state @ MemberState::Delaying(_) | state @ MemberState::Idle(_) => {
                (state, JoinGroupActions::NOOP)
            }
        })
    }

    /// Leaves the group if the group is in a member state.
    ///
    /// Does nothing if the group is in a non-member state.
    fn leave_if_member<C: ProtocolConfig>(&mut self, cfg: &C) -> LeaveGroupActions {
        self.update(|s| s.leave_group(cfg))
    }

    /// When a "leave group" command is received.
    ///
    /// `leave_group` consumes the state machine by value since we don't allow
    /// storing a state machine in the Non-Member state.
    pub(super) fn leave_group<C: ProtocolConfig>(self, cfg: &C) -> LeaveGroupActions {
        // This `unwrap` is safe because we maintain the invariant that `inner`
        // is always `Some`.
        let (_state, actions) = self.inner.unwrap().leave_group(cfg);
        actions
    }

    /// When a query is received, and we have to respond within max_resp_time.
    pub(super) fn query_received<R: Rng, C: ProtocolConfig>(
        &mut self,
        rng: &mut R,
        max_resp_time: Duration,
        now: I,
        cfg: &C,
    ) -> QueryReceivedActions {
        self.update(|s| s.query_received(rng, max_resp_time, now, cfg))
    }

    /// We have received a report from another host on our local network.
    pub(super) fn report_received(&mut self) -> ReportReceivedActions {
        self.update(MemberState::report_received)
    }

    /// The timer installed has expired.
    pub(super) fn report_timer_expired(&mut self) -> ReportTimerExpiredActions {
        self.update(MemberState::report_timer_expired)
    }

    /// Update the state with no argument.
    fn update<A, F: FnOnce(MemberState<I>) -> (MemberState<I>, A)>(&mut self, f: F) -> A {
        let (s, a) = f(self.inner.take().unwrap());
        self.inner = Some(s);
        a
    }

    /// Returns a new state machine to use for a group after a transition from a
    /// different GMP mode.
    ///
    /// Neither the IGMPv3 or MLDv2 RFCs explicitly state what mode a member
    /// should be in as part of a state transition. Our best attempt here is to
    /// create members in Idle state, which will respond to queries but do not
    /// generate any unsolicited reports as the outcome of the transition. That
    /// roughly matches the expectation around the RFC text:
    ///
    /// > Whenever a host changes its compatibility mode, it cancels all its
    /// > pending responses and retransmission timers.
    pub(super) fn new_for_mode_transition() -> Self {
        Self { inner: Some(MemberState::Idle(IdleMember { last_reporter: false })) }
    }

    #[cfg(test)]
    pub(super) fn get_inner(&self) -> &MemberState<I> {
        self.inner.as_ref().unwrap()
    }
}

pub(super) fn handle_timer<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
    timer: DelayedReportTimerId<I>,
) where
    BC: GmpBindingsContext,
    CC: GmpContextInner<I, BC>,
    I: IpExt,
{
    let GmpStateRef { enabled: _, groups, gmp, config: _ } = state;
    debug_assert!(gmp.gmp_mode().is_v1());
    let DelayedReportTimerId(group_addr) = timer;
    let ReportTimerExpiredActions {} = groups
        .get_mut(group_addr.as_ref())
        .expect("get state for group with expired report timer")
        .v1_mut()
        .report_timer_expired();

    core_ctx.send_message_v1(bindings_ctx, &device, &gmp.mode, group_addr, GmpMessageType::Report);
}

pub(super) fn handle_report_message<I, BC, CC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    group_addr: MulticastAddr<I::Addr>,
) -> Result<(), NotAMemberErr<I>>
where
    BC: GmpBindingsContext,
    CC: GmpContext<I, BC>,
    I: IpExt,
{
    core_ctx.with_gmp_state_mut(device, |state| {
        let GmpStateRef { enabled: _, groups, gmp, config: _ } = state;
        // Ignore reports if we're not in v1 mode. We're acting as an
        // IGMPv3/MLDv2 host only. From RFCs:
        //
        //   RFC 3810 8.2.2: In the Presence of MLDv1 Multicast Address
        //   Listeners.
        //
        //     An MLDv2 host may be placed on a link where there are
        //     MLDv1 hosts. A host MAY allow its MLDv2 Multicast Listener Report
        //     to be suppressed by a Version 1 Multicast Listener Report.
        //
        //   RFC 3376 7.2.2: In the Presence of Older Version Group Members.
        //
        //     An IGMPv3 host may be placed on a network where there are hosts
        //     that have not yet been upgraded to IGMPv3.  A host MAY allow its
        //     IGMPv3 Membership Record to be suppressed by either a Version 1
        //     Membership Report, or a Version 2 Membership Report.
        if !gmp.gmp_mode().is_v1() {
            return Ok(());
        }
        let group_addr =
            GmpEnabledGroup::try_new(group_addr).map_err(|addr| NotAMemberErr(*addr))?;
        let ReportReceivedActions { stop_timer } = groups
            .get_mut(group_addr.as_ref())
            .ok_or_else(|| NotAMemberErr(*group_addr.multicast_addr()))
            .map(|a| a.v1_mut().report_received())?;
        if stop_timer {
            assert_matches!(
                gmp.timers.cancel(bindings_ctx, &DelayedReportTimerId(group_addr).into()),
                Some(_)
            );
        }
        Ok(())
    })
}

/// A trait abstracting GMP v1 query messages.
pub(super) trait QueryMessage<I: Ip> {
    /// Returns the group address in the query.
    fn group_addr(&self) -> I::Addr;

    /// Returns tha maximum response time for the query.
    fn max_response_time(&self) -> Duration;
}

pub(super) fn handle_query_message<I, CC, BC, Q>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    query: &Q,
) -> Result<(), NotAMemberErr<I>>
where
    BC: GmpBindingsContext,
    CC: GmpContext<I, BC>,
    I: IpExt,
    Q: QueryMessage<I>,
{
    core_ctx.with_gmp_state_mut_and_ctx(device, |mut core_ctx, mut state| {
        // If the protocol is disabled, we should have already turned every
        // group membership state to `NonMember`. This shortcut also allows us
        // to skip the compatibility timer, which we should not be tracking
        // while disabled - compatibility state is cleared and the proto mode
        // is reverted to v2 on disablement.
        if !state.enabled {
            return Err(NotAMemberErr(query.group_addr()));
        }
        // Allow protocol to decide entering v1 compatibility mode if we see a
        // v1 message and we're not in forced v1.
        //
        //   RFC 3810 8.2.1: The Host Compatibility Mode of an interface is set
        //   to MLDv1 whenever an MLDv1 Multicast Address Listener Query is
        //   received on that interface.
        //
        //   RFC 3376 7.2.1: In order to switch gracefully between versions of
        //   IGMP, hosts keep both an IGMPv1 Querier Present timer and an IGMPv2
        //   Querier Present timer per interface. IGMPv1 Querier Present is set
        //   to Older Version Querier Present Timeout seconds whenever an IGMPv1
        //   Membership Query is received.  IGMPv2 Querier Present is set to
        //   Older Version Querier Present Timeout seconds whenever an IGMPv2
        //   General Query is received.
        let new_mode =
            core_ctx.mode_update_from_v1_query(bindings_ctx, query, &state.gmp, &state.config);
        gmp::enter_mode(bindings_ctx, state.as_mut(), new_mode);
        let compat = match state.gmp.gmp_mode() {
            GmpMode::V1 { compat } => compat,
            GmpMode::V2 => {
                // NB: We don't currently support a configuration that refuses
                // entering compatibility mode, but that's allowed in RFC 3810
                // for MLDv2, we just don't have a use for it. If need be, this
                // panic can be replaced with a return to ignore the v1 query
                // instead.
                panic!("protocol refused to switch to v1");
            }
        };
        if compat {
            gmp::schedule_v1_compat(bindings_ctx, state.as_mut())
        }

        handle_query_message_inner(&mut core_ctx, bindings_ctx, device, state, query)
    })
}

pub(super) fn handle_query_message_inner<I, CC, BC, Q>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
    query: &Q,
) -> Result<(), NotAMemberErr<I>>
where
    BC: GmpBindingsContext,
    CC: GmpContextInner<I, BC>,
    I: IpExt,
    Q: QueryMessage<I>,
{
    let GmpStateRef { enabled: _, groups, gmp, config } = state;

    let now = bindings_ctx.now();

    let target = query.group_addr();
    let target = QueryTarget::new(target).ok_or(NotAMemberErr(target))?;
    let iter = match target {
        QueryTarget::Unspecified => either::Either::Left(
            groups
                .iter_mut()
                .filter_map(|(addr, state)| GmpEnabledGroup::new(*addr).map(|addr| (addr, state))),
        ),
        QueryTarget::Specified(group_addr) => {
            let group_addr =
                GmpEnabledGroup::try_new(group_addr).map_err(|addr| NotAMemberErr(*addr))?;
            let state = groups
                .get_mut(group_addr.as_ref())
                .ok_or_else(|| NotAMemberErr(*group_addr.into_multicast_addr()))?;
            either::Either::Right(core::iter::once((group_addr, state)))
        }
    };

    for (group_addr, state) in iter {
        let actions = state.v1_mut().query_received(
            &mut bindings_ctx.rng(),
            query.max_response_time(),
            now,
            config,
        );
        let send_msg = match actions {
            QueryReceivedActions::None => None,
            QueryReceivedActions::ScheduleTimer(delay) => {
                let _: Option<(BC::Instant, ())> = gmp.timers.schedule_after(
                    bindings_ctx,
                    DelayedReportTimerId(group_addr).into(),
                    (),
                    delay,
                );
                None
            }
            QueryReceivedActions::StopTimerAndSendReport => {
                let _: Option<(BC::Instant, ())> =
                    gmp.timers.cancel(bindings_ctx, &DelayedReportTimerId(group_addr).into());
                Some(GmpMessageType::Report)
            }
        };

        if let Some(msg) = send_msg {
            core_ctx.send_message_v1(bindings_ctx, device, &gmp.mode, group_addr, msg);
        }
    }

    Ok(())
}

pub(super) fn handle_enabled<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) where
    BC: GmpBindingsContext,
    CC: GmpContextInner<I, BC>,
    I: IpExt,
{
    let GmpStateRef { enabled: _, groups, gmp, config } = state;
    debug_assert!(gmp.gmp_mode().is_v1());

    let now = bindings_ctx.now();

    for (group_addr, state) in groups.iter_mut() {
        let group_addr = match GmpEnabledGroup::new(*group_addr) {
            Some(a) => a,
            None => continue,
        };

        let JoinGroupActions { send_report_and_schedule_timer } =
            state.v1_mut().join_if_non_member(&mut bindings_ctx.rng(), now, config);
        let Some(delay) = send_report_and_schedule_timer else {
            continue;
        };
        assert_matches!(
            gmp.timers.schedule_after(
                bindings_ctx,
                DelayedReportTimerId(group_addr).into(),
                (),
                delay
            ),
            None
        );
        core_ctx.send_message_v1(
            bindings_ctx,
            device,
            &gmp.mode,
            group_addr,
            GmpMessageType::Report,
        );
    }
}

pub(super) fn handle_disabled<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) where
    BC: GmpBindingsContext,
    CC: GmpContextInner<I, BC>,
    I: IpExt,
{
    let GmpStateRef { enabled: _, groups, gmp, config } = state;
    debug_assert!(gmp.gmp_mode().is_v1());

    for (group_addr, state) in groups.groups_mut() {
        let group_addr = match GmpEnabledGroup::new(*group_addr) {
            Some(a) => a,
            None => continue,
        };
        let LeaveGroupActions { send_leave, stop_timer } = state.v1_mut().leave_if_member(config);
        if stop_timer {
            assert_matches!(
                gmp.timers.cancel(bindings_ctx, &DelayedReportTimerId(group_addr).into()),
                Some(_)
            );
        }
        if send_leave {
            core_ctx.send_message_v1(
                bindings_ctx,
                device,
                &gmp.mode,
                group_addr,
                GmpMessageType::Leave,
            );
        }
    }
}

pub(super) fn join_group<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    group_addr: MulticastAddr<I::Addr>,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) -> GroupJoinResult
where
    BC: GmpBindingsContext,
    CC: GmpContextInner<I, BC>,
    I: IpExt,
{
    let GmpStateRef { enabled, groups, gmp, config } = state;
    debug_assert!(gmp.gmp_mode().is_v1());
    let now = bindings_ctx.now();

    let group_addr_witness = GmpEnabledGroup::try_new(group_addr);
    let gmp_disabled = !enabled || group_addr_witness.is_err();
    let result = groups.join_group_with(group_addr, || {
        let (state, actions) =
            GmpStateMachine::join_group(&mut bindings_ctx.rng(), now, gmp_disabled, config);
        (GmpGroupState::new_v1(state), actions)
    });
    result.map(|JoinGroupActions { send_report_and_schedule_timer }| {
        if let Some(delay) = send_report_and_schedule_timer {
            // Invariant: if gmp_disabled then a non-member group must not
            // generate any actions.
            let group_addr = group_addr_witness.expect("generated report for GMP-disabled group");
            assert_matches!(
                gmp.timers.schedule_after(
                    bindings_ctx,
                    DelayedReportTimerId(group_addr).into(),
                    (),
                    delay
                ),
                None
            );

            core_ctx.send_message_v1(
                bindings_ctx,
                device,
                &gmp.mode,
                group_addr,
                GmpMessageType::Report,
            );
        }
    })
}

pub(super) fn leave_group<I, CC, BC>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    group_addr: MulticastAddr<I::Addr>,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) -> GroupLeaveResult
where
    BC: GmpBindingsContext,
    CC: GmpContextInner<I, BC>,
    I: IpExt,
{
    let GmpStateRef { enabled: _, groups, gmp, config } = state;
    debug_assert!(gmp.gmp_mode().is_v1());

    groups.leave_group(group_addr).map(|state| {
        let actions = state.into_v1().leave_group(config);
        let group_addr = match GmpEnabledGroup::new(group_addr) {
            Some(addr) => addr,
            None => {
                // Invariant: No actions must be generated for non member
                // groups.
                assert_eq!(actions, LeaveGroupActions::NOOP);
                return;
            }
        };
        let LeaveGroupActions { send_leave, stop_timer } = actions;
        if stop_timer {
            assert_matches!(
                gmp.timers.cancel(bindings_ctx, &DelayedReportTimerId(group_addr).into()),
                Some(_)
            );
        }
        if send_leave {
            core_ctx.send_message_v1(
                bindings_ctx,
                device,
                &gmp.mode,
                group_addr,
                GmpMessageType::Leave,
            );
        }
    })
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use netstack3_base::testutil::{FakeDeviceId, FakeInstant, new_rng};
    use test_util::assert_lt;

    use super::*;

    const DEFAULT_UNSOLICITED_REPORT_INTERVAL: Duration = Duration::from_secs(10);

    /// Whether to send leave group message if our flag is not set.
    #[derive(Debug, Default)]
    struct FakeConfig(bool);

    impl ProtocolConfig for FakeConfig {
        fn unsolicited_report_interval(&self) -> Duration {
            DEFAULT_UNSOLICITED_REPORT_INTERVAL
        }

        fn send_leave_anyway(&self) -> bool {
            let Self(cfg) = self;
            *cfg
        }

        fn get_max_resp_time(&self, resp_time: Duration) -> Option<NonZeroDuration> {
            NonZeroDuration::new(resp_time)
        }
    }

    type FakeGmpStateMachine = GmpStateMachine<FakeInstant>;

    #[test]
    fn test_gmp_state_non_member_to_delay_should_set_flag() {
        let (s, _actions) = FakeGmpStateMachine::join_group(
            &mut new_rng(0),
            FakeInstant::default(),
            false,
            &FakeConfig::default(),
        );
        match s.get_inner() {
            MemberState::Delaying(s) => assert!(s.last_reporter),
            _ => panic!("Wrong State!"),
        }
    }

    #[test]
    fn test_gmp_state_non_member_to_delay_actions() {
        let (_state, actions) = FakeGmpStateMachine::join_group(
            &mut new_rng(0),
            FakeInstant::default(),
            false,
            &FakeConfig::default(),
        );
        assert_matches!(
            actions,
            JoinGroupActions { send_report_and_schedule_timer: Some(d) } if d <= DEFAULT_UNSOLICITED_REPORT_INTERVAL
        );
    }

    #[test]
    fn test_gmp_state_delay_no_reset_timer() {
        let mut rng = new_rng(0);
        let cfg = FakeConfig::default();
        let (mut s, _actions) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        assert_eq!(
            s.query_received(
                &mut rng,
                DEFAULT_UNSOLICITED_REPORT_INTERVAL + Duration::from_secs(1),
                FakeInstant::default(),
                &cfg
            ),
            QueryReceivedActions::None,
        );
    }

    #[test]
    fn test_gmp_state_delay_reset_timer() {
        let mut rng = new_rng(10);
        let cfg = FakeConfig::default();
        let (mut s, JoinGroupActions { send_report_and_schedule_timer }) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        let first_duration = send_report_and_schedule_timer.expect("starts delaying member");
        let actions = s.query_received(
            &mut rng,
            first_duration.checked_sub(Duration::from_micros(1)).unwrap(),
            FakeInstant::default(),
            &cfg,
        );
        let new_duration = assert_matches!(actions,
            QueryReceivedActions::ScheduleTimer(d) => d
        );
        assert_lt!(new_duration, first_duration);
    }

    #[test]
    fn test_gmp_state_delay_to_idle_with_report_no_flag() {
        let (mut s, _actions) = FakeGmpStateMachine::join_group(
            &mut new_rng(0),
            FakeInstant::default(),
            false,
            &FakeConfig::default(),
        );
        assert_eq!(s.report_received(), ReportReceivedActions { stop_timer: true });
        match s.get_inner() {
            MemberState::Idle(s) => {
                assert!(!s.last_reporter);
            }
            _ => panic!("Wrong State!"),
        }
    }

    #[test]
    fn test_gmp_state_delay_to_idle_without_report_set_flag() {
        let (mut s, _actions) = FakeGmpStateMachine::join_group(
            &mut new_rng(0),
            FakeInstant::default(),
            false,
            &FakeConfig::default(),
        );
        assert_eq!(s.report_timer_expired(), ReportTimerExpiredActions,);
        match s.get_inner() {
            MemberState::Idle(s) => {
                assert!(s.last_reporter);
            }
            _ => panic!("Wrong State!"),
        }
    }

    #[test]
    fn test_gmp_state_leave_should_send_leave() {
        let mut rng = new_rng(0);
        let cfg = FakeConfig::default();
        let (s, _actions) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        assert_eq!(s.leave_group(&cfg), LeaveGroupActions { send_leave: true, stop_timer: true });
        let (mut s, _actions) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        assert_eq!(s.report_timer_expired(), ReportTimerExpiredActions,);
        assert_eq!(s.leave_group(&cfg), LeaveGroupActions { send_leave: true, stop_timer: false });
    }

    #[test]
    fn test_gmp_state_delay_to_other_states_should_stop_timer() {
        let mut rng = new_rng(0);
        let cfg = FakeConfig::default();
        let (s, _actions) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        assert_eq!(s.leave_group(&cfg), LeaveGroupActions { send_leave: true, stop_timer: true },);
        let (mut s, _actions) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        assert_eq!(s.report_received(), ReportReceivedActions { stop_timer: true });
    }

    #[test]
    fn test_gmp_state_other_states_to_delay_should_schedule_timer() {
        let mut rng = new_rng(0);
        let cfg = FakeConfig::default();
        let (mut s, actions) =
            FakeGmpStateMachine::join_group(&mut rng, FakeInstant::default(), false, &cfg);
        assert_matches!(
            actions,
            JoinGroupActions { send_report_and_schedule_timer: Some(d) } if d <= DEFAULT_UNSOLICITED_REPORT_INTERVAL
        );
        assert_eq!(s.report_received(), ReportReceivedActions { stop_timer: true });
        assert_eq!(
            s.query_received(&mut rng, Duration::from_secs(1), FakeInstant::default(), &cfg),
            QueryReceivedActions::ScheduleTimer(Duration::from_micros(1))
        );
    }

    #[test]
    fn test_gmp_state_leave_send_anyway_do_send() {
        let mut cfg = FakeConfig::default();
        let (mut s, _actions) =
            FakeGmpStateMachine::join_group(&mut new_rng(0), FakeInstant::default(), false, &cfg);
        cfg = FakeConfig(true);
        assert_eq!(s.report_received(), ReportReceivedActions { stop_timer: true });
        match s.get_inner() {
            MemberState::Idle(s) => assert!(!s.last_reporter),
            _ => panic!("Wrong State!"),
        }
        assert_eq!(s.leave_group(&cfg), LeaveGroupActions { send_leave: true, stop_timer: false });
    }

    #[test]
    fn test_gmp_state_leave_not_the_last_do_nothing() {
        let cfg = FakeConfig::default();
        let (mut s, _actions) =
            FakeGmpStateMachine::join_group(&mut new_rng(0), FakeInstant::default(), false, &cfg);
        assert_eq!(s.report_received(), ReportReceivedActions { stop_timer: true });
        assert_eq!(s.leave_group(&cfg), LeaveGroupActions { send_leave: false, stop_timer: false })
    }

    #[ip_test(I)]
    fn ignores_reports_if_v2<I: gmp::testutil::TestIpExt>() {
        let gmp::testutil::FakeCtx { mut core_ctx, mut bindings_ctx } =
            gmp::testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            handle_report_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            // Report is ignored, we don't even check that we've joined that
            // group.
            Ok(())
        );
        // No timers are installed.
        core_ctx.gmp.timers.assert_timers([]);
        // Mode doesn't change.
        assert_eq!(core_ctx.gmp.mode, GmpMode::V2);
    }
}
