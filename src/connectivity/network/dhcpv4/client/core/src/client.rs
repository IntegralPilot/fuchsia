// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements the DHCP client state machine.

use crate::deps::{self, DatagramInfo, Instant as _, Socket as _};
use crate::inspect::{
    Counters, MessagingRelatedCounters, RebindingCounters, RenewingCounters, RequestingCounters,
    SelectingCounters,
};
use crate::parse::{
    FieldsFromOfferToUseInRequest, FieldsToRetainFromAck, FieldsToRetainFromNak, OptionCodeMap,
    OptionRequested,
};

use anyhow::Context as _;
use bstr::BString;
use dhcp_protocol::{AtLeast, AtMostBytes, CLIENT_PORT, SERVER_PORT};
use diagnostics_traits::Inspector;
use futures::channel::mpsc;
use futures::stream::FusedStream;
use futures::{FutureExt as _, Stream, StreamExt as _, TryStreamExt as _, select, select_biased};
use net_types::ethernet::Mac;
use net_types::{SpecifiedAddr, Witness as _};
use rand::RngExt as _;

use std::fmt::{Debug, Display};
use std::net::Ipv4Addr;
use std::num::{NonZeroU32, NonZeroU64};
use std::ops::Deref as _;
use std::pin::pin;
use std::time::Duration;

/// Unexpected, non-recoverable errors encountered by the DHCP client.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Error encountered while performing a socket operation.
    #[error("error while using socket: {0:?}")]
    Socket(deps::SocketError),
    /// The AddressEvent receiver unexpectedly ended.
    #[error("the address_event_receiver was unexpectedly empty")]
    AddressEventReceiverEnded,
}

/// The reason the DHCP client exited.
#[derive(Debug)]
pub enum ExitReason<R> {
    /// Executed due to a request for graceful shutdown.
    GracefulShutdown,
    /// Exiting because the address was removed.
    AddressRemoved(R),
}

/// All possible core state machine states from the state-transition diagram in
/// [RFC 2131].
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Debug, Clone, Copy)]
pub enum State<I> {
    /// The default initial state of the state machine (no known
    /// currently-assigned IP address or DHCP server).
    Init(Init),
    /// The Selecting state (broadcasting DHCPDISCOVERs and receiving
    /// DHCPOFFERs).
    Selecting(Selecting<I>),
    /// The Requesting state (broadcasting DHCPREQUESTs and receiving DHCPACKs
    /// and DHCPNAKs).
    Requesting(Requesting<I>),
    /// The Bound state (we actively have a lease and are waiting to transition
    /// to Renewing).
    Bound(Bound<I>),
    /// The Renewing state (we actively have a lease that we are trying to
    /// renew by unicasting requests to our known DHCP server).
    Renewing(Renewing<I>),
    /// The Rebinding state (we actively have a lease that we are trying to
    /// renew by broadcasting requests to any DHCP server).
    Rebinding(Rebinding<I>),
    /// Waiting to restart the configuration process (via transitioning to Init).
    WaitingToRestart(WaitingToRestart<I>),
}

/// The next step to take after running the core state machine for one step.
#[derive(Debug)]
pub enum Step<I, R> {
    /// Transition to another state.
    NextState(Transition<I>),
    /// Exit the client.
    Exit(ExitReason<R>),
}

/// A state-transition to execute (see `State` enum variant documentation for a
/// description of each state).
#[derive(Debug)]
pub enum Transition<I> {
    /// Transition to Init.
    Init(Init),
    /// Transition to Selecting.
    Selecting(Selecting<I>),
    /// Transition to Requesting.
    Requesting(Requesting<I>),
    /// Transition to Bound, having newly acquired a lease.
    BoundWithNewLease(Bound<I>, NewlyAcquiredLease<I>),
    /// Transition to Bound::Assigned from Bound::AwaitingAssignment.
    BoundAssigned(Bound<I>),
    /// Transition to Bound, having renewed a previously-acquired lease.
    BoundWithRenewedLease(Bound<I>, LeaseRenewal<I>),
    /// Transition to Renewing.
    Renewing(Renewing<I>),
    /// Transition to Rebinding.
    Rebinding(Rebinding<I>),
    /// Transition to wait to restart the configuration process.
    WaitingToRestart(WaitingToRestart<I>),
}

/// A side-effect of a state transition.
#[must_use]
#[derive(Debug)]
pub enum TransitionEffect<I> {
    /// Drop the existing lease.
    DropLease {
        /// True if the Lease is being dropped because the address was rejected.
        address_rejected: bool,
    },
    /// Handle a newly-acquired lease.
    HandleNewLease(NewlyAcquiredLease<I>),
    /// Handle a renewed lease.
    HandleRenewedLease(LeaseRenewal<I>),
}

/// Outcome of handling an address rejection.
#[derive(Debug)]
pub enum AddressRejectionOutcome<I> {
    /// Observing an address rejection in this state should be impossible due to
    /// not having an active lease.
    ShouldBeImpossible,
    /// Transition to a new state.
    NextState(State<I>),
}

/// State of an address.
#[derive(Debug)]
pub enum AddressAssignmentState {
    /// The address is assigned to the interface and is available for use.
    Assigned,
    /// The address is tentatively assigned to the interface and is unavailable
    /// for use. It will become `Assigned` once Duplicate Address Detection
    /// completes.
    Tentative,
    /// The address is unavailable for use, and is not progressing towards
    /// becoming `Assigned`. This may happen if the address's interface is
    /// offline.
    Unavailable,
}

/// Events for an addr that are produced outside of the core DHCP state machine.
#[derive(Debug)]
pub enum AddressEvent<R> {
    /// The address was rejected during assignment. A DHCPDECLINE message should
    /// be sent.
    Rejected,
    /// The address was removed. Unlike `Rejected`, a DHCPDECLINE message should
    /// not be sent.
    ///
    /// Holds an opaque reason `R` indicating why the address was removed.
    Removed(R),
    /// The address's assignment state changed.
    AssignmentStateChanged(AddressAssignmentState),
}

// Per RFC 2131 section 3.1, after sending a DHCPDECLINE message, "the client
// SHOULD wait a minimum of ten seconds before restarting the configuration
// process to avoid excessive network traffic in case of looping."
const WAIT_TIME_BEFORE_RESTARTING_AFTER_ADDRESS_REJECTION: Duration = Duration::from_secs(10);

impl<I: deps::Instant> State<I> {
    /// Run the client state machine for one "step".
    pub async fn run<C: deps::Clock<Instant = I>, R>(
        &self,
        config: &ClientConfig,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        udp_socket_provider: &impl deps::UdpSocketProvider,
        rng: &mut impl deps::RngProvider,
        clock: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
        counters: &Counters,
    ) -> Result<Step<I, R>, Error> {
        let step = self
            .run_inner(
                config,
                packet_socket_provider,
                udp_socket_provider,
                rng,
                clock,
                stop_receiver,
                address_event_receiver,
                counters,
            )
            .await?;

        match &step {
            Step::NextState(transition) => {
                let counter_to_increment = match transition {
                    Transition::Init(_) => &counters.init.entered,
                    Transition::Selecting(_) => &counters.selecting.entered,
                    Transition::Requesting(_) => &counters.requesting.entered,
                    Transition::BoundWithNewLease(_, _) => &counters.bound.entered,
                    Transition::BoundAssigned(_) => &counters.bound.assigned,
                    Transition::BoundWithRenewedLease(_, _) => &counters.bound.entered,
                    Transition::Renewing(_) => &counters.renewing.entered,
                    Transition::Rebinding(_) => &counters.rebinding.entered,
                    Transition::WaitingToRestart(_) => &counters.waiting_to_restart.entered,
                };
                counter_to_increment.increment();
            }
            Step::Exit(_) => (),
        };

        Ok(step)
    }

    async fn run_inner<C: deps::Clock<Instant = I>, R>(
        &self,
        config: &ClientConfig,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        udp_socket_provider: &impl deps::UdpSocketProvider,
        rng: &mut impl deps::RngProvider,
        clock: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
        counters: &Counters,
    ) -> Result<Step<I, R>, Error> {
        let debug_log_prefix = &config.debug_log_prefix;
        match self {
            State::Init(init) => {
                Ok(Step::NextState(Transition::Selecting(init.do_init(rng, clock))))
            }
            State::Selecting(selecting) => match selecting
                .do_selecting(config, packet_socket_provider, rng, clock, stop_receiver, counters)
                .await?
            {
                SelectingOutcome::GracefulShutdown => Ok(Step::Exit(ExitReason::GracefulShutdown)),
                SelectingOutcome::Requesting(requesting) => {
                    Ok(Step::NextState(Transition::Requesting(requesting)))
                }
            },
            State::Requesting(requesting) => {
                match requesting
                    .do_requesting(
                        config,
                        packet_socket_provider,
                        rng,
                        clock,
                        stop_receiver,
                        counters,
                    )
                    .await?
                {
                    RequestingOutcome::RanOutOfRetransmits => {
                        log::info!(
                            "{debug_log_prefix} Returning to Init due to \
                            running out of DHCPREQUEST retransmits"
                        );
                        Ok(Step::NextState(Transition::Init(Init)))
                    }
                    RequestingOutcome::GracefulShutdown => {
                        Ok(Step::Exit(ExitReason::GracefulShutdown))
                    }
                    RequestingOutcome::Bound(lease_state, parameters) => {
                        let LeaseState {
                            discover_options: _,
                            yiaddr,
                            server_identifier: _,
                            ip_address_lease_time,
                            renewal_time: _,
                            rebinding_time: _,
                            start_time,
                        } = &lease_state;
                        let newly_acquired_lease = NewlyAcquiredLease {
                            ip_address: *yiaddr,
                            start_time: *start_time,
                            lease_time: *ip_address_lease_time.as_ref(),
                            parameters,
                        };
                        Ok(Step::NextState(Transition::BoundWithNewLease(
                            Bound::AwaitingAssignment { lease_state },
                            newly_acquired_lease,
                        )))
                    }
                    RequestingOutcome::Nak(nak) => {
                        // Per RFC 2131 section 3.1: "If the client receives a
                        // DHCPNAK message, the client restarts the
                        // configuration process."
                        log::warn!(
                            "{debug_log_prefix} Returning to Init due to DHCPNAK: {:?}",
                            nak
                        );
                        Ok(Step::NextState(Transition::Init(Init)))
                    }
                }
            }
            State::Bound(bound) => match bound
                .do_bound(
                    config,
                    clock,
                    stop_receiver,
                    packet_socket_provider,
                    address_event_receiver,
                    counters,
                )
                .await?
            {
                BoundOutcome::GracefulShutdown => Ok(Step::Exit(ExitReason::GracefulShutdown)),
                BoundOutcome::Renewing(renewing) => {
                    Ok(Step::NextState(Transition::Renewing(renewing)))
                }
                BoundOutcome::Restart(init) => Ok(Step::NextState(Transition::Init(init))),
                BoundOutcome::AddressRemoved(reason) => {
                    Ok(Step::Exit(ExitReason::AddressRemoved(reason)))
                }
                BoundOutcome::AddressRejected(waiting_to_restart) => {
                    Ok(Step::NextState(Transition::WaitingToRestart(waiting_to_restart)))
                }
                BoundOutcome::Assigned(bound) => {
                    Ok(Step::NextState(Transition::BoundAssigned(bound)))
                }
            },
            State::Renewing(renewing) => {
                match renewing
                    .do_renewing(
                        config,
                        udp_socket_provider,
                        packet_socket_provider,
                        clock,
                        stop_receiver,
                        address_event_receiver,
                        counters,
                    )
                    .await?
                {
                    RenewingOutcome::GracefulShutdown => {
                        Ok(Step::Exit(ExitReason::GracefulShutdown))
                    }
                    RenewingOutcome::Renewed(lease_state, parameters) => {
                        let LeaseState {
                            discover_options: _,
                            yiaddr: _,
                            server_identifier: _,
                            ip_address_lease_time,
                            renewal_time: _,
                            rebinding_time: _,
                            start_time,
                        } = &lease_state;
                        let lease_renewal = LeaseRenewal {
                            start_time: *start_time,
                            lease_time: *ip_address_lease_time.as_ref(),
                            parameters,
                        };
                        Ok(Step::NextState(Transition::BoundWithRenewedLease(
                            Bound::Assigned { lease_state },
                            lease_renewal,
                        )))
                    }
                    RenewingOutcome::NewAddress(lease_state, parameters) => {
                        let LeaseState {
                            discover_options: _,
                            yiaddr,
                            server_identifier: _,
                            ip_address_lease_time,
                            renewal_time: _,
                            rebinding_time: _,
                            start_time,
                        } = &lease_state;
                        let new_lease = NewlyAcquiredLease {
                            ip_address: *yiaddr,
                            start_time: *start_time,
                            lease_time: *ip_address_lease_time.as_ref(),
                            parameters,
                        };
                        Ok(Step::NextState(Transition::BoundWithNewLease(
                            Bound::AwaitingAssignment { lease_state },
                            new_lease,
                        )))
                    }
                    RenewingOutcome::Rebinding(rebinding) => {
                        Ok(Step::NextState(Transition::Rebinding(rebinding)))
                    }
                    RenewingOutcome::Nak(nak) => {
                        // Per RFC 2131 section 3.1: "If the client receives a
                        // DHCPNAK message, the client restarts the
                        // configuration process."

                        let Renewing {
                            lease_state:
                                LeaseState {
                                    discover_options: _,
                                    yiaddr,
                                    server_identifier: _,
                                    ip_address_lease_time: _,
                                    start_time: _,
                                    renewal_time: _,
                                    rebinding_time: _,
                                },
                        } = renewing;
                        log::warn!(
                            "{debug_log_prefix} Dropping lease on {} \
                            and returning to Init due to DHCPNAK: {:?}",
                            yiaddr,
                            nak
                        );
                        Ok(Step::NextState(Transition::Init(Init)))
                    }
                    RenewingOutcome::AddressRemoved(reason) => {
                        Ok(Step::Exit(ExitReason::AddressRemoved(reason)))
                    }
                    RenewingOutcome::AddressRejected(waiting_to_restart) => {
                        Ok(Step::NextState(Transition::WaitingToRestart(waiting_to_restart)))
                    }
                }
            }
            State::Rebinding(rebinding) => {
                match rebinding
                    .do_rebinding(
                        config,
                        udp_socket_provider,
                        packet_socket_provider,
                        clock,
                        stop_receiver,
                        address_event_receiver,
                        counters,
                    )
                    .await?
                {
                    RebindingOutcome::GracefulShutdown => {
                        Ok(Step::Exit(ExitReason::GracefulShutdown))
                    }
                    RebindingOutcome::Renewed(lease_state, parameters) => {
                        let LeaseState {
                            discover_options: _,
                            yiaddr: _,
                            server_identifier: _,
                            ip_address_lease_time,
                            renewal_time: _,
                            rebinding_time: _,
                            start_time,
                        } = &lease_state;
                        let renewal = LeaseRenewal {
                            start_time: *start_time,
                            lease_time: *ip_address_lease_time.as_ref(),
                            parameters,
                        };
                        Ok(Step::NextState(Transition::BoundWithRenewedLease(
                            Bound::Assigned { lease_state },
                            renewal,
                        )))
                    }
                    RebindingOutcome::NewAddress(lease_state, parameters) => {
                        let LeaseState {
                            discover_options: _,
                            yiaddr,
                            server_identifier: _,
                            ip_address_lease_time,
                            renewal_time: _,
                            rebinding_time: _,
                            start_time,
                        } = &lease_state;
                        let new_lease = NewlyAcquiredLease {
                            ip_address: *yiaddr,
                            start_time: *start_time,
                            lease_time: *ip_address_lease_time.as_ref(),
                            parameters,
                        };
                        Ok(Step::NextState(Transition::BoundWithNewLease(
                            Bound::AwaitingAssignment { lease_state },
                            new_lease,
                        )))
                    }
                    RebindingOutcome::Nak(nak) => {
                        // Per RFC 2131 section 3.1: "If the client receives a
                        // DHCPNAK message, the client restarts the
                        // configuration process."

                        let Rebinding {
                            lease_state:
                                LeaseState {
                                    discover_options: _,
                                    yiaddr,
                                    server_identifier: _,
                                    ip_address_lease_time: _,
                                    start_time: _,
                                    renewal_time: _,
                                    rebinding_time: _,
                                },
                        } = rebinding;
                        log::warn!(
                            "{debug_log_prefix} Dropping lease on {} \
                            and returning to Init due to DHCPNAK: {:?}",
                            yiaddr,
                            nak
                        );
                        Ok(Step::NextState(Transition::Init(Init)))
                    }
                    RebindingOutcome::TimedOut => {
                        let Rebinding {
                            lease_state:
                                LeaseState {
                                    discover_options: _,
                                    yiaddr,
                                    server_identifier: _,
                                    ip_address_lease_time: _,
                                    start_time: _,
                                    renewal_time: _,
                                    rebinding_time: _,
                                },
                        } = rebinding;
                        log::warn!(
                            "{debug_log_prefix} Dropping lease on {} \
                            and returning to Init due to lease expiration",
                            yiaddr,
                        );
                        Ok(Step::NextState(Transition::Init(Init)))
                    }
                    RebindingOutcome::AddressRemoved(reason) => {
                        Ok(Step::Exit(ExitReason::AddressRemoved(reason)))
                    }
                    RebindingOutcome::AddressRejected(waiting_to_restart) => {
                        Ok(Step::NextState(Transition::WaitingToRestart(waiting_to_restart)))
                    }
                }
            }
            State::WaitingToRestart(waiting_to_restart) => {
                match waiting_to_restart.do_waiting_to_restart(clock, stop_receiver).await {
                    WaitingToRestartOutcome::GracefulShutdown => {
                        Ok(Step::Exit(ExitReason::GracefulShutdown))
                    }
                    WaitingToRestartOutcome::Init(init) => {
                        Ok(Step::NextState(Transition::Init(init)))
                    }
                }
            }
        }
    }

    /// Provides a human-readable rendition of the state machine state for
    /// exposure in debugging environments.
    pub fn state_name(&self) -> &'static str {
        match self {
            State::Init(_) => "Init",
            State::Selecting(_) => "Selecting",
            State::Requesting(_) => "Requesting",
            State::Bound(Bound::Assigned { .. }) => "Bound and assigned",
            State::Bound(Bound::AwaitingAssignment { .. }) => "Bound and awaiting assignment",
            State::Renewing(_) => "Renewing",
            State::Rebinding(_) => "Rebinding",
            State::WaitingToRestart(_) => "Waiting to Restart",
        }
    }

    fn has_lease(&self) -> bool {
        match self {
            State::Init(_) => false,
            State::Selecting(_) => false,
            State::Requesting(_) => false,
            State::Bound(_) => true,
            State::Renewing(_) => true,
            State::Rebinding(_) => true,
            State::WaitingToRestart(_) => false,
        }
    }

    /// Applies a state-transition to `self`, returning the next state and
    /// effects that need to be performed by bindings as a result of the transition.
    pub fn apply(
        &self,
        config: &ClientConfig,
        transition: Transition<I>,
    ) -> (State<I>, Option<TransitionEffect<I>>) {
        let debug_log_prefix = &config.debug_log_prefix;

        let (next_state, effect) = match transition {
            Transition::Init(init) => (State::Init(init), None),
            Transition::Selecting(selecting) => (State::Selecting(selecting), None),
            Transition::Requesting(requesting) => (State::Requesting(requesting), None),
            Transition::BoundWithRenewedLease(bound, lease_renewal) => {
                (State::Bound(bound), Some(TransitionEffect::HandleRenewedLease(lease_renewal)))
            }
            Transition::BoundWithNewLease(bound, new_lease) => {
                (State::Bound(bound), Some(TransitionEffect::HandleNewLease(new_lease)))
            }
            Transition::BoundAssigned(bound) => (State::Bound(bound), None),
            Transition::Renewing(renewing) => (State::Renewing(renewing), None),
            Transition::Rebinding(rebinding) => (State::Rebinding(rebinding), None),
            Transition::WaitingToRestart(waiting) => (State::WaitingToRestart(waiting), None),
        };

        log::info!(
            "{debug_log_prefix} transitioning from {} to {}",
            self.state_name(),
            next_state.state_name()
        );

        let effect = match effect {
            Some(effect) => Some(effect),
            None => match (self.has_lease(), next_state.has_lease()) {
                (true, false) => {
                    let address_rejected = matches!(next_state, State::WaitingToRestart(..));
                    Some(TransitionEffect::DropLease { address_rejected })
                }
                (false, true) => {
                    unreachable!("should already have decided on TransitionEffect::HandleNewLease")
                }
                (false, false) | (true, true) => None,
            },
        };
        (next_state, effect)
    }
}

