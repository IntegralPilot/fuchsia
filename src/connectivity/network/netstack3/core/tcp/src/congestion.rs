// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Implements loss-based congestion control algorithms.
//!
//! The currently implemented algorithms are CUBIC from [RFC 8312] and RENO
//! style fast retransmit and fast recovery from [RFC 5681].
//!
//! [RFC 8312]: https://www.rfc-editor.org/rfc/rfc8312
//! [RFC 5681]: https://www.rfc-editor.org/rfc/rfc5681

mod cubic;

use core::cmp::Ordering;
use core::num::{NonZeroU8, NonZeroU32};
use core::time::Duration;

use netstack3_base::{EffectiveMss, Instant, Mss, SackBlocks, SeqNum, WindowSize};

use crate::internal::sack_scoreboard::SackScoreboard;

// Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#section-3.2):
///   The fast retransmit algorithm uses the arrival of 3 duplicate ACKs (...)
///   as an indication that a segment has been lost.
pub(crate) const DUP_ACK_THRESHOLD: u8 = 3;

/// Holds the parameters of congestion control that are common to algorithms.
#[derive(Debug)]
struct CongestionControlParams {
    /// Slow start threshold.
    ssthresh: u32,
    /// Congestion control window size, in bytes.
    cwnd: u32,
    /// Sender MSS.
    mss: EffectiveMss,
}

impl CongestionControlParams {
    fn with_mss(mss: EffectiveMss) -> Self {
        let mss_u32 = u32::from(mss);
        // Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#page-5):
        //   IW, the initial value of cwnd, MUST be set using the following
        //   guidelines as an upper bound.
        //   If SMSS > 2190 bytes:
        //       IW = 2 * SMSS bytes and MUST NOT be more than 2 segments
        //   If (SMSS > 1095 bytes) and (SMSS <= 2190 bytes):
        //       IW = 3 * SMSS bytes and MUST NOT be more than 3 segments
        //   if SMSS <= 1095 bytes:
        //       IW = 4 * SMSS bytes and MUST NOT be more than 4 segments
        let cwnd = if mss_u32 > 2190 {
            mss_u32 * 2
        } else if mss_u32 > 1095 {
            mss_u32 * 3
        } else {
            mss_u32 * 4
        };
        Self { cwnd, ssthresh: u32::MAX, mss }
    }

    fn rounded_cwnd(&self) -> CongestionWindow {
        CongestionWindow::new(self.cwnd, self.mss)
    }
}

mod cwnd {
    use super::*;
    /// A witness type for a congestion window that is rounded to a multiple of
    /// MSS.
    ///
    /// This type carries around the mss that was used to calculate it.
    #[derive(Debug, Copy, Clone)]
    #[cfg_attr(test, derive(Eq, PartialEq))]
    pub(crate) struct CongestionWindow {
        cwnd: u32,
        mss: EffectiveMss,
    }

    impl CongestionWindow {
        pub(super) fn new(cwnd: u32, mss: EffectiveMss) -> Self {
            let mss_u32 = u32::from(mss);
            Self { cwnd: cwnd / mss_u32 * mss_u32, mss }
        }

        pub(crate) fn cwnd(&self) -> u32 {
            self.cwnd
        }

        pub(crate) fn mss(&self) -> EffectiveMss {
            self.mss
        }
    }
}
pub(crate) use cwnd::CongestionWindow;

/// Congestion control with five intertwined algorithms.
///
/// - Slow start
/// - Congestion avoidance from a loss-based algorithm
/// - Fast retransmit
/// - Fast recovery: https://datatracker.ietf.org/doc/html/rfc5681#section-3
/// - SACK recovery: https://datatracker.ietf.org/doc/html/rfc6675
#[derive(Debug)]
pub(crate) struct CongestionControl<I> {
    params: CongestionControlParams,
    sack_scoreboard: SackScoreboard,
    algorithm: LossBasedAlgorithm<I>,
    /// The connection is in loss recovery when this field is a [`Some`].
    loss_recovery: Option<LossRecovery>,
}

/// Available congestion control algorithms.
#[derive(Debug)]
enum LossBasedAlgorithm<I> {
    Cubic(cubic::Cubic<I, true /* FAST_CONVERGENCE */>),
}

impl<I: Instant> LossBasedAlgorithm<I> {
    /// Called when there is a loss detected.
    ///
    /// Specifically, packet loss means we have received a certain amount of
    /// duplicate acks.
    fn on_loss_detected(&mut self, params: &mut CongestionControlParams, flight_size: u32) {
        match self {
            LossBasedAlgorithm::Cubic(cubic) => {
                cubic.on_congestion_event(params, CongestionEvent::PacketLoss, flight_size)
            }
        }
    }

    fn on_ack(
        &mut self,
        params: &mut CongestionControlParams,
        bytes_acked: NonZeroU32,
        now: I,
        rtt: Duration,
    ) {
        match self {
            LossBasedAlgorithm::Cubic(cubic) => cubic.on_ack(params, bytes_acked, now, rtt),
        }
    }

    fn on_retransmission_timeout(
        &mut self,
        params: &mut CongestionControlParams,
        flight_size: u32,
    ) {
        match self {
            LossBasedAlgorithm::Cubic(cubic) => {
                cubic.on_congestion_event(params, CongestionEvent::Timeout, flight_size)
            }
        }
    }
}

impl<I: Instant> CongestionControl<I> {
    /// Preprocesses an ACK that may contain selective ack blocks.
    ///
    /// Returns `Some(true)` if this should be considered a duplicate ACK
    /// according to the rules in [RFC 6675 section 2]. Returns `Some(false)`
    /// otherwise.
    ///
    /// If the incoming ACK does not have SACK information, `None` is returned
    /// and the caller should use the classic algorithm to determine if this is
    /// a duplicate ACk.
    ///
    /// [RFC 6675 section 2]:
    ///     https://datatracker.ietf.org/doc/html/rfc6675#section-2
    pub(super) fn preprocess_ack(
        &mut self,
        seg_ack: SeqNum,
        snd_nxt: SeqNum,
        seg_sack_blocks: &SackBlocks,
    ) -> Option<bool> {
        let Self { params, algorithm: _, loss_recovery, sack_scoreboard } = self;
        let high_rxt = loss_recovery.as_ref().and_then(|lr| match lr {
            LossRecovery::FastRecovery(_) => None,
            LossRecovery::SackRecovery(sack_recovery) => sack_recovery.high_rxt(),
        });
        let is_dup_ack =
            sack_scoreboard.process_ack(seg_ack, snd_nxt, high_rxt, seg_sack_blocks, params.mss);
        (!seg_sack_blocks.is_empty()).then_some(is_dup_ack)
    }

    /// Informs the congestion control algorithm that a segment of length
    /// `seg_len` is being sent on the wire.
    ///
    /// This allows congestion control to keep the correct estimate of how many
    /// bytes are in flight.
    pub(super) fn on_will_send_segment(&mut self, seg_len: u32) {
        let Self { params: _, sack_scoreboard, algorithm: _, loss_recovery: _ } = self;
        // From RFC 6675:
        //
        //  (C.4) The estimate of the amount of data outstanding in the
        //  network must be updated by incrementing pipe by the number of
        //  octets transmitted in (C.1).
        sack_scoreboard.increment_pipe(seg_len);
    }

    /// Called when there are previously unacknowledged bytes being acked.
    ///
    /// If a round-trip-time estimation is not available, `rtt` can be `None`,
    /// but the loss-based algorithm is not updated in that case.
    ///
    /// Returns `true` if this ack signals a loss recovery.
    pub(super) fn on_ack(
        &mut self,
        seg_ack: SeqNum,
        bytes_acked: NonZeroU32,
        now: I,
        rtt: Option<Duration>,
    ) -> bool {
        let Self { params, algorithm, loss_recovery, sack_scoreboard: _ } = self;
        // Exit fast recovery since there is an ACK that acknowledges new data.
        let outcome = match loss_recovery {
            None => LossRecoveryOnAckOutcome::None,
            Some(LossRecovery::FastRecovery(fast_recovery)) => fast_recovery.on_ack(params),
            Some(LossRecovery::SackRecovery(sack_recovery)) => sack_recovery.on_ack(seg_ack),
        };

        let recovered = match outcome {
            LossRecoveryOnAckOutcome::None => false,
            LossRecoveryOnAckOutcome::Discard { recovered } => {
                *loss_recovery = None;
                recovered
            }
        };

        // It is possible, however unlikely, that we get here without an RTT
        // estimation - in case the first data segment that we send out gets
        // retransmitted. In that case, simply don't update the congestion
        // parameters with the loss based algorithm which at worst causes slow
        // start to take one extra step.
        if let Some(rtt) = rtt {
            algorithm.on_ack(params, bytes_acked, now, rtt);
        }
        recovered
    }

