// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! GMP v2 common implementation.
//!
//! GMPv2 is the common implementation of a fictitious GMP protocol that covers
//! the common parts of MLDv2 ([RFC 3810]) and IGMPv3 ([RFC 3376]).
//!
//! [RFC 3810]: https://datatracker.ietf.org/doc/html/rfc3810
//! [RFC 3376]: https://datatracker.ietf.org/doc/html/rfc3376

use core::num::NonZeroU8;
use core::time::Duration;

use net_types::ip::{Ip, IpAddress};
use net_types::{MulticastAddr, Witness as _};
use netstack3_base::{Instant as _, LocalTimerHeap};
use netstack3_hashmap::{HashMap, HashSet};
use packet_formats::gmp::{GmpReportGroupRecord, GroupRecordType};
use packet_formats::utils::NonZeroDuration;

use crate::internal::gmp::{
    self, GmpBindingsContext, GmpContext, GmpContextInner, GmpEnabledGroup, GmpGroupState, GmpMode,
    GmpStateRef, GmpTypeLayout, GroupJoinResult, GroupLeaveResult, IpExt, NotAMemberErr,
    QueryTarget,
};

/// The default value for Query Response Interval defined in [RFC 3810
/// section 9.3] and [RFC 3376 section 8.3].
///
/// [RFC 3810 section 9.3]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-9.3
/// [RFC 3376 section 8.3]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-8.3
pub(super) const DEFAULT_QUERY_RESPONSE_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(10).unwrap();

/// The default value for Unsolicited Report Interval defined in [RFC 3810
/// section 9.11] and [RFC 3376 section 8.11].
///
/// [RFC 3810 section 9.11]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-9.3
/// [RFC 3376 section 8.11]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-8.3
pub(super) const DEFAULT_UNSOLICITED_REPORT_INTERVAL: NonZeroDuration =
    NonZeroDuration::from_secs(1).unwrap();

/// The default value for the Robustness Variable defined in [RFC 3810
/// section 9.1] and [RFC 3376 section 8.1].
///
/// [RFC 3810 section 9.1]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-9.1
/// [RFC 3376 section 8.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-8.1
pub(super) const DEFAULT_ROBUSTNESS_VARIABLE: NonZeroU8 = NonZeroU8::new(2).unwrap();

/// The default value for the Query Interval defined in [RFC 3810
/// section 9.2] and [RFC 3376 section 8.2].
///
/// [RFC 3810 section 9.2]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-9.2
/// [RFC 3376 section 8.2]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-8.2
pub(super) const DEFAULT_QUERY_INTERVAL: NonZeroDuration = NonZeroDuration::from_secs(125).unwrap();

/// The maximum number of sources to record for a pending response to a
/// Multicast Address and Source Specific Query.
///
/// As per [RFC 3810 section 10.1], and [RFC 3376 section 9.1]:
///   To protect against such a DoS attack, a node stack implementation
///   could restrict the number of Multicast Address and Source Specific
///   Queries per multicast address within this interval, and/or record
///   only a limited number of sources.
///
/// [RFC 3810 section 10.1]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-10.1
/// [RFC 3376 section 9.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-9.1
pub(super) const MAX_RECORDED_SOURCES: usize = 64;

/// A delay to use before issuing state change reports in response to interface
/// state changes (e.g leaving/joining groups).
///
/// Note that this delay does not exist on any of the related RFCs. The RFCs
/// state that state change reports should be sent immediately when the state
/// change occurs, the delay here is chosen to be small enough that it can be
/// seen as immediate when looking at the network.
///
/// This delay introduces some advantages compared to a to-the-letter RFC
/// implementation:
///
/// - It gives the system some time to consolidate State Change Reports into one
///   in the case of quick successive changes.
/// - Quick successive changes on different multicast groups do not quickly
///   consume the retransmission counters of still pending changes to different
///   groups.
/// - State Change Reports are always sent from the same place in the code: when
///   [`TimerId::StateChange`] timers fire.
///
/// [An equivalent delay is in use on linux][linux-mld].
///
/// [linux-mld]: https://github.com/torvalds/linux/blob/62b5a46999c74497fe10eabd7d19701c505b23e3/net/ipv6/mcast.c#L2670
const STATE_CHANGE_REPORT_DELAY: Duration = Duration::from_millis(5);

#[cfg_attr(test, derive(Debug))]
pub(super) struct GroupState<I: Ip> {
    filter_mode_retransmission_counter: u8,
    recorded_sources: HashSet<I::Addr>,
    // TODO(https://fxbug.dev/381241191): Include per-source retransmission
    // counter when SSM is supported.
}

impl<I: Ip> GroupState<I> {
    pub(super) fn new_for_mode_transition() -> Self {
        Self { recorded_sources: Default::default(), filter_mode_retransmission_counter: 0 }
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub(super) enum TimerId<I: Ip> {
    GeneralQuery,
    MulticastAddress(GmpEnabledGroup<I::Addr>),
    StateChange,
}

/// Global protocol state required for v2 support.
///
/// This is kept always available in protocol-global state since we need to
/// store some possibly network-learned values when entering v1 compat mode (for
/// timers).
#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub(super) struct ProtocolState<I: Ip> {
    /// The robustness variable on the link.
    ///
    /// Defined in [RFC 3810 section 9.1] and [RFC 3376 section 8.1].
    ///
    /// It starts with a default value and may be learned from queriers in the
    /// network.
    ///
    /// [RFC 3810 section 9.1]: https://datatracker.ietf.org/doc/html/rfc3810#section-9.1
    /// [RFC 3376 section 8.1]: https://datatracker.ietf.org/doc/html/rfc3376#section-8.1
    pub robustness_variable: NonZeroU8,
    /// The query interval on the link.
    ///
    /// Defined in [RFC 3810 section 9.2] and [RFC 3376 section 8.2].
    ///
    /// It starts with a default value and may be learned from queriers in the
    /// network.
    ///
    /// [RFC 3810 section 9.2]: https://datatracker.ietf.org/doc/html/rfc3810#section-9.2
    /// [RFC 3376 section 8.2]: https://datatracker.ietf.org/doc/html/rfc3376#section-8.2
    pub query_interval: NonZeroDuration,

    /// GMPv2-only state tracking pending group exit retransmissions.
    ///
    /// This is kept apart from the per-interface multicast group state so we
    /// can keep minimal state on left groups and have an easier statement of
    /// what groups we're part of.
    // TODO(https://fxbug.dev/381241191): Reconsider this field when we
    // introduce SSM. The group membership state-tracking is expected to change
    // and it might become easier to keep left groups alongside still-member
    // groups.
    pub left_groups: HashMap<GmpEnabledGroup<I::Addr>, NonZeroU8>,
}

impl<I: Ip> Default for ProtocolState<I> {
    fn default() -> Self {
        Self {
            robustness_variable: DEFAULT_ROBUSTNESS_VARIABLE,
            query_interval: DEFAULT_QUERY_INTERVAL,
            left_groups: Default::default(),
        }
    }
}

impl<I: Ip> ProtocolState<I> {
    /// Calculates the Older Version Querier Present Timeout.
    ///
    /// From [RFC 3810 section 9.12] and [RFC 3376 section 8.12]:
    ///
    /// > This value MUST be ([Robustness Variable] times (the [Query Interval]
    /// > in the last Query received)) plus ([Query Response Interval]).
    ///
    /// [RFC 3810 section 9.12]: https://datatracker.ietf.org/doc/html/rfc3810#section-9.12
    /// [RFC 3376 section 8.12]: https://datatracker.ietf.org/doc/html/rfc3376#section-8.12
    pub(super) fn older_version_querier_present_timeout<C: ProtocolConfig>(
        &self,
        config: &C,
    ) -> NonZeroDuration {
        self.query_interval
            .saturating_mul(self.robustness_variable.into())
            .saturating_add(config.query_response_interval().into())
    }