/// Takes necessary actions on observing an address rejection (i.e. sends a
/// DHCPDECLINE message).
async fn handle_address_rejection<I: deps::Instant, C: deps::Clock<Instant = I>>(
    config: &ClientConfig,
    lease_state: &LeaseState<I>,
    packet_socket_provider: &impl deps::PacketSocketProvider,
    clock: &C,
) -> Result<WaitingToRestart<I>, Error> {
    let LeaseState {
        discover_options,
        yiaddr,
        server_identifier,
        ip_address_lease_time: _,
        start_time: _,
        renewal_time: _,
        rebinding_time: _,
    } = lease_state;

    let socket = packet_socket_provider.get_packet_socket().await.map_err(Error::Socket)?;
    let message = build_decline(config, discover_options, *yiaddr, *server_identifier);

    // Despite knowing the address of the server, we still send DHCPDECLINE
    // messages via broadcast. See RFC 2131 section 4.4.4:
    //     Because the client is declining the use of the IP address supplied by
    //     the server, the client broadcasts DHCPDECLINE messages.
    socket
        .send_to(
            crate::parse::serialize_dhcp_message_to_ip_packet(
                message,
                Ipv4Addr::UNSPECIFIED,
                CLIENT_PORT,
                Ipv4Addr::BROADCAST,
                SERVER_PORT,
            )
            .as_ref(),
            Mac::BROADCAST,
        )
        .await
        .map_err(Error::Socket)?;

    let debug_log_prefix = config.debug_log_prefix;
    log::info!("{debug_log_prefix} sent DHCPDECLINE for {}; waiting to restart", yiaddr);

    Ok(WaitingToRestart {
        waiting_until: clock.now().add(WAIT_TIME_BEFORE_RESTARTING_AFTER_ADDRESS_REJECTION),
    })
}

impl<I> Default for State<I> {
    fn default() -> Self {
        State::Init(Init::default())
    }
}

impl<I: deps::Instant> diagnostics_traits::InspectableValue for State<I> {
    fn record<II: diagnostics_traits::Inspector>(&self, name: &str, inspector: &mut II) {
        inspector.record_child(name, |inspector| {
            inspector.record_str("Kind", self.state_name());
            match self {
                State::Init(Init) => (),
                State::Selecting(selecting) => {
                    selecting.record(inspector);
                }
                State::Requesting(requesting) => {
                    requesting.record(inspector);
                }
                State::Bound(Bound::Assigned { lease_state }) => {
                    lease_state.record(inspector);
                }
                State::Bound(Bound::AwaitingAssignment { lease_state }) => {
                    lease_state.record(inspector);
                }
                State::Renewing(Renewing { lease_state }) => {
                    lease_state.record(inspector);
                }
                State::Rebinding(Rebinding { lease_state }) => {
                    lease_state.record(inspector);
                }
                State::WaitingToRestart(WaitingToRestart { waiting_until }) => {
                    inspector.record_instant(
                        diagnostics_traits::instant_property_name!("WaitingToRestartUntil"),
                        waiting_until,
                    );
                }
            }
        });
    }
}

/// Debug information to include in log messages about the client.
#[derive(Clone, Copy)]
pub struct DebugLogPrefix {
    /// The numerical interface ID the client is running on.
    pub interface_id: NonZeroU64,
}

impl Display for DebugLogPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { interface_id } = self;
        f.write_fmt(format_args!("(interface_id = {interface_id})"))
    }
}

/// Configuration for the DHCP client to be used while negotiating with DHCP
/// servers.
#[derive(Clone)]
pub struct ClientConfig {
    /// The hardware address of the interface on which the DHCP client is run.
    pub client_hardware_address: Mac,
    /// If set, a unique-on-the-local-network string to be used to identify this
    /// device while negotiating with DHCP servers.
    pub client_identifier:
        Option<AtLeast<2, AtMostBytes<{ dhcp_protocol::U8_MAX_AS_USIZE }, Vec<u8>>>>,
    /// Parameters to request from DHCP servers.
    pub requested_parameters: OptionCodeMap<OptionRequested>,
    /// If set, the preferred IP address lease time in seconds.
    pub preferred_lease_time_secs: Option<NonZeroU32>,
    /// If set, the IP address to request from DHCP servers.
    pub requested_ip_address: Option<SpecifiedAddr<net_types::ip::Ipv4Addr>>,
    /// Debug information to include in log messages about the client.
    pub debug_log_prefix: DebugLogPrefix,
}

#[derive(Clone, Debug, PartialEq, Copy)]
struct DiscoverOptions {
    xid: TransactionId,
}

impl DiscoverOptions {
    fn record(&self, inspector: &mut impl Inspector) {
        let Self { xid: TransactionId(xid) } = self;
        inspector.record_uint("Xid", xid.get());
    }
}

/// Transaction ID for an exchange of DHCP messages.
///
/// Per [RFC 2131], "Transaction ID, a random number chosen by the client, used
/// by the client and server to associate messages and responses between a
/// client and a server."
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.3.1
#[derive(Clone, Copy, Debug, PartialEq)]
struct TransactionId(
    // While the DHCP RFC does not require that the XID be nonzero, it's helpful
    // to maintain that it is nonzero in order to make it clear that it is set
    // while debugging.
    NonZeroU32,
);

/// The initial state as depicted in the state-transition diagram in [RFC 2131].
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Default, Debug, PartialEq, Clone, Copy)]
pub struct Init;

impl Init {
    /// Generates a random transaction ID, records the starting time, and
    /// transitions to Selecting.
    fn do_init<C: deps::Clock>(
        &self,
        rng: &mut impl deps::RngProvider,
        clock: &C,
    ) -> Selecting<C::Instant> {
        let discover_options = DiscoverOptions {
            xid: TransactionId(NonZeroU32::new(rng.get_rng().random_range(1..=u32::MAX)).unwrap()),
        };
        Selecting {
            discover_options,
            // Per RFC 2131 section 4.4.1, "The client records its own local time
            // for later use in computing the lease expiration" when it starts
            // sending DHCPDISCOVERs.
            start_time: clock.now(),
        }
    }
}

/// The state of waiting to restart the configuration process.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaitingToRestart<I> {
    waiting_until: I,
}

#[derive(Debug, PartialEq)]
enum WaitingToRestartOutcome {
    GracefulShutdown,
    Init(Init),
}

impl<I: deps::Instant> WaitingToRestart<I> {
    async fn do_waiting_to_restart<C: deps::Clock<Instant = I>>(
        &self,
        clock: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
    ) -> WaitingToRestartOutcome {
        let Self { waiting_until } = self;
        let wait_fut = clock.wait_until(*waiting_until).fuse();
        let mut wait_fut = pin!(wait_fut);

        select! {
            () = wait_fut => WaitingToRestartOutcome::Init(Init::default()),
            () = stop_receiver.select_next_some() => WaitingToRestartOutcome::GracefulShutdown,
        }
    }
}

fn build_decline(
    client_config: &ClientConfig,
    discover_options: &DiscoverOptions,
    ip_address: SpecifiedAddr<net_types::ip::Ipv4Addr>,
    server_identifier: SpecifiedAddr<net_types::ip::Ipv4Addr>,
) -> dhcp_protocol::Message {
    build_outgoing_message(
        client_config,
        discover_options,
        OutgoingOptions {
            ciaddr: None,
            requested_ip_address: Some(ip_address),
            ip_address_lease_time_secs: None,
            message_type: dhcp_protocol::MessageType::DHCPDECLINE,
            server_identifier: Some(server_identifier),
            include_parameter_request_list: false,
        },
    )
}

async fn send_with_retransmits<T: Clone + Send + Debug>(
    time: &impl deps::Clock,
    retransmit_schedule: impl IntoIterator<Item = Duration>,
    message: &[u8],
    socket: &impl deps::Socket<T>,
    dest: T,
    debug_log_prefix: DebugLogPrefix,
    counters: &MessagingRelatedCounters,
) -> Result<(), Error> {
    send_with_retransmits_at_instants(
        time,
        retransmit_schedule.into_iter().map(|duration| time.now().add(duration)),
        message,
        socket,
        dest,
        debug_log_prefix,
        counters,
    )
    .await
}

async fn send_with_retransmits_at_instants<I: deps::Instant, T: Clone + Send + Debug>(
    time: &impl deps::Clock<Instant = I>,
    retransmit_schedule: impl IntoIterator<Item = I>,
    message: &[u8],
    socket: &impl deps::Socket<T>,
    dest: T,
    debug_log_prefix: DebugLogPrefix,
    counters: &MessagingRelatedCounters,
) -> Result<(), Error> {
    let MessagingRelatedCounters { send_message, recv_time_out, .. } = counters;
    for wait_until in std::iter::once(None).chain(retransmit_schedule.into_iter().map(Some)) {
        if let Some(wait_until) = wait_until {
            time.wait_until(wait_until).await;
            recv_time_out.increment();
        }
        let result = socket.send_to(message, dest.clone()).await;
        match result {
            Ok(()) => {
                send_message.increment();
            }
            Err(e) => match e {
                // We view these errors as non-recoverable, so we bubble them up
                // to bindings:
                deps::SocketError::FailedToOpen(_)
                | deps::SocketError::NoInterface
                | deps::SocketError::NetworkUnreachable
                | deps::SocketError::UnsupportedHardwareType
                | deps::SocketError::AddrNotAvailable
                | deps::SocketError::BrokenPipe
                | deps::SocketError::ConnectionAborted => return Err(Error::Socket(e)),
                // We view EHOSTUNREACH as a recoverable error, as the desired
                // destination could only be temporarily offline, and this does
                // not necessarily indicate an issue with our own network stack.
                // Log a warning and continue retransmitting.
                deps::SocketError::HostUnreachable => {
                    log::warn!("{debug_log_prefix} destination host unreachable: {:?}", dest);
                }
                // For errors that we don't recognize, default to logging an
                // error and continuing to operate.
                deps::SocketError::Other(_) => {
                    log::error!(
                        "{debug_log_prefix} socket error while sending to {:?}: {:?}",
                        dest,
                        e
                    );
                }
            },
        }
    }
    Ok(())
}

fn retransmit_schedule_during_acquisition(
    rng: &mut (impl rand::Rng + ?Sized),
) -> impl Iterator<Item = Duration> + '_ {
    const MILLISECONDS_PER_SECOND: i32 = 1000;
    [4i32, 8, 16, 32]
        .into_iter()
        .chain(std::iter::repeat(64))
        // Per RFC 2131 Section 4.3.1, "the delay before the first
        // retransmission SHOULD be 4 seconds randomized by the value of a
        // uniform random number chosen from the range -1 to +1.  [...] The
        // delay before the next retransmission SHOULD be 8 seconds randomized
        // by the value of a uniform number chosen from the range -1 to +1.  The
        // retransmission delay SHOULD be doubled with subsequent
        // retransmissions up to a maximum of 64 seconds."
        .zip(std::iter::from_fn(|| {
            Some(rng.random_range((-MILLISECONDS_PER_SECOND)..=MILLISECONDS_PER_SECOND))
        }))
        .map(|(base_seconds, jitter_millis)| {
            let millis = u64::try_from(base_seconds * MILLISECONDS_PER_SECOND + jitter_millis)
                .expect("retransmit wait is never negative");
            Duration::from_millis(millis)
        })
}

// This is assumed to be an appropriate buffer size due to Ethernet's common MTU
// of 1500 bytes.
const BUFFER_SIZE: usize = 1500;

fn recv_stream<'a, T: 'a, U: Send>(
    socket: &'a impl deps::Socket<U>,
    recv_buf: &'a mut [u8],
    parser: impl Fn(&[u8], U) -> T + 'a,
    debug_log_prefix: DebugLogPrefix,
    counters: &'a MessagingRelatedCounters,
) -> impl Stream<Item = Result<T, Error>> + 'a {
    let MessagingRelatedCounters {
        recv_message,
        recv_message_fatal_socket_error,
        recv_message_non_fatal_socket_error,
        ..
    } = counters;
    futures::stream::try_unfold((recv_buf, parser), move |(recv_buf, parser)| async move {
        let result = socket.recv_from(recv_buf).await;
        let DatagramInfo { length, address } = match result {
            Ok(datagram_info) => {
                recv_message.increment();
                datagram_info
            }
            Err(e) => match e {
                // We view these errors as non-recoverable, so we bubble them up
                // to bindings:
                deps::SocketError::FailedToOpen(_)
                | deps::SocketError::NoInterface
                | deps::SocketError::NetworkUnreachable
                | deps::SocketError::UnsupportedHardwareType
                | deps::SocketError::AddrNotAvailable
                | deps::SocketError::BrokenPipe
                | deps::SocketError::ConnectionAborted => {
                    recv_message_fatal_socket_error.increment();
                    return Err(Error::Socket(e));
                }
                // We view EHOSTUNREACH as a recoverable error, as the server
                // we're communicating with could only be temporarily offline,
                // and this does not indicate an issue with our own network
                // stack. Log a warning and continue operating.
                // (While it seems like this ought not to be relevant while
                // receiving, there are instances where this could be observed,
                // like when IP_RECVERR is set on the socket and link resolution
                // fails, or as a result of an ICMP message.)
                deps::SocketError::HostUnreachable => {
                    log::warn!("{debug_log_prefix} EHOSTUNREACH from recv_from");
                    recv_message_non_fatal_socket_error.increment();
                    return Ok(Some((None, (recv_buf, parser))));
                }
                // For errors that we don't recognize, default to logging an
                // error and continuing to operate.
                deps::SocketError::Other(_) => {
                    log::error!("{debug_log_prefix} socket error while receiving: {:?}", e);
                    recv_message_non_fatal_socket_error.increment();
                    return Ok(Some((None, (recv_buf, parser))));
                }
            },
        };
        let raw_msg = &recv_buf[..length];
        let parsed = parser(raw_msg, address);
        Ok(Some((Some(parsed), (recv_buf, parser))))
    })
    .try_filter_map(|item| futures::future::ok(item))
}

struct OutgoingOptions {
    ciaddr: Option<SpecifiedAddr<net_types::ip::Ipv4Addr>>,
    requested_ip_address: Option<SpecifiedAddr<net_types::ip::Ipv4Addr>>,
    ip_address_lease_time_secs: Option<NonZeroU32>,
    message_type: dhcp_protocol::MessageType,
    server_identifier: Option<SpecifiedAddr<net_types::ip::Ipv4Addr>>,
    include_parameter_request_list: bool,
}