    /// Called when a duplicate ack is arrived.
    ///
    /// Returns `Some` if loss recovery was initiated as a result of this ACK,
    /// informing which mode was triggered.
    pub(super) fn on_dup_ack(
        &mut self,
        seg_ack: SeqNum,
        snd_nxt: SeqNum,
    ) -> Option<LossRecoveryMode> {
        let flight_size = self.flight_size();
        let Self { params, algorithm, loss_recovery, sack_scoreboard } = self;
        match loss_recovery {
            None => {
                // If we have SACK information, prefer SACK recovery.
                if sack_scoreboard.has_sack_info() {
                    let mut sack_recovery = SackRecovery::new();
                    let started_loss_recovery = sack_recovery
                        .on_dup_ack(seg_ack, snd_nxt, sack_scoreboard)
                        .apply(params, algorithm, flight_size);
                    *loss_recovery = Some(LossRecovery::SackRecovery(sack_recovery));
                    started_loss_recovery.then_some(LossRecoveryMode::SackRecovery)
                } else {
                    *loss_recovery = Some(LossRecovery::FastRecovery(FastRecovery::new()));
                    None
                }
            }
            Some(LossRecovery::SackRecovery(sack_recovery)) => sack_recovery
                .on_dup_ack(seg_ack, snd_nxt, sack_scoreboard)
                .apply(params, algorithm, flight_size)
                .then_some(LossRecoveryMode::SackRecovery),
            Some(LossRecovery::FastRecovery(fast_recovery)) => fast_recovery
                .on_dup_ack(params, algorithm, seg_ack, flight_size)
                .then_some(LossRecoveryMode::FastRecovery),
        }
    }

    /// Called upon a retransmission timeout.
    ///
    /// `snd_nxt` is the value of SND.NXT _before_ it is rewound to SND.UNA as
    /// part of an RTO.
    pub(super) fn on_retransmission_timeout(&mut self, snd_nxt: SeqNum) {
        let flight_size = self.flight_size();
        let Self { params, algorithm, loss_recovery, sack_scoreboard } = self;
        sack_scoreboard.on_retransmission_timeout();
        let discard_loss_recovery = match loss_recovery {
            None | Some(LossRecovery::FastRecovery(_)) => true,
            Some(LossRecovery::SackRecovery(sack_recovery)) => {
                sack_recovery.on_retransmission_timeout(snd_nxt)
            }
        };
        if discard_loss_recovery {
            *loss_recovery = None;
        }
        algorithm.on_retransmission_timeout(params, flight_size);
    }

    pub(super) fn slow_start_threshold(&self) -> u32 {
        self.params.ssthresh
    }

    pub(super) fn flight_size(&self) -> u32 {
        // Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#section-2):
        //    FLIGHT SIZE: The amount of data that has been sent but not yet
        //    cumulatively acknowledged.
        // The RFC text is not prescriptive on how to calculate the flight_size
        // so we instead look to Linux's `tcp_packets_in_flight` function for
        // inspiration. The equation used there is:
        //     Packets sent once on transmission queue MINUS
        //     Packets left network, but not honestly ACKed yet PLUS
        //     Packets fast retransmitted
        // This is effectively the same as how the `pipe` variable is calculated
        // by the SACK scoreboard.
        self.sack_scoreboard.pipe()
    }

    #[cfg(test)]
    pub(super) fn pipe(&self) -> u32 {
        self.sack_scoreboard.pipe()
    }

    /// Inflates the congestion window by `value` to facilitate testing.
    #[cfg(test)]
    pub(super) fn inflate_cwnd(&mut self, inflation: u32) {
        self.params.cwnd += inflation;
    }

    pub(super) fn cubic_with_mss(mss: EffectiveMss) -> Self {
        Self {
            params: CongestionControlParams::with_mss(mss),
            algorithm: LossBasedAlgorithm::Cubic(Default::default()),
            loss_recovery: None,
            sack_scoreboard: SackScoreboard::default(),
        }
    }

    pub(super) fn mss(&self) -> EffectiveMss {
        self.params.mss
    }

    pub(super) fn update_mss(&mut self, mss: Mss, snd_una: SeqNum, snd_nxt: SeqNum) {
        let Self { params, sack_scoreboard, algorithm: _, loss_recovery } = self;
        let orig = u32::from(params.mss);
        params.mss.update_mss(mss);

        // From [RFC 5681 section 3.1]:
        //
        //    When initial congestion windows of more than one segment are
        //    implemented along with Path MTU Discovery [RFC1191], and the MSS
        //    being used is found to be too large, the congestion window cwnd
        //    SHOULD be reduced to prevent large bursts of smaller segments.
        //    Specifically, cwnd SHOULD be reduced by the ratio of the old segment
        //    size to the new segment size.
        //
        // [RFC 5681 section 3.1]: https://datatracker.ietf.org/doc/html/rfc5681#section-3.1
        if params.ssthresh == u32::MAX {
            params.cwnd = params.cwnd.saturating_div(orig).saturating_mul(u32::from(params.mss));
        }

        // Given we'll retransmit after receiving this, we need to update the
        // SACK scoreboard so pipe is recalculated based on this value of
        // snd_nxt and mss.
        let high_rxt = loss_recovery.as_ref().and_then(|lr| match lr {
            LossRecovery::FastRecovery(_) => None,
            LossRecovery::SackRecovery(sack_recovery) => sack_recovery.high_rxt(),
        });
        sack_scoreboard.on_mss_update(snd_una, snd_nxt, high_rxt, params.mss);
    }

    /// Returns the rounded unmodified by loss recovery window size.
    ///
    /// This is meant to be used for inspection only. Congestion calculation for
    /// sending should use [`CongestionControl::poll_send`].
    pub(super) fn inspect_cwnd(&self) -> CongestionWindow {
        self.params.rounded_cwnd()
    }

    /// Returns the current loss recovery mode, if any.
    ///
    /// This method returns the current loss recovery mode.
    ///
    /// *NOTE* It's possible for [`CongestionControl`] to return a
    /// [`LossRecoveryMode`] here even if there was no congestion events. Rely
    /// on the return values from [`CongestionControl::poll_send`],
    /// [`CongestionControl::on_dup_ack`] to catch entering into loss recovery
    /// mode or determining if segments originate from a specific algorithm.
    pub(super) fn inspect_loss_recovery_mode(&self) -> Option<LossRecoveryMode> {
        self.loss_recovery.as_ref().map(|lr| lr.mode())
    }

    /// Returns true if this [`CongestionControl`] is in slow start.
    pub(super) fn in_slow_start(&self) -> bool {
        self.params.cwnd < self.params.ssthresh
    }

    /// Polls congestion control for the next segment to be sent out.
    ///
    /// Receives pertinent parameters from the sender state machine to allow for
    /// this decision:
    ///
    /// - `snd_una` is SND.UNA the highest unacknowledged sequence number.
    /// - `snd_nxt` is SND.NXT, the next sequence number the sender would send
    ///   without loss recovery.
    /// - `snd_wnd` is the total send window, i.e. the allowable receiver window
    ///   after `snd_una`.
    /// - `available_bytes` is the total number of bytes in the send buffer,
    ///   starting at `snd_una`.
    ///
    /// Returns `None` if no segment should be sent right now.
    pub(super) fn poll_send(
        &mut self,
        snd_una: SeqNum,
        snd_nxt: SeqNum,
        snd_wnd: WindowSize,
        available_bytes: usize,
    ) -> Option<CongestionControlSendOutcome> {
        let Self { params, algorithm: _, loss_recovery, sack_scoreboard } = self;
        let cwnd = params.rounded_cwnd();

        match loss_recovery {
            None => {
                let pipe = sack_scoreboard.pipe();
                let congestion_window = cwnd.cwnd();
                let available_window = congestion_window.saturating_sub(pipe);
                let congestion_limit = available_window.min(cwnd.mss().into());
                Some(CongestionControlSendOutcome {
                    next_seg: snd_nxt,
                    congestion_limit,
                    congestion_window,
                    loss_recovery: LossRecoverySegment::No,
                })
            }
            Some(LossRecovery::FastRecovery(fast_recovery)) => {
                Some(fast_recovery.poll_send(cwnd, sack_scoreboard.pipe(), snd_nxt))
            }
            Some(LossRecovery::SackRecovery(sack_recovery)) => sack_recovery.poll_send(
                cwnd,
                snd_una,
                snd_nxt,
                snd_wnd,
                available_bytes,
                sack_scoreboard,
            ),
        }
    }
}

/// Indicates whether the segment yielded in [`CongestionControlSendOutcome`] is
/// a loss recovery segment.
#[derive(Debug)]
#[cfg_attr(test, derive(Copy, Clone, Eq, PartialEq))]
pub(super) enum LossRecoverySegment {
    /// Indicates the segment is a loss recovery segment.
    Yes {
        /// If true, the retransmit timer should be rearmed due to this loss
        /// recovery segment.
        ///
        /// This is used in SACK recovery to prevent RTOs during retransmission,
        /// from [RFC 6675 section 6]:
        ///
        /// > Therefore, we give implementers the latitude to use the standard
        /// > [RFC6298]-style RTO management or, optionally, a more careful
        /// > variant that re-arms the RTO timer on each retransmission that is
        /// > sent during recovery MAY be used.  This provides a more
        /// > conservative timer than specified in [RFC6298], and so may not
        /// > always be an attractive alternative.  However, in some cases it
        /// > may prevent needless retransmissions, go-back-N transmission, and
        /// > further reduction of the congestion window.
        ///
        /// [RFC 6675 section 6]: https://datatracker.ietf.org/doc/html/rfc6675#section-6
        rearm_retransmit: bool,
        /// The recovery mode that caused this loss recovery segment.
        mode: LossRecoveryMode,
    },
    /// Indicates the segment is *not* a loss recovery segment.
    No,
}