    /// Updates [`ProtocolState`] due to a GMP mode change out of v2 mode.
    ///
    /// `ProtocolState` discards any protocol-specific state but *maintains*
    /// network-learned parameters on mode changes.
    pub(super) fn on_enter_v1(&mut self) {
        let Self { robustness_variable: _, query_interval: _, left_groups } = self;
        // left_groups are effectively pending responses and, from RFC 3810
        // section 8.2.1:
        //
        // Whenever a host changes its compatibility mode, it cancels all its
        // pending responses and retransmission timers.
        *left_groups = HashMap::new();
    }
}

/// V2 protocol-specific configuration.
///
/// This trait abstracts over the storage of configurations specified in [RFC
/// 3810] and [RFC 3376] that can be administratively changed.
///
/// [RFC 3810]: https://datatracker.ietf.org/doc/html/rfc3810
/// [RFC 3376]: https://datatracker.ietf.org/doc/html/rfc3376
pub trait ProtocolConfig {
    /// The Query Response Interval defined in [RFC 3810 section 9.3] and [RFC
    /// 3376 section 8.3].
    ///
    /// Note that the RFCs mostly define this value in terms of the maximum
    /// response code sent by queriers (routers), but later text references this
    /// configuration to calculate timeouts.
    ///
    /// [RFC 3810 section 9.3]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-9.3
    /// [RFC 3376 section 8.3]:
    ///     https://datatracker.ietf.org/doc/html/rfc3376#section-8.3
    fn query_response_interval(&self) -> NonZeroDuration;

    /// The Unsolicited Report Interval defined in [RFC 3810 section 9.11] and
    /// [RFC 3376 section 8.11].
    ///
    /// [RFC 3810 section 9.11]:
    ///     https://datatracker.ietf.org/doc/html/rfc3810#section-9.11
    /// [RFC 3376 section 8.11]:
    ///     https://datatracker.ietf.org/doc/html/rfc3376#section-8.11
    fn unsolicited_report_interval(&self) -> NonZeroDuration;
}

/// Trait abstracting a GMPv2 query.
///
/// The getters in this trait represent fields in the membership query messages
/// defined in [RFC 3376 section 4.1] and [RFC 3810 section 5.1].
///
/// [RFC 3376 section 4.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-4.1
/// [RFC 3810 section 5.1]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-5.1
pub(super) trait QueryMessage<I: Ip> {
    /// Reinterprets this as a v1 query message.
    fn as_v1(&self) -> impl gmp::v1::QueryMessage<I> + '_;

    /// Gets the Querier's Robustness Variable (QRV).
    fn robustness_variable(&self) -> u8;

    /// Gets the Querier's Query Interval Code (QQIC) interpreted as a duration.
    fn query_interval(&self) -> Duration;

    /// Gets the group address.
    fn group_address(&self) -> I::Addr;

    /// Gets the maximum response time.
    fn max_response_time(&self) -> Duration;

    /// Gets an iterator to the source addresses being queried.
    fn sources(&self) -> impl Iterator<Item = I::Addr> + '_;
}

#[derive(Eq, PartialEq, Debug)]
pub(super) enum QueryError<I: Ip> {
    NotAMember(I::Addr),
    Disabled,
}

impl<I: Ip> From<NotAMemberErr<I>> for QueryError<I> {
    fn from(NotAMemberErr(addr): NotAMemberErr<I>) -> Self {
        Self::NotAMember(addr)
    }
}

/// An enhancement to [`GmpReportGroupRecord`] that guarantees the yielded group
/// address is [`GmpEnabledGroup`].
pub(super) trait VerifiedReportGroupRecord<A: IpAddress>: GmpReportGroupRecord<A> {
    // NB: We don't have any use for this method. It exists as a statement that
    // the type implementing it holds a reference to GmpEnabledGroup.
    #[expect(unused)]
    fn gmp_enabled_group_addr(&self) -> &GmpEnabledGroup<A>;
}

#[derive(Clone)]
pub(super) struct GroupRecord<A, Iter> {
    group: GmpEnabledGroup<A>,
    record_type: GroupRecordType,
    iter: Iter,
}

impl<A> GroupRecord<A, core::iter::Empty<A>> {
    pub(super) fn new(group: GmpEnabledGroup<A>, record_type: GroupRecordType) -> Self {
        Self { group, record_type, iter: core::iter::empty() }
    }
}

impl<A, Iter> GroupRecord<A, Iter> {
    pub(super) fn new_with_sources(
        group: GmpEnabledGroup<A>,
        record_type: GroupRecordType,
        iter: Iter,
    ) -> Self {
        Self { group, record_type, iter }
    }
}

impl<A: IpAddress<Version: IpExt>, Iter: Iterator<Item: core::borrow::Borrow<A>> + Clone>
    GmpReportGroupRecord<A> for GroupRecord<A, Iter>
{
    fn group(&self) -> MulticastAddr<A> {
        self.group.multicast_addr()
    }

    fn record_type(&self) -> GroupRecordType {
        self.record_type
    }

    fn sources(&self) -> impl Iterator<Item: core::borrow::Borrow<A>> + '_ {
        self.iter.clone()
    }
}

impl<A: IpAddress<Version: IpExt>, Iter: Iterator<Item: core::borrow::Borrow<A>> + Clone>
    VerifiedReportGroupRecord<A> for GroupRecord<A, Iter>
{
    fn gmp_enabled_group_addr(&self) -> &GmpEnabledGroup<A> {
        &self.group
    }
}

/// Handles a query message from the network.
///
/// The RFC algorithm is specified on [RFC 3376 section 5.2] and [RFC 3810
/// section 6.2].
///
/// [RFC 3376 section 5.2]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-5.2
/// [RFC 3810 section 6.2]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-6.2
pub(super) fn handle_query_message<
    I: IpExt,
    CC: GmpContext<I, BC>,
    BC: GmpBindingsContext,
    Q: QueryMessage<I>,
