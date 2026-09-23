// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::fmt::Debug;
use core::time::Duration;

use derivative::Derivative;
use netstack3_base::{Inspectable, Inspector, Instant};

use crate::internal::buffer::{ReceiveBuffer, SendBuffer};
use crate::internal::congestion::LossRecoveryMode;
use crate::internal::counters::TcpCountersWithSocketInner;
use crate::internal::state::{
    CloseWait, Closed, Closing, Established, FinWait1, FinWait2, LastAck, Listen, Recv, RecvParams,
    Send, State, SynRcvd, SynSent, TimeWait,
};

/// Information about a TCP socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpSocketInfo<I> {
    /// The current state of the TCP state machine.
    pub state: netstack3_base::TcpSocketState,
    /// The congestion control state.
    pub ca_state: CongestionControlState,
    /// The current RTO.
    pub rto: Option<Duration>,
    /// The estimated RTT.
    pub rtt: Option<Duration>,
    /// The RTT variance.
    pub rtt_var: Option<Duration>,
    /// The slow start threshold.
    pub snd_ssthresh: u32,
    /// The congestion window.
    pub snd_cwnd: u32,
    /// The number of retransmissions.
    pub retransmits: u64,
    /// Timestamp of the last ACK received.
    pub last_ack_recv: Option<I>,
    /// Segments sent.
    pub segs_out: u64,
    /// Segments received.
    pub segs_in: u64,
    /// The sender maximum segment size.
    pub snd_mss: Option<u32>,
    /// The receiver maximum segment size.
    pub rcv_mss: Option<u32>,
    /// Timestamp of the last data sent.
    pub last_data_sent: Option<I>,
}

impl<I: Instant> Inspectable for TcpSocketInfo<I> {
    fn record<II: Inspector>(&self, inspector: &mut II) {
        let Self {
            ca_state,
            rto,
            rtt,
            rtt_var,
            snd_ssthresh,
            snd_cwnd,
            last_ack_recv,
            last_data_sent,
            snd_mss,
            rcv_mss,

            // Already recorded by the caller.
            state: _,
            // These metrics are exported under the`Counters` inspect node.
            retransmits: _,
            segs_out: _,
            segs_in: _,
        } = self;

        inspector.record_debug("CongestionControlState", ca_state);
        if let Some(rto) = rto {
            inspector.record_uint("RtoMs", u64::try_from(rto.as_millis()).unwrap_or(u64::MAX));
        }
        if let Some(rtt) = rtt {
            inspector.record_uint("RttMs", u64::try_from(rtt.as_millis()).unwrap_or(u64::MAX));
        }
        if let Some(rtt_var) = rtt_var {
            inspector
                .record_uint("RttVarMs", u64::try_from(rtt_var.as_millis()).unwrap_or(u64::MAX));
        }
        inspector.record_uint("SndSsthresh", *snd_ssthresh);
        inspector.record_uint("SndCwnd", *snd_cwnd);
        if let Some(last_ack_recv) = last_ack_recv {
            inspector.record_inspectable_value("LastAckRecv", last_ack_recv);
        }
        if let Some(last_data_sent) = last_data_sent {
            inspector.record_inspectable_value("LastDataSent", last_data_sent);
        }
        if let Some(snd_mss) = snd_mss {
            inspector.record_uint("SndMss", *snd_mss);
        }
        if let Some(rcv_mss) = rcv_mss {
            inspector.record_uint("RcvMss", *rcv_mss);
        }
    }
}

/// The state of congestion control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CongestionControlState {
    /// Normal state, no congestion detected.
    #[default]
    Open,
    /// Disorder detected (e.g. DUP-ACKs).
    Disorder,
    /// We received an Explicit Congestion Notification.
    CongestionWindowReduced,
    /// In recovery (Fast Recovery).
    Recovery,
    /// Loss detected (RTO).
    Loss,
}

/// Helper struct to hold parameters extracted from [`Send`] that will end
/// up in [`TcpSocketInfo`].
#[derive(Debug, Clone, Derivative)]
#[derivative(Default(bound = ""))]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub(crate) struct SendInfo<I> {
    rto: Option<Duration>,
    rtt: Option<Duration>,
    rtt_var: Option<Duration>,
    snd_ssthresh: u32,
    snd_cwnd: u32,
    snd_mss: Option<u32>,
    last_data_sent: Option<I>,
    ca_state: CongestionControlState,
}