/// The outcome of [`CongestionControl::poll_send`].
#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
pub(super) struct CongestionControlSendOutcome {
    /// The next segment to be sent out.
    pub next_seg: SeqNum,
    /// The maximum number of bytes post next_seg that can be sent.
    ///
    /// This limit does not account for the unused/open window on the receiver.
    ///
    /// This is limited by the current congestion limit and the sender MSS.
    pub congestion_limit: u32,
    /// The congestion window used to calculate `congestion limit`.
    ///
    /// This is the estimated total congestion window, including loss
    /// recovery-based inflation.
    pub congestion_window: u32,
    /// Whether this is a loss recovery segment.
    pub loss_recovery: LossRecoverySegment,
}

/// The current loss recovery mode.
#[derive(Debug)]
pub enum LossRecovery {
    FastRecovery(FastRecovery),
    SackRecovery(SackRecovery),
}

impl LossRecovery {
    fn mode(&self) -> LossRecoveryMode {
        match self {
            LossRecovery::FastRecovery(_) => LossRecoveryMode::FastRecovery,
            LossRecovery::SackRecovery(_) => LossRecoveryMode::SackRecovery,
        }
    }
}

/// An equivalent to [`LossRecovery`] that simply informs the loss recovery
/// mode, without carrying state.
#[derive(Debug)]
#[cfg_attr(test, derive(Copy, Clone, Eq, PartialEq))]
pub enum LossRecoveryMode {
    FastRecovery,
    SackRecovery,
}

#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
enum LossRecoveryOnAckOutcome {
    None,
    Discard { recovered: bool },
}

/// Reno style Fast Recovery algorithm as described in
/// [RFC 5681](https://tools.ietf.org/html/rfc5681).
#[derive(Debug)]
pub struct FastRecovery {
    /// Holds the sequence number of the segment to fast retransmit, if any.
    fast_retransmit: Option<SeqNum>,
    /// The running count of consecutive duplicate ACKs we have received so far.
    ///
    /// Here we limit the maximum number of duplicate ACKS we track to 255, as
    /// per a note in the RFC:
    ///
    /// Note: [SCWA99] discusses a receiver-based attack whereby many
    /// bogus duplicate ACKs are sent to the data sender in order to
    /// artificially inflate cwnd and cause a higher than appropriate
    /// sending rate to be used.  A TCP MAY therefore limit the number of
    /// times cwnd is artificially inflated during loss recovery to the
    /// number of outstanding segments (or, an approximation thereof).
    ///
    /// [SCWA99]: https://homes.cs.washington.edu/~tom/pubs/CCR99.pdf
    dup_acks: NonZeroU8,
}

impl FastRecovery {
    fn new() -> Self {
        Self { dup_acks: NonZeroU8::new(1).unwrap(), fast_retransmit: None }
    }

    fn poll_send(
        &mut self,
        cwnd: CongestionWindow,
        used_congestion_window: u32,
        snd_nxt: SeqNum,
    ) -> CongestionControlSendOutcome {
        let Self { fast_retransmit, dup_acks } = self;
        // Per RFC 3042 (https://www.rfc-editor.org/rfc/rfc3042#section-2): ...
        // the Limited Transmit algorithm, which calls for a TCP sender to
        //   transmit new data upon the arrival of the first two consecutive
        //   duplicate ACKs ... The amount of outstanding data would remain less
        //   than or equal to the congestion window plus 2 segments.  In other
        //   words, the sender can only send two segments beyond the congestion
        //   window (cwnd).
        //
        // Note: We don't directly change cwnd in the loss-based algorithm
        // because the RFC says one MUST NOT do that. We follow the requirement
        // here by not changing the cwnd of the algorithm - if a new ACK is
        // received after the two dup acks, the loss-based algorithm will
        // continue to operate the same way as if the 2 SMSS is never added to
        // cwnd.
        let congestion_window = if dup_acks.get() < DUP_ACK_THRESHOLD {
            cwnd.cwnd().saturating_add(u32::from(dup_acks.get()) * u32::from(cwnd.mss()))
        } else {
            cwnd.cwnd()
        };

        // Elect fast retransmit sequence number or snd_nxt if we don't have
        // one.
        let (next_seg, loss_recovery, congestion_limit) = match fast_retransmit.take() {
            // From RFC 5681:
            //
            //  3. The lost segment starting at SND.UNA MUST be retransmitted
            //     [...].
            //
            // So we always set the congestion limit to be just the mss.
            Some(f) => (
                f,
                LossRecoverySegment::Yes {
                    rearm_retransmit: false,
                    mode: LossRecoveryMode::FastRecovery,
                },
                cwnd.mss().into(),
            ),
            // There's no fast retransmit pending, use snd_nxt applying the used
            // congestion window.
            None => (
                snd_nxt,
                LossRecoverySegment::No,
                congestion_window.saturating_sub(used_congestion_window).min(cwnd.mss().into()),
            ),
        };
        CongestionControlSendOutcome {
            next_seg,
            congestion_limit,
            congestion_window,
            loss_recovery,
        }
    }

    fn on_ack(&mut self, params: &mut CongestionControlParams) -> LossRecoveryOnAckOutcome {
        let recovered = self.dup_acks.get() >= DUP_ACK_THRESHOLD;
        if recovered {
            // Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#section-3.2):
            //   When the next ACK arrives that acknowledges previously
            //   unacknowledged data, a TCP MUST set cwnd to ssthresh (the value
            //   set in step 2).  This is termed "deflating" the window.
            params.cwnd = params.ssthresh;
        }
        LossRecoveryOnAckOutcome::Discard { recovered }
    }

    /// Processes a duplicate ack with sequence number `seg_ack`.
    ///
    /// Returns `true` if loss recovery is triggered.
    fn on_dup_ack<I: Instant>(
        &mut self,
        params: &mut CongestionControlParams,
        loss_based: &mut LossBasedAlgorithm<I>,
        seg_ack: SeqNum,
        flight_size: u32,
    ) -> bool {
        self.dup_acks = self.dup_acks.saturating_add(1);

        match self.dup_acks.get().cmp(&DUP_ACK_THRESHOLD) {
            Ordering::Less => false,
            Ordering::Equal => {
                loss_based.on_loss_detected(params, flight_size);
                // Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#section-3.2):
                //   The lost segment starting at SND.UNA MUST be retransmitted
                //   and cwnd set to ssthresh plus 3*SMSS.  This artificially
                //   "inflates" the congestion window by the number of segments
                //   (three) that have left the network and which the receiver
                //   has buffered.
                self.fast_retransmit = Some(seg_ack);
                params.cwnd =
                    params.ssthresh + u32::from(DUP_ACK_THRESHOLD) * u32::from(params.mss);
                true
            }
            Ordering::Greater => {
                // Per RFC 5681 (https://www.rfc-editor.org/rfc/rfc5681#section-3.2):
                //   For each additional duplicate ACK received (after the third),
                //   cwnd MUST be incremented by SMSS. This artificially inflates
                //   the congestion window in order to reflect the additional
                //   segment that has left the network.
                params.cwnd = params.cwnd.saturating_add(u32::from(params.mss));
                false
            }
        }
    }
}

/// The state kept by [`SackRecovery`] indicating the recovery state.
#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq, Copy, Clone))]
enum SackRecoveryState {
    /// SACK is currently in active recovery.
    InRecovery(SackInRecoveryState),
    /// SACK is holding off starting new recovery after an RTO.
    PostRto { recovery_point: SeqNum },
    /// SACK is not in active recovery.
    NotInRecovery,
}

/// The state kept by [`SackInRecoveryState::InRecovery`].
#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq, Copy, Clone))]
struct SackInRecoveryState {
    /// The sequence number that marks the end of the current loss recovery
    /// phase.
    recovery_point: SeqNum,
    /// The highest retransmitted sequence number during the current loss
    /// recovery phase.
    ///
    /// Tracks the "HighRxt" variable defined in [RFC 6675 section 2].
    ///
    /// [RFC 6675 section 2]: https://datatracker.ietf.org/doc/html/rfc6675#section-2
    high_rxt: SeqNum,
    /// The highest sequence number that has been optimistically retransmitted.
    ///
    /// Tracks the "RescureRxt" variable defined in [RFC 6675 section 2].
    ///
    /// [RFC 6675 section 2]: https://datatracker.ietf.org/doc/html/rfc6675#section-2
    rescue_rxt: Option<SeqNum>,
}

/// Implements the SACK based recovery from [RFC 6675].
///
/// [RFC 6675]: https://datatracker.ietf.org/doc/html/rfc6675
#[derive(Debug)]
pub(crate) struct SackRecovery {
    /// Keeps track of the number of duplicate ACKs received during SACK
    /// recovery.
    ///
    /// Tracks the "DupAcks" variable defined in [RFC 6675 section 2].
    ///
    /// [RFC 6675 section 2]: https://datatracker.ietf.org/doc/html/rfc6675#section-2
    dup_acks: u8,
    /// Statekeeping for loss recovery.
    ///
    /// Set to `Some` when we're in recovery state.
    recovery: SackRecoveryState,
}

impl SackRecovery {
    fn new() -> Self {
        Self {
            // Unlike FastRecovery, we start with zero duplicate ACKs,
            // congestion control calls on_dup_ack after creation.
            dup_acks: 0,
            recovery: SackRecoveryState::NotInRecovery,
        }
    }