>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    query: &Q,
) -> Result<(), QueryError<I>> {
    core_ctx.with_gmp_state_mut_and_ctx(device, |mut core_ctx, state| {
        // Ignore queries if we're not in enabled state.
        if !state.enabled {
            return Err(QueryError::Disabled);
        }
        match state.gmp.gmp_mode() {
            GmpMode::V1 { .. } => {
                return gmp::v1::handle_query_message_inner(
                    &mut core_ctx,
                    bindings_ctx,
                    device,
                    state,
                    &query.as_v1(),
                )
                .map_err(Into::into);
            }
            GmpMode::V2 => {}
        }
        let GmpStateRef { enabled: _, groups, gmp, config: _ } = state;
        // Update parameters if non zero given in query.
        if let Some(qrv) = NonZeroU8::new(query.robustness_variable()) {
            gmp.v2_proto.robustness_variable = qrv;
        }
        if let Some(qqic) = NonZeroDuration::new(query.query_interval()) {
            gmp.v2_proto.query_interval = qqic;
        }

        let target = query.group_address();
        let target = QueryTarget::new(target).ok_or(QueryError::NotAMember(target))?;

        // Common early bailout.
        let target = match target {
            // General query.
            QueryTarget::Unspecified => {
                // RFC: When a new valid General Query arrives on an interface,
                // the node checks whether it has any per-interface listening
                // state record to report on, or not.
                if groups.is_empty() {
                    return Ok(());
                }

                // None target from now on marks a general query.
                None
            }
            // Group-Specific or Group-And-Source-Specific query.
            QueryTarget::Specified(multicast_addr) => {
                // RFC: Similarly, when a new valid Multicast Address (and
                // Source) Specific Query arrives on an interface, the node
                // checks whether it has a per-interface listening state record
                // that corresponds to the queried multicast address (and
                // source), or not.

                // TODO(https://fxbug.dev/381241191): We should also consider
                // source lists here when we support SSM.

                let group = groups
                    .get_mut(&multicast_addr)
                    .ok_or_else(|| QueryError::NotAMember(multicast_addr.get()))?;

                // `Some` target marks a specific query.
                Some((group.v2_mut(), multicast_addr))
            }
        };

        // RFC: If it does, a delay for a response is randomly selected
        // in the range (0, [Maximum Response Delay]).
        let now = bindings_ctx.now();
        let delay = now.saturating_add(gmp::random_report_timeout(
            &mut bindings_ctx.rng(),
            query.max_response_time(),
        ));

        // RFC: If there is a pending response to a previous General Query
        // scheduled sooner than the selected delay, no additional response
        // needs to be scheduled.
        match gmp.timers.get(&TimerId::GeneralQuery.into()) {
            Some((instant, ())) => {
                if instant <= delay {
                    return Ok(());
                }
            }
            None => {}
        }

        let (group, addr) = match target {
            // RFC: If the received Query is a General Query, the Interface
            // Timer is used to schedule a response to the General Query after
            // the selected delay.  Any previously pending response to a General
            // Query is canceled.
            None => {
                let _: Option<_> = gmp.timers.schedule_instant(
                    bindings_ctx,
                    TimerId::GeneralQuery.into(),
                    (),
                    delay,
                );
                return Ok(());
            }
            Some(specific) => specific,
        };

        // The RFC quote for the next part is a bit long-winded but the
        // algorithm is simple. Full quote:
        //
        //  If the received Query is a Multicast Address Specific Query or a
        //  Multicast Address and Source Specific Query and there is no pending
        //  response to a previous Query for this multicast address, then the
        //  Multicast Address Timer is used to schedule a report.  If the
        //  received Query is a Multicast Address and Source Specific Query, the
        //  list of queried sources is recorded to be used when generating a
        //  response.
        //
        //  If there is already a pending response to a previous Query scheduled
        //  for this multicast address, and either the new Query is a Multicast
        //  Address Specific Query or the recorded source list associated with
        //  the multicast address is empty, then the multicast address source
        //  list is cleared and a single response is scheduled, using the
        //  Multicast Address Timer.  The new response is scheduled to be sent
        //  at the earliest of the remaining time for the pending report and the
        //  selected delay.
        //
        //  If the received Query is a Multicast Address and Source Specific
        //  Query and there is a pending response for this multicast address
        //  with a non-empty source list, then the multicast address source list
        //  is augmented to contain the list of sources in the new Query, and a
        //  single response is scheduled using the Multicast Address Timer.  The
        //  new response is scheduled to be sent at the earliest of the
        //  remaining time for the pending report and the selected delay.

        // Ignore any queries to non GMP-enabled groups.
        let addr = GmpEnabledGroup::try_new(addr)
            .map_err(|addr| QueryError::NotAMember(addr.into_addr()))?;

        let timer_id = TimerId::MulticastAddress(addr).into();
        let scheduled = gmp.timers.get(&timer_id);
        let mut sources = query.sources().peekable();

        let (delay, clear_sources) = match scheduled {
            // There is a scheduled report.
            Some((t, ())) => {
                // Only reschedule the timer if scheduling for earlier.
                let delay = (delay < t).then_some(delay);
                // Per the second paragraph above, clear sources if address
                // query or if the pending report is already for an empty source
                // list (meaning we don't restrict the old report to the new
                // sources).
                let is_address_query = sources.peek().is_none();
                let clear_sources = group.recorded_sources.is_empty() || is_address_query;
                (delay, clear_sources)
            }
            // No scheduled report, use new delay and record sources.
            None => (Some(delay), false),
        };

        if clear_sources {
            group.recorded_sources = Default::default();
        } else {
            for src in sources {
                if group.recorded_sources.len() >= MAX_RECORDED_SOURCES {
                    break;
                }
                let _: bool = group.recorded_sources.insert(src);
            }
        }

        if let Some(delay) = delay {
            let _: Option<_> = gmp.timers.schedule_instant(bindings_ctx, timer_id, (), delay);
        }

        Ok(())
    })
}

/// Joins `group_addr`.
///
/// This is called whenever a socket joins a group, network actions are only
/// taken when the action actually results in a newly joined group, otherwise
/// the group's reference counter is simply updated.
///
/// The reference for changing interface state is in [RFC 3376 section 5.1] and
/// [RFC 3810 section 6.1].
///
/// [RFC 3376 section 5.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-5.1
/// [RFC 3810 section 6.1]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-6.1
pub(super) fn join_group<I: IpExt, CC: GmpTypeLayout<I, BC>, BC: GmpBindingsContext>(
    bindings_ctx: &mut BC,
    group_addr: MulticastAddr<I::Addr>,
    state: GmpStateRef<'_, I, CC, BC>,
) -> GroupJoinResult {
    let GmpStateRef { enabled, groups, gmp, config: _ } = state;
    debug_assert!(gmp.gmp_mode().is_v2());
    groups.join_group_with(group_addr, || {
        let filter_mode_retransmission_counter = match GmpEnabledGroup::new(group_addr) {
            Some(group_addr) => {
                // We've just joined a group, remove anything any pending state from the
                // left groups.
                let _: Option<_> = gmp.v2_proto.left_groups.remove(&group_addr);

                if enabled {
                    trigger_state_change_report(bindings_ctx, &mut gmp.timers);
                    gmp.v2_proto.robustness_variable.get()
                } else {
                    0
                }
            }
            None => 0,
        };

        let state =
            GroupState { recorded_sources: Default::default(), filter_mode_retransmission_counter };

        (GmpGroupState::new_v2(state), ())
    })
}

/// Leaves `group_addr`.
///
/// This is called whenever a socket leaves a group, network actions are only
/// taken when the action actually results in a newly left group, otherwise the
/// group's reference counter is simply updated.
///
/// The reference for changing interface state is in [RFC 3376 section 5.1] and
/// [RFC 3810 section 6.1].
///
/// [RFC 3376 section 5.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-5.1
/// [RFC 3810 section
///     6.1]:https://datatracker.ietf.org/doc/html/rfc3810#section-6.1
pub(super) fn leave_group<I: IpExt, CC: GmpTypeLayout<I, BC>, BC: GmpBindingsContext>(
    bindings_ctx: &mut BC,
    group_addr: MulticastAddr<I::Addr>,
    state: GmpStateRef<'_, I, CC, BC>,
) -> GroupLeaveResult {
    let GmpStateRef { enabled, groups, gmp, config: _ } = state;
    debug_assert!(gmp.gmp_mode().is_v2());
    groups.leave_group(group_addr).map(|state| {
        let group_addr = if let Some(a) = GmpEnabledGroup::new(group_addr) { a } else { return };

        // Cancel existing query timers since we've left the group.
        let _: Option<_> =
            gmp.timers.cancel(bindings_ctx, &TimerId::MulticastAddress(group_addr).into());

        // Nothing to do with old state since we're resetting the retransmission
        // counter.
        let GroupState { filter_mode_retransmission_counter: _, recorded_sources: _ } =
            state.into_v2();

        if !enabled {
            return;
        }
        assert_eq!(
            gmp.v2_proto.left_groups.insert(group_addr, gmp.v2_proto.robustness_variable),
            None
        );
        trigger_state_change_report(bindings_ctx, &mut gmp.timers);
    })
}