impl<I: Instant> SendInfo<I> {
    pub(super) fn from_send<S: SendBuffer, const FIN: bool>(snd: &Send<I, S, FIN>) -> Self {
        let cc = &snd.congestion_control;
        let est = &snd.rtt_estimator;

        Self {
            snd_cwnd: cc.inspect_cwnd().cwnd(),
            snd_ssthresh: cc.slow_start_threshold(),
            ca_state: match cc.inspect_loss_recovery_mode() {
                Some(LossRecoveryMode::FastRecovery) => CongestionControlState::Recovery,
                Some(LossRecoveryMode::SackRecovery) => CongestionControlState::Recovery,
                None => CongestionControlState::Open,
            },
            rtt: est.srtt(),
            rtt_var: est.rtt_var(),
            rto: Some(est.rto().into()),
            snd_mss: Some(u32::from(cc.mss())),
            last_data_sent: snd.last_data_sent,
        }
    }
}

/// Helper struct to hold parameters extracted from [`Recv`] that will end
/// up in [`TcpSocketInfo`].
#[derive(Debug, Clone, Derivative)]
#[derivative(Default(bound = ""))]
#[cfg_attr(test, derive(PartialEq, Eq))]
struct RecvInfo<I> {
    last_ack_recv: Option<I>,
    rcv_mss: Option<u32>,
}

impl<I: Instant> RecvInfo<I> {
    fn from_recv<R: ReceiveBuffer>(rcv: &Recv<I, R>) -> Self {
        Self { last_ack_recv: rcv.last_segment_at, rcv_mss: Some(u32::from(rcv.mss)) }
    }

    fn from_recv_params(rcv: &RecvParams<I>) -> Self {
        let RecvParams { last_ack_recv, ack: _, wnd_scale: _, wnd: _, ts_opt: _ } = rcv;
        Self { last_ack_recv: *last_ack_recv, rcv_mss: None }
    }
}

/// Helper struct to hold parameters extracted from socket counters that
/// will end up in [`TcpSocketInfo`].
struct CounterParams {
    retransmits: u64,
    segs_out: u64,
    segs_in: u64,
}

impl CounterParams {
    fn from_counters(counters: &TcpCountersWithSocketInner) -> Self {
        Self {
            retransmits: counters.retransmits.get(),
            segs_out: counters.segments_sent.get(),
            segs_in: counters.received_segments_dispatched.get(),
        }
    }
}

impl<I: Instant> TcpSocketInfo<I> {
    /// Constructs a [`TcpSocketInfo`] from just the state machine state and
    /// counters. This is useful when the full state machine doesn't exist.
    pub(crate) fn from_partial_state(
        state: netstack3_base::TcpSocketState,
        counters: &TcpCountersWithSocketInner,
    ) -> Self {
        let SendInfo {
            rto,
            rtt,
            rtt_var,
            snd_ssthresh,
            snd_cwnd,
            last_data_sent,
            ca_state,
            snd_mss: _,
        } = SendInfo::default();
        let RecvInfo { last_ack_recv, rcv_mss: _ } = RecvInfo::default();
        let CounterParams { retransmits, segs_out, segs_in } =
            CounterParams::from_counters(counters);

        Self {
            state,
            retransmits,
            segs_out,
            ca_state,
            rto,
            rtt,
            rtt_var,
            snd_ssthresh,
            snd_cwnd,
            last_ack_recv,
            segs_in,
            snd_mss: None,
            rcv_mss: None,
            last_data_sent,
        }
    }