    fn high_rxt(&self) -> Option<SeqNum> {
        match &self.recovery {
            SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point: _,
                high_rxt,
                rescue_rxt: _,
            }) => Some(*high_rxt),
            SackRecoveryState::PostRto { recovery_point: _ } | SackRecoveryState::NotInRecovery => {
                None
            }
        }
    }

    fn on_ack(&mut self, seg_ack: SeqNum) -> LossRecoveryOnAckOutcome {
        let Self { dup_acks, recovery } = self;
        match recovery {
            SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point,
                high_rxt: _,
                rescue_rxt: _,
            })
            | SackRecoveryState::PostRto { recovery_point } => {
                // From RFC 6675:
                //  An incoming cumulative ACK for a sequence number greater than
                //  RecoveryPoint signals the end of loss recovery, and the loss
                //  recovery phase MUST be terminated.
                if seg_ack.after_or_eq(*recovery_point) {
                    LossRecoveryOnAckOutcome::Discard {
                        recovered: matches!(recovery, SackRecoveryState::InRecovery(_)),
                    }
                } else {
                    // From RFC 6675:
                    //  If the incoming ACK is a cumulative acknowledgment, the
                    //  TCP MUST reset DupAcks to zero.
                    *dup_acks = 0;
                    LossRecoveryOnAckOutcome::None
                }
            }
            SackRecoveryState::NotInRecovery => {
                // We're not in loss recovery, we seem to have moved things
                // forward. Discard loss recovery information.
                LossRecoveryOnAckOutcome::Discard { recovered: false }
            }
        }
    }

    /// Processes a duplicate acknowledgement.
    fn on_dup_ack(
        &mut self,
        seq_ack: SeqNum,
        snd_nxt: SeqNum,
        sack_scoreboard: &SackScoreboard,
    ) -> SackDupAckOutcome {
        let Self { dup_acks, recovery } = self;
        match recovery {
            SackRecoveryState::InRecovery(_) | SackRecoveryState::PostRto { .. } => {
                // Already in recovery mode, nothing to do.
                return SackDupAckOutcome(false);
            }
            SackRecoveryState::NotInRecovery => (),
        }
        *dup_acks += 1;
        // From RFC 6675:
        //  (1) If DupAcks >= DupThresh, [...].
        //  (2) If DupAcks < DupThresh but IsLost (HighACK + 1) returns true
        //  [...]
        if *dup_acks >= DUP_ACK_THRESHOLD || sack_scoreboard.is_first_hole_lost() {
            // Enter loss recovery:
            //  (4.1) RecoveryPoint = HighData
            //  When the TCP sender receives a cumulative ACK for this data
            //  octet, the loss recovery phase is terminated.
            *recovery = SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point: snd_nxt,
                high_rxt: seq_ack,
                rescue_rxt: None,
            });
            SackDupAckOutcome(true)
        } else {
            SackDupAckOutcome(false)
        }
    }

    /// Updates SACK recovery to account for a retransmission timeout during
    /// recovery.
    ///
    /// From [RFC 6675 section 5.1]:
    ///
    /// > If an RTO occurs during loss recovery as specified in this document,
    /// > RecoveryPoint MUST be set to HighData.  Further, the new value of
    /// > RecoveryPoint MUST be preserved and the loss recovery algorithm
    /// > outlined in this document MUST be terminated.  In addition, a new
    /// > recovery phase (as described in Section 5) MUST NOT be initiated until
    /// > HighACK is greater than or equal to the new value of RecoveryPoint.
    ///
    /// [RFC 6675 section 5.1]: https://datatracker.ietf.org/doc/html/rfc6675#section-5.1
    ///
    /// Returns `true` iff we can clear all recovery state due to the timeout.
    pub(crate) fn on_retransmission_timeout(&mut self, snd_nxt: SeqNum) -> bool {
        let Self { dup_acks: _, recovery } = self;
        match recovery {
            SackRecoveryState::InRecovery(SackInRecoveryState { .. }) => {
                *recovery = SackRecoveryState::PostRto { recovery_point: snd_nxt };
                false
            }
            SackRecoveryState::PostRto { recovery_point: _ } => {
                // NB: The RFC is not exactly clear on what to do here, but the
                // best interpretation is that we should maintain the old
                // recovery point until we've hit that point and don't update to
                // the new (assumedly rewound) snd_nxt.
                false
            }
            SackRecoveryState::NotInRecovery => {
                // Not in recovery we can reset our state.
                true
            }
        }
    }

    /// SACK recovery based congestion control next segment selection.
    ///
    /// Argument semantics are the same as [`CongestionControl::poll_send`].
    fn poll_send(
        &mut self,
        cwnd: CongestionWindow,
        snd_una: SeqNum,
        snd_nxt: SeqNum,
        snd_wnd: WindowSize,
        available_bytes: usize,
        sack_scoreboard: &SackScoreboard,
    ) -> Option<CongestionControlSendOutcome> {
        let Self { dup_acks: _, recovery } = self;

        let pipe = sack_scoreboard.pipe();
        let congestion_window = cwnd.cwnd();
        let available_window = congestion_window.saturating_sub(pipe);
        // Don't send anything if we can't send at least full MSS, following the
        // RFC. All outcomes require at least one MSS of available window:
        //
        // (3.3) If (cwnd - pipe) >= 1 SMSS [...]
        // (C) If cwnd - pipe >= 1 SMSS [...]
        if available_window < cwnd.mss().into() {
            return None;
        }
        let congestion_limit = available_window.min(cwnd.mss().into());

        // If we're not in recovery, use the regular congestion calculation,
        // adjusting the congestion window with the pipe value.
        //
        // From RFC 6675:
        //
        //  (3.3) If (cwnd - pipe) >= 1 SMSS, there exists previously unsent
        //  data, and the receiver's advertised window allows, transmit up
        //  to 1 SMSS of data starting with the octet HighData+1 and update
        //  HighData to reflect this transmission, then return to (3.2).
        let SackInRecoveryState { recovery_point, high_rxt, rescue_rxt } = match recovery {
            SackRecoveryState::InRecovery(sack_in_recovery_state) => sack_in_recovery_state,
            SackRecoveryState::PostRto { recovery_point: _ } | SackRecoveryState::NotInRecovery => {
                return Some(CongestionControlSendOutcome {
                    next_seg: snd_nxt,
                    congestion_limit,
                    congestion_window,
                    loss_recovery: LossRecoverySegment::No,
                });
            }
        };

        // From RFC 6675 section 6:
        //
        //  we give implementers the latitude to use the standard
        //  [RFC6298]-style RTO management or, optionally, a more careful
        //  variant that re-arms the RTO timer on each retransmission that is
        //  sent during recovery MAY be used.  This provides a more conservative
        //  timer than specified in [RFC6298].
        //
        // As a local decision, we only rearm the retransmit timer for rules 1
        // and 3 (regular retransmissions) when the next segment trying to be
        // sent out is _before_ the recovery point that initiated this loss
        // recovery. Given the recovery algorithm greedily keeps sending more
        // data as long as it's available to keep the ACK clock running, there's
        // a catastrophic scenario where the data sent past the recovery point
        // creates new holes in the sack scoreboard that are filled by rules 1
        // and 3 and rearm the RTO, even if the retransmissions from holes
        // before RecoveryPoint might be lost themselves. Hence, once the
        // algorithm has moved past trying to fix things past the RecoveryPoint
        // we stop rearming the RTO in case the ACK for RecoveryPoint never
        // arrives.
        //
        // Note that rule 4 always rearms the retransmission timer because it
        // sents only a single segment per entry into recovery.¡
        let rearm_retransmit = |next_seg: SeqNum| next_seg.before(*recovery_point);

        // run NextSeg() as defined in RFC 6675.

        // (1) If there exists a smallest unSACKed sequence number 'S2' that
        //   meets the following three criteria for determining loss, the
        //   sequence range of one segment of up to SMSS octets starting
        //   with S2 MUST be returned.
        //
        //   (1.a) S2 is greater than HighRxt.
        //   (1.b) S2 is less than the highest octet covered by any received
        //         SACK.
        //   (1.c) IsLost (S2) returns true.

        let first_unsacked_range =
            sack_scoreboard.first_unsacked_range_from(snd_una.latest(*high_rxt));

        if let Some(first_hole) = &first_unsacked_range {
            // Meta is the IsLost value.
            if *first_hole.meta() {
                let hole_size = first_hole.len();
                let congestion_limit = congestion_limit.min(hole_size);
                *high_rxt = first_hole.start() + congestion_limit;

                // If we haven't set RescueRxt yet, set it to prevent eager
                // rescue. From RFC 6675:
                //
                //  Retransmit the first data segment presumed dropped --
                //  the segment starting with sequence number HighACK + 1.
                //  To prevent repeated retransmission of the same data or a
                //  premature rescue retransmission, set both HighRxt and
                //  RescueRxt to the highest sequence number in the
                //  retransmitted segment.
                if rescue_rxt.is_none() {
                    *rescue_rxt = Some(*high_rxt);
                }

                return Some(CongestionControlSendOutcome {
                    next_seg: first_hole.start(),
                    congestion_limit,
                    congestion_window,
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: rearm_retransmit(first_hole.start()),
                        mode: LossRecoveryMode::SackRecovery,
                    },
                });
            }
        }

        // Run next rule, from RFC 6675:
        //
        // (2) If no sequence number 'S2' per rule (1) exists but there
        // exists available unsent data and the receiver's advertised window
        // allows, the sequence range of one segment of up to SMSS octets of
        // previously unsent data starting with sequence number HighData+1
        // MUST be returned.
        let total_sent = u32::try_from(snd_nxt - snd_una).unwrap();
        if available_bytes > usize::try_from(total_sent).unwrap() && u32::from(snd_wnd) > total_sent
        {
            return Some(CongestionControlSendOutcome {
                next_seg: snd_nxt,
                // We only need to send out the congestion limit, the window
                // limit is applied by the sender state machine.
                congestion_limit,
                congestion_window,
                // NB: even though we're sending new bytes, we're still
                // signaling that we're in loss recovery. Our goal here is
                // to keep the ACK clock running and prevent an RTO, so we
                // don't want this segment to be delayed by anything.
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: false,
                    mode: LossRecoveryMode::SackRecovery,
                },
            });
        }

        // Run next rule, from RFC 6675:
        //
        //  (3) If the conditions for rules (1) and (2) fail, but there
        //  exists an unSACKed sequence number 'S3' that meets the criteria
        //  for detecting loss given in steps (1.a) and (1.b) above
        //  (specifically excluding step (1.c)), then one segment of up to
        //  SMSS octets starting with S3 SHOULD be returned.
        if let Some(first_hole) = first_unsacked_range {
            let hole_size = first_hole.len();
            let congestion_limit = congestion_limit.min(hole_size);
            *high_rxt = first_hole.start() + congestion_limit;

            return Some(CongestionControlSendOutcome {
                next_seg: first_hole.start(),
                congestion_limit,
                congestion_window,
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: rearm_retransmit(first_hole.start()),
                    mode: LossRecoveryMode::SackRecovery,
                },
            });
        }

        // Run next rule, from RFC 6675:
        //
        //  (4) If the conditions for (1), (2), and (3) fail, but there
        //  exists outstanding unSACKed data, we provide the opportunity for
        //  a single "rescue" retransmission per entry into loss recovery.
        //  If HighACK is greater than RescueRxt (or RescueRxt is
        //  undefined), then one segment of up to SMSS octets that MUST
        //  include the highest outstanding unSACKed sequence number SHOULD
        //  be returned, and RescueRxt set to RecoveryPoint. HighRxt MUST
        //  NOT be updated.
        if rescue_rxt.is_none_or(|rescue_rxt| snd_una.after_or_eq(rescue_rxt)) {
            if let Some(right_edge) = sack_scoreboard.right_edge() {
                let left = right_edge.latest(snd_nxt - congestion_limit);
                // This can't send any new data, so figure out how much space we
                // have left. If SND.NXT got rewound and is now before the right
                // edge, unwrap the calculation to zero to avoid sending the
                // rescue segment.
                let congestion_limit = u32::try_from(snd_nxt - left).unwrap_or(0);
                if congestion_limit > 0 {
                    *rescue_rxt = Some(*recovery_point);
                    return Some(CongestionControlSendOutcome {
                        next_seg: left,
                        congestion_limit,
                        congestion_window,
                        // NB: Rescue retransmissions can only happen once in
                        // every recovery enter, so always rearm the RTO.
                        loss_recovery: LossRecoverySegment::Yes {
                            rearm_retransmit: true,
                            mode: LossRecoveryMode::SackRecovery,
                        },
                    });
                }
            }
        }

        None
    }
}