/// Schedules a state change report to be sent in response to an interface state
/// change.
///
/// Schedule the State Change timer if it's not scheduled already or if it's
/// scheduled to fire later than the [`STATE_CHANGE_REPORT_DELAY`] in the
/// future. This guarantees that the report will go out at most
/// [`STATE_CHANGE_REPORT_DELAY`] in the future, which should be seen as
/// "immediate". See documentation on [`STATE_CHANGE_REPORT_DELAY`] for details.
fn trigger_state_change_report<I: IpExt, BC: GmpBindingsContext>(
    bindings_ctx: &mut BC,
    timers: &mut LocalTimerHeap<gmp::TimerIdInner<I>, (), BC>,
) {
    let now = bindings_ctx.now();
    let timer_id = TimerId::StateChange.into();
    let schedule_timer = timers.get(&timer_id).is_none_or(|(scheduled, ())| {
        scheduled.saturating_duration_since(now) > STATE_CHANGE_REPORT_DELAY
    });
    if schedule_timer {
        let _: Option<_> =
            timers.schedule_after(bindings_ctx, timer_id, (), STATE_CHANGE_REPORT_DELAY);
    }
}

/// Handles an expire timer.
///
/// The timer expiration algorithm is described in [RFC 3376 section 5.1] and
/// [RFC 3376 section 5.2] for IGMP and [RFC 3810 section 6.3] for MLD.
///
/// [RFC 3376 section 5.1]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-5.1
/// [RFC 3376 section 5.2]:
///     https://datatracker.ietf.org/doc/html/rfc3376#section-5.2
/// [RFC 3810 section 6.3]:
///     https://datatracker.ietf.org/doc/html/rfc3810#section-6.3
pub(super) fn handle_timer<I: IpExt, CC: GmpContextInner<I, BC>, BC: GmpBindingsContext>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    timer: TimerId<I>,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) {
    match timer {
        TimerId::GeneralQuery => handle_general_query_timer(core_ctx, bindings_ctx, device, state),
        TimerId::MulticastAddress(multicast_addr) => {
            handle_multicast_address_timer(core_ctx, bindings_ctx, device, multicast_addr, state)
        }
        TimerId::StateChange => handle_state_change_timer(core_ctx, bindings_ctx, device, state),
    }
}

/// Handles general query timers.
///
/// Quote from RFC 3810:
///
/// > If the expired timer is the Interface Timer (i.e., there is a pending
/// > response to a General Query), then one Current State Record is sent for
/// > each multicast address for which the specified interface has listening
/// > state [...]. The Current State Record carries the multicast address and
/// > its associated filter mode (MODE_IS_INCLUDE or MODE_IS_EXCLUDE) and Source
/// > list. Multiple Current State Records are packed into individual Report
/// > messages, to the extent possible.
fn handle_general_query_timer<I: IpExt, CC: GmpContextInner<I, BC>, BC: GmpBindingsContext>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) {
    let GmpStateRef { enabled: _, groups, gmp: _, config: _ } = state;
    let report = groups.iter().filter_map(|(addr, state)| {
        // TODO(https://fxbug.dev/381241191): Update to include SSM in group
        // records.
        let _ = state;

        // Ignore any groups that are not enabled for GMP.
        let group = GmpEnabledGroup::new(*addr)?;

        // Given we don't support SSM, all the groups we're currently joined
        // should be reported in exclude mode with an empty source list.
        //
        // See https://datatracker.ietf.org/doc/html/rfc3810#section-5.2.12 and
        // https://datatracker.ietf.org/doc/html/rfc3376#section-4.2.12 for
        // group record type descriptions.

        Some(GroupRecord::new(group, GroupRecordType::ModeIsExclude))
    });
    core_ctx.send_report_v2(bindings_ctx, device, report)
}

/// Handles a multicast address timer for `multicast_addr`.
///
/// RFC 3810 quote:
///
/// > If the expired timer is a Multicast Address Timer and the list of recorded
/// > sources for that multicast address is empty (i.e., there is a pending
/// > response to a Multicast Address Specific Query), then if, and only if, the
/// > interface has listening state for that multicast address, a single Current
/// > State Record is sent for that address. The Current State Record carries
/// > the multicast address and its associated filter mode (MODE_IS_INCLUDE or
/// > MODE_IS_EXCLUDE) and source list, if any.
/// >
/// > If the expired timer is a Multicast Address Timer and the list of recorded
/// > sources for that multicast address is non-empty (i.e., there is a pending
/// > response to a Multicast Address and Source Specific Query), then if, and
/// > only if, the interface has listening state for that multicast address, the
/// > contents of the corresponding Current State Record are determined from the
/// > per- interface state and the pending response record, as specified in the
/// > following table:
/// >
/// >                        set of sources in the
/// > per-interface state  pending response record  Current State Record
/// > -------------------  -----------------------  --------------------
/// >  INCLUDE (A)                   B                IS_IN (A*B)
/// >
/// >  EXCLUDE (A)                   B                IS_IN (B-A)
fn handle_multicast_address_timer<I: IpExt, CC: GmpContextInner<I, BC>, BC: GmpBindingsContext>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    multicast_addr: GmpEnabledGroup<I::Addr>,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) {
    let GmpStateRef { enabled: _, groups, gmp: _, config: _ } = state;
    // Invariant: multicast address timers are removed when we remove interest
    // from the group.
    let state = groups
        .get_mut(multicast_addr.as_ref())
        .expect("multicast timer fired for removed address")
        .v2_mut();
    let recorded_sources = core::mem::take(&mut state.recorded_sources);

    let (mode, sources) = if recorded_sources.is_empty() {
        // Multicast Address Specific Query.

        // TODO(https://fxbug.dev/381241191): Update to include SSM-enabled
        // filter mode. For now, ModeIsExclude is all that needs to be reported
        // for any group we're a member of.

        (GroupRecordType::ModeIsExclude, either::Either::Left(core::iter::empty::<&I::Addr>()))
    } else {
        // Multicast Address And Source Specific Query. The mode is always
        // include.

        // TODO(https://fxbug.dev/381241191): Actually calculate set
        // intersection or union when SSM is available.

        (GroupRecordType::ModeIsInclude, either::Either::Right(recorded_sources.iter()))
    };
    core_ctx.send_report_v2(
        bindings_ctx,
        device,
        core::iter::once(GroupRecord::new_with_sources(multicast_addr, mode, sources)),
    );
}