fn build_outgoing_message(
    ClientConfig {
        client_hardware_address,
        client_identifier,
        requested_parameters,
        preferred_lease_time_secs: _,
        requested_ip_address: _,
        debug_log_prefix: _,
    }: &ClientConfig,
    DiscoverOptions { xid: TransactionId(xid) }: &DiscoverOptions,
    OutgoingOptions {
        ciaddr,
        requested_ip_address,
        ip_address_lease_time_secs,
        message_type,
        server_identifier,
        include_parameter_request_list,
    }: OutgoingOptions,
) -> dhcp_protocol::Message {
    use dhcp_protocol::DhcpOption;

    dhcp_protocol::Message {
        op: dhcp_protocol::OpCode::BOOTREQUEST,
        xid: xid.get(),
        secs: 0,
        bdcast_flag: false,
        ciaddr: ciaddr.map(|ip| ip.get().into()).unwrap_or(Ipv4Addr::UNSPECIFIED),
        yiaddr: Ipv4Addr::UNSPECIFIED,
        siaddr: Ipv4Addr::UNSPECIFIED,
        giaddr: Ipv4Addr::UNSPECIFIED,
        chaddr: *client_hardware_address,
        sname: BString::default(),
        file: BString::default(),
        // NB: The DHCP Options RFC (RFC 2132) does not impose any requirements
        // on the order of options.  However, there is an industry norm to place
        // the DHCP Message Type first.
        //
        // For the sake of maximum compliance with DHCP Servers in the wild,
        // we write our options in the same order as Netstack2 did:
        // * Message Type,
        // * Requested Parameters,
        // * Server Identifier,
        // * Requested IP Address,
        //
        // Note, NS2 didn't support any other options, so we tag all remaining
        // options on the end of the list.
        options: [
            Some(DhcpOption::DhcpMessageType(message_type)),
            include_parameter_request_list
                .then(|| requested_parameters.try_to_parameter_request_list())
                .flatten()
                .map(DhcpOption::ParameterRequestList),
            server_identifier.map(|ip| DhcpOption::ServerIdentifier(ip.get().into())),
            requested_ip_address.map(|ip| DhcpOption::RequestedIpAddress(ip.get().into())),
            ip_address_lease_time_secs.map(|time| DhcpOption::IpAddressLeaseTime(time.get())),
            client_identifier.clone().map(DhcpOption::ClientIdentifier),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

fn build_discover(
    client_config: &ClientConfig,
    discover_options: &DiscoverOptions,
) -> dhcp_protocol::Message {
    let ClientConfig {
        client_hardware_address: _,
        client_identifier: _,
        requested_parameters: _,
        preferred_lease_time_secs,
        requested_ip_address,
        debug_log_prefix: _,
    } = client_config;

    // Per the table in RFC 2131 section 4.4.1:
    //
    // 'ciaddr'                 0 (DHCPDISCOVER)
    // Requested IP Address     MAY (DISCOVER)
    // IP address lease time    MAY
    // DHCP message type        DHCPDISCOVER
    // Server Identifier        MUST NOT
    // Parameter Request List   MAY
    build_outgoing_message(
        client_config,
        discover_options,
        OutgoingOptions {
            ciaddr: None,
            requested_ip_address: *requested_ip_address,
            ip_address_lease_time_secs: *preferred_lease_time_secs,
            message_type: dhcp_protocol::MessageType::DHCPDISCOVER,
            server_identifier: None,
            include_parameter_request_list: true,
        },
    )
}

// Returns Ok(Some) if a DHCP message was successfully parsed, Ok(None) if the
// IP packet should just be discarded (but does not indicate an error), and
// Err if the IP packet indicates that some error should be logged.
fn parse_incoming_dhcp_message_from_ip_packet(
    packet: &[u8],
    debug_log_prefix: DebugLogPrefix,
) -> Result<Option<(net_types::ip::Ipv4Addr, dhcp_protocol::Message)>, anyhow::Error> {
    match crate::parse::parse_dhcp_message_from_ip_packet(packet, CLIENT_PORT) {
        Ok(message) => Ok(Some(message)),
        Err(err) => match err {
            crate::parse::ParseError::NotUdp => {
                log::debug!("{debug_log_prefix} ignoring non-UDP incoming packet");
                return Ok(None);
            }
            crate::parse::ParseError::WrongPort(port) => {
                log::debug!(
                    "{debug_log_prefix} ignoring incoming UDP packet \
                    to non-DHCP-client port {port}"
                );
                return Ok(None);
            }
            err @ (crate::parse::ParseError::Ipv4(_)
            | crate::parse::ParseError::Udp(_)
            | crate::parse::ParseError::WrongSource(_)
            | crate::parse::ParseError::Dhcp(_)) => {
                return Err(err).context("error while parsing DHCP message from IP packet");
            }
        },
    }
}

#[derive(Debug)]
pub(crate) enum SelectingOutcome<I> {
    GracefulShutdown,
    Requesting(Requesting<I>),
}

/// The Selecting state as depicted in the state-transition diagram in [RFC 2131].
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Debug, Clone, Copy)]
pub struct Selecting<I> {
    discover_options: DiscoverOptions,
    // The time at which the DHCP transaction was initiated (used as the offset
    // from which lease expiration times are computed).
    start_time: I,
}

impl<I: deps::Instant> Selecting<I> {
    /// Executes the Selecting state.
    ///
    /// Transmits (and retransmits, if necessary) DHCPDISCOVER messages, and
    /// receives DHCPOFFER messages, on a packet socket. Tries to select a
    /// DHCPOFFER. If successful, transitions to Requesting.
    async fn do_selecting<C: deps::Clock<Instant = I>>(
        &self,
        client_config: &ClientConfig,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        rng: &mut impl deps::RngProvider,
        time: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        counters: &Counters,
    ) -> Result<SelectingOutcome<I>, Error> {
        let Counters { selecting: SelectingCounters { messaging, recv_error, .. }, .. } = counters;
        // TODO(https://fxbug.dev/42075580): avoid dropping/recreating the packet
        // socket unnecessarily by taking an `&impl
        // deps::Socket<net_types::ethernet::Mac>` here instead.
        let socket = packet_socket_provider.get_packet_socket().await.map_err(Error::Socket)?;
        let Selecting { discover_options, start_time } = self;
        let message = build_discover(client_config, discover_options);

        let ClientConfig {
            client_hardware_address: _,
            client_identifier: _,
            requested_parameters,
            preferred_lease_time_secs: _,
            requested_ip_address: _,
            debug_log_prefix,
        } = client_config;

        let message = crate::parse::serialize_dhcp_message_to_ip_packet(
            message,
            Ipv4Addr::UNSPECIFIED, // src_ip
            CLIENT_PORT,
            Ipv4Addr::BROADCAST, // dst_ip
            SERVER_PORT,
        );

        let mut send_fut = pin!(
            send_with_retransmits(
                time,
                retransmit_schedule_during_acquisition(rng.get_rng()),
                message.as_ref(),
                &socket,
                /* dest= */ Mac::BROADCAST,
                *debug_log_prefix,
                messaging
            )
            .fuse()
        );

        let mut recv_buf = [0u8; BUFFER_SIZE];
        let mut offer_fields_stream = pin!(
            recv_stream(
                &socket,
                &mut recv_buf,
                |packet, src_addr| {
                    // We don't care about the src addr of incoming offers, because we
                    // identify DHCP servers via the Server Identifier option.
                    let _: Mac = src_addr;

                    let (src_addr, message) =
                        match parse_incoming_dhcp_message_from_ip_packet(packet, *debug_log_prefix)
                            .inspect_err(|_| messaging.recv_failed_dhcp_parse.increment())?
                        {
                            Some(message) => message,
                            None => return Ok(None),
                        };
                    validate_message(discover_options, client_config, &message)
                        .inspect_err(|e| e.increment(&messaging))
                        .context("invalid DHCP message")?;
                    crate::parse::fields_to_retain_from_selecting(requested_parameters, message)
                        .inspect_err(|e| recv_error.increment(&e))
                        .map(|(fields, soft_errors)| {
                            messaging.increment_soft_errors(soft_errors);
                            Some((src_addr, fields))
                        })
                        .context(
                            "error while retrieving fields to use in DHCPREQUEST from DHCP message",
                        )
                },
                *debug_log_prefix,
                messaging
            )
            .try_filter_map(|parse_result| {
                futures::future::ok(match parse_result {
                    Ok(fields) => fields,
                    Err(error) => {
                        log::warn!("{debug_log_prefix} discarding incoming packet: {:?}", error);
                        None
                    }
                })
            })
            .fuse()
        );

        select_biased! {
            fields_to_use_in_request_result = offer_fields_stream.select_next_some() => {
                let (src_addr, fields_from_offer_to_use_in_request) =
                    fields_to_use_in_request_result?;
                log_offer(&debug_log_prefix, &src_addr, &fields_from_offer_to_use_in_request);
                // Currently, we take the naive approach of accepting the first
                // DHCPOFFER we see without doing any special selection logic.
                Ok(SelectingOutcome::Requesting(Requesting {
                    discover_options: discover_options.clone(),
                    fields_from_offer_to_use_in_request,
                    start_time: *start_time,
                }))
            },
            () = stop_receiver.select_next_some() => {
                Ok(SelectingOutcome::GracefulShutdown)
            },
            send_discovers_result = send_fut => {
                send_discovers_result?;
                unreachable!("should never stop retransmitting DHCPDISCOVER unless we hit an error");
            }
        }
    }
}

impl<I: deps::Instant> Selecting<I> {
    fn record(&self, inspector: &mut impl Inspector) {
        let Self { discover_options, start_time } = self;
        inspector.record_instant(diagnostics_traits::instant_property_name!("Start"), start_time);
        discover_options.record(inspector);
    }
}

#[derive(thiserror::Error, Debug, PartialEq)]
enum ValidateMessageError {
    #[error("xid {actual} doesn't match expected xid {expected}")]
    WrongXid { expected: u32, actual: u32 },
    #[error("chaddr {actual} doesn't match expected chaddr {expected}")]
    WrongChaddr { expected: Mac, actual: Mac },
}

impl ValidateMessageError {
    fn increment(&self, counters: &MessagingRelatedCounters) {
        match self {
            ValidateMessageError::WrongXid { .. } => counters.recv_wrong_xid.increment(),
            ValidateMessageError::WrongChaddr { .. } => counters.recv_wrong_chaddr.increment(),
        }
    }
}

fn validate_message(
    DiscoverOptions { xid: TransactionId(my_xid) }: &DiscoverOptions,
    ClientConfig {
        client_hardware_address: my_chaddr,
        client_identifier: _,
        requested_parameters: _,
        preferred_lease_time_secs: _,
        requested_ip_address: _,
        debug_log_prefix: _,
    }: &ClientConfig,
    dhcp_protocol::Message {
        op: _,
        xid: msg_xid,
        secs: _,
        bdcast_flag: _,
        ciaddr: _,
        yiaddr: _,
        siaddr: _,
        giaddr: _,
        chaddr: msg_chaddr,
        sname: _,
        file: _,
        options: _,
    }: &dhcp_protocol::Message,
) -> Result<(), ValidateMessageError> {
    if *msg_xid != u32::from(*my_xid) {
        return Err(ValidateMessageError::WrongXid { expected: my_xid.get(), actual: *msg_xid });
    }

    if msg_chaddr != my_chaddr {
        return Err(ValidateMessageError::WrongChaddr {
            expected: *my_chaddr,
            actual: *msg_chaddr,
        });
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub(crate) enum RequestingOutcome<I> {
    RanOutOfRetransmits,
    GracefulShutdown,
    Bound(LeaseState<I>, Vec<dhcp_protocol::DhcpOption>),
    Nak(FieldsToRetainFromNak),
}

/// The Requesting state as depicted in the state-transition diagram in [RFC 2131].
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Requesting<I> {
    discover_options: DiscoverOptions,
    fields_from_offer_to_use_in_request: crate::parse::FieldsFromOfferToUseInRequest,
    start_time: I,
}

// Per RFC 2131, section 3.1: "a client retransmitting as described in section
// 4.1 might retransmit the DHCPREQUEST message four times, for a total delay of
// 60 seconds, before restarting the initialization procedure".
const NUM_REQUEST_RETRANSMITS: usize = 4;

/// The default lease time of an IP Address if none was present in the DHCPACK.
///
/// RFC 2131 section 4.3.1 states that DHCP Servers MUST explicitly provide the
/// "IP address lease time" option when sending a DHCPACK in response to a
/// DHCPREQUEST:
///
///   Option                    DHCPOFFER    DHCPACK            DHCPNAK
///   ------                    ---------    -------            -------
///   ...
///   IP address lease time     MUST         MUST (DHCPREQUEST) MUST NOT
///
/// However, we've observed DHCP servers in the field that fail to set the lease
/// time (https://fxbug.dev/486403240).
///
/// The RFC does not specify how a client should handle a DHCPACK that omits the
/// lease time. After sampling several DHCP Client implementations it's clear
/// that the community is split on the best approach.
/// * ISC's dhclient chooses to reject the ACK and restart the client.
/// * FreeBSD's dhclient chooses to fill in a default lease time (12 hours).
/// * Netstack2's DHCP client (this implementations predecessor) choose to fill
///   in a default lease time (12 hours).
///
/// For the sake of compatibility with such servers, we've chosen to fill in a
/// default lease time.
const DEFAULT_LEASE_TIME: Duration = Duration::from_hours(12);

impl<I: deps::Instant> Requesting<I> {
    /// Executes the Requesting state.
    ///
    /// Transmits (and retransmits, if necessary) DHCPREQUEST messages, and
    /// receives DHCPACK and DHCPNAK messages, on a packet socket. Upon
    /// receiving a DHCPACK, transitions to Bound.
    async fn do_requesting<C: deps::Clock<Instant = I>>(
        &self,
        client_config: &ClientConfig,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        rng: &mut impl deps::RngProvider,
        time: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        counters: &Counters,
    ) -> Result<RequestingOutcome<I>, Error> {
        let Counters {
            requesting: RequestingCounters { messaging, recv_error, recv_nak, .. }, ..
        } = counters;
        let socket = packet_socket_provider.get_packet_socket().await.map_err(Error::Socket)?;
        let Requesting { discover_options, fields_from_offer_to_use_in_request, start_time } = self;
        let message = build_request_during_address_acquisition(
            client_config,
            discover_options,
            fields_from_offer_to_use_in_request,
        );

        let message = crate::parse::serialize_dhcp_message_to_ip_packet(
            message,
            Ipv4Addr::UNSPECIFIED, // src_ip
            CLIENT_PORT,
            Ipv4Addr::BROADCAST, // dst_ip
            SERVER_PORT,
        );

        let ClientConfig {
            client_hardware_address: _,
            client_identifier: _,
            requested_parameters,
            preferred_lease_time_secs: _,
            requested_ip_address: _,
            debug_log_prefix,
        } = client_config;

        let mut send_fut = pin!(
            send_with_retransmits(
                time,
                retransmit_schedule_during_acquisition(rng.get_rng()).take(NUM_REQUEST_RETRANSMITS),
                message.as_ref(),
                &socket,
                Mac::BROADCAST,
                *debug_log_prefix,
                messaging
            )
            .fuse()
        );

        let mut recv_buf = [0u8; BUFFER_SIZE];

        let mut ack_or_nak_stream = pin!(
            recv_stream(
                &socket,
                &mut recv_buf,
                |packet, src_addr| {
                    // We don't care about the src addr of incoming messages, because we
                    // identify DHCP servers via the Server Identifier option.
                    let _: Mac = src_addr;
                    let (src_addr, message) =
                        match parse_incoming_dhcp_message_from_ip_packet(packet, *debug_log_prefix)
                            .inspect_err(|_| {
                                messaging.recv_failed_dhcp_parse.increment();
                            })? {
                            Some(message) => message,
                            None => return Ok(None),
                        };
                    validate_message(discover_options, client_config, &message)
                        .inspect_err(|e| e.increment(&messaging))
                        .context("invalid DHCP message")?;

                    crate::parse::fields_to_retain_from_response_to_request(
                        requested_parameters,
                        message,
                    )
                    .inspect_err(|e| recv_error.increment(e))
                    .context("error extracting needed fields from DHCP message during Requesting")
                    .map(|(fields, soft_errors)| {
                        messaging.increment_soft_errors(soft_errors);
                        fields
                    })
                    .map(|fields| {
                        // If the DHCP Server did not provide an IP Address
                        // Lease Time in the DHCPACK, this is in violation of
                        // the RFC, but we accept the ACK regardless for
                        // compatibility and fall back to the lease length from
                        // the offer if one was included, or
                        // `DEFAULT_LEASE_TIME` if not.
                        let fallback_lease_time = fields_from_offer_to_use_in_request
                            .ip_address_lease_time_secs
                            .map(|t| Duration::from_secs(t.get().into()));
                        let (validated, soft_errors) =
                            validate_response_to_request(fields, fallback_lease_time);
                        soft_errors.increment(&messaging);

                        Some((src_addr, validated))
                    })
                },
                *debug_log_prefix,
                messaging
            )
            .try_filter_map(|parse_result| {
                futures::future::ok(match parse_result {
                    Ok(msg) => msg,
                    Err(error) => {
                        log::warn!("{debug_log_prefix} discarding incoming packet: {:?}", error);
                        None
                    }
                })
            })
            .fuse()
        );

        let (src_addr, validated) = select_biased! {
            validated_result = ack_or_nak_stream.select_next_some() => {
                validated_result?
            },
            () = stop_receiver.select_next_some() => {
                return Ok(RequestingOutcome::GracefulShutdown)
            },
            send_requests_result = send_fut => {
                send_requests_result?;
                messaging.recv_time_out.increment();
                return Ok(RequestingOutcome::RanOutOfRetransmits)
            }
        };

        match validated {
            ValidatedResponse::Ack { ack, lease_time, renewal_time, rebinding_time } => {
                log_ack(&debug_log_prefix, &src_addr, &ack);
                let FieldsToRetainFromAck {
                    yiaddr,
                    server_identifier,
                    ip_address_lease_time_secs: _,
                    renewal_time_value_secs: _,
                    rebinding_time_value_secs: _,
                    parameters,
                } = ack;
                let server_identifier = server_identifier.unwrap_or({
                    let crate::parse::FieldsFromOfferToUseInRequest {
                        server_identifier,
                        ip_address_lease_time_secs: _,
                        ip_address_to_request: _,
                    } = fields_from_offer_to_use_in_request;
                    *server_identifier
                });

                match lease_time {
                    RelativeTime::Explicit(_) => {}
                    r @ (RelativeTime::Previous(_) | RelativeTime::Default(_)) => {
                        messaging.recv_ack_no_addr_lease_time.increment();
                        log::warn!(
                            "{debug_log_prefix} accepting DHCPACK from {src_addr} that did \
                            not provide an IP Address Lease Time. Using {r:?} instead."
                        );
                    }
                }

                Ok(RequestingOutcome::Bound(
                    LeaseState {
                        discover_options: discover_options.clone(),
                        yiaddr,
                        server_identifier,
                        ip_address_lease_time: lease_time,
                        renewal_time,
                        rebinding_time,
                        start_time: *start_time,
                    },
                    parameters,
                ))
            }
            ValidatedResponse::Nak(nak) => {
                log_nak(&debug_log_prefix, &src_addr, &nak);
                recv_nak.increment();
                Ok(RequestingOutcome::Nak(nak))
            }
        }
    }

    fn record(&self, inspector: &mut impl Inspector) {
        let Self { discover_options, start_time, fields_from_offer_to_use_in_request } = self;
        inspector.record_instant(diagnostics_traits::instant_property_name!("Start"), start_time);
        discover_options.record(inspector);
        fields_from_offer_to_use_in_request.record(inspector);
    }
}

fn build_request_during_address_acquisition(
    client_config: &ClientConfig,
    discover_options: &DiscoverOptions,
    crate::parse::FieldsFromOfferToUseInRequest {
        server_identifier,
        ip_address_lease_time_secs,
        ip_address_to_request,
    }: &crate::parse::FieldsFromOfferToUseInRequest,
) -> dhcp_protocol::Message {
    let ClientConfig {
        client_hardware_address: _,
        client_identifier: _,
        requested_parameters: _,
        preferred_lease_time_secs,
        requested_ip_address: _,
        debug_log_prefix: _,
    } = client_config;

    // Per the table in RFC 2131 section 4.4.1:
    //
    // 'ciaddr'                 0 or client's network address (currently 0)
    // Requested IP Address     MUST (in SELECTING)
    // IP address lease time    MAY
    // DHCP message type        DHCPREQUEST
    // Server Identifier        MUST (after SELECTING)
    // Parameter Request List   MAY
    build_outgoing_message(
        client_config,
        discover_options,
        OutgoingOptions {
            ciaddr: None,
            requested_ip_address: Some(*ip_address_to_request),
            ip_address_lease_time_secs: ip_address_lease_time_secs.or(*preferred_lease_time_secs),
            message_type: dhcp_protocol::MessageType::DHCPREQUEST,
            server_identifier: Some(*server_identifier),
            include_parameter_request_list: true,
        },
    )
}

/// A newly-acquired DHCP lease.
#[derive(Debug, PartialEq)]
pub struct NewlyAcquiredLease<I> {
    /// The IP address acquired.
    pub ip_address: SpecifiedAddr<net_types::ip::Ipv4Addr>,
    /// The start time of the lease.
    pub start_time: I,
    /// The length of the lease.
    pub lease_time: Duration,
    /// Configuration parameters acquired from the server. Guaranteed to be a
    /// subset of the parameters requested in the `parameter_request_list` in
    /// `ClientConfig`.
    pub parameters: Vec<dhcp_protocol::DhcpOption>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum BoundOutcome<I, R> {
    GracefulShutdown,
    Renewing(Renewing<I>),
    Restart(Init),
    AddressRemoved(R),
    AddressRejected(WaitingToRestart<I>),
    Assigned(Bound<I>),
}

/// The Bound state as depicted in the state-transition diagram in [RFC 2131].
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Bound<I> {
    /// The address is undergoing Duplicate Address Detection and is not yet
    /// assigned.
    AwaitingAssignment {
        /// State about the lease.
        lease_state: LeaseState<I>,
    },
    /// The address has finished Duplicate Address Detection and is now
    /// available for use.
    Assigned {
        /// State about the lease.
        lease_state: LeaseState<I>,
    },
}

/// The state for an acquired lease held by the DHCP state machine.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct LeaseState<I> {
    discover_options: DiscoverOptions,
    yiaddr: SpecifiedAddr<net_types::ip::Ipv4Addr>,
    server_identifier: SpecifiedAddr<net_types::ip::Ipv4Addr>,
    ip_address_lease_time: RelativeTime,
    start_time: I,
    renewal_time: RelativeTime,
    rebinding_time: RelativeTime,
}

impl<I: deps::Instant> Bound<I> {
    async fn do_bound<C: deps::Clock<Instant = I>, R>(
        &self,
        client_config: &ClientConfig,
        time: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
        // The Bound state currently doesn't need to increment any counters,
        // because all it does is wait.
        _counters: &Counters,
    ) -> Result<BoundOutcome<I, R>, Error> {
        match self {
            Bound::AwaitingAssignment { lease_state } => {
                do_bound_awaiting_assignment(
                    lease_state,
                    client_config,
                    time,
                    stop_receiver,
                    packet_socket_provider,
                    address_event_receiver,
                )
                .await
            }
            Bound::Assigned { lease_state } => {
                do_bound_assigned(
                    lease_state,
                    client_config,
                    time,
                    stop_receiver,
                    packet_socket_provider,
                    address_event_receiver,
                )
                .await
            }
        }
    }
}

async fn do_bound_awaiting_assignment<I: deps::Instant, C: deps::Clock<Instant = I>, R>(
    lease_state: &LeaseState<I>,
    client_config: &ClientConfig,
    time: &C,
    stop_receiver: &mut mpsc::UnboundedReceiver<()>,
    packet_socket_provider: &impl deps::PacketSocketProvider,
    address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
) -> Result<BoundOutcome<I, R>, Error> {
    let LeaseState {
        discover_options: _,
        yiaddr,
        server_identifier: _,
        ip_address_lease_time,
        start_time,
        renewal_time: _,
        rebinding_time: _,
    } = lease_state;

    // Wait for the duration of the lease for the address to become assigned.
    //
    // Note: depending on how long it takes for the address to
    // become assigned, we may only exit `Bound::AwaitingAssigned`
    // after T1 (renewing time) and T2 (rebinding time). Because all
    // of the timeout calculations in this mod are based off of
    // `start_time`, we'll simply wait for times in the past, which
    // will allow the client to proceed to the renewing/rebinding
    // phases without waiting.
    let lease_timeout_fut = time.wait_until(start_time.add(*ip_address_lease_time.as_ref())).fuse();
    let mut lease_timeout_fut = pin!(lease_timeout_fut);

    let mut address_removed_or_assigned = pin!(address_event_receiver.filter_map(async |event| {
        match event {
            AddressEvent::Rejected => {
                handle_address_rejection(client_config, lease_state, packet_socket_provider, time)
                    .map(|result| Some(result.map(BoundOutcome::AddressRejected)))
                    .await
            }
            AddressEvent::Removed(reason) => Some(Ok(BoundOutcome::AddressRemoved(reason))),
            AddressEvent::AssignmentStateChanged(new_state) => {
                match new_state {
                    AddressAssignmentState::Assigned => {
                        Some(Ok(BoundOutcome::Assigned(Bound::Assigned {
                            lease_state: lease_state.clone(),
                        })))
                    }
                    // TODO(https://fxbug.dev/421941195): Handle addresses
                    // becoming unavailable.
                    s @ AddressAssignmentState::Tentative
                    | s @ AddressAssignmentState::Unavailable => {
                        log::warn!(
                            "{yiaddr} address state became {s:?} during \
                            Bound::AwaitingAssignment; ignoring"
                        );
                        None
                    }
                }
            }
        }
    }));

    let debug_log_prefix = &client_config.debug_log_prefix;
    select! {
        // Note: We won't be sending a DHCPDECLINE, and therefore
        // can restart immediately (rather than waiting for
        // `WAIT_TIME_BEFORE_RESTARTING_AFTER_ADDRESS_REJECTION`).
        () = lease_timeout_fut => {
            log::info!(
                "{debug_log_prefix} Returning to Init due to failing to observe address \
                assignment for {yiaddr} before the lease ({ip_address_lease_time:?}) \
                expired."
            );
            Ok(BoundOutcome::Restart(Init::default()))
        },
        () = stop_receiver.select_next_some() => Ok(BoundOutcome::GracefulShutdown),
        result = address_removed_or_assigned.next() => {
            result.unwrap_or(Err(Error::AddressEventReceiverEnded))
        }
    }
}

async fn do_bound_assigned<I: deps::Instant, C: deps::Clock<Instant = I>, R>(
    lease_state: &LeaseState<I>,
    client_config: &ClientConfig,
    time: &C,
    stop_receiver: &mut mpsc::UnboundedReceiver<()>,
    packet_socket_provider: &impl deps::PacketSocketProvider,
    address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
) -> Result<BoundOutcome<I, R>, Error> {
    let LeaseState {
        discover_options: _,
        yiaddr,
        server_identifier,
        ip_address_lease_time,
        start_time,
        renewal_time,
        rebinding_time: _,
    } = lease_state;

    let renewal_time = *renewal_time.as_ref();

    let debug_log_prefix = &client_config.debug_log_prefix;
    log::info!(
        "{debug_log_prefix} In Bound state; ip_address_lease_time = {}, \
        renewal_time = {}, server_identifier = {server_identifier}",
        ip_address_lease_time.as_ref().as_secs(),
        renewal_time.as_secs(),
    );

    let renewal_timeout_fut = time.wait_until(start_time.add(renewal_time)).fuse();
    let mut renewal_timeout_fut = pin!(renewal_timeout_fut);

    let mut address_removed = pin!(address_event_receiver.filter_map(async |event| match event {
        AddressEvent::Rejected => {
            handle_address_rejection(client_config, lease_state, packet_socket_provider, time)
                .map(|result| Some(result.map(BoundOutcome::AddressRejected)))
                .await
        }
        AddressEvent::Removed(reason) => Some(Ok(BoundOutcome::AddressRemoved(reason))),
        AddressEvent::AssignmentStateChanged(new_state) => {
            // TODO(https://fxbug.dev/421941195): Handle addresses
            // becoming unavailable.
            log::warn!(
                "{yiaddr} address state became {new_state:?} during Bound::Assigned; ignoring"
            );
            None
        }
    }));

    select! {
        () = renewal_timeout_fut => Ok(BoundOutcome::Renewing(Renewing {
                lease_state: lease_state.clone()
            })),

        () = stop_receiver.select_next_some() => Ok(BoundOutcome::GracefulShutdown),
        result = address_removed.next() => result.unwrap_or(Err(Error::AddressEventReceiverEnded)),
    }
}

impl<I: deps::Instant> LeaseState<I> {
    fn record(&self, inspector: &mut impl Inspector) {
        let Self {
            discover_options,
            yiaddr,
            server_identifier,
            ip_address_lease_time,
            start_time,
            renewal_time,
            rebinding_time,
        } = self;
        inspector.record_instant(diagnostics_traits::instant_property_name!("Start"), start_time);
        discover_options.record(inspector);
        inspector.record_ip_addr("Yiaddr", **yiaddr);
        inspector.record_ip_addr("ServerIdentifier", **server_identifier);
        ip_address_lease_time.record(
            inspector,
            "IpAddressLeaseTimeSecs",
            "IpAddressLeaseTimeSource",
        );
        renewal_time.record(inspector, "RenewalTimeSecs", "RenewalTimeSource");
        rebinding_time.record(inspector, "RebindingTimeSecs", "RebindingTimeSource");
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum RenewingOutcome<I, R> {
    GracefulShutdown,
    Renewed(LeaseState<I>, Vec<dhcp_protocol::DhcpOption>),
    // It might be surprising to see that it's possible to yield a _new_ address
    // while renewing, but per RFC 2131 section 4.4.5, "if the client is given a
    // new network address, it MUST NOT continue using the previous network
    // address and SHOULD notify the local users of the problem." This suggests
    // that we should be prepared for a DHCP server to send us a different
    // address from the one we asked for while renewing.
    NewAddress(LeaseState<I>, Vec<dhcp_protocol::DhcpOption>),
    Nak(FieldsToRetainFromNak),
    Rebinding(Rebinding<I>),
    AddressRemoved(R),
    AddressRejected(WaitingToRestart<I>),
}

/// The Renewing state as depicted in the state-transition diagram in [RFC 2131].
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Renewing<I> {
    lease_state: LeaseState<I>,
}

// Per RFC 2131 section 4.4.5: "In both RENEWING and REBINDING states,
// if the client receives no response to its DHCPREQUEST message, the
// client SHOULD wait one-half of the remaining time until T2 (in
// RENEWING state) and one-half of the remaining lease time (in
// REBINDING state), down to a minimum of 60 seconds, before
// retransmitting the DHCPREQUEST message."
const RENEW_RETRANSMIT_MINIMUM_DELAY: Duration = Duration::from_secs(60);

impl<I: deps::Instant> Renewing<I> {
    async fn do_renewing<C: deps::Clock<Instant = I>, R>(
        &self,
        client_config: &ClientConfig,
        // TODO(https://fxbug.dev/42075580): avoid dropping/recreating the packet
        // socket unnecessarily by taking an `&impl
        // deps::Socket<std::net::SocketAddr>` here instead.
        udp_socket_provider: &impl deps::UdpSocketProvider,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        time: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
        counters: &Counters,
    ) -> Result<RenewingOutcome<I, R>, Error> {
        let Counters { renewing: RenewingCounters { messaging, recv_error, recv_nak, .. }, .. } =
            counters;
        let renewal_start_time = time.now();
        let debug_log_prefix = client_config.debug_log_prefix;

        let Self {
            lease_state:
                lease_state @ LeaseState {
                    discover_options,
                    yiaddr,
                    server_identifier,
                    ip_address_lease_time,
                    start_time,
                    renewal_time: _,
                    rebinding_time,
                },
        } = self;
        let rebinding_time = *rebinding_time.as_ref();
        let mut address_removed =
            pin!(address_event_receiver.filter_map(async |event| match event {
                AddressEvent::Rejected => {
                    handle_address_rejection(
                        client_config,
                        lease_state,
                        packet_socket_provider,
                        time,
                    )
                    .map(|result| Some(result.map(RenewingOutcome::AddressRejected)))
                    .await
                }
                AddressEvent::Removed(reason) => {
                    Some(Ok(RenewingOutcome::AddressRemoved(reason)))
                }
                AddressEvent::AssignmentStateChanged(new_state) => {
                    // TODO(https://fxbug.dev/421941195): Handle addresses
                    // becoming unavailable.
                    log::warn!(
                        "{yiaddr} address state became {new_state:?} during Renewing; ignoring"
                    );
                    None
                }
            }));

        let socket = match udp_socket_provider
            .bind_new_udp_socket(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                yiaddr.get().into(),
                CLIENT_PORT.get(),
            )))
            .await
        {
            Ok(s) => s,
            Err(e @ deps::SocketError::AddrNotAvailable) => {
                // Do a best-effort check by polling the address event receiver. The event
                // is not guaranteed to be available, so just return the socket error if
                // it isn't.
                if let Some(Some(result)) = address_removed.next().now_or_never() {
                    return result;
                }
                return Err(Error::Socket(e));
            }
            Err(e) => return Err(Error::Socket(e)),
        };

        // Per the table in RFC 2131 section 4.4.1:
        //
        // 'ciaddr'                 client's network address (BOUND/RENEW/REBIND)
        // Requested IP Address     MUST NOT (in BOUND or RENEWING)
        // IP address lease time    MAY
        // DHCP message type        DHCPREQUEST
        // Server Identifier        MUST NOT (after INIT-REBOOT, BOUND, RENEWING or REBINDING)
        // Parameter Request List   MAY
        let message = build_outgoing_message(
            client_config,
            discover_options,
            OutgoingOptions {
                ciaddr: Some(*yiaddr),
                requested_ip_address: None,
                ip_address_lease_time_secs: client_config.preferred_lease_time_secs,
                message_type: dhcp_protocol::MessageType::DHCPREQUEST,
                server_identifier: None,
                include_parameter_request_list: true,
            },
        );
        let message_bytes = message.serialize();

        let t2 = start_time.add(rebinding_time);
        let server_sockaddr = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            server_identifier.get().into(),
            SERVER_PORT.get(),
        ));
        let mut send_fut = pin!(
            send_with_retransmits_at_instants(
                time,
                std::iter::from_fn(|| {
                    let now = time.now();
                    let half_time_until_t2 = now.average(t2);
                    Some(half_time_until_t2.max(now.add(RENEW_RETRANSMIT_MINIMUM_DELAY)))
                }),
                message_bytes.as_ref(),
                &socket,
                server_sockaddr,
                debug_log_prefix,
                messaging
            )
            .fuse()
        );

        let mut recv_buf = [0u8; BUFFER_SIZE];
        let mut responses_stream = pin!(
            recv_stream(
                &socket,
                &mut recv_buf,
                |packet, addr| {
                    if addr != server_sockaddr {
                        return Err(anyhow::Error::from(crate::parse::ParseError::WrongSource(
                            addr,
                        )));
                    }
                    let message = dhcp_protocol::Message::from_buffer(packet)
                        .map_err(crate::parse::ParseError::Dhcp)
                        .context("error while parsing DHCP message from UDP datagram")
                        .inspect_err(|_| messaging.recv_failed_dhcp_parse.increment())?;
                    validate_message(discover_options, client_config, &message)
                        .inspect_err(|e| e.increment(&messaging))
                        .context("invalid DHCP message")?;
                    crate::parse::fields_to_retain_from_response_to_request(
                        &client_config.requested_parameters,
                        message,
                    )
                    .inspect_err(|e| recv_error.increment(e))
                    .context("error extracting needed fields from DHCP message during Renewing")
                    .map(|(fields, soft_errors)| {
                        messaging.increment_soft_errors(soft_errors);
                        fields
                    })
                    .map(|fields| {
                        let (validated, soft_errors) = validate_response_to_request(
                            fields,
                            // If the DHCP Server did not provide an IP Address
                            // Lease Time in the DHCPACK, this is in violation
                            // of the RFC, but we accept the ACK regardless for
                            // compatibility and fall back to the lease time we
                            // previously had.
                            Some(*ip_address_lease_time.as_ref()),
                        );
                        soft_errors.increment(&messaging);

                        (addr, validated)
                    })
                },
                debug_log_prefix,
                messaging
            )
            .try_filter_map(|parse_result| {
                futures::future::ok(match parse_result {
                    Ok(msg) => Some(msg),
                    Err(error) => {
                        log::warn!("{debug_log_prefix} discarding incoming packet: {:?}", error);
                        None
                    }
                })
            })
            .fuse()
        );

        let mut timeout_fut = pin!(time.wait_until(t2).fuse());

        let (src_addr, validated) = select_biased! {
            response = responses_stream.select_next_some() => {
                response?
            },
            () = stop_receiver.select_next_some() => return Ok(RenewingOutcome::GracefulShutdown),
            result = address_removed.next() => {
                return result.unwrap_or(Err(Error::AddressEventReceiverEnded))
            }
            send_result = send_fut => {
                return Err(send_result.expect_err("send_fut should never complete without error"))
            },
            () = timeout_fut => {
                messaging.recv_time_out.increment();
                return Ok(RenewingOutcome::Rebinding(
                    Rebinding { lease_state: lease_state.clone() }
                ))
            }
        };

        match validated {
            ValidatedResponse::Ack { ack, lease_time, renewal_time, rebinding_time } => {
                log_ack(&debug_log_prefix, &src_addr, &ack);
                let FieldsToRetainFromAck {
                    yiaddr: new_yiaddr,
                    server_identifier: _,
                    ip_address_lease_time_secs: _,
                    renewal_time_value_secs: _,
                    rebinding_time_value_secs: _,
                    parameters,
                } = ack;
                let variant = if new_yiaddr == *yiaddr {
                    RenewingOutcome::Renewed
                } else {
                    log::info!(
                        "{debug_log_prefix} obtained different address from renewal: {}",
                        new_yiaddr
                    );
                    RenewingOutcome::NewAddress
                };

                match lease_time {
                    RelativeTime::Explicit(_) => {}
                    r @ (RelativeTime::Previous(_) | RelativeTime::Default(_)) => {
                        messaging.recv_ack_no_addr_lease_time.increment();
                        log::warn!(
                            "{debug_log_prefix} accepting DHCPACK from {src_addr} that did \
                            not provide an IP Address Lease Time. Using {r:?} instead."
                        );
                    }
                }

                Ok(variant(
                    LeaseState {
                        discover_options: discover_options.clone(),
                        yiaddr: new_yiaddr,
                        server_identifier: *server_identifier,
                        ip_address_lease_time: lease_time,
                        renewal_time,
                        rebinding_time,
                        start_time: renewal_start_time,
                    },
                    parameters,
                ))
            }
            ValidatedResponse::Nak(nak) => {
                log_nak(&debug_log_prefix, &src_addr, &nak);
                recv_nak.increment();
                Ok(RenewingOutcome::Nak(nak))
            }
        }
    }
}

/// A renewal of a DHCP lease.
#[derive(Debug, PartialEq)]
pub struct LeaseRenewal<I> {
    /// The start time of the lease after renewal.
    pub start_time: I,
    /// The length of the lease after renewal.
    pub lease_time: Duration,
    /// Configuration parameters acquired from the server. Guaranteed to be a
    /// subset of the parameters requested in the `parameter_request_list` in
    /// `ClientConfig`.
    pub parameters: Vec<dhcp_protocol::DhcpOption>,
}

/// The Rebinding state as depicted in the state-transition diagram in
/// [RFC 2131].
///
/// [RFC 2131]: https://datatracker.ietf.org/doc/html/rfc2131#section-4.4
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Rebinding<I> {
    lease_state: LeaseState<I>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum RebindingOutcome<I, R> {
    GracefulShutdown,
    Renewed(LeaseState<I>, Vec<dhcp_protocol::DhcpOption>),
    // It might be surprising to see that it's possible to yield a _new_ address
    // while rebinding, but per RFC 2131 section 4.4.5, "if the client is given a
    // new network address, it MUST NOT continue using the previous network
    // address and SHOULD notify the local users of the problem." This suggests
    // that we should be prepared for a DHCP server to send us a different
    // address from the one we asked for while rebinding.
    NewAddress(LeaseState<I>, Vec<dhcp_protocol::DhcpOption>),
    Nak(FieldsToRetainFromNak),
    TimedOut,
    AddressRemoved(R),
    AddressRejected(WaitingToRestart<I>),
}

impl<I: deps::Instant> Rebinding<I> {
    async fn do_rebinding<C: deps::Clock<Instant = I>, R>(
        &self,
        client_config: &ClientConfig,
        // TODO(https://fxbug.dev/42075580): avoid dropping/recreating the packet
        // socket unnecessarily by taking an `&impl
        // deps::Socket<std::net::SocketAddr>` here instead.
        udp_socket_provider: &impl deps::UdpSocketProvider,
        packet_socket_provider: &impl deps::PacketSocketProvider,
        time: &C,
        stop_receiver: &mut mpsc::UnboundedReceiver<()>,
        address_event_receiver: impl FusedStream<Item = AddressEvent<R>>,
        counters: &Counters,
    ) -> Result<RebindingOutcome<I, R>, Error> {
        let Counters {
            rebinding: RebindingCounters { messaging, recv_error, recv_nak, .. }, ..
        } = counters;
        let rebinding_start_time = time.now();
        let debug_log_prefix = client_config.debug_log_prefix;

        let Self {
            lease_state:
                lease_state @ LeaseState {
                    discover_options,
                    yiaddr,
                    server_identifier: _,
                    ip_address_lease_time,
                    start_time,
                    renewal_time: _,
                    rebinding_time: _,
                },
        } = self;
        let mut address_removed =
            pin!(address_event_receiver.filter_map(async |event| match event {
                AddressEvent::Rejected => {
                    handle_address_rejection(
                        client_config,
                        lease_state,
                        packet_socket_provider,
                        time,
                    )
                    .map(|result| Some(result.map(RebindingOutcome::AddressRejected)))
                    .await
                }
                AddressEvent::Removed(reason) => {
                    Some(Ok(RebindingOutcome::AddressRemoved(reason)))
                }
                AddressEvent::AssignmentStateChanged(new_state) => {
                    // TODO(https://fxbug.dev/421941195): Handle addresses
                    // becoming unavailable.
                    log::warn!(
                        "{yiaddr} address state became {new_state:?} during Rebinding; ignoring"
                    );
                    None
                }
            }));

        let socket = match udp_socket_provider
            .bind_new_udp_socket(std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                yiaddr.get().into(),
                CLIENT_PORT.get(),
            )))
            .await
        {
            Ok(s) => s,
            Err(e @ deps::SocketError::AddrNotAvailable) => {
                // Do a best-effort check by polling the address event receiver. The event
                // is not guaranteed to be available, so just return the socket error if
                // it isn't.
                if let Some(Some(result)) = address_removed.next().now_or_never() {
                    return result;
                }
                return Err(Error::Socket(e));
            }
            Err(e) => return Err(Error::Socket(e)),
        };

        // Per the table in RFC 2131 section 4.4.1:
        //
        // 'ciaddr'                 client's network address (BOUND/RENEW/REBIND)
        // Requested IP Address     MUST NOT (in BOUND or RENEWING)
        // IP address lease time    MAY
        // DHCP message type        DHCPREQUEST
        // Server Identifier        MUST NOT (after INIT-REBOOT, BOUND, RENEWING or REBINDING)
        // Parameter Request List   MAY
        let message = build_outgoing_message(
            client_config,
            discover_options,
            OutgoingOptions {
                ciaddr: Some(*yiaddr),
                requested_ip_address: None,
                ip_address_lease_time_secs: client_config.preferred_lease_time_secs,
                message_type: dhcp_protocol::MessageType::DHCPREQUEST,
                server_identifier: None,
                include_parameter_request_list: true,
            },
        );
        let message_bytes = message.serialize();

        let lease_expiry = start_time.add(*ip_address_lease_time.as_ref());
        let server_sockaddr = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            Ipv4Addr::BROADCAST,
            SERVER_PORT.get(),
        ));
        let mut send_fut = pin!(
            send_with_retransmits_at_instants(
                time,
                std::iter::from_fn(|| {
                    let now = time.now();
                    let half_time_until_lease_expiry = now.average(lease_expiry);
                    Some(half_time_until_lease_expiry.max(now.add(RENEW_RETRANSMIT_MINIMUM_DELAY)))
                }),
                message_bytes.as_ref(),
                &socket,
                server_sockaddr,
                debug_log_prefix,
                messaging
            )
            .fuse()
        );

        let mut recv_buf = [0u8; BUFFER_SIZE];
        let mut responses_stream = pin!(
            recv_stream(
                &socket,
                &mut recv_buf,
                |packet, addr| {
                    let message = dhcp_protocol::Message::from_buffer(packet)
                        .map_err(crate::parse::ParseError::Dhcp)
                        .context("error while parsing DHCP message from UDP datagram")
                        .inspect_err(|_| messaging.recv_failed_dhcp_parse.increment())?;
                    validate_message(discover_options, client_config, &message)
                        .inspect_err(|e| e.increment(&messaging))
                        .context("invalid DHCP message")?;
                    crate::parse::fields_to_retain_from_response_to_request(
                        &client_config.requested_parameters,
                        message,
                    )
                    .map(|(response, soft_errors)| {
                        messaging.increment_soft_errors(soft_errors);
                        response
                    })
                    .and_then(|response| match response {
                        crate::parse::IncomingResponseToRequest::Ack(ack) => {
                            // We need to enforce that DHCPACKs in REBINDING include
                            // a server identifier, as otherwise we won't know which
                            // server to send future renewal requests to.
                            Ok(crate::parse::IncomingResponseToRequest::Ack(
                                ack.map_server_identifier(|server_identifier| {
                                    server_identifier.ok_or(
                                crate::parse::IncomingResponseToRequestError::NoServerIdentifier,
                            )
                                })?,
                            ))
                        }
                        crate::parse::IncomingResponseToRequest::Nak(nak) => {
                            Ok(crate::parse::IncomingResponseToRequest::Nak(nak))
                        }
                    })
                    .inspect_err(|e| recv_error.increment(e))
                    .context("error extracting needed fields from DHCP message during Rebinding")
                    .map(|response| {
                        let (validated, soft_errors) = validate_response_to_request(
                            response,
                            // If the DHCP Server did not provide an IP Address
                            // Lease Time in the DHCPACK, this is in violation of
                            // the RFC, but we accept the ACK regardless for
                            // compatibility and fall back to the lease time we
                            // previously had.
                            Some(*ip_address_lease_time.as_ref()),
                        );
                        soft_errors.increment(&messaging);
                        (addr, validated)
                    })
                },
                debug_log_prefix,
                messaging
            )
            .try_filter_map(|parse_result| {
                futures::future::ok(match parse_result {
                    Ok(msg) => Some(msg),
                    Err(error) => {
                        log::warn!("{debug_log_prefix} discarding incoming packet: {:?}", error);
                        None
                    }
                })
            })
            .fuse()
        );

        let mut timeout_fut = pin!(time.wait_until(lease_expiry).fuse());

        let (src_addr, validated) = select_biased! {
            response = responses_stream.select_next_some() => {
                response?
            },
            () = stop_receiver.select_next_some() => return Ok(RebindingOutcome::GracefulShutdown),
            result = address_removed.next() => {
                return result.unwrap_or(Err(Error::AddressEventReceiverEnded))
            }
            send_result = send_fut => {
                return Err(send_result.expect_err("send_fut should never complete without error"))
            },
            () = timeout_fut => {
                messaging.recv_time_out.increment();
                return Ok(RebindingOutcome::TimedOut)
            }
        };

        match validated {
            ValidatedResponse::Ack { ack, lease_time, renewal_time, rebinding_time } => {
                log_ack(&debug_log_prefix, &src_addr, &ack);
                let FieldsToRetainFromAck {
                    yiaddr: new_yiaddr,
                    server_identifier,
                    ip_address_lease_time_secs: _,
                    renewal_time_value_secs: _,
                    rebinding_time_value_secs: _,
                    parameters,
                } = ack;
                let variant = if new_yiaddr == *yiaddr {
                    RebindingOutcome::Renewed
                } else {
                    log::info!(
                        "{debug_log_prefix} obtained different address from rebinding: {}",
                        new_yiaddr
                    );
                    RebindingOutcome::NewAddress
                };

                match lease_time {
                    RelativeTime::Explicit(_) => {}
                    r @ (RelativeTime::Previous(_) | RelativeTime::Default(_)) => {
                        messaging.recv_ack_no_addr_lease_time.increment();
                        log::warn!(
                            "{debug_log_prefix} accepting DHCPACK from {src_addr} that did \
                            not provide an IP Address Lease Time. Using {r:?} instead."
                        );
                    }
                }

                Ok(variant(
                    LeaseState {
                        discover_options: discover_options.clone(),
                        yiaddr: new_yiaddr,
                        server_identifier,
                        ip_address_lease_time: lease_time,
                        renewal_time,
                        rebinding_time,
                        start_time: rebinding_start_time,
                    },
                    parameters,
                ))
            }
            ValidatedResponse::Nak(nak) => {
                log_nak(&debug_log_prefix, &src_addr, &nak);
                recv_nak.increment();
                Ok(RebindingOutcome::Nak(nak))
            }
        }
    }
}