/// The value returned by [`SackRecovery::on_dup_ack`].
///
/// It contains a boolean indicating whether loss recovery started due to a
/// duplicate ACK. [`SackDupAckOutcome::apply`] is used to retrieve the boolean
/// and notify loss recovery algorithm as needed and update the congestion
/// parameters.
///
/// This is its own type so [`SackRecovery::on_dup_ack`] can be tested in
/// isolation from [`LossBasedAlgorithm`].
#[derive(Debug)]
#[cfg_attr(test, derive(Eq, PartialEq))]
struct SackDupAckOutcome(bool);

impl SackDupAckOutcome {
    /// Consumes this outcome, notifying `algorithm` that loss was detected if
    /// needed.
    ///
    /// Returns the inner boolean indicating whether loss recovery started.
    fn apply<I: Instant>(
        self,
        params: &mut CongestionControlParams,
        algorithm: &mut LossBasedAlgorithm<I>,
        flight_size: u32,
    ) -> bool {
        let Self(loss_recovery) = self;
        if loss_recovery {
            algorithm.on_loss_detected(params, flight_size);
        }
        loss_recovery
    }
}

/// The various types of events that are indicative of congestion.
#[derive(Clone, Copy, Debug)]
pub(crate) enum CongestionEvent {
    // Packet Loss was observed (e.g. via Duplicate Acknowledgements).
    PacketLoss,
    // A retransmission timeout occurred.
    Timeout,
    // TODO(https://fxbug.dev/352585572): Support ECN for TCP.
}

#[cfg(test)]
mod test {
    use core::ops::Range;

    use assert_matches::assert_matches;
    use netstack3_base::testutil::FakeInstant;
    use netstack3_base::{EffectiveMss, MssSizeLimiters, SackBlock};
    use test_case::{test_case, test_matrix};

    use super::*;
    use crate::internal::testutil;

    const MSS_1: EffectiveMss =
        EffectiveMss::from_mss(Mss::DEFAULT_IPV4, MssSizeLimiters { timestamp_enabled: false });
    const MSS_2: EffectiveMss =
        EffectiveMss::from_mss(Mss::DEFAULT_IPV6, MssSizeLimiters { timestamp_enabled: false });

    enum StartingAck {
        One,
        Wraparound,
        WraparoundAfter(u32),
    }

    impl StartingAck {
        fn into_seqnum(self, mss: EffectiveMss) -> SeqNum {
            let mss = u32::from(mss);
            match self {
                StartingAck::One => SeqNum::new(1),
                StartingAck::Wraparound => SeqNum::new((mss / 2).wrapping_sub(mss)),
                StartingAck::WraparoundAfter(n) => SeqNum::new((mss / 2).wrapping_sub(n * mss)),
            }
        }
    }

    impl SackRecovery {
        #[track_caller]
        fn assert_in_recovery(&mut self) -> &mut SackInRecoveryState {
            assert_matches!(&mut self.recovery, SackRecoveryState::InRecovery(s) => s)
        }
    }

    impl<I> CongestionControl<I> {
        #[track_caller]
        fn assert_sack_recovery(&mut self) -> &mut SackRecovery {
            assert_matches!(&mut self.loss_recovery, Some(LossRecovery::SackRecovery(s)) => s)
        }
    }

    fn nth_segment_from(base: SeqNum, mss: EffectiveMss, n: u32) -> Range<SeqNum> {
        let mss = u32::from(mss);
        let start = base + n * mss;
        Range { start, end: start + mss }
    }

    fn nth_range(base: SeqNum, mss: EffectiveMss, range: Range<u32>) -> Range<SeqNum> {
        let mss = u32::from(mss);
        let Range { start, end } = range;
        let start = base + start * mss;
        let end = base + end * mss;
        Range { start, end }
    }

    #[test]
    fn no_recovery_before_reaching_threshold() {
        let mut congestion_control = CongestionControl::cubic_with_mss(MSS_1);
        let old_cwnd = congestion_control.params.cwnd;
        assert_eq!(congestion_control.params.ssthresh, u32::MAX);
        assert_eq!(congestion_control.on_dup_ack(SeqNum::new(0), SeqNum::new(1)), None);
        assert!(!congestion_control.on_ack(
            SeqNum::new(1),
            NonZeroU32::new(1).unwrap(),
            FakeInstant::from(Duration::from_secs(0)),
            Some(Duration::from_secs(1)),
        ));
        // We have only received one duplicate ack, receiving a new ACK should
        // not mean "loss recovery" - we should not bump our cwnd to initial
        // ssthresh (u32::MAX) and then overflow.
        assert_eq!(old_cwnd + 1, congestion_control.params.cwnd);
    }

    #[test]
    fn preprocess_ack_result() {
        let ack = SeqNum::new(1);
        let snd_nxt = SeqNum::new(100);
        let mut congestion_control = CongestionControl::<FakeInstant>::cubic_with_mss(MSS_1);
        assert_eq!(congestion_control.preprocess_ack(ack, snd_nxt, &SackBlocks::EMPTY), None);
        assert_eq!(
            congestion_control.preprocess_ack(ack, snd_nxt, &testutil::sack_blocks([10..20])),
            Some(true)
        );
        assert_eq!(congestion_control.preprocess_ack(ack, snd_nxt, &SackBlocks::EMPTY), None);
        assert_eq!(
            congestion_control.preprocess_ack(ack, snd_nxt, &testutil::sack_blocks([10..20])),
            Some(false)
        );
        assert_eq!(
            congestion_control.preprocess_ack(
                ack,
                snd_nxt,
                &testutil::sack_blocks([10..20, 20..30])
            ),
            Some(true)
        );
    }