/// Handles the interface state change timer.
///
/// Note: sometimes referred to in the RFCs as `Retransmission Timer for a
/// multicast address`. This is actually an interface-wide timer that
/// "synchronizes" the retransmission instant for all the multicast addresses
/// with pending reports.
///
/// RFC quote:
///
/// > If the expired timer is a Retransmission Timer for a multicast address
/// > (i.e., there is a pending State Change Report for that multicast address),
/// > the contents of the report are determined as follows. If the report should
/// > contain a Filter Mode Change Record, i.e., the Filter Mode Retransmission
/// > Counter for that multicast address has a value higher than zero, then, if
/// > the current filter mode of the interface is INCLUDE, a TO_IN record is
/// > included in the report; otherwise a TO_EX record is included.  In both
/// > cases, the Filter Mode Retransmission Counter for that multicast address
/// > is decremented by one unit after the transmission of the report.
/// >
/// > If instead the report should contain Source List Change Records, i.e., the
/// > Filter Mode Retransmission Counter for that multicast address is zero, an
/// > ALLOW and a BLOCK record is included.
fn handle_state_change_timer<I: IpExt, CC: GmpContextInner<I, BC>, BC: GmpBindingsContext>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) {
    let GmpStateRef { enabled: _, groups, gmp, config } = state;

    let joined_groups = groups.iter().filter_map(|(multicast_addr, state)| {
        let GroupState { filter_mode_retransmission_counter, recorded_sources: _ } = state.v2();
        if *filter_mode_retransmission_counter == 0 {
            return None;
        }
        let multicast_addr = GmpEnabledGroup::new(*multicast_addr)?;
        Some(GroupRecord::new(
            multicast_addr,
            // TODO(https://fxbug.dev/381241191): Take the filter mode from
            // group state. Joined groups for now are always exclude mode.
            GroupRecordType::ChangeToExcludeMode,
        ))
    });
    let left_groups = gmp.v2_proto.left_groups.keys().map(|multicast_addr| {
        GroupRecord::new(
            *multicast_addr,
            // TODO(https://fxbug.dev/381241191): Take the filter mode from
            // group state. Left groups for now are always include mode.
            GroupRecordType::ChangeToIncludeMode,
        )
    });
    let state_change_report = joined_groups.chain(left_groups);
    core_ctx.send_report_v2(bindings_ctx, device, state_change_report);

    // Subtract the retransmission counters across the board.
    let has_more = groups.iter_mut().fold(false, |has_more, (_, g)| {
        let v2 = g.v2_mut();
        v2.filter_mode_retransmission_counter =
            v2.filter_mode_retransmission_counter.saturating_sub(1);
        has_more || v2.filter_mode_retransmission_counter != 0
    });
    gmp.v2_proto.left_groups.retain(|_, counter| match NonZeroU8::new(counter.get() - 1) {
        None => false,
        Some(new_value) => {
            *counter = new_value;
            true
        }
    });
    let has_more = has_more || !gmp.v2_proto.left_groups.is_empty();
    if has_more {
        let delay = gmp::random_report_timeout(
            &mut bindings_ctx.rng(),
            config.unsolicited_report_interval().get(),
        );
        assert_eq!(
            gmp.timers.schedule_after(bindings_ctx, TimerId::StateChange.into(), (), delay),
            None
        );
    }
}

/// Takes GMP actions when GMP becomes enabled.
///
/// This happens whenever the GMP switches to on or IP is enabled on an
/// interface (i.e. interface up). The side-effects here are not _quite_ covered
/// by the RFC, but the interpretation is that enablement is equivalent to all
/// the tracked groups becoming newly joined and we want to inform routers on
/// the network about it.
pub(super) fn handle_enabled<I: IpExt, CC: GmpTypeLayout<I, BC>, BC: GmpBindingsContext>(
    bindings_ctx: &mut BC,
    state: GmpStateRef<'_, I, CC, BC>,
) {
    let GmpStateRef { enabled: _, groups, gmp, config: _ } = state;

    let needs_report = groups.iter_mut().fold(false, |needs_report, (multicast_addr, state)| {
        if !I::should_perform_gmp(*multicast_addr) {
            return needs_report;
        }
        let GroupState { filter_mode_retransmission_counter, recorded_sources: _ } = state.v2_mut();
        *filter_mode_retransmission_counter = gmp.v2_proto.robustness_variable.get();
        true
    });
    if needs_report {
        trigger_state_change_report(bindings_ctx, &mut gmp.timers);
    }
}