    /// Constructs a [`TcpSocketInfo`] from the full state machine state
    /// and counters.
    pub(crate) fn from_full_state<R, S, ActiveOpen>(
        state: &State<I, R, S, ActiveOpen>,
        counters: &TcpCountersWithSocketInner,
    ) -> Self
    where
        R: ReceiveBuffer,
        S: SendBuffer,
    {
        let (state, send_params, recv_params) = match state {
            State::Closed(Closed { reason: _ }) => {
                (netstack3_base::TcpSocketState::Close, SendInfo::default(), RecvInfo::default())
            }
            State::Listen(Listen {
                iss: _,
                timestamp_offset: _,
                buffer_sizes: _,
                device_mss: _,
                default_mss: _,
                user_timeout: _,
            }) => {
                (netstack3_base::TcpSocketState::Listen, SendInfo::default(), RecvInfo::default())
            }
            State::SynSent(SynSent {
                iss: _,
                rtt_sampler: _,
                retrans_timer: _,
                active_open: _,
                buffer_sizes: _,
                device_mss: _,
                default_mss: _,
                rcv_wnd_scale: _,
                ts_opt: _,
            }) => {
                (netstack3_base::TcpSocketState::SynSent, SendInfo::default(), RecvInfo::default())
            }
            State::SynRcvd(SynRcvd {
                iss: _,
                irs: _,
                rtt_sampler: _,
                retrans_timer: _,
                simultaneous_open: _,
                buffer_sizes: _,
                smss: _,
                rcv_wnd_scale: _,
                snd_wnd_scale: _,
                sack_permitted: _,
                // NB: This is of type RecvParams (unrelated), not Recv.
                rcv: _,
            }) => {
                (netstack3_base::TcpSocketState::SynRecv, SendInfo::default(), RecvInfo::default())
            }
            State::Established(Established { snd, rcv }) => (
                netstack3_base::TcpSocketState::Established,
                SendInfo::from_send(snd.get()),
                RecvInfo::from_recv(rcv.get()),
            ),
            State::FinWait1(FinWait1 { snd, rcv }) => (
                netstack3_base::TcpSocketState::FinWait1,
                SendInfo::from_send(snd.get()),
                RecvInfo::from_recv(rcv.get()),
            ),
            State::FinWait2(FinWait2 { last_seq: _, rcv, timeout_at: _, snd_info }) => (
                netstack3_base::TcpSocketState::FinWait2,
                snd_info.clone(),
                RecvInfo::from_recv(rcv),
            ),
            State::CloseWait(CloseWait { snd, closed_rcv }) => (
                netstack3_base::TcpSocketState::CloseWait,
                SendInfo::from_send(snd.get()),
                RecvInfo::from_recv_params(closed_rcv),
            ),
            State::Closing(Closing { snd, closed_rcv }) => (
                netstack3_base::TcpSocketState::Closing,
                SendInfo::from_send(snd),
                RecvInfo::from_recv_params(closed_rcv),
            ),
            State::LastAck(LastAck { snd, closed_rcv }) => (
                netstack3_base::TcpSocketState::LastAck,
                SendInfo::from_send(snd),
                RecvInfo::from_recv_params(closed_rcv),
            ),
            State::TimeWait(TimeWait { last_seq: _, expiry: _, closed_rcv, snd_info }) => (
                netstack3_base::TcpSocketState::TimeWait,
                snd_info.clone(),
                RecvInfo::from_recv_params(closed_rcv),
            ),
        };

        let SendInfo {
            snd_cwnd,
            snd_ssthresh,
            ca_state,
            rtt,
            rtt_var,
            rto,
            snd_mss,
            last_data_sent,
        } = send_params;
        let RecvInfo { last_ack_recv, rcv_mss } = recv_params;
        let CounterParams { retransmits, segs_out, segs_in } =
            CounterParams::from_counters(counters);

        Self {
            state,
            ca_state,
            rto,
            rtt,
            rtt_var,
            snd_ssthresh,
            snd_cwnd,
            retransmits,
            last_ack_recv,
            segs_out,
            segs_in,
            snd_mss,
            rcv_mss,
            last_data_sent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use core::time::Duration;

    use netstack3_base::testutil::FakeInstant;
    use netstack3_base::{
        EffectiveMss, Mss, MssSizeLimiters, SeqNum, TcpSocketState, Timestamp, WindowScale,
        WindowSize,
    };

    use crate::internal::buffer::Assembler;
    use crate::internal::congestion::CongestionControl;
    use crate::internal::rtt::{Estimator, SamplingStrategy};
    use crate::internal::state::{Established, Listen, Recv, RecvBufferState, Send, State};
    use crate::internal::timestamp::TimestampOptionState;
    use crate::testutil::RingBuffer;

    impl TcpCountersWithSocketInner {
        fn new_for_test(retransmits: u64, segs_out: u64, segs_in: u64) -> Self {
            let counters = Self::default();
            counters.retransmits.add(retransmits);
            counters.segments_sent.add(segs_out);
            counters.received_segments_dispatched.add(segs_in);
            counters
        }
    }

    #[test]
    fn test_from_partial_state() {
        let state = TcpSocketState::Listen;
        let counters = TcpCountersWithSocketInner::new_for_test(5, 10, 20);

        let info = TcpSocketInfo::<FakeInstant>::from_partial_state(state, &counters);

        assert_eq!(
            info,
            TcpSocketInfo {
                state: TcpSocketState::Listen,
                retransmits: 5,
                segs_out: 10,
                segs_in: 20,

                // These are just the defaults.
                ca_state: CongestionControlState::Open,
                rto: None,
                rtt: None,
                rtt_var: None,
                snd_ssthresh: 0,
                snd_cwnd: 0,
                last_ack_recv: None,
                snd_mss: None,
                rcv_mss: None,
                last_data_sent: None,
            }
        );
    }

    #[test]
    fn test_from_full_state_listen() {
        let state = State::<FakeInstant, RingBuffer, RingBuffer, ()>::Listen(Listen {
            iss: SeqNum::new(100),
            timestamp_offset: Timestamp::new(0),
            buffer_sizes: Default::default(),
            device_mss: Mss::new(1460).unwrap(),
            default_mss: Mss::new(536).unwrap(),
            user_timeout: None,
        });

        let counters = TcpCountersWithSocketInner::new_for_test(5, 10, 20);

        let info = TcpSocketInfo::from_full_state(&state, &counters);

        assert_eq!(
            info,
            TcpSocketInfo {
                state: TcpSocketState::Listen,
                retransmits: 5,
                segs_out: 10,
                segs_in: 20,

                // These are all defaults.
                ca_state: CongestionControlState::Open,
                rto: None,
                rtt: None,
                rtt_var: None,
                snd_ssthresh: 0,
                snd_cwnd: 0,
                last_ack_recv: None,
                snd_mss: None,
                rcv_mss: None,
                last_data_sent: None,
            }
        );
    }

    #[test]
    fn test_from_full_state_established() {
        let now = FakeInstant::from(Duration::from_secs(10));
        let mss = Mss::new(1460).unwrap();
        let effective_mss =
            EffectiveMss::from_mss(mss, MssSizeLimiters { timestamp_enabled: false });

        let mut congestion_control = CongestionControl::cubic_with_mss(effective_mss);
        congestion_control.inflate_cwnd(u32::from(mss));

        let mut rtt_estimator = Estimator::default();
        let rtt_sampler = SamplingStrategy::default();
        let rtt = Duration::from_millis(50);
        rtt_estimator.sample(
            rtt,
            rtt_sampler.samples_per_round_trip(&effective_mss, congestion_control.flight_size()),
        );

        let send = Send {
            nxt: SeqNum::new(200),
            max: SeqNum::new(200),
            una: SeqNum::new(100),
            wnd: WindowSize::new(1000).unwrap(),
            wnd_scale: WindowScale::default(),
            wnd_max: WindowSize::new(1000).unwrap(),
            wl1: SeqNum::new(100),
            wl2: SeqNum::new(100),
            last_push: SeqNum::new(200),
            rtt_sampler,
            rtt_estimator,
            timer: None,
            congestion_control,
            last_data_sent: Some(now - Duration::from_secs(1)),
            buffer: RingBuffer::new(1000),
        };

        let recv = Recv {
            buffer: RecvBufferState::Open {
                buffer: RingBuffer::new(1000),
                assembler: Assembler::new(SeqNum::new(50)),
            },
            remaining_quickacks: Default::default(),
            last_segment_at: Some(now - Duration::from_secs(2)),
            timer: None,
            mss: effective_mss,
            wnd_scale: WindowScale::default(),
            last_window_update: (SeqNum::new(50), WindowSize::new(1000).unwrap()),
            sack_permitted: false,
            ts_opt: TimestampOptionState::Disabled,
        };

        let state = State::<FakeInstant, RingBuffer, RingBuffer, ()>::Established(Established {
            snd: send.into(),
            rcv: recv.into(),
        });

        let counters = TcpCountersWithSocketInner::new_for_test(5, 10, 20);
        let info = TcpSocketInfo::from_full_state(&state, &counters);

        assert_eq!(
            info,
            TcpSocketInfo {
                state: TcpSocketState::Established,
                ca_state: CongestionControlState::Open,
                rto: Some(Duration::from_millis(200)),
                rtt: Some(rtt),
                rtt_var: Some(rtt / 2),
                snd_ssthresh: u32::MAX,
                snd_cwnd: 4380 + 1460,
                retransmits: 5,
                last_ack_recv: Some(now - Duration::from_secs(2)),
                segs_out: 10,
                segs_in: 20,
                snd_mss: Some(1460),
                rcv_mss: Some(1460),
                last_data_sent: Some(now - Duration::from_secs(1))
            }
        );
    }

    #[test]
    fn test_from_full_state_recovery() {
        let mss = Mss::new(1460).unwrap();
        let effective_mss =
            EffectiveMss::from_mss(mss, MssSizeLimiters { timestamp_enabled: false });

        let mut congestion_control = CongestionControl::cubic_with_mss(effective_mss);
        // Trigger recovery by receiving 3 duplicate ACKs
        let ack = SeqNum::new(100);
        let nxt = SeqNum::new(200);
        let _ = congestion_control.on_dup_ack(ack, nxt);
        let _ = congestion_control.on_dup_ack(ack, nxt);
        let _ = congestion_control.on_dup_ack(ack, nxt);

        let send = Send {
            nxt,
            max: nxt,
            una: ack,
            wnd: WindowSize::new(1000).unwrap(),
            wnd_scale: WindowScale::default(),
            wnd_max: WindowSize::new(1000).unwrap(),
            wl1: ack,
            wl2: ack,
            last_push: nxt,
            rtt_sampler: SamplingStrategy::default(),
            rtt_estimator: Estimator::default(),
            timer: None,
            congestion_control,
            last_data_sent: None,
            buffer: RingBuffer::new(1000),
        };

        let recv = Recv {
            buffer: RecvBufferState::Open {
                buffer: RingBuffer::new(1000),
                assembler: Assembler::new(SeqNum::new(50)),
            },
            remaining_quickacks: Default::default(),
            last_segment_at: None,
            timer: None,
            mss: effective_mss,
            wnd_scale: WindowScale::default(),
            last_window_update: (SeqNum::new(50), WindowSize::new(1000).unwrap()),
            sack_permitted: false,
            ts_opt: TimestampOptionState::Disabled,
        };

        let state = State::<FakeInstant, RingBuffer, RingBuffer, ()>::Established(Established {
            snd: send.into(),
            rcv: recv.into(),
        });

        let counters = TcpCountersWithSocketInner::new_for_test(5, 10, 20);
        let info = TcpSocketInfo::from_full_state(&state, &counters);

        assert_eq!(
            info,
            TcpSocketInfo {
                state: TcpSocketState::Established,
                retransmits: 5,
                segs_out: 10,
                segs_in: 20,
                ca_state: CongestionControlState::Recovery,
                rto: Some(Duration::from_secs(1)),
                rtt: None,
                rtt_var: None,
                snd_ssthresh: 2920,
                snd_cwnd: 7300,
                last_ack_recv: None,
                snd_mss: Some(1460),
                rcv_mss: Some(1460),
                last_data_sent: None,
            }
        );
    }

    #[cfg(target_os = "fuchsia")]
    #[test]
    fn test_inspect_tcp_socket_info() {
        use diagnostics_assertions::assert_data_tree;
        use diagnostics_traits::FuchsiaInspector;
        use fuchsia_inspect::Inspector;

        let info = TcpSocketInfo::<FakeInstant> {
            state: TcpSocketState::Established,
            ca_state: CongestionControlState::Open,
            rto: Some(Duration::from_millis(200)),
            rtt: Some(Duration::from_millis(50)),
            rtt_var: Some(Duration::from_millis(25)),
            snd_ssthresh: 1000,
            snd_cwnd: 2000,
            retransmits: 5,
            last_ack_recv: None,
            segs_out: 10,
            segs_in: 20,
            snd_mss: Some(1460),
            rcv_mss: Some(1460),
            last_data_sent: None,
        };

        let inspector = Inspector::new(Default::default());
        let mut bindings_inspector = FuchsiaInspector::<()>::new(inspector.root());
        info.record(&mut bindings_inspector);

        let mut exec = fuchsia_async::TestExecutor::new();

        assert_data_tree!(@executor exec, inspector, "root": {
            "CongestionControlState": "Open",
            "RtoMs": 200u64,
            "RttMs": 50u64,
            "RttVarMs": 25u64,
            "SndSsthresh": 1000u64,
            "SndCwnd": 2000u64,
            "SndMss": 1460u64,
            "RcvMss": 1460u64,
        });
    }
}