    #[test_case(DUP_ACK_THRESHOLD-1; "no loss")]
    #[test_case(DUP_ACK_THRESHOLD; "exact threshold")]
    #[test_case(DUP_ACK_THRESHOLD+1; "over threshold")]
    fn sack_recovery_enter_exit_loss_dupacks(dup_acks: u8) {
        let mut congestion_control = CongestionControl::cubic_with_mss(MSS_1);
        let mss = congestion_control.mss();

        let ack = SeqNum::new(1);
        let snd_nxt = nth_segment_from(ack, mss, 10).end;

        let expect_recovery =
            SackInRecoveryState { recovery_point: snd_nxt, high_rxt: ack, rescue_rxt: None };

        let mut sack = SackBlock::try_from(nth_segment_from(ack, mss, 1)).unwrap();
        for n in 1..=dup_acks {
            assert_eq!(
                congestion_control.preprocess_ack(ack, snd_nxt, &[sack].into_iter().collect()),
                Some(true)
            );
            assert_eq!(
                congestion_control.on_dup_ack(ack, snd_nxt),
                (n == DUP_ACK_THRESHOLD).then_some(LossRecoveryMode::SackRecovery)
            );
            let sack_recovery = congestion_control.assert_sack_recovery();
            // We stop counting duplicate acks after the threshold.
            assert_eq!(sack_recovery.dup_acks, n.min(DUP_ACK_THRESHOLD));

            let expect_recovery = if n >= DUP_ACK_THRESHOLD {
                SackRecoveryState::InRecovery(expect_recovery.clone())
            } else {
                SackRecoveryState::NotInRecovery
            };
            assert_eq!(congestion_control.assert_sack_recovery().recovery, expect_recovery);

            let (start, end) = sack.into_parts();
            // Don't increase by full MSS to prove that duplicate ACKs alone are
            // putting us in this state.
            sack = SackBlock::try_new(start, end + u32::from(mss) / 4).unwrap();
        }

        let end = sack.right();
        let bytes_acked = NonZeroU32::new(u32::try_from(end - ack).unwrap()).unwrap();
        let ack = end;
        assert_eq!(congestion_control.preprocess_ack(ack, snd_nxt, &SackBlocks::EMPTY), None);

        let now = FakeInstant::default();
        let rtt = Some(Duration::from_millis(1));

        // A cumulative ACK not covering the recovery point arrives.
        assert_eq!(congestion_control.on_ack(ack, bytes_acked, now, rtt), false);
        if dup_acks >= DUP_ACK_THRESHOLD {
            assert_eq!(
                congestion_control.assert_sack_recovery().recovery,
                SackRecoveryState::InRecovery(expect_recovery)
            );
        } else {
            assert_matches!(congestion_control.loss_recovery, None);
        }

        // A cumulative ACK covering the recovery point arrives.
        let bytes_acked = NonZeroU32::new(u32::try_from(snd_nxt - ack).unwrap()).unwrap();
        let ack = snd_nxt;
        assert_eq!(
            congestion_control.on_ack(ack, bytes_acked, now, rtt),
            dup_acks >= DUP_ACK_THRESHOLD
        );
        assert_matches!(congestion_control.loss_recovery, None);

        // A later cumulative ACK arrives.
        let snd_nxt = snd_nxt + 20;
        let ack = ack + 10;
        assert_eq!(congestion_control.preprocess_ack(ack, snd_nxt, &SackBlocks::EMPTY), None);
        assert_eq!(congestion_control.on_ack(ack, bytes_acked, now, rtt), false);
        assert_matches!(congestion_control.loss_recovery, None);
    }