/// Takes GMP actions when GMP becomes disabled.
///
/// This happens whenever the GMP switches to off or IP is disabled on an
/// interface (i.e. interface down). The side-effects here are not _quite_
/// covered by the RFC, but the interpretation is that disablement is equivalent
/// to all the tracked groups being left and we want to inform routers on the
/// network about it.
///
/// Unlike [`handle_enabled`], however, given this may be a last-ditch effort to
/// notify a router that an admin is turning off an interface, we immediately
/// send a _single_ report saying we've left all our groups. Given the interface
/// is possibly about to go off, we can't schedule any timers.
pub(super) fn handle_disabled<I: IpExt, CC: GmpContextInner<I, BC>, BC: GmpBindingsContext>(
    core_ctx: &mut CC,
    bindings_ctx: &mut BC,
    device: &CC::DeviceId,
    state: GmpStateRef<'_, I, CC::TypeLayout, BC>,
) {
    let GmpStateRef { enabled: _, groups, gmp, config: _ } = state;
    // Clear all group retransmission state and cancel all timers.
    for (_, state) in groups.iter_mut() {
        *state.v2_mut() = GroupState {
            filter_mode_retransmission_counter: 0,
            recorded_sources: Default::default(),
        };
    }

    let member_groups =
        groups.iter().filter_map(|(multicast_addr, _)| GmpEnabledGroup::new(*multicast_addr));
    // Also include any non-member groups that might've been waiting
    // retransmissions.
    let non_member_groups = gmp.v2_proto.left_groups.keys().copied();

    let mut report = member_groups
        .chain(non_member_groups)
        .map(|addr| GroupRecord::new(addr, GroupRecordType::ChangeToIncludeMode))
        .peekable();
    if report.peek().is_none() {
        // Nothing to report.
        return;
    }
    core_ctx.send_report_v2(bindings_ctx, device, report);
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use assert_matches::assert_matches;
    use ip_test_macro::ip_test;
    use netstack3_base::InstantContext as _;
    use netstack3_base::testutil::{FakeDeviceId, FakeTimerCtxExt, FakeWeakDeviceId};
    use test_case::{test_case, test_matrix};

    use super::*;
    use crate::gmp::GmpTimerId;
    use crate::internal::gmp::testutil::{
        self, FakeCtx, FakeGmpBindingsContext, FakeGmpContext, FakeV2Query, TestIpExt,
    };
    use crate::internal::gmp::{GmpHandler as _, GroupJoinResult};

    #[derive(Debug, Eq, PartialEq)]
    enum SpecificQuery {
        Multicast,
        MulticastAndSource,
    }

    fn join_and_ignore_unsolicited<I: IpExt>(
        ctx: &mut FakeCtx<I>,
        groups: impl IntoIterator<Item = MulticastAddr<I::Addr>>,
    ) {
        let FakeCtx { core_ctx, bindings_ctx } = ctx;
        for group in groups {
            assert_eq!(
                core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, group),
                GroupJoinResult::Joined(())
            );
        }
        while !core_ctx.gmp.timers.is_empty() {
            assert_eq!(
                bindings_ctx.trigger_next_timer(core_ctx),
                Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
            );
        }
        core_ctx.inner.v2_messages.clear();
    }

    impl<I: IpExt> TimerId<I> {
        fn multicast(addr: MulticastAddr<I::Addr>) -> Self {
            Self::MulticastAddress(GmpEnabledGroup::new(addr).unwrap())
        }
    }

    #[ip_test(I)]
    fn v2_query_handoff_in_v1_mode<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V1 { compat: true });
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );
        assert_eq!(
            bindings_ctx.trigger_next_timer(&mut core_ctx),
            Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
        );
        // v1 group should be idle now.
        assert_matches!(
            core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().v1().get_inner(),
            gmp::v1::MemberState::Idle(_)
        );
        handle_query_message(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            &FakeV2Query { group_addr: I::GROUP_ADDR1.get(), ..Default::default() },
        )
        .expect("handle query");
        // v1 group reacts to the query.
        assert_matches!(
            core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().v1().get_inner(),
            gmp::v1::MemberState::Delaying(_)
        );
    }

    #[ip_test(I)]
    fn general_query_ignored_if_no_groups<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        handle_query_message(
            &mut core_ctx,
            &mut bindings_ctx,
            &FakeDeviceId,
            &FakeV2Query { group_addr: I::UNSPECIFIED_ADDRESS, ..Default::default() },
        )
        .expect("handle query");
        assert_eq!(core_ctx.gmp.timers.get(&TimerId::GeneralQuery.into()), None);
    }

    #[ip_test(I)]
    fn query_errors_if_not_multicast<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        let query = FakeV2Query { group_addr: I::LOOPBACK_ADDRESS.get(), ..Default::default() };
        assert_eq!(
            handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query,),
            Err(QueryError::NotAMember(query.group_addr))
        );
    }

    #[ip_test(I)]
    fn general_query_scheduled<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );
        let query = FakeV2Query { group_addr: I::UNSPECIFIED_ADDRESS, ..Default::default() };

        let general_query_timer = TimerId::GeneralQuery.into();

        handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query)
            .expect("handle query");
        let now = bindings_ctx.now();
        let (scheduled, ()) = core_ctx.gmp.timers.assert_range_single(
            &general_query_timer,
            now..=now.panicking_add(query.max_response_time),
        );

        // Any further queries are ignored  if we have a pending general query
        // in the past.

        // Advance time enough to guarantee we can't pick an earlier time.
        bindings_ctx.timers.instant.sleep(query.max_response_time);

        let query = FakeV2Query { group_addr: I::UNSPECIFIED_ADDRESS, ..Default::default() };
        handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query)
            .expect("handle query");
        assert_eq!(core_ctx.gmp.timers.get(&general_query_timer), Some((scheduled, &())));

        let query = FakeV2Query { group_addr: I::GROUP_ADDR1.get(), ..Default::default() };
        handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query)
            .expect("handle query");
        assert_eq!(core_ctx.gmp.timers.get(&TimerId::multicast(I::GROUP_ADDR1).into()), None);
    }

    #[ip_test(I)]
    fn specific_query_ignored_if_not_member<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR2),
            GroupJoinResult::Joined(())
        );
        let query = FakeV2Query { group_addr: I::GROUP_ADDR1.get(), ..Default::default() };
        assert_eq!(
            handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query),
            Err(QueryError::NotAMember(query.group_addr))
        );
    }

    #[ip_test(I)]
    fn leave_group_cancels_multicast_address_timer<I: TestIpExt>() {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );
        let query = FakeV2Query { group_addr: I::GROUP_ADDR1.get(), ..Default::default() };
        handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query)
            .expect("handle query");
        assert_matches!(
            core_ctx.gmp.timers.get(&TimerId::multicast(I::GROUP_ADDR1).into()),
            Some(_)
        );
        assert_eq!(
            core_ctx.gmp_leave_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupLeaveResult::Left(())
        );
        assert_matches!(core_ctx.gmp.timers.get(&TimerId::multicast(I::GROUP_ADDR1).into()), None);
    }

    #[ip_test(I)]
    #[test_matrix(
        [SpecificQuery::Multicast, SpecificQuery::MulticastAndSource],
        [SpecificQuery::Multicast, SpecificQuery::MulticastAndSource]
    )]
    fn schedule_specific_query<I: TestIpExt>(first: SpecificQuery, second: SpecificQuery) {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );

        let sources = match first {
            SpecificQuery::Multicast => Default::default(),
            SpecificQuery::MulticastAndSource => {
                (1..3).map(|i| I::get_other_ip_address(i).get()).collect()
            }
        };

        let query1 =
            FakeV2Query { group_addr: I::GROUP_ADDR1.get(), sources, ..Default::default() };
        handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query1)
            .expect("handle query");
        // Sources are recorded.
        assert_eq!(
            core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().v2().recorded_sources,
            query1.sources.iter().copied().collect()
        );
        // Timer is scheduled.
        let now = bindings_ctx.now();
        let (scheduled, ()) = core_ctx.gmp.timers.assert_range_single(
            &TimerId::multicast(I::GROUP_ADDR1).into(),
            now..=now.panicking_add(query1.max_response_time),
        );

        let sources = match second {
            SpecificQuery::Multicast => Default::default(),
            SpecificQuery::MulticastAndSource => {
                (3..5).map(|i| I::get_other_ip_address(i).get()).collect()
            }
        };
        let query2 = FakeV2Query {
            group_addr: I::GROUP_ADDR1.get(),
            // Send a follow up query on a shorter timeline.
            max_response_time: DEFAULT_QUERY_RESPONSE_INTERVAL.get() / 2,
            sources,
            ..Default::default()
        };
        handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query2)
            .expect("handle query");

        let (new_scheduled, ()) = core_ctx.gmp.timers.assert_range_single(
            &TimerId::multicast(I::GROUP_ADDR1).into(),
            now..=now.panicking_add(query2.max_response_time),
        );
        // Scheduled time is allowed to change, but always to an earlier time.
        assert!(new_scheduled <= scheduled, "{new_scheduled:?} <= {scheduled:?}");
        // Now check the group state.
        let recorded_sources = &core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().v2().recorded_sources;
        match (first, second) {
            (SpecificQuery::Multicast, _) | (_, SpecificQuery::Multicast) => {
                // If any of the queries is multicast-specific then:
                // - Never added any sources.
                // - Newer sources must not override previous
                //   multicast-specific.
                // - New multicast-specific overrides previous sources.
                assert_eq!(recorded_sources, &HashSet::new());
            }
            (SpecificQuery::MulticastAndSource, SpecificQuery::MulticastAndSource) => {
                // List is augmented with the union.
                assert_eq!(
                    recorded_sources,
                    &query1.sources.iter().chain(query2.sources.iter()).copied().collect()
                );
            }
        }
    }

    #[ip_test(I)]
    #[test_case(1, u8::try_from(MAX_RECORDED_SOURCES).unwrap(); "single_query")]
    #[test_case(2, u8::try_from(MAX_RECORDED_SOURCES - 1).unwrap(); "multi_query")]
    fn recorded_sources_truncated<I: TestIpExt>(num_queries: u8, num_sources_per_query: u8) {
        let FakeCtx { mut core_ctx, mut bindings_ctx } =
            testutil::new_context_with_mode::<I>(GmpMode::V2);
        assert_eq!(
            core_ctx.gmp_join_group(&mut bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );

        for i in 0..num_queries {
            let start = i * num_sources_per_query;
            let end = start + num_sources_per_query;
            let sources: Vec<_> = (start..end).map(|i| I::get_other_ip_address(i).get()).collect();
            let query = FakeV2Query {
                group_addr: I::GROUP_ADDR1.get(),
                sources: sources,
                ..Default::default()
            };
            handle_query_message(&mut core_ctx, &mut bindings_ctx, &FakeDeviceId, &query)
                .expect("handle query 1");
        }

        let recorded_sources = &core_ctx.groups.get(&I::GROUP_ADDR1).unwrap().v2().recorded_sources;
        assert_eq!(recorded_sources.len(), MAX_RECORDED_SOURCES);
        let expected_sources: HashSet<_> = (0..u8::try_from(MAX_RECORDED_SOURCES).unwrap())
            .map(|i| I::get_other_ip_address(i).get())
            .collect();
        assert_eq!(recorded_sources, &expected_sources);
    }

    #[ip_test(I)]
    fn send_general_query_response<I: TestIpExt>() {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1, I::GROUP_ADDR2]);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        handle_query_message(core_ctx, bindings_ctx, &FakeDeviceId, &FakeV2Query::default())
            .expect("handle query");
        assert_eq!(
            bindings_ctx.trigger_next_timer(core_ctx),
            Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
        );
        assert_eq!(
            core_ctx.inner.v2_messages,
            vec![vec![
                (I::GROUP_ADDR1, GroupRecordType::ModeIsExclude, vec![]),
                (I::GROUP_ADDR2, GroupRecordType::ModeIsExclude, vec![]),
            ]]
        );
    }

    #[ip_test(I)]
    fn send_multicast_address_specific_query_response<I: TestIpExt>() {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1]);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        handle_query_message(
            core_ctx,
            bindings_ctx,
            &FakeDeviceId,
            &FakeV2Query { group_addr: I::GROUP_ADDR1.get(), ..Default::default() },
        )
        .expect("handle query");
        assert_eq!(
            bindings_ctx.trigger_next_timer(core_ctx),
            Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
        );
        assert_eq!(
            core_ctx.inner.v2_messages,
            vec![vec![(I::GROUP_ADDR1, GroupRecordType::ModeIsExclude, vec![])]]
        );
    }

    #[ip_test(I)]
    fn send_multicast_address_and_source_specific_query_response<I: TestIpExt>() {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1]);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        let query = FakeV2Query {
            group_addr: I::GROUP_ADDR1.get(),
            sources: vec![I::get_other_ip_address(1).get(), I::get_other_ip_address(2).get()],
            ..Default::default()
        };
        handle_query_message(core_ctx, bindings_ctx, &FakeDeviceId, &query).expect("handle query");
        assert_eq!(
            bindings_ctx.trigger_next_timer(core_ctx),
            Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
        );
        assert_eq!(
            core_ctx.inner.v2_messages,
            vec![vec![(I::GROUP_ADDR1, GroupRecordType::ModeIsInclude, query.sources)]]
        );
    }

    #[ip_test(I)]
    #[test_case(2)]
    #[test_case(4)]
    fn join_group_unsolicited_reports<I: TestIpExt>(robustness_variable: u8) {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        core_ctx.gmp.v2_proto.robustness_variable = NonZeroU8::new(robustness_variable).unwrap();
        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );
        // Nothing is sent immediately.
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
        let now = bindings_ctx.now();
        assert_eq!(
            core_ctx.gmp.timers.get(&TimerId::StateChange.into()),
            Some((now.panicking_add(STATE_CHANGE_REPORT_DELAY), &()))
        );
        let mut count = 0;
        while let Some(timer) = bindings_ctx.trigger_next_timer(core_ctx) {
            count += 1;
            assert_eq!(timer, GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)));
            let messages = core::mem::take(&mut core_ctx.inner.v2_messages);
            assert_eq!(
                messages,
                vec![vec![(I::GROUP_ADDR1, GroupRecordType::ChangeToExcludeMode, vec![])]]
            );

            if count != robustness_variable {
                let now = bindings_ctx.now();
                core_ctx.gmp.timers.assert_range([(
                    &TimerId::StateChange.into(),
                    now..=now.panicking_add(core_ctx.config.unsolicited_report_interval().get()),
                )]);
            }
        }
        assert_eq!(count, robustness_variable);
        core_ctx.gmp.timers.assert_timers([]);

        // Joining again has no side-effects.
        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::AlreadyMember
        );
        // No timers, no messages.
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
    }

    #[ip_test(I)]
    #[test_case(2)]
    #[test_case(4)]
    fn leave_group_unsolicited_reports<I: TestIpExt>(robustness_variable: u8) {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1]);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        core_ctx.gmp.v2_proto.robustness_variable = NonZeroU8::new(robustness_variable).unwrap();

        // Join the same group again. Like two sockets are interested in this
        // group.
        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::AlreadyMember
        );

        // Leaving non member has no side-effects.
        assert_eq!(
            core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR2),
            GroupLeaveResult::NotMember
        );
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());

        // First leave we're still member and no side-effects.
        assert_eq!(
            core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupLeaveResult::StillMember
        );
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());

        assert_eq!(
            core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupLeaveResult::Left(())
        );
        let mut count = 0;
        while let Some(timer) = bindings_ctx.trigger_next_timer(core_ctx) {
            count += 1;
            assert_eq!(timer, GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)));

            let messages = core::mem::take(&mut core_ctx.inner.v2_messages);
            assert_eq!(
                messages,
                vec![vec![(I::GROUP_ADDR1, GroupRecordType::ChangeToIncludeMode, vec![])]]
            );

            if count != robustness_variable {
                let now = bindings_ctx.now();
                core_ctx.gmp.timers.assert_range([(
                    &TimerId::StateChange.into(),
                    now..=now.panicking_add(core_ctx.config.unsolicited_report_interval().get()),
                )]);
            }
        }
        assert_eq!(count, robustness_variable);
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.gmp.v2_proto.left_groups, HashMap::new());

        // Leave same group again, no side-effects.
        assert_eq!(
            core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupLeaveResult::NotMember
        );
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
    }

    #[ip_test(I)]
    #[test_matrix(
        0..=3,
        0..=3
    )]
    fn join_and_leave<I: TestIpExt>(wait_join: u8, wait_leave: u8) {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        // NB: This matches the maximum value given to test inputs, but the
        // test_matrix macro only accepts literals.
        core_ctx.gmp.v2_proto.robustness_variable = NonZeroU8::new(3).unwrap();

        let wait_reports = |core_ctx: &mut FakeGmpContext<I>,
                            bindings_ctx: &mut FakeGmpBindingsContext<I>,
                            mode,
                            count: u8| {
            for _ in 0..count {
                assert_eq!(
                    bindings_ctx.trigger_next_timer(core_ctx),
                    Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
                );
            }
            let messages = core::mem::take(&mut core_ctx.inner.v2_messages);
            assert_eq!(messages.len(), usize::from(count));
            for m in messages {
                assert_eq!(m, vec![(I::GROUP_ADDR1, mode, vec![])]);
            }
        };

        for _ in 0..3 {
            assert_eq!(
                core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
                GroupJoinResult::Joined(())
            );
            assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
            let now = bindings_ctx.now();
            core_ctx.gmp.timers.assert_range([(
                &TimerId::StateChange.into(),
                now..=now.panicking_add(STATE_CHANGE_REPORT_DELAY),
            )]);
            wait_reports(core_ctx, bindings_ctx, GroupRecordType::ChangeToExcludeMode, wait_join);

            assert_eq!(
                core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
                GroupLeaveResult::Left(())
            );
            assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
            let now = bindings_ctx.now();
            core_ctx.gmp.timers.assert_range([(
                &TimerId::StateChange.into(),
                now..=now.panicking_add(STATE_CHANGE_REPORT_DELAY),
            )]);
            wait_reports(core_ctx, bindings_ctx, GroupRecordType::ChangeToIncludeMode, wait_leave);
        }
    }

    #[derive(Debug)]
    enum GroupOp {
        Join,
        Leave,
    }
    #[ip_test(I)]
    #[test_matrix(
        0..=3,
        [GroupOp::Join, GroupOp::Leave]
    )]
    fn merge_reports<I: TestIpExt>(wait_reports: u8, which_op: GroupOp) {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        match which_op {
            GroupOp::Join => {}
            GroupOp::Leave => {
                // If we're testing leave, join the group first.
                join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1]);
            }
        }

        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        // NB: This matches the maximum value given to test inputs, but the
        // test_matrix macro only accepts literals.
        core_ctx.gmp.v2_proto.robustness_variable = NonZeroU8::new(3).unwrap();

        // Join another group that we'll have our report merged with.
        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR2),
            GroupJoinResult::Joined(())
        );
        for _ in 0..wait_reports {
            assert_eq!(
                bindings_ctx.trigger_next_timer(core_ctx),
                Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
            );
        }
        // Drop all messages this is tested elsewhere, just ensure the number of
        // reports sent out so far is what we expect.
        assert_eq!(
            core::mem::take(&mut core_ctx.inner.v2_messages).len(),
            usize::from(wait_reports)
        );
        let expect_record_type = match which_op {
            GroupOp::Join => {
                assert_eq!(
                    core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
                    GroupJoinResult::Joined(())
                );
                GroupRecordType::ChangeToExcludeMode
            }
            GroupOp::Leave => {
                assert_eq!(
                    core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
                    GroupLeaveResult::Left(())
                );
                GroupRecordType::ChangeToIncludeMode
            }
        };
        // No messages are generated immediately:
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
        // The next report is at _most_ the delay away.
        let now = bindings_ctx.now();
        core_ctx.gmp.timers.assert_range([(
            &TimerId::StateChange.into(),
            now..=now.panicking_add(STATE_CHANGE_REPORT_DELAY),
        )]);
        // We should see robustness_variable reports, the first (reports -
        // wait_reports) should contain the join group retransmission still.
        let reports = core_ctx.gmp.v2_proto.robustness_variable.get();

        // Collect all the messages we expect to see as we drive the timer.
        let expected_messages = (0..reports)
            .map(|count| {
                assert_eq!(
                    bindings_ctx.trigger_next_timer(core_ctx),
                    Some(GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)))
                );
                let mut expect = vec![(I::GROUP_ADDR1, expect_record_type, vec![])];
                if count < reports - wait_reports {
                    expect.push((I::GROUP_ADDR2, GroupRecordType::ChangeToExcludeMode, vec![]));
                }
                expect
            })
            .collect::<Vec<_>>();
        assert_eq!(core_ctx.inner.v2_messages, expected_messages);
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.gmp.v2_proto.left_groups, HashMap::new());
    }

    #[ip_test(I)]
    fn enable_disable<I: TestIpExt>() {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1, I::GROUP_ADDR2]);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;

        // We call maybe enable again, but if we're already enabled there
        // are no side-effects.
        core_ctx.gmp_handle_maybe_enabled(bindings_ctx, &FakeDeviceId);
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());

        // Disable and observe a single leave report and no timers.
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(bindings_ctx, &FakeDeviceId);
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(
            core::mem::take(&mut core_ctx.inner.v2_messages),
            vec![vec![
                (I::GROUP_ADDR1, GroupRecordType::ChangeToIncludeMode, vec![],),
                (I::GROUP_ADDR2, GroupRecordType::ChangeToIncludeMode, vec![],),
            ]]
        );

        // Disable again no side-effects.
        core_ctx.gmp_handle_disabled(bindings_ctx, &FakeDeviceId);
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());

        // Re-enable and observe robustness_variable state changes.
        core_ctx.enabled = true;
        core_ctx.gmp_handle_maybe_enabled(bindings_ctx, &FakeDeviceId);
        let now = bindings_ctx.now();
        core_ctx.gmp.timers.assert_range([(
            &TimerId::StateChange.into(),
            now..=now.panicking_add(STATE_CHANGE_REPORT_DELAY),
        )]);
        // No messages yet, this behaves exactly like joining many groups all
        // at once.
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());

        while let Some(timer) = bindings_ctx.trigger_next_timer(core_ctx) {
            assert_eq!(timer, GmpTimerId::new(FakeWeakDeviceId(FakeDeviceId)));
        }
        let expect_messages = core::iter::repeat_with(|| {
            vec![
                (I::GROUP_ADDR1, GroupRecordType::ChangeToExcludeMode, vec![]),
                (I::GROUP_ADDR2, GroupRecordType::ChangeToExcludeMode, vec![]),
            ]
        })
        .take(core_ctx.gmp.v2_proto.robustness_variable.get().into())
        .collect::<Vec<_>>();
        assert_eq!(core::mem::take(&mut core_ctx.inner.v2_messages), expect_messages);

        // Disable one more time while we're in the process of leaving one of
        // the groups to show that we allow it to piggyback on the last report
        // once.
        assert_eq!(
            core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupLeaveResult::Left(())
        );
        assert_eq!(
            core_ctx.gmp.v2_proto.left_groups.get(&GmpEnabledGroup::new(I::GROUP_ADDR1).unwrap()),
            Some(&core_ctx.gmp.v2_proto.robustness_variable)
        );
        // Disable and observe a single leave report INCLUDING the already left
        // group and no timers.
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(bindings_ctx, &FakeDeviceId);
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(
            core::mem::take(&mut core_ctx.inner.v2_messages),
            vec![vec![
                (I::GROUP_ADDR1, GroupRecordType::ChangeToIncludeMode, vec![],),
                (I::GROUP_ADDR2, GroupRecordType::ChangeToIncludeMode, vec![],),
            ]]
        );
    }

    #[ip_test(I)]
    fn ignore_query_if_disabled<I: TestIpExt>() {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        core_ctx.enabled = false;
        core_ctx.gmp_handle_disabled(bindings_ctx, &FakeDeviceId);

        assert_eq!(
            core_ctx.gmp_join_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupJoinResult::Joined(())
        );

        // Receive a general query.
        assert_eq!(
            handle_query_message(core_ctx, bindings_ctx, &FakeDeviceId, &FakeV2Query::default()),
            Err(QueryError::Disabled)
        );
        // No side-effects.
        core_ctx.gmp.timers.assert_timers([]);
        assert_eq!(core_ctx.inner.v2_messages, Vec::<Vec<_>>::new());
    }

    #[ip_test(I)]
    fn clears_v2_proto_state_on_mode_change<I: TestIpExt>() {
        let mut ctx = testutil::new_context_with_mode::<I>(GmpMode::V2);
        join_and_ignore_unsolicited(&mut ctx, [I::GROUP_ADDR1]);

        let FakeCtx { core_ctx, bindings_ctx } = &mut ctx;
        let query = FakeV2Query {
            robustness_variable: DEFAULT_ROBUSTNESS_VARIABLE.get() + 1,
            query_interval: DEFAULT_QUERY_INTERVAL.get() + Duration::from_secs(1),
            ..Default::default()
        };
        handle_query_message(core_ctx, bindings_ctx, &FakeDeviceId, &query).expect("handle query");
        assert_eq!(
            core_ctx.gmp_leave_group(bindings_ctx, &FakeDeviceId, I::GROUP_ADDR1),
            GroupLeaveResult::Left(())
        );
        let robustness_variable = NonZeroU8::new(query.robustness_variable).unwrap();
        let query_interval = NonZeroDuration::new(query.query_interval).unwrap();
        assert_eq!(
            core_ctx.gmp.v2_proto,
            ProtocolState {
                robustness_variable,
                query_interval,
                left_groups: [(GmpEnabledGroup::new(I::GROUP_ADDR1).unwrap(), robustness_variable)]
                    .into_iter()
                    .collect()
            }
        );

        core_ctx.with_gmp_state_mut(&FakeDeviceId, |state| {
            gmp::enter_mode(bindings_ctx, state, GmpMode::V1 { compat: false });
        });

        assert_eq!(
            core_ctx.gmp.v2_proto,
            ProtocolState { robustness_variable, query_interval, left_groups: HashMap::new() }
        );
    }
}