/// A relative time value, which can be explicit, based on a previous value, or
/// a default value.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum RelativeTime {
    /// An explicitly-provided value.
    Explicit(Duration),
    /// A value based on a previous `RelativeTime` value.
    Previous(Duration),
    /// A default value.
    Default(Duration),
}

impl AsRef<Duration> for RelativeTime {
    fn as_ref(&self) -> &Duration {
        match self {
            RelativeTime::Explicit(d) | RelativeTime::Previous(d) | RelativeTime::Default(d) => d,
        }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum ValidatedResponse<S> {
    Ack {
        ack: crate::parse::FieldsToRetainFromAck<S>,
        lease_time: RelativeTime,
        renewal_time: RelativeTime,
        rebinding_time: RelativeTime,
    },
    Nak(crate::parse::FieldsToRetainFromNak),
}

impl RelativeTime {
    fn record(&self, inspector: &mut impl Inspector, value_key: &str, source_key: &str) {
        inspector.record_uint(value_key, self.as_ref().as_secs());
        match self {
            RelativeTime::Explicit(_) => {}
            RelativeTime::Previous(_) => {
                inspector.record_str(source_key, "Previous");
            }
            RelativeTime::Default(_) => {
                inspector.record_str(source_key, "Default");
            }
        }
    }
}

/// Determines the default renewal time (T1) for a given lease time.
///
/// Per RFC 2131 section 4.4.5, T1 defaults to (0.5 * duration_of_lease).
fn default_renewal_time(lease_time: Duration) -> RelativeTime {
    RelativeTime::Default(lease_time / 2)
}

/// Determines the default rebinding time (T2) for a given lease time.
///
/// Per RFC 2131 section 4.4.5, T2 defaults to (0.875 * duration_of_lease).
fn default_rebinding_time(lease_time: Duration) -> RelativeTime {
    RelativeTime::Default(lease_time / 8 * 7)
}

#[derive(Default)]
struct ResponseValidationSoftErrors {
    t1_outlives_t2: bool,
    t2_outlives_lease: bool,
}

impl ResponseValidationSoftErrors {
    fn increment(&self, counters: &MessagingRelatedCounters) {
        if self.t1_outlives_t2 {
            counters.recv_ack_renewal_time_after_rebinding_time.increment();
        }
        if self.t2_outlives_lease {
            counters.recv_ack_rebinding_time_outlives_lease.increment();
        }
    }
}

/// Validates the server response.
///
///
/// If the response does not provide an explicit lease time,
/// `fallback_lease_time` is used.
fn validate_response_to_request<S>(
    response: crate::parse::IncomingResponseToRequest<S>,
    fallback_lease_time: Option<Duration>,
) -> (ValidatedResponse<S>, ResponseValidationSoftErrors) {
    let mut soft_errors = ResponseValidationSoftErrors::default();
    match response {
        crate::parse::IncomingResponseToRequest::Ack(ack) => {
            let lease_time = match ack.ip_address_lease_time_secs {
                Some(t) => RelativeTime::Explicit(Duration::from_secs(t.get().into())),
                None => match fallback_lease_time {
                    Some(t) => RelativeTime::Previous(t),
                    None => RelativeTime::Default(DEFAULT_LEASE_TIME),
                },
            };
            let mut t1 = ack
                .renewal_time_value_secs
                .map(|t| RelativeTime::Explicit(Duration::from_secs(t.into())))
                .unwrap_or_else(|| default_renewal_time(*lease_time.as_ref()));
            let mut t2 = ack
                .rebinding_time_value_secs
                .map(|t| RelativeTime::Explicit(Duration::from_secs(t.into())))
                .unwrap_or_else(|| default_rebinding_time(*lease_time.as_ref()));
            // Per RFC 2131 section 4.4.5, T1 MUST be earlier than T2, which, in
            // turn, MUST be earlier than the time at which the client's lease
            // will expire.
            //
            // If the server-provided values violate either of these properties
            // then we fall back to the default values for T1 and T2 and accept
            // the lease for robustness' sake.
            if t1.as_ref() >= t2.as_ref() {
                soft_errors.t1_outlives_t2 = true;
            }
            if t2.as_ref() >= lease_time.as_ref() {
                soft_errors.t2_outlives_lease = true;
            }
            if soft_errors.t1_outlives_t2 || soft_errors.t2_outlives_lease {
                if !matches!(t1, RelativeTime::Default(_)) {
                    t1 = default_renewal_time(*lease_time.as_ref());
                }
                if !matches!(t2, RelativeTime::Default(_)) {
                    t2 = default_rebinding_time(*lease_time.as_ref());
                }
            }
            (
                ValidatedResponse::Ack { ack, lease_time, renewal_time: t1, rebinding_time: t2 },
                soft_errors,
            )
        }
        crate::parse::IncomingResponseToRequest::Nak(nak) => {
            (ValidatedResponse::Nak(nak), soft_errors)
        }
    }
}

fn log_offer(
    prefix: &DebugLogPrefix,
    src_addr: &net_types::ip::Ipv4Addr,
    offer_fields: &FieldsFromOfferToUseInRequest,
) {
    let FieldsFromOfferToUseInRequest {
        server_identifier,
        ip_address_lease_time_secs: lease_time,
        ip_address_to_request: yiaddr,
    } = offer_fields;
    log::info!(
        "{prefix} received DHCPOFFER from {src_addr}: \
        yiaddr={yiaddr}, lease_time={lease_time:?}, server_identifier={server_identifier}"
    );
}

fn log_ack<A: Display, S: Debug>(
    prefix: &DebugLogPrefix,
    src_addr: &A,
    ack_fields: &FieldsToRetainFromAck<S>,
) {
    let FieldsToRetainFromAck {
        yiaddr,
        server_identifier,
        ip_address_lease_time_secs: lease_time,
        renewal_time_value_secs: renew_time,
        rebinding_time_value_secs: rebinding_time,
        parameters,
    } = ack_fields;
    // NB: Don't log the raw parameters, they may contain unstructured PII.
    // Instead extract the parameters that are known to be useful for debugging.
    let (prefix_len, routers, dns_servers): (
        Option<u8>,
        Option<Vec<Ipv4Addr>>,
        Option<Vec<Ipv4Addr>>,
    ) = parameters.iter().fold(
        (None, None, None),
        |(mut prefix_len, mut routers, mut dns_servers), option| {
            match option {
                dhcp_protocol::DhcpOption::SubnetMask(prefix_length) => {
                    prefix_len = Some(prefix_length.get());
                }
                dhcp_protocol::DhcpOption::Router(at_least) => {
                    routers.get_or_insert_default().extend(at_least.deref());
                }
                dhcp_protocol::DhcpOption::DomainNameServer(at_least) => {
                    dns_servers.get_or_insert_default().extend(at_least.deref());
                }
                _ => {}
            }
            (prefix_len, routers, dns_servers)
        },
    );
    log::info!(
        "{prefix} received DHCPACK from {src_addr}: \
        yiaddr={yiaddr}, lease_time={lease_time:?}, renewal_time={renew_time:?}, \
        rebinding_time={rebinding_time:?}, server_identifier={server_identifier:?}, \
        prefix_len={prefix_len:?}, routers={routers:?}, dns_servers={dns_servers:?}"
    );
}

fn log_nak<A: Display>(prefix: &DebugLogPrefix, src_addr: &A, nak_fields: &FieldsToRetainFromNak) {
    let FieldsToRetainFromNak { server_identifier, message, client_identifier: _ } = nak_fields;
    log::info!(
        "{prefix} received DHCPNAK from {src_addr}: \
        server_identifier={server_identifier}, message={message:?}"
    );
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::deps::Clock as _;
    use crate::deps::testutil::{
        FakeRngProvider, FakeSocket, FakeSocketProvider, FakeTimeController, TestInstant, advance,
        run_until_next_timers_fire,
    };
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::{Future, join};
    use itertools::Itertools as _;
    use net_declare::net::prefix_length_v4;
    use net_declare::{net_mac, std_ip_v4};
    use net_types::ip::{Ipv4, PrefixLength};
    use simplelog::{Config, LevelFilter, WriteLogger};
    use std::cell::RefCell;
    use std::rc::Rc;
    use test_case::test_case;

    fn initialize_logging() {
        WriteLogger::init(LevelFilter::Info, Config::default(), std::io::stderr()).unwrap();
    }

    const TEST_MAC_ADDRESS: Mac = net_mac!("01:01:01:01:01:01");
    const TEST_SERVER_MAC_ADDRESS: Mac = net_mac!("02:02:02:02:02:02");
    const OTHER_MAC_ADDRESS: Mac = net_mac!("03:03:03:03:03:03");

    const SERVER_IP: Ipv4Addr = std_ip_v4!("192.168.1.1");
    const OTHER_SERVER_IP: Ipv4Addr = std_ip_v4!("192.168.1.11");
    const YIADDR: Ipv4Addr = std_ip_v4!("198.168.1.5");
    const OTHER_ADDR: Ipv4Addr = std_ip_v4!("198.168.1.6");
    const DEFAULT_LEASE_LENGTH_SECONDS: u32 = 100;
    const DEFAULT_RENEWAL_TIME: RelativeTime = RelativeTime::Default(Duration::from_secs(50));
    const DEFAULT_REBINDING_TIME: RelativeTime =
        RelativeTime::Default(Duration::from_millis(87500));
    const MAX_LEASE_LENGTH_SECONDS: u32 = 200;
    const TEST_PREFIX_LENGTH: PrefixLength<Ipv4> = prefix_length_v4!(24);

    fn test_requested_parameters() -> OptionCodeMap<OptionRequested> {
        use dhcp_protocol::OptionCode;
        [
            (OptionCode::SubnetMask, OptionRequested::Required),
            (OptionCode::Router, OptionRequested::Optional),
            (OptionCode::DomainNameServer, OptionRequested::Optional),
        ]
        .into_iter()
        .collect::<OptionCodeMap<_>>()
    }

    fn test_parameter_values_excluding_subnet_mask() -> [dhcp_protocol::DhcpOption; 2] {
        [
            dhcp_protocol::DhcpOption::Router([SERVER_IP].into()),
            dhcp_protocol::DhcpOption::DomainNameServer([SERVER_IP, std_ip_v4!("8.8.8.8")].into()),
        ]
    }

    fn test_parameter_values() -> impl IntoIterator<Item = dhcp_protocol::DhcpOption> {
        std::iter::once(dhcp_protocol::DhcpOption::SubnetMask(TEST_PREFIX_LENGTH))
            .chain(test_parameter_values_excluding_subnet_mask())
    }

    fn test_client_config() -> ClientConfig {
        ClientConfig {
            client_hardware_address: TEST_MAC_ADDRESS,
            client_identifier: None,
            requested_parameters: test_requested_parameters(),
            preferred_lease_time_secs: None,
            requested_ip_address: None,
            debug_log_prefix: DebugLogPrefix { interface_id: NonZeroU64::new(2).unwrap() },
        }
    }

    // A fake address removal reason for tests.
    #[derive(Debug, PartialEq)]
    struct FakeRemovedReason;

    #[test]
    fn do_init_uses_rng() {
        let mut rng = FakeRngProvider::new(0);
        let time = FakeTimeController::new();
        let arbitrary_start_time = std::time::Duration::from_secs(42);
        advance(&time, arbitrary_start_time);

        let Selecting {
            discover_options: DiscoverOptions { xid: xid_a },
            start_time: start_time_a,
        } = Init.do_init(&mut rng, &time);
        let Selecting {
            discover_options: DiscoverOptions { xid: xid_b },
            start_time: start_time_b,
        } = Init.do_init(&mut rng, &time);
        assert_ne!(xid_a, xid_b);
        assert_eq!(start_time_a, TestInstant(arbitrary_start_time));
        assert_eq!(start_time_b, TestInstant(arbitrary_start_time));
    }

    fn run_with_accelerated_time<F>(
        executor: &mut fasync::TestExecutor,
        time: &Rc<RefCell<FakeTimeController>>,
        main_future: &mut F,
    ) -> F::Output
    where
        F: Future + Unpin,
    {
        loop {
            match run_until_next_timers_fire(executor, time, main_future) {
                std::task::Poll::Ready(result) => break result,
                std::task::Poll::Pending => (),
            }
        }
    }

    fn build_test_selecting_state() -> Selecting<TestInstant> {
        Selecting {
            discover_options: DiscoverOptions { xid: TransactionId(NonZeroU32::new(1).unwrap()) },
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }
    }

    #[test]
    fn do_selecting_obeys_graceful_shutdown() {
        initialize_logging();
        let counters = Counters::default();

        let mut executor = fasync::TestExecutor::new();
        let time = FakeTimeController::new();

        let selecting = build_test_selecting_state();
        let mut rng = FakeRngProvider::new(0);

        let (_server_end, client_end) = FakeSocket::new_pair();
        let test_socket_provider = FakeSocketProvider::new(client_end);

        let client_config = test_client_config();

        let (stop_sender, mut stop_receiver) = mpsc::unbounded();

        let mut selecting_fut = pin!(
            selecting
                .do_selecting(
                    &client_config,
                    &test_socket_provider,
                    &mut rng,
                    &time,
                    &mut stop_receiver,
                    &counters,
                )
                .fuse()
        );

        let time = &time;

        let mut wait_fut = pin!(
            async {
                // Wait some arbitrary amount of time to ensure `do_selecting` is waiting on a reply.
                // Note that this is fake time, not 30 actual seconds.
                time.wait_until(TestInstant(std::time::Duration::from_secs(30))).await;
            }
            .fuse()
        );

        {
            let main_future = async {
                select! {
                    _ = selecting_fut => unreachable!("should keep retransmitting DHCPDISCOVER forever"),
                    () = wait_fut => (),
                }
            };
            let mut main_future = pin!(main_future);

            run_with_accelerated_time(&mut executor, time, &mut main_future);
        }

        stop_sender.unbounded_send(()).expect("sending stop signal should succeed");

        let selecting_result = selecting_fut.now_or_never().expect(
            "selecting_fut should complete after single poll after stop signal has been sent",
        );

        assert_matches!(selecting_result, Ok(SelectingOutcome::GracefulShutdown));
    }

    struct VaryingOutgoingMessageFields {
        xid: u32,
        options: Vec<dhcp_protocol::DhcpOption>,
    }

    #[track_caller]
    fn assert_outgoing_message_when_not_assigned_address(
        got_message: &dhcp_protocol::Message,
        fields: VaryingOutgoingMessageFields,
    ) {
        let VaryingOutgoingMessageFields { xid, options } = fields;
        let want_message = dhcp_protocol::Message {
            op: dhcp_protocol::OpCode::BOOTREQUEST,
            xid,
            secs: 0,
            bdcast_flag: false,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: TEST_MAC_ADDRESS,
            sname: BString::default(),
            file: BString::default(),
            options,
        };
        assert_eq!(got_message, &want_message);
    }

    #[test]
    fn do_selecting_sends_discover() {
        initialize_logging();
        let counters = Counters::default();

        let mut executor = fasync::TestExecutor::new();
        let time = FakeTimeController::new();

        let selecting = Selecting {
            discover_options: DiscoverOptions { xid: TransactionId(NonZeroU32::new(1).unwrap()) },
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        };
        let mut rng = FakeRngProvider::new(0);

        let (server_end, client_end) = FakeSocket::new_pair();
        let test_socket_provider = FakeSocketProvider::new(client_end);

        let client_config = test_client_config();

        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();

        let mut selecting_fut = pin!(
            selecting
                .do_selecting(
                    &client_config,
                    &test_socket_provider,
                    &mut rng,
                    &time,
                    &mut stop_receiver,
                    &counters,
                )
                .fuse()
        );

        let time = &time;

        // These are the time ranges in which we expect to see messages from the
        // DHCP client. They are ranges in order to account for randomized
        // delays.
        const EXPECTED_RANGES: [(u64, u64); 7] =
            [(0, 0), (3, 5), (7, 9), (15, 17), (31, 33), (63, 65), (63, 65)];

        let mut receive_fut =
            pin!(
                async {
                    let mut previous_time = std::time::Duration::from_secs(0);

                    for (start, end) in EXPECTED_RANGES {
                        let mut recv_buf = [0u8; BUFFER_SIZE];
                        let DatagramInfo { length, address } = server_end
                            .recv_from(&mut recv_buf)
                            .await
                            .expect("recv_from on test socket should succeed");

                        assert_eq!(address, Mac::BROADCAST);

                        let (_src_addr, msg) = crate::parse::parse_dhcp_message_from_ip_packet(
                            &recv_buf[..length],
                            dhcp_protocol::SERVER_PORT,
                        )
                        .expect("received packet should parse as DHCP message");

                        assert_outgoing_message_when_not_assigned_address(
                            &msg,
                            VaryingOutgoingMessageFields {
                                xid: msg.xid,
                                options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPDISCOVER,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                        ],
                            },
                        );

                        let TestInstant(received_time) = time.now();

                        let duration_range = std::time::Duration::from_secs(start)
                            ..=std::time::Duration::from_secs(end);
                        assert!(duration_range.contains(&(received_time - previous_time)));

                        previous_time = received_time;
                    }
                }
                .fuse()
            );

        let main_future = async {
            select! {
                _ = selecting_fut => unreachable!("should keep retransmitting DHCPDISCOVER forever"),
                () = receive_fut => (),
            }
        };
        let mut main_future = pin!(main_future);

        run_with_accelerated_time(&mut executor, time, &mut main_future);
        assert_eq!(counters.selecting.messaging.send_message.load(), EXPECTED_RANGES.len());
        assert_eq!(counters.selecting.messaging.recv_time_out.load(), EXPECTED_RANGES.len() - 1);
    }

    const XID: NonZeroU32 = NonZeroU32::new(1).unwrap();
    #[test_case(u32::from(XID), TEST_MAC_ADDRESS => Ok(()) ; "accepts good reply")]
    #[test_case(u32::from(XID), TEST_SERVER_MAC_ADDRESS => Err(
        ValidateMessageError::WrongChaddr {
            expected: TEST_MAC_ADDRESS,
            actual: TEST_SERVER_MAC_ADDRESS,
        }) ; "rejects wrong chaddr")]
    #[test_case(u32::from(XID).wrapping_add(1), TEST_MAC_ADDRESS => Err(
        ValidateMessageError::WrongXid {
            expected: u32::from(XID),
            actual: u32::from(XID).wrapping_add(1),
        }) ; "rejects wrong xid")]
    fn test_validate_message(
        message_xid: u32,
        message_chaddr: Mac,
    ) -> Result<(), ValidateMessageError> {
        let discover_options = DiscoverOptions { xid: TransactionId(XID) };
        let client_config = ClientConfig {
            client_hardware_address: TEST_MAC_ADDRESS,
            client_identifier: None,
            requested_parameters: test_requested_parameters(),
            preferred_lease_time_secs: None,
            requested_ip_address: None,
            debug_log_prefix: DebugLogPrefix { interface_id: NonZeroU64::new(2).unwrap() },
        };

        let reply = dhcp_protocol::Message {
            op: dhcp_protocol::OpCode::BOOTREPLY,
            xid: message_xid,
            secs: 0,
            bdcast_flag: false,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: message_chaddr,
            sname: BString::default(),
            file: BString::default(),
            options: Vec::new(),
        };

        validate_message(&discover_options, &client_config, &reply)
    }

    #[test_case(false ; "with no garbage traffic on link")]
    #[test_case(true ; "ignoring garbage replies to discover")]
    fn do_selecting_good_offer(reply_to_discover_with_garbage: bool) {
        initialize_logging();
        let counters = Counters::default();

        let mut rng = FakeRngProvider::new(0);
        let time = FakeTimeController::new();

        let arbitrary_start_time = std::time::Duration::from_secs(42);
        advance(&time, arbitrary_start_time);

        let selecting = Init.do_init(&mut rng, &time);
        let TransactionId(xid) = selecting.discover_options.xid;

        let (server_end, client_end) = FakeSocket::<Mac>::new_pair();
        let test_socket_provider = FakeSocketProvider::new(client_end);

        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();

        let client_config = test_client_config();

        let selecting_fut = pin!(
            selecting
                .do_selecting(
                    &client_config,
                    &test_socket_provider,
                    &mut rng,
                    &time,
                    &mut stop_receiver,
                    &counters,
                )
                .fuse()
        );

        let server_fut = pin!(
            async {
                let mut recv_buf = [0u8; BUFFER_SIZE];

                if reply_to_discover_with_garbage {
                    let DatagramInfo { length: _, address: dst_addr } = server_end
                        .recv_from(&mut recv_buf)
                        .await
                        .expect("recv_from on test socket should succeed");
                    assert_eq!(dst_addr, Mac::BROADCAST);

                    server_end
                        .send_to(b"hello", OTHER_MAC_ADDRESS)
                        .await
                        .expect("send_to with garbage data should succeed");
                }

                let DatagramInfo { length, address } = server_end
                    .recv_from(&mut recv_buf)
                    .await
                    .expect("recv_from on test socket should succeed");
                assert_eq!(address, Mac::BROADCAST);

                // `dhcp_protocol::Message` intentionally doesn't implement `Clone`,
                // so we re-parse instead for testing purposes.
                let parse_msg = || {
                    let (_src_addr, msg) = crate::parse::parse_dhcp_message_from_ip_packet(
                        &recv_buf[..length],
                        dhcp_protocol::SERVER_PORT,
                    )
                    .expect("received packet on test socket should parse as DHCP message");
                    msg
                };

                let msg = parse_msg();
                assert_outgoing_message_when_not_assigned_address(
                    &parse_msg(),
                    VaryingOutgoingMessageFields {
                        xid: msg.xid,
                        options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPDISCOVER,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                        ],
                    },
                );

                let build_reply = || {
                    dhcpv4::server::build_offer(
                        parse_msg(),
                        dhcpv4::server::OfferOptions {
                            offered_ip: YIADDR,
                            server_ip: SERVER_IP,
                            lease_length_config: dhcpv4::configuration::LeaseLength {
                                default_seconds: DEFAULT_LEASE_LENGTH_SECONDS,
                                max_seconds: MAX_LEASE_LENGTH_SECONDS,
                            },
                            // The following fields don't matter for this test, as the
                            // client will read them from the DHCPACK rather than
                            // remembering them from the DHCPOFFER.
                            renewal_time_value: Some(20),
                            rebinding_time_value: Some(30),
                            subnet_mask: TEST_PREFIX_LENGTH,
                        },
                        &dhcpv4::server::options_repo(test_parameter_values()),
                    )
                    .expect("dhcp server crate error building offer")
                };

                let reply_with_wrong_xid = dhcp_protocol::Message {
                    xid: (u32::from(xid).wrapping_add(1)),
                    // Provide a different yiaddr in order to distinguish whether
                    // the client correctly discarded this one, since we check which
                    // `yiaddr` the client uses as its requested IP address later.
                    yiaddr: OTHER_ADDR,
                    ..build_reply()
                };

                let reply_without_subnet_mask = {
                    let mut reply = build_reply();
                    let options = std::mem::take(&mut reply.options);
                    let (subnet_masks, other_options): (Vec<_>, Vec<_>) =
                        options.into_iter().partition_map(|option| match option {
                            dhcp_protocol::DhcpOption::SubnetMask(_) => {
                                itertools::Either::Left(option)
                            }
                            _ => itertools::Either::Right(option),
                        });
                    assert_matches!(
                        &subnet_masks[..],
                        &[dhcp_protocol::DhcpOption::SubnetMask(TEST_PREFIX_LENGTH)]
                    );
                    reply.options = other_options;

                    // Provide a different yiaddr in order to distinguish whether
                    // the client correctly discarded this one, since we check which
                    // `yiaddr` the client uses as its requested IP address later.
                    reply.yiaddr = OTHER_ADDR;
                    reply
                };

                let good_reply = build_reply();

                let send_reply = |reply: dhcp_protocol::Message| async {
                    let dst_ip = reply.yiaddr;
                    server_end
                        .send_to(
                            crate::parse::serialize_dhcp_message_to_ip_packet(
                                reply,
                                SERVER_IP,
                                SERVER_PORT,
                                dst_ip,
                                CLIENT_PORT,
                            )
                            .as_ref(),
                            // Note that this is the address the client under test
                            // observes in `recv_from`.
                            TEST_SERVER_MAC_ADDRESS,
                        )
                        .await
                        .expect("send_to on test socket should succeed");
                };

                // The DHCP client should ignore the reply with an incorrect xid.
                send_reply(reply_with_wrong_xid).await;

                // The DHCP client should ignore the reply without a subnet mask.
                send_reply(reply_without_subnet_mask).await;

                send_reply(good_reply).await;
            }
            .fuse()
        );

        let main_future = async move {
            let (selecting_result, ()) = join!(selecting_fut, server_fut);
            selecting_result
        }
        .fuse();
        let mut main_future = pin!(main_future);
        let mut executor = fasync::TestExecutor::new();
        let selecting_result = run_with_accelerated_time(&mut executor, &time, &mut main_future);

        let requesting = assert_matches!(
            selecting_result,
            Ok(SelectingOutcome::Requesting(requesting)) => requesting,
            "should have successfully transitioned to Requesting"
        );

        assert_eq!(
            requesting,
            Requesting {
                discover_options: DiscoverOptions { xid: requesting.discover_options.xid },
                fields_from_offer_to_use_in_request: crate::parse::FieldsFromOfferToUseInRequest {
                    server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                        .try_into()
                        .expect("should be specified"),
                    ip_address_lease_time_secs: Some(
                        NonZeroU32::new(DEFAULT_LEASE_LENGTH_SECONDS).unwrap()
                    ),
                    ip_address_to_request: net_types::ip::Ipv4Addr::from(YIADDR)
                        .try_into()
                        .expect("should be specified"),
                },
                start_time: TestInstant(arbitrary_start_time),
            }
        );
        assert_eq!(
            counters.selecting.messaging.send_message.load(),
            if reply_to_discover_with_garbage { 2 } else { 1 }
        );
        assert_eq!(
            counters.selecting.messaging.recv_time_out.load(),
            if reply_to_discover_with_garbage { 1 } else { 0 }
        );
        assert_eq!(
            counters.selecting.messaging.recv_failed_dhcp_parse.load(),
            if reply_to_discover_with_garbage { 1 } else { 0 }
        );
        assert_eq!(counters.selecting.messaging.recv_wrong_xid.load(), 1);
        assert_eq!(counters.selecting.messaging.recv_wrong_chaddr.load(), 0);
        assert_eq!(counters.selecting.recv_error.missing_required_option.load(), 1);
    }

    const TEST_XID: TransactionId = TransactionId(NonZeroU32::new(1).unwrap());
    const TEST_DISCOVER_OPTIONS: DiscoverOptions = DiscoverOptions { xid: TEST_XID };

    fn build_test_requesting_state() -> Requesting<TestInstant> {
        Requesting {
            discover_options: TEST_DISCOVER_OPTIONS,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
            fields_from_offer_to_use_in_request: crate::parse::FieldsFromOfferToUseInRequest {
                server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                    .try_into()
                    .expect("should be specified"),
                ip_address_lease_time_secs: Some(
                    NonZeroU32::new(DEFAULT_LEASE_LENGTH_SECONDS).unwrap(),
                ),
                ip_address_to_request: net_types::ip::Ipv4Addr::from(YIADDR)
                    .try_into()
                    .expect("should be specified"),
            },
        }
    }

    #[test]
    fn do_requesting_obeys_graceful_shutdown() {
        initialize_logging();
        let counters = Counters::default();

        let time = FakeTimeController::new();

        let requesting = build_test_requesting_state();
        let mut rng = FakeRngProvider::new(0);

        let (_server_end, client_end) = FakeSocket::new_pair();
        let test_socket_provider = FakeSocketProvider::new(client_end);

        let client_config = test_client_config();

        let (stop_sender, mut stop_receiver) = mpsc::unbounded();

        let requesting_fut = requesting
            .do_requesting(
                &client_config,
                &test_socket_provider,
                &mut rng,
                &time,
                &mut stop_receiver,
                &counters,
            )
            .fuse();
        let mut requesting_fut = pin!(requesting_fut);

        let mut executor = fasync::TestExecutor::new();
        assert_matches!(executor.run_until_stalled(&mut requesting_fut), std::task::Poll::Pending);

        stop_sender.unbounded_send(()).expect("sending stop signal should succeed");

        let requesting_result = requesting_fut.now_or_never().expect(
            "requesting_fut should complete after single poll after stop signal has been sent",
        );

        assert_matches!(requesting_result, Ok(RequestingOutcome::GracefulShutdown));
    }

    #[test]
    fn do_requesting_sends_requests() {
        initialize_logging();
        let counters = Counters::default();

        let mut executor = fasync::TestExecutor::new();
        let time = FakeTimeController::new();

        let requesting = build_test_requesting_state();
        let mut rng = FakeRngProvider::new(0);

        let (server_end, client_end) = FakeSocket::new_pair();
        let test_socket_provider = FakeSocketProvider::new(client_end);

        let client_config = test_client_config();

        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();

        let requesting_fut = pin!(
            requesting
                .do_requesting(
                    &client_config,
                    &test_socket_provider,
                    &mut rng,
                    &time,
                    &mut stop_receiver,
                    &counters,
                )
                .fuse()
        );

        let time = &time;

        // These are the time ranges in which we expect to see messages from the
        // DHCP client. They are ranges in order to account for randomized
        // delays.
        const EXPECTED_RANGES: [(u64, u64); NUM_REQUEST_RETRANSMITS + 1] =
            [(0, 0), (3, 5), (7, 9), (15, 17), (31, 33)];

        let receive_fut = pin!(
            async {
                let mut previous_time = std::time::Duration::from_secs(0);

                for (start, end) in EXPECTED_RANGES {
                    let mut recv_buf = [0u8; BUFFER_SIZE];
                    let DatagramInfo { length, address } = server_end
                        .recv_from(&mut recv_buf)
                        .await
                        .expect("recv_from on test socket should succeed");

                    assert_eq!(address, Mac::BROADCAST);

                    let (_src_addr, msg) = crate::parse::parse_dhcp_message_from_ip_packet(
                        &recv_buf[..length],
                        dhcp_protocol::SERVER_PORT,
                    )
                    .expect("received packet should parse as DHCP message");

                    assert_outgoing_message_when_not_assigned_address(
                        &msg,
                        VaryingOutgoingMessageFields {
                            xid: msg.xid,
                            options: vec![
                                dhcp_protocol::DhcpOption::DhcpMessageType(
                                    dhcp_protocol::MessageType::DHCPREQUEST,
                                ),
                                dhcp_protocol::DhcpOption::ParameterRequestList(
                                    test_requested_parameters()
                                        .iter_keys()
                                        .collect::<Vec<_>>()
                                        .try_into()
                                        .expect(
                                            "should fit parameter request list size constraints",
                                        ),
                                ),
                                dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
                                dhcp_protocol::DhcpOption::RequestedIpAddress(YIADDR),
                                dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                                    DEFAULT_LEASE_LENGTH_SECONDS,
                                ),
                            ],
                        },
                    );

                    let TestInstant(received_time) = time.now();

                    let duration_range =
                        std::time::Duration::from_secs(start)..=std::time::Duration::from_secs(end);
                    assert!(duration_range.contains(&(received_time - previous_time)));

                    previous_time = received_time;
                }
            }
            .fuse()
        );

        let main_future = async { join!(requesting_fut, receive_fut) };
        let mut main_future = pin!(main_future);

        let (requesting_result, ()) =
            run_with_accelerated_time(&mut executor, time, &mut main_future);

        assert_matches!(requesting_result, Ok(RequestingOutcome::RanOutOfRetransmits));
        assert_eq!(counters.requesting.messaging.send_message.load(), EXPECTED_RANGES.len());
        assert_eq!(counters.requesting.messaging.recv_time_out.load(), EXPECTED_RANGES.len());
    }

    struct VaryingIncomingMessageFields {
        yiaddr: Ipv4Addr,
        options: Vec<dhcp_protocol::DhcpOption>,
    }

    fn build_incoming_message(
        xid: u32,
        fields: VaryingIncomingMessageFields,
    ) -> dhcp_protocol::Message {
        let VaryingIncomingMessageFields { yiaddr, options } = fields;

        dhcp_protocol::Message {
            op: dhcp_protocol::OpCode::BOOTREPLY,
            xid,
            secs: 0,
            bdcast_flag: false,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: TEST_MAC_ADDRESS,
            sname: BString::default(),
            file: BString::default(),
            options,
        }
    }

    const NAK_MESSAGE: &str = "something went wrong";

    #[derive(PartialEq, Debug)]
    struct RequestingTestResult {
        outcome: RequestingOutcome<TestInstant>,
        counters: RequestingTestCounters,
    }

    #[derive(PartialEq, Eq, Debug, Default)]
    struct RequestingTestCounters {
        send_message: usize,
        recv_time_out: usize,
        recv_failed_dhcp_parse: usize,
        recv_wrong_xid: usize,
        recv_wrong_chaddr: usize,
        recv_message: usize,
        recv_nak: usize,
        recv_missing_option: usize,
        recv_ack_renewal_time_after_rebinding_time: usize,
        recv_ack_rebinding_time_outlives_lease: usize,
    }

    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RequestingTestResult {
        outcome: RequestingOutcome::Bound(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RequestingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
     } ; "transitions to Bound after receiving DHCPACK")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RequestingTestResult {
        outcome: RequestingOutcome::Bound(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Previous(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RequestingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
     } ; "transitions to Bound after receiving DHCPACK without lease time")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values_excluding_subnet_mask())
        .collect(),
    } => RequestingTestResult {
        outcome: RequestingOutcome::RanOutOfRetransmits,
        counters: RequestingTestCounters {
            send_message: 5,
            recv_time_out: 5,
            recv_message: 1,
            recv_missing_option: 1,
            ..Default::default()
        },
    }; "ignores replies lacking required option SubnetMask")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: Ipv4Addr::UNSPECIFIED,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPNAK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::Message(NAK_MESSAGE.to_owned()),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RequestingTestResult {
        outcome: RequestingOutcome::Nak(crate::parse::FieldsToRetainFromNak {
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            message: Some(NAK_MESSAGE.to_owned()),
            client_identifier: None,
        }),
        counters: RequestingTestCounters {
            send_message: 1,
            recv_message: 1,
            recv_nak: 1,
            ..Default::default()
        },
    }; "transitions to Init after receiving DHCPNAK")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
            dhcp_protocol::DhcpOption::RenewalTimeValue(DEFAULT_LEASE_LENGTH_SECONDS - 1),
            dhcp_protocol::DhcpOption::RebindingTimeValue(DEFAULT_LEASE_LENGTH_SECONDS - 2),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RequestingTestResult {
        outcome: RequestingOutcome::Bound(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RequestingTestCounters {
            send_message: 1,
            recv_message: 1,
            recv_ack_renewal_time_after_rebinding_time: 1,
            recv_ack_rebinding_time_outlives_lease: 0,
            ..Default::default()
        },
    }; "transitions to Bound with default T1 and T2 if provided T1 greater than T2")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
            dhcp_protocol::DhcpOption::RebindingTimeValue(DEFAULT_LEASE_LENGTH_SECONDS + 1),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RequestingTestResult {
        outcome: RequestingOutcome::Bound(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RequestingTestCounters {
            send_message: 1,
            recv_message: 1,
            recv_ack_renewal_time_after_rebinding_time: 0,
            recv_ack_rebinding_time_outlives_lease: 1,
            ..Default::default()
        },
    }; "transitions to Bound with default T1 and T2 if provided T2 greater than lease_time")]
    fn do_requesting_transitions_on_reply(
        incoming_message: VaryingIncomingMessageFields,
    ) -> RequestingTestResult {
        initialize_logging();
        let counters = Counters::default();

        let time = &FakeTimeController::new();

        let requesting = build_test_requesting_state();
        let mut rng = FakeRngProvider::new(0);

        let (server_end, client_end) = FakeSocket::new_pair();
        let test_socket_provider = FakeSocketProvider::new(client_end);

        let client_config = test_client_config();

        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();

        let requesting_fut = pin!(
            requesting
                .do_requesting(
                    &client_config,
                    &test_socket_provider,
                    &mut rng,
                    time,
                    &mut stop_receiver,
                    &counters,
                )
                .fuse()
        );

        let server_fut = pin!(
            async {
                let mut recv_buf = [0u8; BUFFER_SIZE];

                let DatagramInfo { length, address } = server_end
                    .recv_from(&mut recv_buf)
                    .await
                    .expect("recv_from on test socket should succeed");
                assert_eq!(address, Mac::BROADCAST);

                let (_src_addr, msg) = crate::parse::parse_dhcp_message_from_ip_packet(
                    &recv_buf[..length],
                    dhcp_protocol::SERVER_PORT,
                )
                .expect("received packet on test socket should parse as DHCP message");

                assert_outgoing_message_when_not_assigned_address(
                    &msg,
                    VaryingOutgoingMessageFields {
                        xid: msg.xid,
                        options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPREQUEST,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
                            dhcp_protocol::DhcpOption::RequestedIpAddress(YIADDR),
                            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                                DEFAULT_LEASE_LENGTH_SECONDS,
                            ),
                        ],
                    },
                );

                let reply = build_incoming_message(msg.xid, incoming_message);

                server_end
                    .send_to(
                        crate::parse::serialize_dhcp_message_to_ip_packet(
                            reply,
                            SERVER_IP,
                            SERVER_PORT,
                            YIADDR,
                            CLIENT_PORT,
                        )
                        .as_ref(),
                        // Note that this is the address the client under test
                        // observes in `recv_from`.
                        TEST_SERVER_MAC_ADDRESS,
                    )
                    .await
                    .expect("send_to on test socket should succeed");
            }
            .fuse()
        );

        let main_future = async move {
            let (requesting_result, ()) = join!(requesting_fut, server_fut);
            requesting_result
        }
        .fuse();

        let mut main_future = pin!(main_future);

        let mut executor = fasync::TestExecutor::new();
        let requesting_result = run_with_accelerated_time(&mut executor, time, &mut main_future);

        let outcome = assert_matches!(requesting_result, Ok(outcome) => outcome);
        let counters = RequestingTestCounters {
            send_message: counters.requesting.messaging.send_message.load(),
            recv_time_out: counters.requesting.messaging.recv_time_out.load(),
            recv_failed_dhcp_parse: counters.requesting.messaging.recv_failed_dhcp_parse.load(),
            recv_wrong_xid: counters.requesting.messaging.recv_wrong_xid.load(),
            recv_wrong_chaddr: counters.requesting.messaging.recv_wrong_chaddr.load(),
            recv_message: counters.requesting.messaging.recv_message.load(),
            recv_nak: counters.requesting.recv_nak.load(),
            recv_missing_option: counters.requesting.recv_error.missing_required_option.load(),
            recv_ack_renewal_time_after_rebinding_time: counters
                .requesting
                .messaging
                .recv_ack_renewal_time_after_rebinding_time
                .load(),
            recv_ack_rebinding_time_outlives_lease: counters
                .requesting
                .messaging
                .recv_ack_rebinding_time_outlives_lease
                .load(),
        };
        RequestingTestResult { outcome, counters }
    }

    fn build_test_lease_state() -> LeaseState<TestInstant> {
        build_test_lease_state_with_times(
            Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into()),
            None,
            None,
        )
    }

    fn build_test_lease_state_with_times(
        lease_length: Duration,
        renewal_time: Option<Duration>,
        rebinding_time: Option<Duration>,
    ) -> LeaseState<TestInstant> {
        LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR).try_into().expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(lease_length),
            start_time: TestInstant(std::time::Duration::from_secs(0)),
            renewal_time: renewal_time
                .map(RelativeTime::Explicit)
                .unwrap_or_else(|| default_renewal_time(lease_length)),
            rebinding_time: rebinding_time
                .map(RelativeTime::Explicit)
                .unwrap_or_else(|| default_rebinding_time(lease_length)),
        }
    }

    fn build_test_newly_acquired_lease() -> NewlyAcquiredLease<TestInstant> {
        NewlyAcquiredLease {
            ip_address: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            start_time: TestInstant(std::time::Duration::from_secs(0)),
            lease_time: Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into()),
            parameters: Vec::new(),
        }
    }

    #[test_case(
        (
            State::Init(Init::default()),
            Transition::BoundWithNewLease(
                Bound::AwaitingAssignment{lease_state: build_test_lease_state()},
                build_test_newly_acquired_lease()
            )
        ) => matches Some(TransitionEffect::HandleNewLease(_));
        "yields newly-acquired lease effect"
    )]
    #[test_case(
        (
            State::Bound(Bound::Assigned{lease_state: build_test_lease_state()}),
            Transition::Init(Init),
        ) => matches Some(TransitionEffect::DropLease {address_rejected: false});
        "recognizes loss of lease"
    )]
    #[test_case(
        (
            State::Bound(Bound::Assigned{lease_state: build_test_lease_state()}),
            Transition::WaitingToRestart(WaitingToRestart {
                waiting_until: TestInstant(WAIT_TIME_BEFORE_RESTARTING_AFTER_ADDRESS_REJECTION)
            }),
        ) => matches Some(TransitionEffect::DropLease {address_rejected: true});
        "recognizes address rejection"
    )]

    fn apply_transition(
        (state, transition): (State<TestInstant>, Transition<TestInstant>),
    ) -> Option<TransitionEffect<TestInstant>> {
        let (_next_state, effect) = state.apply(&test_client_config(), transition);
        effect
    }

    enum AddrRemovedExpect {
        Decline,
        Exit,
        Ignore,
    }

    #[test_case(
        State::Selecting(build_test_selecting_state()),
        AddrRemovedExpect::Ignore;
        "selecting_ignore"
    )]
    #[test_case(
        State::Requesting(build_test_requesting_state()),
        AddrRemovedExpect::Ignore;
        "requesting_ignore"
    )]
    #[test_case(
        State::Bound(Bound::AwaitingAssignment{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Exit;
        "bound_awaiting_assignment_exit"
    )]
    #[test_case(
        State::Bound(Bound::AwaitingAssignment{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Decline;
        "bound_awaiting_assignment_decline"
    )]
    #[test_case(
        State::Bound(Bound::Assigned{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Exit;
        "bound_assigned_exit"
    )]
    #[test_case(
        State::Bound(Bound::Assigned{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Decline;
        "bound_assigned_decline"
    )]
    #[test_case(
        State::Renewing(Renewing{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Exit;
        "renewing_exit"
    )]
    #[test_case(
        State::Renewing(Renewing{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Decline;
        "renewing_decline"
    )]
    #[test_case(
        State::Rebinding(Rebinding{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Exit;
        "rebinding_exit"
    )]
    #[test_case(
        State::Rebinding(Rebinding{lease_state: build_test_lease_state()}),
        AddrRemovedExpect::Decline;
        "rebinding_decline"
    )]
    #[test_case(
        State::WaitingToRestart(WaitingToRestart {
            waiting_until: TestInstant(WAIT_TIME_BEFORE_RESTARTING_AFTER_ADDRESS_REJECTION)
        }),
        AddrRemovedExpect::Ignore;
        "waiting_to_restart_ignore"
    )]
    fn on_address_removed(state: State<TestInstant>, expect: AddrRemovedExpect) {
        let config = &test_client_config();
        let time = &FakeTimeController::new();
        let mut rng = FakeRngProvider::new(0);
        let (server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (_server_end, udp_client_end) = FakeSocket::new_pair();
        let udp_socket_provider = &FakeSocketProvider::new(udp_client_end);
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let counters = Counters::default();

        let address_event_receiver = futures::stream::once(async {
            match expect {
                AddrRemovedExpect::Decline => AddressEvent::Rejected,
                AddrRemovedExpect::Exit => AddressEvent::Removed(FakeRemovedReason),
                AddrRemovedExpect::Ignore => panic!("address_event_receiver should not be polled"),
            }
        })
        .chain(futures::stream::pending());

        let run_result = state
            .run(
                config,
                packet_socket_provider,
                udp_socket_provider,
                &mut rng,
                time,
                &mut stop_receiver,
                address_event_receiver,
                &counters,
            )
            .now_or_never();

        match expect {
            AddrRemovedExpect::Decline => {
                let run_result = run_result.expect("fut should finish when declining");
                let WaitingToRestart { waiting_until } = assert_matches!(
                    run_result,
                    Ok(Step::NextState(Transition::WaitingToRestart(waiting))) => waiting
                );
                assert_eq!(waiting_until, TestInstant(Duration::from_secs(10)));

                let mut buf = [0u8; BUFFER_SIZE];
                let DatagramInfo { length, address } = server_end
                    .recv_from(&mut buf)
                    .now_or_never()
                    .expect("should be ready")
                    .expect("should succeed");
                assert_eq!(address, Mac::BROADCAST);

                let (_src_addr, message) =
                    crate::parse::parse_dhcp_message_from_ip_packet(&buf[..length], SERVER_PORT)
                        .expect("should succeed");

                use dhcp_protocol::DhcpOption;
                assert_outgoing_message_when_not_assigned_address(
                    &message,
                    VaryingOutgoingMessageFields {
                        xid: message.xid,
                        options: vec![
                            DhcpOption::DhcpMessageType(dhcp_protocol::MessageType::DHCPDECLINE),
                            DhcpOption::ServerIdentifier(SERVER_IP),
                            DhcpOption::RequestedIpAddress(YIADDR),
                        ],
                    },
                );
            }
            AddrRemovedExpect::Exit => {
                let run_result = run_result.expect("fut should finish when exiting");
                assert_matches!(
                    run_result,
                    Ok(Step::Exit(ExitReason::AddressRemoved(FakeRemovedReason)))
                );
            }
            AddrRemovedExpect::Ignore => {
                assert_matches!(run_result, None, "fut should not finish when ignored.");
            }
        }
    }

    enum AddrAssignmentChangeExpect {
        Ignore,
        EnterAssigned,
    }

    #[test_case(
        State::Selecting(build_test_selecting_state()),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::Ignore;
        "selecting_ignores"
    )]
    #[test_case(
        State::Requesting(build_test_requesting_state()),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::Ignore;
        "requesting_ignores"
    )]
    #[test_case(
        State::Bound(Bound::AwaitingAssignment{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Tentative,
        AddrAssignmentChangeExpect::Ignore;
        "bound_awaiting_assignment_ignores_tentative"
    )]
    #[test_case(
        State::Bound(Bound::AwaitingAssignment{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::EnterAssigned;
        "bound_awaiting_assignment_enter_assigned"
    )]
    #[test_case(
        State::Bound(Bound::Assigned{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Tentative,
        AddrAssignmentChangeExpect::Ignore;
        "bound_assigned_ignores_tentative"
    )]
    #[test_case(
        State::Bound(Bound::Assigned{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::Ignore;
        "bound_assigned_ignores_assigned"
    )]
    #[test_case(
        State::Renewing(Renewing{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Tentative,
        AddrAssignmentChangeExpect::Ignore;
        "renewing_ignores_tentative"
    )]
    #[test_case(
        State::Renewing(Renewing{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::Ignore;
        "renewing_ignores_assigned"
    )]
    #[test_case(
        State::Rebinding(Rebinding{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Tentative,
        AddrAssignmentChangeExpect::Ignore;
        "rebinding_ignores_tentative"
    )]
    #[test_case(
        State::Rebinding(Rebinding{lease_state: build_test_lease_state()}),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::Ignore;
        "rebinding_ignores_assigned"
    )]
    #[test_case(
        State::WaitingToRestart(WaitingToRestart {
            waiting_until: TestInstant(WAIT_TIME_BEFORE_RESTARTING_AFTER_ADDRESS_REJECTION)
        }),
        AddressAssignmentState::Assigned,
        AddrAssignmentChangeExpect::Ignore;
        "waiting_to_restart_ignores"
    )]
    fn on_address_assignment_change(
        state: State<TestInstant>,
        change: AddressAssignmentState,
        expect: AddrAssignmentChangeExpect,
    ) {
        let config = &test_client_config();
        let time = &FakeTimeController::new();
        let mut rng = FakeRngProvider::new(0);
        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (_server_end, udp_client_end) = FakeSocket::new_pair();
        let udp_socket_provider = &FakeSocketProvider::new(udp_client_end);
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let counters = Counters::default();

        let address_event_receiver = futures::stream::once(futures::future::ready(
            AddressEvent::<FakeRemovedReason>::AssignmentStateChanged(change),
        ))
        .chain(futures::stream::pending());
        let run_result = state
            .run(
                config,
                packet_socket_provider,
                udp_socket_provider,
                &mut rng,
                time,
                &mut stop_receiver,
                address_event_receiver,
                &counters,
            )
            .now_or_never();

        match expect {
            AddrAssignmentChangeExpect::EnterAssigned => {
                let run_result = run_result.expect("fut should finish when exiting");
                assert_matches!(
                    run_result,
                    Ok(Step::NextState(Transition::BoundAssigned(Bound::Assigned { .. })))
                );
            }
            AddrAssignmentChangeExpect::Ignore => {
                assert_matches!(run_result, None, "fut should not finish when ignored.");
            }
        }
    }

    #[test]
    fn waiting_to_restart() {
        let time = &FakeTimeController::new();

        const WAITING_UNTIL: TestInstant = TestInstant(Duration::from_secs(10));

        // Set the start time to some arbitrary time below WAITING_UNTIL to show
        // that `WaitingToRestart` waits until an absolute time rather than for
        // a particular duration.
        advance(time, Duration::from_secs(3));

        let waiting = WaitingToRestart { waiting_until: WAITING_UNTIL };
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let main_fut = waiting.do_waiting_to_restart(time, &mut stop_receiver).fuse();
        let mut main_fut = pin!(main_fut);
        let mut executor = fasync::TestExecutor::new();
        let outcome = run_with_accelerated_time(&mut executor, time, &mut main_fut);
        assert_eq!(outcome, WaitingToRestartOutcome::Init(Init));

        assert_eq!(time.now(), WAITING_UNTIL);
    }

    #[test]
    fn bound_awaiting_assignment_times_out() {
        let time = &FakeTimeController::new();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let config = &test_client_config();
        let counters = Counters::default();
        let bound = Bound::AwaitingAssignment { lease_state: build_test_lease_state() };
        let main_fut = bound
            .do_bound(
                config,
                time,
                &mut stop_receiver,
                packet_socket_provider,
                address_event_receiver,
                &counters,
            )
            .fuse();
        let mut main_fut = pin!(main_fut);
        let mut executor = fasync::TestExecutor::new();
        let outcome = run_with_accelerated_time(&mut executor, time, &mut main_fut)
            .expect("do_bound should succeed");
        assert_eq!(outcome, BoundOutcome::Restart(Init::default()));
        assert_eq!(
            time.now(),
            TestInstant(Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into()))
        );
    }

    #[test_case(
        build_test_lease_state() =>
        TestInstant(Duration::from_secs(u64::from(DEFAULT_LEASE_LENGTH_SECONDS) / 2));
        "waits default renewal time when not specified")]
    #[test_case(
        LeaseState {
            renewal_time: RelativeTime::Explicit(Duration::from_secs(10)),
            ..build_test_lease_state()
        } => TestInstant(Duration::from_secs(10));
        "waits specified renewal time")]
    fn bound_assigned_waits_for_renewal_time(lease_state: LeaseState<TestInstant>) -> TestInstant {
        let time = &FakeTimeController::new();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let config = &test_client_config();
        let counters = Counters::default();
        let bound = Bound::Assigned { lease_state };
        let main_fut = bound
            .do_bound(
                config,
                time,
                &mut stop_receiver,
                packet_socket_provider,
                address_event_receiver,
                &counters,
            )
            .fuse();
        let mut main_fut = pin!(main_fut);
        let mut executor = fasync::TestExecutor::new();
        let outcome = run_with_accelerated_time(&mut executor, time, &mut main_fut)
            .expect("do_bound should succeed");
        assert_eq!(outcome, BoundOutcome::Renewing(Renewing { lease_state: lease_state.clone() }));
        time.now()
    }

    #[test_case(Bound::Assigned {lease_state: build_test_lease_state()}; "assigned")]
    #[test_case(Bound::AwaitingAssignment {lease_state: build_test_lease_state()};
        "awaiting_assignment")]
    fn bound_obeys_graceful_shutdown(bound: Bound<TestInstant>) {
        let time = &FakeTimeController::new();
        let (stop_sender, mut stop_receiver) = mpsc::unbounded();
        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let config = &test_client_config();
        let counters = Counters::default();
        let bound_fut = bound
            .do_bound(
                &config,
                time,
                &mut stop_receiver,
                packet_socket_provider,
                address_event_receiver,
                &counters,
            )
            .fuse();

        stop_sender.unbounded_send(()).expect("send should succeed");
        assert_eq!(
            bound_fut
                .now_or_never()
                .expect("should have completed")
                .expect("do_bound should succeed"),
            BoundOutcome::GracefulShutdown
        );
    }

    fn build_test_renewing_state(
        lease_length: Duration,
        renewal_time: Option<Duration>,
        rebinding_time: Option<Duration>,
    ) -> Renewing<TestInstant> {
        Renewing {
            lease_state: build_test_lease_state_with_times(
                lease_length,
                renewal_time,
                rebinding_time,
            ),
        }
    }

    #[test]
    fn do_renewing_obeys_graceful_shutdown() {
        initialize_logging();
        let counters = Counters::default();

        let renewing = build_test_renewing_state(
            Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into()),
            None,
            None,
        );
        let client_config = &test_client_config();

        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (_server_end, udp_client_end) = FakeSocket::new_pair();
        let udp_socket_provider = &FakeSocketProvider::new(udp_client_end);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let (stop_sender, mut stop_receiver) = mpsc::unbounded();
        let time = &FakeTimeController::new();

        let renewing_fut = renewing
            .do_renewing(
                client_config,
                udp_socket_provider,
                packet_socket_provider,
                time,
                &mut stop_receiver,
                address_event_receiver,
                &counters,
            )
            .fuse();
        let mut renewing_fut = pin!(renewing_fut);

        let mut executor = fasync::TestExecutor::new();
        assert_matches!(executor.run_until_stalled(&mut renewing_fut), std::task::Poll::Pending);

        stop_sender.unbounded_send(()).expect("sending stop signal should succeed");

        let renewing_result = renewing_fut.now_or_never().expect(
            "renewing_fut should complete after single poll after stop signal has been sent",
        );

        assert_matches!(renewing_result, Ok(RenewingOutcome::GracefulShutdown));
    }

    #[track_caller]
    fn assert_outgoing_message_when_assigned_address(
        got_message: &dhcp_protocol::Message,
        fields: VaryingOutgoingMessageFields,
    ) {
        let VaryingOutgoingMessageFields { xid, options } = fields;
        let want_message = dhcp_protocol::Message {
            op: dhcp_protocol::OpCode::BOOTREQUEST,
            xid,
            secs: 0,
            bdcast_flag: false,
            ciaddr: YIADDR,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: TEST_MAC_ADDRESS,
            sname: BString::default(),
            file: BString::default(),
            options,
        };
        assert_eq!(got_message, &want_message);
    }

    #[test]
    fn do_renewing_sends_requests() {
        initialize_logging();

        // Just needs to be larger than rebinding time.
        const LEASE_LENGTH: Duration = Duration::from_secs(100000);

        // Set to have timestamps be conveniently derivable from powers of 2.
        const RENEWAL_TIME: Duration = Duration::from_secs(0);
        const REBINDING_TIME: Duration = Duration::from_secs(1024);

        let renewing =
            build_test_renewing_state(LEASE_LENGTH, Some(RENEWAL_TIME), Some(REBINDING_TIME));
        let client_config = &test_client_config();

        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (udp_server_end, udp_client_end) = FakeSocket::new_pair();
        let (binds_sender, mut binds_receiver) = mpsc::unbounded();
        let udp_socket_provider =
            &FakeSocketProvider::new_with_events(udp_client_end, binds_sender);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let time = &FakeTimeController::new();
        let counters = Counters::default();
        let renewing_fut = pin!(
            renewing
                .do_renewing(
                    client_config,
                    udp_socket_provider,
                    packet_socket_provider,
                    time,
                    &mut stop_receiver,
                    address_event_receiver,
                    &counters
                )
                .fuse()
        );

        // Observe the "64, 4" instead of "64, 32" due to the 60 second minimum
        // retransmission delay.
        let expected_times_requests_are_sent =
            [1024, 512, 256, 128, 64, 4].map(|time_remaining_when_request_is_sent| {
                Duration::from_secs(1024 - time_remaining_when_request_is_sent)
            });

        let receive_fut =
            pin!(
                async {
                    for expected_time in expected_times_requests_are_sent {
                        let mut recv_buf = [0u8; BUFFER_SIZE];
                        let DatagramInfo { length, address } = udp_server_end
                            .recv_from(&mut recv_buf)
                            .await
                            .expect("recv_from on test socket should succeed");

                        assert_eq!(
                            address,
                            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                                SERVER_IP,
                                SERVER_PORT.get()
                            ))
                        );
                        assert_eq!(time.now(), TestInstant(expected_time));
                        let msg = dhcp_protocol::Message::from_buffer(&recv_buf[..length])
                            .expect("received packet should parse as DHCP message");

                        assert_outgoing_message_when_assigned_address(
                            &msg,
                            VaryingOutgoingMessageFields {
                                xid: msg.xid,
                                options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPREQUEST,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                        ],
                            },
                        );
                    }
                }
                .fuse()
            );

        let main_future = async { join!(renewing_fut, receive_fut) };
        let mut main_future = pin!(main_future);

        let mut executor = fasync::TestExecutor::new();
        let (requesting_result, ()) =
            run_with_accelerated_time(&mut executor, time, &mut main_future);

        assert_matches!(requesting_result, Ok(RenewingOutcome::Rebinding(_)));
        assert_matches!(udp_server_end.recv_from(&mut []).now_or_never(), None);

        let bound_socket_addr = binds_receiver
            .next()
            .now_or_never()
            .expect("should have completed")
            .expect("should be present");
        assert_eq!(
            bound_socket_addr,
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(YIADDR, CLIENT_PORT.into()))
        );
        assert_eq!(
            counters.renewing.messaging.send_message.load(),
            expected_times_requests_are_sent.len()
        );
        assert_eq!(
            counters.renewing.messaging.recv_time_out.load(),
            expected_times_requests_are_sent.len()
        );
    }

    #[derive(PartialEq, Debug)]
    struct RenewingTestResult {
        outcome: RenewingOutcome<TestInstant, FakeRemovedReason>,
        counters: RenewingTestCounters,
    }

    #[derive(PartialEq, Eq, Debug, Default)]
    struct RenewingTestCounters {
        send_message: usize,
        recv_time_out: usize,
        recv_failed_dhcp_parse: usize,
        recv_wrong_xid: usize,
        recv_wrong_chaddr: usize,
        recv_message: usize,
        recv_nak: usize,
        recv_missing_option: usize,
    }

    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RenewingTestResult {
        outcome: RenewingOutcome::Renewed(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RenewingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
    }; "successfully renews after receiving DHCPACK")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RenewingTestResult {
        outcome: RenewingOutcome::Renewed(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Previous(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RenewingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
    }; "successfully renews after receiving DHCPACK without lease time")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: OTHER_ADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RenewingTestResult {
        outcome: RenewingOutcome::NewAddress(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(OTHER_ADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(
                std::time::Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into())
            ),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RenewingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
    }; "observes new address from DHCPACK")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values_excluding_subnet_mask())
        .collect(),
    } => RenewingTestResult {
        outcome: RenewingOutcome::Rebinding(
            Rebinding {
                lease_state: build_test_lease_state()
        }),
        counters: RenewingTestCounters {
            send_message: 2,
            recv_time_out: 2,
            recv_message: 1,
            recv_missing_option: 1,
            ..Default::default()
        },
    }; "ignores replies lacking required option SubnetMask")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: Ipv4Addr::UNSPECIFIED,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPNAK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(SERVER_IP),
            dhcp_protocol::DhcpOption::Message(NAK_MESSAGE.to_owned()),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RenewingTestResult {
        outcome: RenewingOutcome::Nak(crate::parse::FieldsToRetainFromNak {
            server_identifier: net_types::ip::Ipv4Addr::from(SERVER_IP)
                .try_into()
                .expect("should be specified"),
            message: Some(NAK_MESSAGE.to_owned()),
            client_identifier: None,
        }),
        counters: RenewingTestCounters {
            send_message: 1,
            recv_message: 1,
            recv_nak: 1,
            ..Default::default()
        },
    }; "transitions to Init after receiving DHCPNAK")]
    fn do_renewing_transitions_on_reply(
        incoming_message: VaryingIncomingMessageFields,
    ) -> RenewingTestResult {
        initialize_logging();

        let renewing = build_test_renewing_state(
            Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into()),
            None,
            None,
        );
        let client_config = &test_client_config();

        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (udp_server_end, udp_client_end) = FakeSocket::new_pair();
        let udp_socket_provider = &FakeSocketProvider::new(udp_client_end);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let time = &FakeTimeController::new();
        let counters = Counters::default();
        let renewing_fut = pin!(
            renewing
                .do_renewing(
                    client_config,
                    udp_socket_provider,
                    packet_socket_provider,
                    time,
                    &mut stop_receiver,
                    address_event_receiver,
                    &counters
                )
                .fuse()
        );
        let renewing_fut = pin!(renewing_fut);

        let server_socket_addr =
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(SERVER_IP, SERVER_PORT.into()));

        let server_fut = pin!(
            async {
                let mut recv_buf = [0u8; BUFFER_SIZE];

                let DatagramInfo { length, address } = udp_server_end
                    .recv_from(&mut recv_buf)
                    .await
                    .expect("recv_from on test socket should succeed");
                assert_eq!(address, server_socket_addr);

                let msg = dhcp_protocol::Message::from_buffer(&recv_buf[..length])
                    .expect("received packet on test socket should parse as DHCP message");

                assert_outgoing_message_when_assigned_address(
                    &msg,
                    VaryingOutgoingMessageFields {
                        xid: msg.xid,
                        options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPREQUEST,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                        ],
                    },
                );

                let reply = build_incoming_message(msg.xid, incoming_message);

                udp_server_end
                    .send_to(
                        &reply.serialize(),
                        // Note that this is the address the client under test
                        // observes in `recv_from`.
                        server_socket_addr,
                    )
                    .await
                    .expect("send_to on test socket should succeed");
            }
            .fuse()
        );

        let main_future = async move {
            let (renewing_result, ()) = join!(renewing_fut, server_fut);
            renewing_result
        }
        .fuse();

        let mut main_future = pin!(main_future);

        let mut executor = fasync::TestExecutor::new();
        let renewing_result = run_with_accelerated_time(&mut executor, time, &mut main_future);

        let outcome = assert_matches!(renewing_result, Ok(outcome) => outcome);
        let counters = RenewingTestCounters {
            send_message: counters.renewing.messaging.send_message.load(),
            recv_time_out: counters.renewing.messaging.recv_time_out.load(),
            recv_failed_dhcp_parse: counters.renewing.messaging.recv_failed_dhcp_parse.load(),
            recv_wrong_xid: counters.renewing.messaging.recv_wrong_xid.load(),
            recv_wrong_chaddr: counters.renewing.messaging.recv_wrong_chaddr.load(),
            recv_message: counters.renewing.messaging.recv_message.load(),
            recv_nak: counters.renewing.recv_nak.load(),
            recv_missing_option: counters.renewing.recv_error.missing_required_option.load(),
        };
        RenewingTestResult { outcome, counters }
    }

    fn build_test_rebinding_state(
        lease_length: Duration,
        renewal_time: Option<Duration>,
        rebinding_time: Option<Duration>,
    ) -> Rebinding<TestInstant> {
        Rebinding {
            lease_state: build_test_lease_state_with_times(
                lease_length,
                renewal_time,
                rebinding_time,
            ),
        }
    }

    #[test]
    fn do_rebinding_sends_requests() {
        initialize_logging();

        // Set to have timestamps be conveniently derivable from powers of 2.
        const RENEWAL_TIME: Duration = Duration::from_secs(0);
        const REBINDING_TIME: Duration = Duration::from_secs(0);
        const LEASE_LENGTH: Duration = Duration::from_secs(1024);

        let rebinding =
            build_test_rebinding_state(LEASE_LENGTH, Some(RENEWAL_TIME), Some(REBINDING_TIME));
        let client_config = &test_client_config();

        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (udp_server_end, udp_client_end) = FakeSocket::new_pair();
        let (binds_sender, mut binds_receiver) = mpsc::unbounded();
        let udp_socket_provider =
            &FakeSocketProvider::new_with_events(udp_client_end, binds_sender);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let time = &FakeTimeController::new();
        let counters = Counters::default();
        let rebinding_fut = pin!(
            rebinding
                .do_rebinding(
                    client_config,
                    udp_socket_provider,
                    packet_socket_provider,
                    time,
                    &mut stop_receiver,
                    address_event_receiver,
                    &counters
                )
                .fuse()
        );

        // Observe the "64, 4" instead of "64, 32" due to the 60 second minimum
        // retransmission delay.
        let expected_times_requests_are_sent =
            [1024, 512, 256, 128, 64, 4].map(|time_remaining_when_request_is_sent| {
                Duration::from_secs(1024 - time_remaining_when_request_is_sent)
            });

        let receive_fut =
            pin!(
                async {
                    for expected_time in expected_times_requests_are_sent {
                        let mut recv_buf = [0u8; BUFFER_SIZE];
                        let DatagramInfo { length, address } = udp_server_end
                            .recv_from(&mut recv_buf)
                            .await
                            .expect("recv_from on test socket should succeed");

                        assert_eq!(
                            address,
                            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                                std::net::Ipv4Addr::BROADCAST,
                                SERVER_PORT.get()
                            ))
                        );
                        assert_eq!(time.now(), TestInstant(expected_time));
                        let msg = dhcp_protocol::Message::from_buffer(&recv_buf[..length])
                            .expect("received packet should parse as DHCP message");

                        assert_outgoing_message_when_assigned_address(
                            &msg,
                            VaryingOutgoingMessageFields {
                                xid: msg.xid,
                                options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPREQUEST,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                        ],
                            },
                        );
                    }
                }
                .fuse()
            );

        let main_future = async { join!(rebinding_fut, receive_fut) };
        let mut main_future = pin!(main_future);

        let mut executor = fasync::TestExecutor::new();
        let (requesting_result, ()) =
            run_with_accelerated_time(&mut executor, time, &mut main_future);

        assert_matches!(requesting_result, Ok(RebindingOutcome::TimedOut));
        assert_matches!(udp_server_end.recv_from(&mut []).now_or_never(), None);

        let bound_socket_addr = binds_receiver
            .next()
            .now_or_never()
            .expect("should have completed")
            .expect("should be present");
        assert_eq!(
            bound_socket_addr,
            std::net::SocketAddr::V4(std::net::SocketAddrV4::new(YIADDR, CLIENT_PORT.into()))
        );
    }

    #[derive(PartialEq, Debug)]
    struct RebindingTestResult {
        outcome: RebindingOutcome<TestInstant, FakeRemovedReason>,
        counters: RebindingTestCounters,
    }

    #[derive(PartialEq, Eq, Debug, Default)]
    struct RebindingTestCounters {
        send_message: usize,
        recv_time_out: usize,
        recv_failed_dhcp_parse: usize,
        recv_wrong_xid: usize,
        recv_wrong_chaddr: usize,
        recv_message: usize,
        recv_nak: usize,
        recv_missing_option: usize,
    }

    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(OTHER_SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RebindingTestResult {
        outcome: RebindingOutcome::Renewed(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(OTHER_SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RebindingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
    }; "successfully rebinds after receiving DHCPACK")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(OTHER_SERVER_IP),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RebindingTestResult {
        outcome: RebindingOutcome::Renewed(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(YIADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(OTHER_SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Previous(std::time::Duration::from_secs(
                DEFAULT_LEASE_LENGTH_SECONDS.into()
            )),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
        counters: RebindingTestCounters {
            send_message: 1,
            recv_message: 1,
            ..Default::default()
        }
    }; "successfully rebinds after receiving DHCPACK without lease time")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: OTHER_ADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(OTHER_SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RebindingTestResult {
        outcome: RebindingOutcome::NewAddress(LeaseState {
            discover_options: TEST_DISCOVER_OPTIONS,
            yiaddr: net_types::ip::Ipv4Addr::from(OTHER_ADDR)
                .try_into()
                .expect("should be specified"),
            server_identifier: net_types::ip::Ipv4Addr::from(OTHER_SERVER_IP)
                .try_into()
                .expect("should be specified"),
            ip_address_lease_time: RelativeTime::Explicit(
                std::time::Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into())
            ),
            renewal_time: DEFAULT_RENEWAL_TIME,
            rebinding_time: DEFAULT_REBINDING_TIME,
            start_time: TestInstant(std::time::Duration::from_secs(0)),
        }, test_parameter_values().into_iter().collect()),
    counters: RebindingTestCounters {
        send_message: 1,
        recv_message: 1,
        ..Default::default()
    }
 } ; "observes new address from DHCPACK")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: YIADDR,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPACK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(OTHER_SERVER_IP),
            dhcp_protocol::DhcpOption::IpAddressLeaseTime(
                DEFAULT_LEASE_LENGTH_SECONDS,
            ),
        ]
        .into_iter()
        .chain(test_parameter_values_excluding_subnet_mask())
        .collect(),
    } => RebindingTestResult {
        outcome: RebindingOutcome::TimedOut,
        counters: RebindingTestCounters {
            send_message: 2,
            recv_time_out: 2,
            recv_message: 1,
            recv_missing_option: 1,
            ..Default::default()
        }
    } ; "ignores replies lacking required option SubnetMask")]
    #[test_case(VaryingIncomingMessageFields {
        yiaddr: Ipv4Addr::UNSPECIFIED,
        options: [
            dhcp_protocol::DhcpOption::DhcpMessageType(
                dhcp_protocol::MessageType::DHCPNAK,
            ),
            dhcp_protocol::DhcpOption::ServerIdentifier(OTHER_SERVER_IP),
            dhcp_protocol::DhcpOption::Message(NAK_MESSAGE.to_owned()),
        ]
        .into_iter()
        .chain(test_parameter_values())
        .collect(),
    } => RebindingTestResult {
        outcome: RebindingOutcome::Nak(crate::parse::FieldsToRetainFromNak {
            server_identifier: net_types::ip::Ipv4Addr::from(OTHER_SERVER_IP)
                .try_into()
                .expect("should be specified"),
            message: Some(NAK_MESSAGE.to_owned()),
            client_identifier: None,
        }),
        counters: RebindingTestCounters {
            send_message: 1,
            recv_message: 1,
            recv_nak: 1,
            ..Default::default()
        },
    } ; "transitions to Init after receiving DHCPNAK")]
    fn do_rebinding_transitions_on_reply(
        incoming_message: VaryingIncomingMessageFields,
    ) -> RebindingTestResult {
        initialize_logging();

        let rebinding = build_test_rebinding_state(
            Duration::from_secs(DEFAULT_LEASE_LENGTH_SECONDS.into()),
            None,
            None,
        );
        let client_config = &test_client_config();

        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let (udp_server_end, udp_client_end) = FakeSocket::new_pair();
        let udp_socket_provider = &FakeSocketProvider::new(udp_client_end);
        let address_event_receiver = futures::stream::pending::<AddressEvent<FakeRemovedReason>>();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let time = &FakeTimeController::new();
        let counters = Counters::default();
        let rebinding_fut = pin!(
            rebinding
                .do_rebinding(
                    client_config,
                    udp_socket_provider,
                    packet_socket_provider,
                    time,
                    &mut stop_receiver,
                    address_event_receiver,
                    &counters
                )
                .fuse()
        );

        let server_socket_addr = std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
            OTHER_SERVER_IP,
            SERVER_PORT.into(),
        ));

        let server_fut = pin!(
            async {
                let mut recv_buf = [0u8; BUFFER_SIZE];

                let DatagramInfo { length, address } = udp_server_end
                    .recv_from(&mut recv_buf)
                    .await
                    .expect("recv_from on test socket should succeed");
                assert_eq!(
                    address,
                    std::net::SocketAddr::V4(std::net::SocketAddrV4::new(
                        std::net::Ipv4Addr::BROADCAST,
                        SERVER_PORT.into()
                    ))
                );

                let msg = dhcp_protocol::Message::from_buffer(&recv_buf[..length])
                    .expect("received packet on test socket should parse as DHCP message");

                assert_outgoing_message_when_assigned_address(
                    &msg,
                    VaryingOutgoingMessageFields {
                        xid: msg.xid,
                        options: vec![
                            dhcp_protocol::DhcpOption::DhcpMessageType(
                                dhcp_protocol::MessageType::DHCPREQUEST,
                            ),
                            dhcp_protocol::DhcpOption::ParameterRequestList(
                                test_requested_parameters()
                                    .iter_keys()
                                    .collect::<Vec<_>>()
                                    .try_into()
                                    .expect("should fit parameter request list size constraints"),
                            ),
                        ],
                    },
                );

                let reply = build_incoming_message(msg.xid, incoming_message);

                udp_server_end
                    .send_to(
                        &reply.serialize(),
                        // Note that this is the address the client under test
                        // observes in `recv_from`.
                        server_socket_addr,
                    )
                    .await
                    .expect("send_to on test socket should succeed");
            }
            .fuse()
        );

        let main_future = async move {
            let (rebinding_result, ()) = join!(rebinding_fut, server_fut);
            rebinding_result
        }
        .fuse();

        let mut main_future = pin!(main_future);

        let mut executor = fasync::TestExecutor::new();
        let rebinding_result = run_with_accelerated_time(&mut executor, time, &mut main_future);

        let outcome = assert_matches!(rebinding_result, Ok(outcome) => outcome);
        let counters = RebindingTestCounters {
            send_message: counters.rebinding.messaging.send_message.load(),
            recv_time_out: counters.rebinding.messaging.recv_time_out.load(),
            recv_failed_dhcp_parse: counters.rebinding.messaging.recv_failed_dhcp_parse.load(),
            recv_wrong_xid: counters.rebinding.messaging.recv_wrong_xid.load(),
            recv_wrong_chaddr: counters.rebinding.messaging.recv_wrong_chaddr.load(),
            recv_message: counters.rebinding.messaging.recv_message.load(),
            recv_nak: counters.rebinding.recv_nak.load(),
            recv_missing_option: counters.rebinding.recv_error.missing_required_option.load(),
        };
        RebindingTestResult { outcome, counters }
    }

    struct FailingUdpSocketProvider;

    impl deps::UdpSocketProvider for FailingUdpSocketProvider {
        type Sock = Rc<FakeSocket<std::net::SocketAddr>>;
        async fn bind_new_udp_socket(
            &self,
            _bound_addr: std::net::SocketAddr,
        ) -> Result<Self::Sock, deps::SocketError> {
            Err(deps::SocketError::AddrNotAvailable)
        }
    }

    #[test_case(
        State::Renewing(build_test_renewing_state(Duration::from_secs(100), None, None));
        "renewing"
    )]
    #[test_case(
        State::Rebinding(build_test_rebinding_state(Duration::from_secs(100), None, None));
        "rebinding"
    )]
    fn return_address_removed_on_udp_socket_addr_not_available(state: State<TestInstant>) {
        initialize_logging();
        let counters = Counters::default();
        let client_config = &test_client_config();
        let (_server_end, packet_client_end) = FakeSocket::new_pair();
        let packet_socket_provider = &FakeSocketProvider::new(packet_client_end);
        let udp_socket_provider = &FailingUdpSocketProvider;
        let address_event_receiver =
            futures::stream::once(async { AddressEvent::Removed(FakeRemovedReason) }).fuse();
        let (_stop_sender, mut stop_receiver) = mpsc::unbounded();
        let time = &FakeTimeController::new();
        let mut rng = FakeRngProvider::new(0);

        let mut executor = fasync::TestExecutor::new();
        let mut fut = pin!(state.run(
            client_config,
            packet_socket_provider,
            udp_socket_provider,
            &mut rng,
            time,
            &mut stop_receiver,
            address_event_receiver,
            &counters,
        ));

        let result = executor.run_until_stalled(&mut fut);
        let result = assert_matches!(result, std::task::Poll::Ready(r) => r);
        let step = assert_matches!(result, Ok(s) => s);
        assert_matches!(step, Step::Exit(ExitReason::AddressRemoved(FakeRemovedReason)));
    }
}