    #[test]
    fn sack_recovery_enter_loss_single_dupack() {
        let mut congestion_control = CongestionControl::<FakeInstant>::cubic_with_mss(MSS_1);

        // SACK can enter recovery after a *single* duplicate ACK provided
        // enough information is in the scoreboard:
        let snd_nxt = SeqNum::new(100);
        let ack = SeqNum::new(0);
        assert_eq!(
            congestion_control.preprocess_ack(
                ack,
                snd_nxt,
                &testutil::sack_blocks([5..15, 25..35, 45..55])
            ),
            Some(true)
        );
        assert_eq!(
            congestion_control.on_dup_ack(ack, snd_nxt),
            Some(LossRecoveryMode::SackRecovery)
        );
        assert_eq!(
            congestion_control.assert_sack_recovery().recovery,
            SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point: snd_nxt,
                high_rxt: ack,
                rescue_rxt: None
            })
        );
    }

    #[test]
    fn sack_recovery_poll_send_not_recovery() {
        let mut scoreboard = SackScoreboard::default();
        let mut recovery = SackRecovery::new();
        let mss = MSS_1;
        let cwnd_mss = 10u32;
        let cwnd = CongestionWindow::new(cwnd_mss * u32::from(mss), mss);
        let snd_una = SeqNum::new(1);

        let in_flight = 5u32;
        let snd_nxt = nth_segment_from(snd_una, mss, in_flight).start;

        // When not in recovery, we delegate all the receiver window calculation
        // out. Prove that that's the case by telling SACK there's nothing to
        // send.
        let snd_wnd = WindowSize::ZERO;
        let available_bytes = 0;

        let sack_block = SackBlock::try_from(nth_segment_from(snd_una, mss, 1)).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            None,
            &[sack_block].into_iter().collect(),
            mss
        ));

        // With 1 SACK block this is how much window we expect to have available
        // in multiples of mss.
        let wnd_used = in_flight - 1;
        assert_eq!(scoreboard.pipe(), wnd_used * u32::from(mss));
        let wnd_available = cwnd_mss - wnd_used;

        for i in 0..wnd_available {
            let snd_nxt = snd_nxt + i * u32::from(mss);
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg: snd_nxt,
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::No,
                })
            );
            scoreboard.increment_pipe(mss.into());
        }

        // Used all of the window.
        assert_eq!(scoreboard.pipe(), cwnd.cwnd());
        // Poll send stops this round.
        assert_eq!(
            recovery.poll_send(
                cwnd,
                snd_una,
                snd_nxt + (wnd_used + 1) * u32::from(mss),
                snd_wnd,
                available_bytes,
                &scoreboard
            ),
            None
        );
    }

    #[test_matrix(
        [MSS_1, MSS_2],
        [1, 3, 5],
        [StartingAck::One, StartingAck::Wraparound]
    )]
    fn sack_recovery_next_seg_rule_1(mss: EffectiveMss, lost_segments: u32, snd_una: StartingAck) {
        let mut scoreboard = SackScoreboard::default();
        let mut recovery = SackRecovery::new();

        let snd_una = snd_una.into_seqnum(mss);

        let sacked_segments = u32::from(DUP_ACK_THRESHOLD);
        let sacked_range = lost_segments..(lost_segments + sacked_segments);
        let in_flight = sacked_range.end + 5;
        let snd_nxt = nth_segment_from(snd_una, mss, in_flight).start;

        // Define a congestion window that will only let us fill part of the
        // lost segments with rule 1.
        let cwnd_mss = in_flight - sacked_segments - 1;
        let cwnd = CongestionWindow::new(cwnd_mss * u32::from(mss), mss);

        // Rule 1 should not care about available window size, since it's
        // retransmitting a lost segment.
        let snd_wnd = WindowSize::ZERO;
        let available_bytes = 0;

        let sack_block = SackBlock::try_from(nth_range(snd_una, mss, sacked_range)).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            None,
            &[sack_block].into_iter().collect(),
            mss
        ));

        // Verify that our set up math here is correct, we want recovery to be
        // able to fill only a part of the hole.
        assert_eq!(cwnd.cwnd() - scoreboard.pipe(), (lost_segments - 1) * u32::from(mss));
        // Enter recovery.
        assert_eq!(recovery.on_dup_ack(snd_una, snd_nxt, &scoreboard), SackDupAckOutcome(true));

        for i in 0..(lost_segments - 1) {
            let next_seg = snd_una + i * u32::from(mss);
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg,
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: true,
                        mode: LossRecoveryMode::SackRecovery,
                    },
                })
            );
            scoreboard.increment_pipe(mss.into());
            assert_eq!(
                recovery.recovery,
                SackRecoveryState::InRecovery(SackInRecoveryState {
                    recovery_point: snd_nxt,
                    high_rxt: nth_segment_from(snd_una, mss, i).end,
                    // RescueRxt is always set to the first retransmitted
                    // segment.
                    rescue_rxt: Some(snd_una + u32::from(mss)),
                })
            );
        }
        // Ran out of CWND.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
    }

    #[test_matrix(
        [MSS_1, MSS_2],
        [1, 3, 5],
        [StartingAck::One, StartingAck::Wraparound]
    )]
    fn sack_recovery_next_seg_rule_2(mss: EffectiveMss, expect_send: u32, snd_una: StartingAck) {
        let mut scoreboard = SackScoreboard::default();
        let mut recovery = SackRecovery::new();

        let snd_una = snd_una.into_seqnum(mss);

        let lost_segments = 1;
        let sacked_segments = u32::from(DUP_ACK_THRESHOLD);
        let sacked_range = lost_segments..(lost_segments + sacked_segments);
        let in_flight = sacked_range.end + 5;
        let mut snd_nxt = nth_segment_from(snd_una, mss, in_flight).start;

        let sack_block = SackBlock::try_from(nth_range(snd_una, mss, sacked_range)).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            None,
            &[sack_block].into_iter().collect(),
            mss
        ));

        // Define a congestion window that will allow us to send only the
        // desired segments.
        let cwnd = CongestionWindow::new(scoreboard.pipe() + expect_send * u32::from(mss), mss);
        // Enter recovery.
        assert_eq!(recovery.on_dup_ack(snd_una, snd_nxt, &scoreboard), SackDupAckOutcome(true));
        // Force HighRxt to the end of the lost block to skip rules 1 and 3.
        let recovery_state = recovery.assert_in_recovery();
        recovery_state.high_rxt = nth_segment_from(snd_una, mss, lost_segments - 1).end;
        // Force RecoveryRxt to skip rule 4.
        recovery_state.rescue_rxt = Some(snd_nxt);
        let state_snapshot = recovery_state.clone();

        // Available bytes is always counted from SND.UNA.
        let baseline = u32::try_from(snd_nxt - snd_una).unwrap();
        // If there is no window or nothing to send, return.
        for (snd_wnd, available_bytes) in [(0, 0), (1, 0), (0, 1)] {
            let snd_wnd = WindowSize::from_u32(baseline + snd_wnd).unwrap();
            let available_bytes = usize::try_from(baseline + available_bytes).unwrap();
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                None
            );
            assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(state_snapshot));
        }

        let baseline = baseline + (expect_send - 1) * u32::from(mss) + 1;
        let snd_wnd = WindowSize::from_u32(baseline).unwrap();
        let available_bytes = usize::try_from(baseline).unwrap();
        for _ in 0..expect_send {
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg: snd_nxt,
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: false,
                        mode: LossRecoveryMode::SackRecovery,
                    },
                })
            );
            assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(state_snapshot));
            scoreboard.increment_pipe(mss.into());
            snd_nxt = snd_nxt + u32::from(mss);
        }
        // Ran out of CWND.
        let snd_wnd = WindowSize::MAX;
        let available_bytes = usize::MAX;
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(state_snapshot));
    }

    #[test_matrix(
        [MSS_1, MSS_2],
        [1, 3, 5],
        [StartingAck::One, StartingAck::Wraparound]
    )]
    fn sack_recovery_next_seg_rule_3(
        mss: EffectiveMss,
        not_lost_segments: u32,
        snd_una: StartingAck,
    ) {
        let mut scoreboard = SackScoreboard::default();
        let mut recovery = SackRecovery::new();

        let snd_una = snd_una.into_seqnum(mss);

        let first_lost_block = 1;
        let first_sacked_segments = u32::from(DUP_ACK_THRESHOLD);
        let first_sacked_range = first_lost_block..(first_lost_block + first_sacked_segments);

        // "not_lost_segments" segments will not be considered lost by the
        // scoreboard, but they will not be sacked.
        let sacked_segments = 1;
        let sacked_range_start = first_sacked_range.end + not_lost_segments;
        let sacked_range = sacked_range_start..(sacked_range_start + sacked_segments);

        let in_flight = sacked_range.end + 5;
        let snd_nxt = nth_segment_from(snd_una, mss, in_flight).start;

        let sack_block1 =
            SackBlock::try_from(nth_range(snd_una, mss, first_sacked_range.clone())).unwrap();
        let sack_block2 = SackBlock::try_from(nth_range(snd_una, mss, sacked_range)).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            None,
            &[sack_block1, sack_block2].into_iter().collect(),
            mss
        ));

        // Define a congestion window that will only let us fill part of the
        // lost segments with rule 3.
        let expect_send = (not_lost_segments - 1).max(1);
        let cwnd_mss =
            in_flight - first_sacked_segments - sacked_segments - first_lost_block + expect_send;
        let cwnd = CongestionWindow::new(cwnd_mss * u32::from(mss), mss);

        // Rule 3 is only hit if we don't have enough available data to send.
        let snd_wnd = WindowSize::ZERO;
        let available_bytes = 0;

        // Verify that our set up math here is correct, we want recovery to be
        // able to fill only a part of the hole.
        assert_eq!(cwnd.cwnd() - scoreboard.pipe(), expect_send * u32::from(mss));
        // Enter recovery.
        assert_eq!(recovery.on_dup_ack(snd_una, snd_nxt, &scoreboard), SackDupAckOutcome(true));
        // Poll while we expect to hit rule 1. Don't increment pipe here because
        // we set up our congestion window to stop only rule 3.
        for i in 0..first_lost_block {
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg: nth_segment_from(snd_una, mss, i).start,
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: true,
                        mode: LossRecoveryMode::SackRecovery,
                    },
                })
            );
        }
        let expect_recovery = SackInRecoveryState {
            recovery_point: snd_nxt,
            high_rxt: nth_segment_from(snd_una, mss, first_sacked_range.start).start,
            rescue_rxt: Some(nth_segment_from(snd_una, mss, 0).end),
        };
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(expect_recovery));

        for i in 0..expect_send {
            let next_seg = snd_una + (first_sacked_range.end + i) * u32::from(mss);
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg,
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: true,
                        mode: LossRecoveryMode::SackRecovery,
                    },
                })
            );
            scoreboard.increment_pipe(mss.into());
            assert_eq!(
                recovery.recovery,
                SackRecoveryState::InRecovery(SackInRecoveryState {
                    high_rxt: next_seg + u32::from(mss),
                    ..expect_recovery
                })
            );
        }
        // Ran out of CWND.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
    }

    #[test_matrix(
        [MSS_1, MSS_2],
        [0, 1, 3],
        [StartingAck::One, StartingAck::Wraparound]
    )]
    fn sack_recovery_next_seg_rule_4(
        mss: EffectiveMss,
        right_edge_segments: u32,
        snd_una: StartingAck,
    ) {
        let mut scoreboard = SackScoreboard::default();
        let mut recovery = SackRecovery::new();

        let snd_una = snd_una.into_seqnum(mss);

        let lost_segments = 1;
        let sacked_segments = u32::from(DUP_ACK_THRESHOLD);
        let sacked_range = lost_segments..(lost_segments + sacked_segments);
        let in_flight = sacked_range.end + right_edge_segments + 2;
        let snd_nxt = nth_segment_from(snd_una, mss, in_flight).start;

        // Rule 4 should only be hit if we don't have available data to send.
        let snd_wnd = WindowSize::ZERO;
        let available_bytes = 0;

        let sack_block =
            SackBlock::try_from(nth_range(snd_una, mss, sacked_range.clone())).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            None,
            &[sack_block].into_iter().collect(),
            mss
        ));

        // Define a very large congestion window, given rule 4 should only
        // retransmit a single segment.
        let cwnd = CongestionWindow::new((in_flight + 500) * u32::from(mss), mss);

        // Enter recovery.
        assert_eq!(recovery.on_dup_ack(snd_una, snd_nxt, &scoreboard), SackDupAckOutcome(true));
        // Send the segments that match rule 1. Don't increment pipe here, we
        // want to show that rule 4 stops even when cwnd is entirely open.
        for i in 0..lost_segments {
            let next_seg = snd_una + i * u32::from(mss);
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg,
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: true,
                        mode: LossRecoveryMode::SackRecovery,
                    },
                })
            );
        }
        let expect_recovery = SackInRecoveryState {
            recovery_point: snd_nxt,
            high_rxt: nth_segment_from(snd_una, mss, lost_segments).start,
            // RescueRxt is always set to the first retransmitted
            // segment.
            rescue_rxt: Some(nth_segment_from(snd_una, mss, 0).end),
        };
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(expect_recovery));

        // Rule 4 should only hit after we receive an ACK past the first
        // RescueRxt value that was set.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
        // Acknowledge up to the sacked range, with one new sack block.
        let snd_una = nth_segment_from(snd_una, mss, sacked_range.end).start;
        let sack_block = SackBlock::try_from(nth_range(snd_una, mss, 1..2)).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            Some(expect_recovery.high_rxt),
            &[sack_block].into_iter().collect(),
            mss
        ));
        assert_eq!(recovery.on_ack(snd_una), LossRecoveryOnAckOutcome::None);
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(expect_recovery));
        // Rule 3 will hit once here because we have a single not lost segment.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: snd_una,
                congestion_limit: mss.into(),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: true,
                    mode: LossRecoveryMode::SackRecovery,
                },
            })
        );
        let expect_recovery =
            SackInRecoveryState { high_rxt: snd_una + u32::from(mss), ..expect_recovery };
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(expect_recovery));

        // Now we should hit Rule 4, as long as we have unacknowledged data.
        if right_edge_segments > 0 {
            assert_eq!(
                recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
                Some(CongestionControlSendOutcome {
                    next_seg: snd_nxt - u32::from(mss),
                    congestion_limit: mss.into(),
                    congestion_window: cwnd.cwnd(),
                    loss_recovery: LossRecoverySegment::Yes {
                        rearm_retransmit: true,
                        mode: LossRecoveryMode::SackRecovery,
                    },
                })
            );
            assert_eq!(
                recovery.recovery,
                SackRecoveryState::InRecovery(SackInRecoveryState {
                    rescue_rxt: Some(expect_recovery.recovery_point),
                    ..expect_recovery
                })
            );
        }

        // Once we've done the rescue it can't happen again.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
    }

    #[test_matrix(
        [MSS_1, MSS_2],
        [
            StartingAck::One,
            StartingAck::WraparoundAfter(1),
            StartingAck::WraparoundAfter(2),
            StartingAck::WraparoundAfter(3),
            StartingAck::WraparoundAfter(4)
        ]
    )]
    fn sack_recovery_all_rules(mss: EffectiveMss, snd_una: StartingAck) {
        let snd_una = snd_una.into_seqnum(mss);

        // Set up the scoreboard so we have 1 hole considered lost, that is hit
        // by Rule 1, and another that is not lost, hit by Rule 3.
        let mut scoreboard = SackScoreboard::default();
        let first_sacked_range = 1..(u32::from(DUP_ACK_THRESHOLD) + 1);
        let first_sack_block =
            SackBlock::try_from(nth_range(snd_una, mss, first_sacked_range.clone())).unwrap();

        let second_sacked_range = (first_sacked_range.end + 1)..(first_sacked_range.end + 2);
        let second_sack_block =
            SackBlock::try_from(nth_range(snd_una, mss, second_sacked_range.clone())).unwrap();

        let snd_nxt = nth_segment_from(snd_una, mss, second_sacked_range.end + 1).start;

        // To hit Rule 4 in one run, set up a recovery state that looks
        // like we've already tried to fill one hole with Rule 1 and
        // received an ack for it.
        let high_rxt = snd_una;
        let rescue_rxt = Some(snd_una);

        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            Some(high_rxt),
            &[first_sack_block, second_sack_block].into_iter().collect(),
            mss
        ));

        // Create a situation where a single sequential round of calls to
        // poll_send will hit each rule.
        let recovery_state = SackInRecoveryState { recovery_point: snd_nxt, high_rxt, rescue_rxt };
        let mut recovery = SackRecovery {
            dup_acks: DUP_ACK_THRESHOLD,
            recovery: SackRecoveryState::InRecovery(recovery_state),
        };

        // Define a congestion window that allows sending a single segment,
        // we'll not update the pipe variable at each call so we should never
        // hit the congestion limit.
        let cwnd = CongestionWindow::new(scoreboard.pipe() + u32::from(mss), mss);

        // Make exactly one segment available in the receiver window and send
        // buffer so we hit Rule 2 exactly once.
        let available = u32::try_from(snd_nxt - snd_una).unwrap() + 1;
        let snd_wnd = WindowSize::from_u32(available).unwrap();
        let available_bytes = usize::try_from(available).unwrap();

        // Hit Rule 1.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: snd_una,
                congestion_limit: u32::from(mss),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: true,
                    mode: LossRecoveryMode::SackRecovery,
                },
            })
        );
        let recovery_state =
            SackInRecoveryState { high_rxt: snd_una + u32::from(mss), ..recovery_state };
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(recovery_state));

        // Hit Rule 2.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: snd_nxt,
                congestion_limit: u32::from(mss),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: false,
                    mode: LossRecoveryMode::SackRecovery,
                },
            })
        );
        // snd_nxt should advance.
        let snd_nxt = snd_nxt + u32::from(mss);
        // No change to recovery state.
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(recovery_state));

        // Hit Rule 3.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: nth_segment_from(snd_una, mss, first_sacked_range.end).start,
                congestion_limit: u32::from(mss),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: true,
                    mode: LossRecoveryMode::SackRecovery,
                },
            })
        );
        let recovery_state = SackInRecoveryState {
            high_rxt: nth_segment_from(snd_una, mss, second_sacked_range.start).start,
            ..recovery_state
        };
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(recovery_state));

        // Hit Rule 4.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: snd_nxt - u32::from(mss),
                congestion_limit: u32::from(mss),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: true,
                    mode: LossRecoveryMode::SackRecovery,
                },
            })
        );
        let recovery_state = SackInRecoveryState {
            rescue_rxt: Some(recovery_state.recovery_point),
            ..recovery_state
        };
        assert_eq!(recovery.recovery, SackRecoveryState::InRecovery(recovery_state));

        // Hit all the rules. Nothing to send even if we still have cwnd.
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
        assert!(cwnd.cwnd() - scoreboard.pipe() >= u32::from(mss));
    }

    #[test]
    fn sack_rto() {
        let mss = MSS_1;
        let mut congestion_control = CongestionControl::<FakeInstant>::cubic_with_mss(mss);

        let rto_snd_nxt = SeqNum::new(50);
        // Set ourselves up not in recovery.
        congestion_control.loss_recovery = Some(LossRecovery::SackRecovery(SackRecovery {
            dup_acks: DUP_ACK_THRESHOLD - 1,
            recovery: SackRecoveryState::NotInRecovery,
        }));
        congestion_control.on_retransmission_timeout(rto_snd_nxt);
        assert_matches!(congestion_control.loss_recovery, None);

        // Set ourselves up in loss recovery.
        congestion_control.loss_recovery = Some(LossRecovery::SackRecovery(SackRecovery {
            dup_acks: DUP_ACK_THRESHOLD,
            recovery: SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point: SeqNum::new(10),
                high_rxt: SeqNum::new(0),
                rescue_rxt: None,
            }),
        }));
        congestion_control.on_retransmission_timeout(rto_snd_nxt);
        assert_eq!(
            congestion_control.assert_sack_recovery().recovery,
            SackRecoveryState::PostRto { recovery_point: rto_snd_nxt }
        );

        let snd_una = SeqNum::new(0);
        let snd_nxt = SeqNum::new(10);
        // While in RTO held off state, we always send next data as if we were
        // not in recovery.
        assert_eq!(
            congestion_control.poll_send(snd_una, snd_nxt, WindowSize::ZERO, 0),
            Some(CongestionControlSendOutcome {
                next_seg: snd_nxt,
                congestion_limit: u32::from(mss),
                congestion_window: congestion_control.inspect_cwnd().cwnd(),
                loss_recovery: LossRecoverySegment::No,
            })
        );
        // Receiving duplicate acks does not enter recovery.
        for _ in 0..DUP_ACK_THRESHOLD {
            assert_eq!(congestion_control.on_dup_ack(snd_una, snd_nxt), None);
        }

        let now = FakeInstant::default();
        let rtt = Some(Duration::from_millis(1));

        // Receiving an ack before the RTO recovery point does not stop
        // recovery.
        let bytes_acked = NonZeroU32::new(u32::try_from(snd_nxt - snd_una).unwrap()).unwrap();
        let snd_una = snd_nxt;
        assert!(!congestion_control.on_ack(snd_una, bytes_acked, now, rtt));
        assert_eq!(
            congestion_control.assert_sack_recovery().recovery,
            SackRecoveryState::PostRto { recovery_point: rto_snd_nxt }
        );

        // Covering the recovery point allows us to discard recovery state.
        let bytes_acked = NonZeroU32::new(u32::try_from(rto_snd_nxt - snd_una).unwrap()).unwrap();
        let snd_una = rto_snd_nxt;

        // Not considered a recovery event since RTO is the thing that recovered
        // us.
        assert_eq!(congestion_control.on_ack(snd_una, bytes_acked, now, rtt), false);
        assert_matches!(congestion_control.loss_recovery, None);
    }

    #[test]
    fn dont_rearm_rto_past_recovery_point() {
        let mut scoreboard = SackScoreboard::default();
        let mss = MSS_1;
        let snd_una = SeqNum::new(1);

        let recovery_point = nth_segment_from(snd_una, mss, 100).start;
        let snd_nxt = recovery_point + 100 * u32::from(mss);

        let mut recovery = SackRecovery {
            dup_acks: DUP_ACK_THRESHOLD,
            recovery: SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point,
                high_rxt: recovery_point,
                rescue_rxt: Some(recovery_point),
            }),
        };

        let block1 = nth_range(snd_una, mss, 101..110);
        let block2 = nth_range(snd_una, mss, 111..112);
        assert!(
            scoreboard.process_ack(
                snd_una,
                snd_nxt,
                recovery.high_rxt(),
                &[SackBlock::try_from(block1).unwrap(), SackBlock::try_from(block2).unwrap()]
                    .into_iter()
                    .collect(),
                mss,
            )
        );

        let cwnd = CongestionWindow::new(u32::MAX, mss);

        let snd_wnd = WindowSize::ZERO;
        let available_bytes = 0;

        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: nth_segment_from(snd_una, mss, 100).start,
                congestion_limit: mss.into(),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: false,
                    mode: LossRecoveryMode::SackRecovery,
                }
            })
        );
        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            Some(CongestionControlSendOutcome {
                next_seg: nth_segment_from(snd_una, mss, 110).start,
                congestion_limit: mss.into(),
                congestion_window: cwnd.cwnd(),
                loss_recovery: LossRecoverySegment::Yes {
                    rearm_retransmit: false,
                    mode: LossRecoveryMode::SackRecovery,
                }
            })
        );
    }

    // Parts of the state machine may end up rewinding SND.NXT to SND.UNA.
    // Ensure that NextSeg rule 4 implementation in SackRecovery (which is
    // sensitive to SND.NXT) gracefully handles that.
    #[test]
    fn sack_snd_nxt_rewind() {
        let mut scoreboard = SackScoreboard::default();
        let mss = MSS_1;
        let snd_una = SeqNum::new(1);

        let recovery_point = nth_segment_from(snd_una, mss, 100).start;
        let snd_nxt = nth_segment_from(recovery_point, mss, 100).start;

        let mut recovery = SackRecovery {
            dup_acks: DUP_ACK_THRESHOLD,
            recovery: SackRecoveryState::InRecovery(SackInRecoveryState {
                recovery_point,
                high_rxt: recovery_point,
                rescue_rxt: None,
            }),
        };
        let sack_block = SackBlock::try_from(nth_range(snd_una, mss, 1..5)).unwrap();
        assert!(scoreboard.process_ack(
            snd_una,
            snd_nxt,
            Some(recovery_point),
            &[sack_block].into_iter().collect(),
            mss,
        ));
        // Rewind.
        let snd_nxt = snd_una;

        let cwnd = CongestionWindow::new(u32::MAX, mss);
        let snd_wnd = WindowSize::ZERO;
        let available_bytes = 0;

        assert_eq!(
            recovery.poll_send(cwnd, snd_una, snd_nxt, snd_wnd, available_bytes, &scoreboard),
            None
        );
    }
}
