// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TCP state machine per [RFC 793](https://tools.ietf.org/html/rfc793).
// Note: All RFC quotes (with two extra spaces at the beginning of each line) in
// this file are from https://tools.ietf.org/html/rfc793#section-3.9 if not
// specified otherwise.

pub(crate) mod info;

use core::convert::{Infallible, TryFrom as _};
use core::fmt::Debug;
use core::num::{NonZeroU8, NonZeroU32, NonZeroUsize, TryFromIntError};
use core::ops::{Deref, DerefMut};
use core::time::Duration;

use assert_matches::assert_matches;
use derivative::Derivative;
use explicit::ResultExt as _;
use netstack3_base::{
    Control, EffectiveMss, HandshakeOptions, IcmpErrorCode, Instant, Milliseconds, Mss,
    MssSizeLimiters, Options, Payload, PayloadLen as _, ResetOptions, RxTimestampOption,
    SackBlocks, Segment, SegmentHeader, SegmentOptions, SeqNum, Timestamp, TimestampOption,
    TxTimestampOption, UnscaledWindowSize, WindowScale, WindowSize,
};
use netstack3_trace::{TraceResourceId, trace_instant};
use packet_formats::utils::NonZeroDuration;
use replace_with::{replace_with, replace_with_and};

use crate::internal::base::{
    BufferSizes, BuffersRefMut, ConnectionError, IcmpErrorResult, KeepAlive, SocketOptions,
};
use crate::internal::buffer::{
    Assembler, BufferLimits, IntoBuffers, ReceiveBuffer, SackBlockSizeLimiters, SendBuffer,
};
use crate::internal::congestion::{
    CongestionControl, CongestionControlSendOutcome, LossRecoveryMode, LossRecoverySegment,
};
use crate::internal::counters::TcpCountersRefs;
use crate::internal::rtt::{Estimator, MeasuredSampler, Rto, SamplingStrategy};
use crate::internal::timestamp::{TimestampOptionNegotiationState, TimestampOptionState};

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-81):
/// MSL
///       Maximum Segment Lifetime, the time a TCP segment can exist in
///       the internetwork system.  Arbitrarily defined to be 2 minutes.
pub(super) const MSL: Duration = Duration::from_secs(2 * 60);

/// The default number of retransmits before a timeout is called.
///
/// This value is picked so that at [`Rto::MIN`] initial RTO the timeout happens
/// in about 15min. This value is achieved by calculating the sum of the
/// geometric progression of doubling timeouts starting at [`Rto::MIN`] and
/// capped at [`Rto::MAX`].
const DEFAULT_MAX_RETRIES: NonZeroU8 = NonZeroU8::new(15).unwrap();

/// Default maximum SYN's to send before giving up an attempt to connect.
// TODO(https://fxbug.dev/42077087): Make these constants configurable.
pub(super) const DEFAULT_MAX_SYN_RETRIES: NonZeroU8 = NonZeroU8::new(6).unwrap();
const DEFAULT_MAX_SYNACK_RETRIES: NonZeroU8 = NonZeroU8::new(5).unwrap();

/// Time duration by which an ACK is delayed.
///
/// We pick a value that matches the minimum value used by linux and the fixed
/// value used by FreeBSD.
///
/// Per [RFC 9293](https://tools.ietf.org/html/rfc9293#section-3.8.6.3), the
/// delay MUST be less than 0.5 seconds.
const ACK_DELAY_THRESHOLD: Duration = Duration::from_millis(40);
/// Per RFC 9293 Section 3.8.6.2.1:
///  ... The override timeout should be in the range 0.1 - 1.0 seconds.
/// Note that we pick the lower end of the range because this case should be
/// rare and the installing a timer itself represents a high probability of
/// receiver having reduced its window so that our MAX(SND.WND) is an
/// overestimation, so we choose the value to avoid unnecessary delay.
const SWS_PROBE_TIMEOUT: Duration = Duration::from_millis(100);
/// Per RFC 9293 Section 3.8.6.2.2 and 3.8.6.2.1:
///   where Fr is a fraction whose recommended value is 1/2,
/// Note that we use the inverse since we want to avoid floating point.
const SWS_BUFFER_FACTOR: u32 = 2;

/// Whether netstack3 senders support receiving selective acks.
const SACK_PERMITTED: bool = true;

/// A trait abstracting an identifier for a state machine.
///
/// This allows the socket layer to pass its identifier to the state machine
/// opaquely, and that it be ignored in tests.
pub(crate) trait StateMachineDebugId: Debug {
    fn trace_id(&self) -> TraceResourceId<'_>;
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-22):
///
///   CLOSED - represents no connection state at all.
///
/// Allowed operations:
///   - listen
///   - connect
/// Disallowed operations:
///   - send
///   - recv
///   - shutdown
///   - accept
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Closed<Error> {
    /// Describes a reason why the connection was closed.
    pub(crate) reason: Error,
}

/// An uninhabited type used together with [`Closed`] to sugest that it is in
/// initial condition and no errors have occurred yet.
pub(crate) enum Initial {}

impl Closed<Initial> {
    /// Corresponds to the [OPEN](https://tools.ietf.org/html/rfc793#page-54)
    /// user call.
    ///
    /// `iss`is The initial send sequence number. Which is effectively the
    /// sequence number of SYN.
    pub(crate) fn connect<I: Instant, ActiveOpen>(
        iss: SeqNum,
        timestamp_offset: Timestamp<Milliseconds>,
        now: I,
        active_open: ActiveOpen,
        buffer_sizes: BufferSizes,
        device_mss: Mss,
        default_mss: Mss,
        SocketOptions {
            keep_alive: _,
            nagle_enabled: _,
            user_timeout,
            delayed_ack: _,
            fin_wait2_timeout: _,
            max_syn_retries,
            ip_options: _,
        }: &SocketOptions,
    ) -> (SynSent<I, ActiveOpen>, Segment<()>) {
        let rcv_wnd_scale = buffer_sizes.rwnd().scale();
        // RFC 7323 Section 2.2:
        //  The window field in a segment where the SYN bit is set (i.e., a
        //  <SYN> or <SYN,ACK>) MUST NOT be scaled.
        let rwnd = buffer_sizes.rwnd_unscaled();
        // Per RFC 7323 section 3.2:
        //   A TCP MAY send the TSopt in an initial <SYN> segment (i.e., segment
        //   containing a SYN bit and no ACK bit)
        let ts_opt = TimestampOptionNegotiationState::new(now, timestamp_offset);
        let timestamp = ts_opt.make_option_for_syn(now).map(TxTimestampOption::into);
        let mut rtt_sampler = MeasuredSampler::default();
        rtt_sampler.on_will_send_segment(now, iss..iss + 1, iss);
        (
            SynSent {
                iss,
                rtt_sampler,
                retrans_timer: RetransTimer::new(
                    now,
                    Rto::DEFAULT,
                    *user_timeout,
                    *max_syn_retries,
                ),
                active_open,
                buffer_sizes,
                device_mss,
                default_mss,
                rcv_wnd_scale,
                ts_opt,
            },
            Segment::syn(
                iss,
                rwnd,
                HandshakeOptions {
                    mss: Some(device_mss),
                    window_scale: Some(rcv_wnd_scale),
                    sack_permitted: SACK_PERMITTED,
                    timestamp,
                },
            ),
        )
    }

    pub(crate) fn listen(
        iss: SeqNum,
        timestamp_offset: Timestamp<Milliseconds>,
        buffer_sizes: BufferSizes,
        device_mss: Mss,
        default_mss: Mss,
        user_timeout: Option<NonZeroDuration>,
    ) -> Listen {
        Listen { iss, timestamp_offset, buffer_sizes, device_mss, default_mss, user_timeout }
    }
}

impl<Error> Closed<Error> {
    /// Processes an incoming segment in the CLOSED state.
    ///
    /// TCP will either drop the incoming segment or generate a RST.
    pub(crate) fn on_segment(&self, segment: &Segment<impl Payload>) -> Option<Segment<()>> {
        let segment_len = segment.len();
        let SegmentHeader { seq: seg_seq, ack: seg_ack, wnd: _, control, options: _, push: _ } =
            segment.header();

        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-65):
        //   If the state is CLOSED (i.e., TCB does not exist) then
        //   all data in the incoming segment is discarded.  An incoming
        //   segment containing a RST is discarded.  An incoming segment
        //   not containing a RST causes a RST to be sent in response.
        //   The acknowledgment and sequence field values are selected to
        //   make the reset sequence acceptable to the TCP that sent the
        //   offending segment.
        //   If the ACK bit is off, sequence number zero is used,
        //    <SEQ=0><ACK=SEG.SEQ+SEG.LEN><CTL=RST,ACK>
        //   If the ACK bit is on,
        //    <SEQ=SEG.ACK><CTL=RST>
        //   Return.
        if *control == Some(Control::RST) {
            return None;
        }
        // Although RFC 7323 section 3.2 states that the timestamp option, once
        // negotiated, SHOULD be included in all reset segments, we don't do so
        // here. In the `Closed` state we don't known whether the timestamp
        // option was previously negotiated.
        let timestamp = None;
        Some(match seg_ack {
            Some(seg_ack) => Segment::rst(*seg_ack, ResetOptions { timestamp }),
            None => Segment::rst_ack(
                SeqNum::from(0),
                *seg_seq + segment_len,
                ResetOptions { timestamp },
            ),
        })
    }
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
///   LISTEN - represents waiting for a connection request from any remote
///   TCP and port.
///
/// Allowed operations:
///   - send (queued until connection is established)
///   - recv (queued until connection is established)
///   - connect
///   - shutdown
///   - accept
/// Disallowed operations:
///   - listen
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Listen {
    iss: SeqNum,
    timestamp_offset: Timestamp<Milliseconds>,
    buffer_sizes: BufferSizes,
    device_mss: Mss,
    default_mss: Mss,
    user_timeout: Option<NonZeroDuration>,
}

/// Dispositions of [`Listen::on_segment`].
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
enum ListenOnSegmentDisposition<I: Instant> {
    SendSynAckAndEnterSynRcvd(Segment<()>, SynRcvd<I, Infallible>),
    SendRst(Segment<()>),
    Ignore,
}

impl Listen {
    fn on_segment<I: Instant>(
        &self,
        seg: Segment<impl Payload>,
        now: I,
    ) -> ListenOnSegmentDisposition<I> {
        let (header, _data) = seg.into_parts();
        let SegmentHeader { seq, ack, wnd: _, control, options, push: _ } = header;
        let Listen { iss, timestamp_offset, buffer_sizes, device_mss, default_mss, user_timeout } =
            *self;
        let smss = options.mss().unwrap_or(default_mss).min(device_mss);
        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-65):
        //   first check for an RST
        //   An incoming RST should be ignored.  Return.
        if control == Some(Control::RST) {
            return ListenOnSegmentDisposition::Ignore;
        }
        if let Some(ack) = ack {
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-65):
            //   second check for an ACK
            //   Any acknowledgment is bad if it arrives on a connection still in
            //   the LISTEN state.  An acceptable reset segment should be formed
            //   for any arriving ACK-bearing segment.  The RST should be
            //   formatted as follows:
            //     <SEQ=SEG.ACK><CTL=RST>
            //   Return.

            // Note: We haven't negotiated the timestamp option if we're in the
            // Listen state. Don't send it as part of the reset.
            let options = ResetOptions { timestamp: None };

            return ListenOnSegmentDisposition::SendRst(Segment::rst(ack, options));
        }
        if control == Some(Control::SYN) {
            let sack_permitted = options.sack_permitted();
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-65):
            //   third check for a SYN
            //   Set RCV.NXT to SEG.SEQ+1, IRS is set to SEG.SEQ and any other
            //   control or text should be queued for processing later.  ISS
            //   should be selected and a SYN segment sent of the form:
            //     <SEQ=ISS><ACK=RCV.NXT><CTL=SYN,ACK>
            //   SND.NXT is set to ISS+1 and SND.UNA to ISS.  The connection
            //   state should be changed to SYN-RECEIVED.  Note that any other
            //   incoming control or data (combined with SYN) will be processed
            //   in the SYN-RECEIVED state, but processing of SYN and ACK should
            //   not be repeated.

            let rcv_wnd_scale = buffer_sizes.rwnd().scale();
            // RFC 7323 Section 2.2:
            //  The window field in a segment where the SYN bit is set (i.e., a
            //  <SYN> or <SYN,ACK>) MUST NOT be scaled.
            let rwnd = buffer_sizes.rwnd_unscaled();
            // Per RFC 7323 section 3.2:
            //   A TCP MAY send a TSopt in <SYN,ACK> only if it received a TSopt
            //   in the initial <SYN> segment for the connection.
            let initial_ack_sent = seq + 1;
            let ts_opt = TimestampOptionState::negotiate(
                TimestampOptionNegotiationState::new(now, timestamp_offset),
                options.timestamp().map(RxTimestampOption::from),
                initial_ack_sent,
            );
            let mut rtt_sampler = MeasuredSampler::default();
            rtt_sampler.on_will_send_segment(now, iss..iss + 1, iss);
            return ListenOnSegmentDisposition::SendSynAckAndEnterSynRcvd(
                Segment::syn_ack(
                    iss,
                    initial_ack_sent,
                    rwnd,
                    HandshakeOptions {
                        mss: Some(smss),
                        // Per RFC 7323 Section 2.3:
                        //   If a TCP receives a <SYN> segment containing a
                        //   Window Scale option, it SHOULD send its own Window
                        //   Scale option in the <SYN,ACK> segment.
                        window_scale: options.window_scale().map(|_| rcv_wnd_scale),
                        sack_permitted: SACK_PERMITTED,
                        timestamp: ts_opt.make_option_for_ack(now).map(TxTimestampOption::into),
                    },
                ),
                SynRcvd {
                    iss,
                    irs: seq,
                    rtt_sampler,
                    retrans_timer: RetransTimer::new(
                        now,
                        Rto::DEFAULT,
                        user_timeout,
                        DEFAULT_MAX_SYNACK_RETRIES,
                    ),
                    simultaneous_open: None,
                    buffer_sizes,
                    smss: EffectiveMss::from_mss(
                        smss,
                        MssSizeLimiters { timestamp_enabled: ts_opt.is_enabled() },
                    ),
                    rcv_wnd_scale,
                    snd_wnd_scale: options.window_scale(),
                    sack_permitted,
                    rcv: RecvParams {
                        ack: initial_ack_sent,
                        ts_opt,
                        // We advertised an unscaled window, now
                        // set up the rcv state accordingly.
                        wnd_scale: WindowScale::default(),
                        wnd: rwnd << WindowScale::default(),
                        last_ack_recv: None,
                    },
                },
            );
        }
        ListenOnSegmentDisposition::Ignore
    }
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
///   SYN-SENT - represents waiting for a matching connection request
///   after having sent a connection request.
///
/// Allowed operations:
///   - send (queued until connection is established)
///   - recv (queued until connection is established)
///   - shutdown
/// Disallowed operations:
///   - listen
///   - accept
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct SynSent<I, ActiveOpen> {
    iss: SeqNum,
    rtt_sampler: MeasuredSampler<I>,
    retrans_timer: RetransTimer<I>,
    active_open: ActiveOpen,
    buffer_sizes: BufferSizes,
    device_mss: Mss,
    default_mss: Mss,
    rcv_wnd_scale: WindowScale,
    // The state used to negotiate the TCP timestamp option.
    ts_opt: TimestampOptionNegotiationState<I>,
}

/// Dispositions of [`SynSent::on_segment`].
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
enum SynSentOnSegmentDisposition<I: Instant, ActiveOpen> {
    SendAckAndEnterEstablished(Established<I, (), ()>),
    SendSynAckAndEnterSynRcvd(Segment<()>, SynRcvd<I, ActiveOpen>),
    SendRst(Segment<()>),
    EnterClosed(Closed<Option<ConnectionError>>),
    Ignore,
}

impl<I: Instant + 'static, ActiveOpen> SynSent<I, ActiveOpen> {
    /// Processes an incoming segment in the SYN-SENT state.
    ///
    /// Transitions to ESTABLSHED if the incoming segment is a proper SYN-ACK.
    /// Transitions to SYN-RCVD if the incoming segment is a SYN. Otherwise,
    /// the segment is dropped or an RST is generated.
    fn on_segment(
        &self,
        seg: Segment<impl Payload>,
        now: I,
    ) -> SynSentOnSegmentDisposition<I, ActiveOpen> {
        let (header, _data) = seg.into_parts();
        let SegmentHeader { seq: seg_seq, ack: seg_ack, wnd: seg_wnd, control, options, push: _ } =
            header;
        let SynSent {
            iss,
            rtt_sampler,
            retrans_timer: RetransTimer { user_timeout_until, remaining_retries: _, at: _, rto: _ },
            active_open: _,
            buffer_sizes,
            device_mss,
            default_mss,
            rcv_wnd_scale,
            ts_opt,
        } = self;
        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-65):
        //   first check the ACK bit
        //   If the ACK bit is set
        //     If SEG.ACK =< ISS, or SEG.ACK > SND.NXT, send a reset (unless
        //     the RST bit is set, if so drop the segment and return)
        //       <SEQ=SEG.ACK><CTL=RST>
        //     and discard the segment.  Return.
        //     If SND.UNA =< SEG.ACK =< SND.NXT then the ACK is acceptable.
        let has_ack = match seg_ack {
            Some(ack) => {
                // In our implementation, because we don't carry data in our
                // initial SYN segment, SND.UNA == ISS, SND.NXT == ISS+1.
                if ack.before(*iss) || ack.after(*iss + 1) {
                    return if control == Some(Control::RST) {
                        SynSentOnSegmentDisposition::Ignore
                    } else {
                        // NB: The segment is unacceptable, so don't consider
                        // its timestamp option (if included).
                        SynSentOnSegmentDisposition::SendRst(Segment::rst(
                            ack,
                            ResetOptions { timestamp: None },
                        ))
                    };
                }
                true
            }
            None => false,
        };

        match control {
            Some(Control::RST) => {
                // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-67):
                //   second check the RST bit
                //   If the RST bit is set
                //     If the ACK was acceptable then signal the user "error:
                //     connection reset", drop the segment, enter CLOSED state,
                //     delete TCB, and return.  Otherwise (no ACK) drop the
                //     segment and return.
                if has_ack {
                    SynSentOnSegmentDisposition::EnterClosed(Closed {
                        reason: Some(ConnectionError::ConnectionRefused),
                    })
                } else {
                    SynSentOnSegmentDisposition::Ignore
                }
            }
            Some(Control::SYN) => {
                let smss = options.mss().unwrap_or(*default_mss).min(*device_mss);
                let sack_permitted = options.sack_permitted();
                // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-67):
                //   fourth check the SYN bit
                //   This step should be reached only if the ACK is ok, or there
                //   is no ACK, and it [sic] the segment did not contain a RST.
                match seg_ack {
                    Some(seg_ack) => {
                        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-67):
                        //   If the SYN bit is on and the security/compartment
                        //   and precedence are acceptable then, RCV.NXT is set
                        //   to SEG.SEQ+1, IRS is set to SEG.SEQ.  SND.UNA
                        //   should be advanced to equal SEG.ACK (if there is an
                        //   ACK), and any segments on the retransmission queue
                        //   which are thereby acknowledged should be removed.

                        //   If SND.UNA > ISS (our SYN has been ACKed), change
                        //   the connection state to ESTABLISHED, form an ACK
                        //   segment
                        //     <SEQ=SND.NXT><ACK=RCV.NXT><CTL=ACK>
                        //   and send it.  Data or controls which were queued
                        //   for transmission may be included.  If there are
                        //   other controls or text in the segment then
                        //   continue processing at the sixth step below where
                        //   the URG bit is checked, otherwise return.
                        if seg_ack.after(*iss) {
                            let irs = seg_seq;
                            // We received a SYN-ACK, check if TSopt has been
                            // negotiated.
                            //
                            // Per RFC 7323 section 3.2, a
                            //   TSopt has been successfully negotiated, that is
                            //   both <SYN> and <SYN,ACK> contain TSopt.
                            let initial_ack_sent = irs + 1;
                            let rx_timestamp_opt = options.timestamp().map(RxTimestampOption::from);
                            let ts_opt = TimestampOptionState::negotiate(
                                ts_opt.clone(),
                                rx_timestamp_opt.clone(),
                                initial_ack_sent,
                            );
                            let smss = EffectiveMss::from_mss(
                                smss,
                                MssSizeLimiters { timestamp_enabled: ts_opt.is_enabled() },
                            );
                            let congestion_control = CongestionControl::cubic_with_mss(smss);
                            let mut rtt_estimator = Estimator::default();
                            let mut rtt_sampler = SamplingStrategy::new(&ts_opt, &rtt_sampler);
                            if let Some(rtt) = rtt_sampler.on_ack(now, seg_ack, rx_timestamp_opt) {
                                rtt_estimator.sample(
                                    rtt,
                                    rtt_sampler.samples_per_round_trip(
                                        &congestion_control.mss(),
                                        congestion_control.flight_size(),
                                    ),
                                );
                            }
                            let (rcv_wnd_scale, snd_wnd_scale) = options
                                .window_scale()
                                .map(|snd_wnd_scale| (*rcv_wnd_scale, snd_wnd_scale))
                                .unwrap_or_default();
                            let next = *iss + 1;
                            let established = Established {
                                snd: Send {
                                    nxt: next,
                                    max: next,
                                    una: seg_ack,
                                    // This segment has a SYN, do not scale.
                                    wnd: seg_wnd << WindowScale::default(),
                                    wl1: seg_seq,
                                    wl2: seg_ack,
                                    last_push: next,
                                    buffer: (),
                                    rtt_sampler,
                                    rtt_estimator,
                                    timer: None,
                                    congestion_control,
                                    wnd_scale: snd_wnd_scale,
                                    wnd_max: seg_wnd << WindowScale::default(),
                                    last_data_sent: None,
                                }
                                .into(),
                                rcv: Recv {
                                    buffer: RecvBufferState::Open {
                                        buffer: (),
                                        assembler: Assembler::new(initial_ack_sent),
                                    },
                                    remaining_quickacks: quickack_counter(
                                        buffer_sizes.rcv_limits(),
                                        smss,
                                    ),
                                    last_segment_at: None,
                                    timer: None,
                                    mss: smss,
                                    wnd_scale: rcv_wnd_scale,
                                    last_window_update: (initial_ack_sent, buffer_sizes.rwnd()),
                                    sack_permitted,
                                    ts_opt,
                                }
                                .into(),
                            };
                            SynSentOnSegmentDisposition::SendAckAndEnterEstablished(established)
                        } else {
                            SynSentOnSegmentDisposition::Ignore
                        }
                    }
                    None => {
                        if user_timeout_until.is_none_or(|t| now < t) {
                            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-68):
                            //   Otherwise enter SYN-RECEIVED, form a SYN,ACK
                            //   segment
                            //     <SEQ=ISS><ACK=RCV.NXT><CTL=SYN,ACK>
                            //   and send it.  If there are other controls or text
                            //   in the segment, queue them for processing after the
                            //   ESTABLISHED state has been reached, return.
                            let rcv_wnd_scale = buffer_sizes.rwnd().scale();
                            // RFC 7323 Section 2.2:
                            //  The window field in a segment where the SYN bit
                            //  is set (i.e., a <SYN> or <SYN,ACK>) MUST NOT be
                            //  scaled.
                            let rwnd = buffer_sizes.rwnd_unscaled();
                            // Per RFC 7323 section 3.2:
                            //   A TCP MAY send a TSopt in <SYN,ACK> only if it
                            //   received a TSopt in the initial <SYN> segment
                            //   for the connection.
                            // Here since we're in `SynSent` we decide only to
                            // send a timestamp option if we previously sent one
                            // as part of the initial SYN.
                            let initial_ack_sent = seg_seq + 1;
                            let ts_opt = TimestampOptionState::negotiate(
                                ts_opt.clone(),
                                options.timestamp().map(RxTimestampOption::from),
                                initial_ack_sent,
                            );
                            let smss = EffectiveMss::from_mss(
                                smss,
                                MssSizeLimiters { timestamp_enabled: ts_opt.is_enabled() },
                            );
                            SynSentOnSegmentDisposition::SendSynAckAndEnterSynRcvd(
                                Segment::syn_ack(
                                    *iss,
                                    initial_ack_sent,
                                    rwnd,
                                    HandshakeOptions {
                                        mss: Some(*smss.mss()),
                                        window_scale: options.window_scale().map(|_| rcv_wnd_scale),
                                        sack_permitted: SACK_PERMITTED,
                                        timestamp: ts_opt
                                            .make_option_for_ack(now)
                                            .map(TxTimestampOption::into),
                                    },
                                ),
                                SynRcvd {
                                    iss: *iss,
                                    irs: seg_seq,
                                    // Simultaneous Open should keep the
                                    // rtt_sampler from the original SYN sent.
                                    rtt_sampler: rtt_sampler.clone(),
                                    retrans_timer: RetransTimer::new_with_user_deadline(
                                        now,
                                        Rto::DEFAULT,
                                        *user_timeout_until,
                                        DEFAULT_MAX_SYNACK_RETRIES,
                                    ),
                                    // This should be set to active_open by the caller:
                                    simultaneous_open: None,
                                    buffer_sizes: *buffer_sizes,
                                    smss,
                                    rcv_wnd_scale,
                                    snd_wnd_scale: options.window_scale(),
                                    sack_permitted,
                                    rcv: RecvParams {
                                        ack: initial_ack_sent,
                                        ts_opt,
                                        // We advertised an unscaled window, now
                                        // set up the rcv state accordingly.
                                        wnd_scale: WindowScale::default(),
                                        wnd: rwnd << WindowScale::default(),
                                        last_ack_recv: None,
                                    },
                                },
                            )
                        } else {
                            SynSentOnSegmentDisposition::EnterClosed(Closed { reason: None })
                        }
                    }
                }
            }
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-68):
            //   fifth, if neither of the SYN or RST bits is set then drop the
            //   segment and return.
            Some(Control::FIN) | None => SynSentOnSegmentDisposition::Ignore,
        }
    }
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
///   SYN-RECEIVED - represents waiting for a confirming connection
///   request acknowledgment after having both received and sent a
///   connection request.
///
/// Allowed operations:
///   - send (queued until connection is established)
///   - recv (queued until connection is established)
///   - shutdown
/// Disallowed operations:
///   - listen
///   - accept
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct SynRcvd<I, ActiveOpen> {
    iss: SeqNum,
    irs: SeqNum,
    rtt_sampler: MeasuredSampler<I>,
    retrans_timer: RetransTimer<I>,
    /// Indicates that we arrive this state from [`SynSent`], i.e., this was an
    /// active open connection. Store this information so that we don't use the
    /// wrong routines to construct buffers.
    simultaneous_open: Option<ActiveOpen>,
    buffer_sizes: BufferSizes,
    /// The sender MSS negotiated as described in [RFC 9293 section 3.7.1].
    ///
    /// [RFC 9293 section 3.7.1]: https://datatracker.ietf.org/doc/html/rfc9293#name-maximum-segment-size-option
    smss: EffectiveMss,
    rcv_wnd_scale: WindowScale,
    snd_wnd_scale: Option<WindowScale>,
    sack_permitted: bool,
    // Parameters used to receive packets.
    //
    // Note, the window scaling here differs from `rcv_wnd_scale`. That is
    // because of RFC 7323 section 2.2:
    //   The window field in a segment where the SYN bit is
    //   (i.e., a <SYN> or <SYN,ACK>) MUST NOT be scaled.
    // Put another way, we've advertised an unscaled window, and should only
    // accept segments within that unscaled window.
    rcv: RecvParams<I>,
}

impl<I: Instant, R: ReceiveBuffer, S: SendBuffer, ActiveOpen> From<SynRcvd<I, Infallible>>
    for State<I, R, S, ActiveOpen>
{
    fn from(
        SynRcvd {
            iss,
            irs,
            rtt_sampler,
            retrans_timer,
            simultaneous_open,
            buffer_sizes,
            smss,
            rcv_wnd_scale,
            snd_wnd_scale,
            sack_permitted,
            rcv,
        }: SynRcvd<I, Infallible>,
    ) -> Self {
        match simultaneous_open {
            None => State::SynRcvd(SynRcvd {
                iss,
                irs,
                rtt_sampler,
                retrans_timer,
                simultaneous_open: None,
                buffer_sizes,
                smss,
                rcv_wnd_scale,
                snd_wnd_scale,
                sack_permitted,
                rcv,
            }),
        }
    }
}
enum FinQueued {}

impl FinQueued {
    // TODO(https://github.com/rust-lang/rust/issues/95174): Before we can use
    // enum for const generics, we define the following constants to give
    // meaning to the bools when used.
    const YES: bool = true;
    const NO: bool = false;
}

/// TCP control block variables that are responsible for sending.
#[derive(Derivative)]
#[derivative(Debug)]
#[cfg_attr(test, derivative(PartialEq, Eq))]
pub(crate) struct Send<I, S, const FIN_QUEUED: bool> {
    nxt: SeqNum,
    pub(crate) max: SeqNum,
    una: SeqNum,
    wnd: WindowSize,
    wnd_scale: WindowScale,
    wnd_max: WindowSize,
    wl1: SeqNum,
    wl2: SeqNum,
    last_push: SeqNum,
    rtt_sampler: SamplingStrategy<I>,
    rtt_estimator: Estimator,
    timer: Option<SendTimer<I>>,
    #[derivative(PartialEq = "ignore")]
    congestion_control: CongestionControl<I>,
    last_data_sent: Option<I>,
    buffer: S,
}

impl<I> Send<I, (), false> {
    fn with_buffer<S>(self, buffer: S) -> Send<I, S, false> {
        let Self {
            nxt,
            max,
            una,
            wnd,
            wnd_scale,
            wnd_max,
            wl1,
            wl2,
            last_push,
            rtt_sampler,
            rtt_estimator,
            timer,
            congestion_control,
            last_data_sent,
            buffer: _,
        } = self;
        Send {
            nxt,
            max,
            una,
            wnd,
            wnd_scale,
            wnd_max,
            wl1,
            wl2,
            last_push,
            rtt_sampler,
            rtt_estimator,
            timer,
            congestion_control,
            last_data_sent,
            buffer,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq, Eq))]
struct RetransTimer<I> {
    user_timeout_until: Option<I>,
    remaining_retries: Option<NonZeroU8>,
    at: I,
    rto: Rto,
}

impl<I: Instant> RetransTimer<I> {
    fn new(
        now: I,
        rto: Rto,
        user_timeout: Option<NonZeroDuration>,
        max_retries: NonZeroU8,
    ) -> Self {
        let user_timeout_until = user_timeout.map(|t| now.saturating_add(t.get()));
        Self::new_with_user_deadline(now, rto, user_timeout_until, max_retries)
    }

    fn new_with_user_deadline(
        now: I,
        rto: Rto,
        user_timeout_until: Option<I>,
        max_retries: NonZeroU8,
    ) -> Self {
        let rto_at = now.panicking_add(rto.get());
        let at = user_timeout_until.map(|i| i.min(rto_at)).unwrap_or(rto_at);
        Self { at, rto, user_timeout_until, remaining_retries: Some(max_retries) }
    }

    fn backoff(&mut self, now: I) {
        let Self { at, rto, user_timeout_until, remaining_retries } = self;
        *remaining_retries = remaining_retries.and_then(|r| NonZeroU8::new(r.get() - 1));
        *rto = rto.double();
        let rto_at = now.panicking_add(rto.get());
        *at = user_timeout_until.map(|i| i.min(rto_at)).unwrap_or(rto_at);
    }

    fn timed_out(&self, now: I) -> bool {
        let RetransTimer { user_timeout_until, remaining_retries, at, rto: _ } = self;
        (remaining_retries.is_none() && now >= *at) || user_timeout_until.is_some_and(|t| now >= t)
    }
}

/// Possible timers for a sender.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum SendTimer<I> {
    /// A retransmission timer can only be installed when there is outstanding
    /// data.
    Retrans(RetransTimer<I>),
    /// A keep-alive timer can only be installed when the connection is idle,
    /// i.e., the connection must not have any outstanding data.
    KeepAlive(KeepAliveTimer<I>),
    /// A zero window probe timer is installed when the receiver advertises a
    /// zero window but we have data to send. RFC 9293 Section 3.8.6.1 suggests
    /// that:
    ///   The transmitting host SHOULD send the first zero-window probe when a
    ///   zero window has existed for the retransmission timeout period, and
    ///   SHOULD increase exponentially the interval between successive probes.
    /// So we choose a retransmission timer as its implementation.
    ZeroWindowProbe(RetransTimer<I>),
    /// A timer installed to override silly window avoidance, when the receiver
    /// reduces its buffer size to be below 1 MSS (should happen very rarely),
    /// it's possible for the connection to make no progress if there is no such
    /// timer. Per RFC 9293 Section 3.8.6.2.1:
    ///   To avoid a resulting deadlock, it is necessary to have a timeout to
    ///   force transmission of data, overriding the SWS avoidance algorithm.
    ///   In practice, this timeout should seldom occur.
    SWSProbe { at: I },
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum ReceiveTimer<I> {
    DelayedAck { at: I },
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(test, derive(PartialEq, Eq))]
struct KeepAliveTimer<I> {
    at: I,
    already_sent: u8,
    user_timeout_until: Option<I>,
}

impl<I: Instant> KeepAliveTimer<I> {
    fn idle(now: I, keep_alive: &KeepAlive) -> Self {
        // Guard against keep alive operations if disabled.
        assert!(keep_alive.enabled);
        let at = now.saturating_add(keep_alive.idle.into());
        Self { at, already_sent: 0, user_timeout_until: None }
    }

    /// Checks if it's time to send a new keep alive packet.
    ///
    /// Updates the timer's internal state and returns true if a keep alive
    /// segment must be sent now.
    fn poll_send(
        &mut self,
        now: I,
        keep_alive: &KeepAlive,
        user_timeout: &Option<NonZeroDuration>,
    ) -> bool {
        // Guard against keep alive operations if disabled.
        assert!(keep_alive.enabled);
        let Self { at, already_sent, user_timeout_until } = self;
        if *at > now {
            return false;
        }

        // Only update the user timeout once. The timer is cleared when we call
        // KeepAliveTimer::idle upon receiving ACKs.
        if user_timeout_until.is_none() {
            *user_timeout_until = user_timeout.map(|t| now.saturating_add(t.get()));
        }

        *already_sent = already_sent.saturating_add(1);
        let interval = now.saturating_add(keep_alive.interval.into());
        *at = user_timeout_until.map(|i| i.min(interval)).unwrap_or(interval);

        log::debug!(
            "sending a keep alive probe, already_sent: {already_sent}, in the next {:?}",
            at.saturating_duration_since(now)
        );

        true
    }

    fn timed_out(&self, now: I, keep_alive: &KeepAlive) -> bool {
        // Guard against keep alive operations if disabled.
        assert!(keep_alive.enabled);
        let Self { at: _, already_sent, user_timeout_until } = self;
        *already_sent >= keep_alive.count.get() || user_timeout_until.is_some_and(|t| now >= t)
    }
}

impl<I: Instant> SendTimer<I> {
    fn expiry(&self) -> I {
        match self {
            SendTimer::Retrans(RetransTimer {
                at,
                rto: _,
                user_timeout_until: _,
                remaining_retries: _,
            })
            | SendTimer::KeepAlive(KeepAliveTimer { at, already_sent: _, user_timeout_until: _ })
            | SendTimer::ZeroWindowProbe(RetransTimer {
                at,
                rto: _,
                user_timeout_until: _,
                remaining_retries: _,
            }) => *at,
            SendTimer::SWSProbe { at } => *at,
        }
    }
}

impl<I: Instant> ReceiveTimer<I> {
    fn expiry(&self) -> I {
        match self {
            ReceiveTimer::DelayedAck { at } => *at,
        }
    }
}

/// Pair of receive buffer and `Assembler` Both dropped when receive is shut down.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
enum RecvBufferState<R> {
    Open { buffer: R, assembler: Assembler },
    Closed { buffer_size: usize, nxt: SeqNum },
}

impl<R: ReceiveBuffer> RecvBufferState<R> {
    fn is_closed(&self) -> bool {
        matches!(self, Self::Closed { .. })
    }

    fn has_out_of_order(&self) -> bool {
        match self {
            Self::Open { assembler, .. } => assembler.has_out_of_order(),
            Self::Closed { .. } => false,
        }
    }

    fn close(&mut self) {
        let new_state = match self {
            Self::Open { buffer, assembler } => {
                Self::Closed { nxt: assembler.nxt(), buffer_size: buffer.limits().capacity }
            }
            Self::Closed { .. } => return,
        };
        *self = new_state;
    }

    fn limits(&self) -> BufferLimits {
        match self {
            RecvBufferState::Open { buffer, .. } => buffer.limits(),
            RecvBufferState::Closed { buffer_size, .. } => {
                BufferLimits { capacity: *buffer_size, len: 0 }
            }
        }
    }
}

/// Calculates the number of quick acks to send to accelerate slow start.
fn quickack_counter(rcv_limits: BufferLimits, mss: EffectiveMss) -> usize {
    /// The minimum number of quickacks to send. Same value used by linux.
    const MIN_QUICKACK: usize = 2;
    /// An upper bound on the number of quick acks to send.
    ///
    /// Linux uses 16, we're more conservative here.
    const MAX_QUICKACK: usize = 32;

    let BufferLimits { capacity, len } = rcv_limits;
    let window = capacity - len;
    // Quick ack for enough segments that would fill half the receive window.
    //
    // This means we'll stop quickacking 2 RTTs before CWND matches RWND:
    // - Quickack is turned off when CWND = RWND/2. So it can send WND/2
    //   segments, half of which will be acknowledged.
    // - 1 RTT passes CWND is now 3*RWND/2. CWND will become full at the next
    //   RTT.
    //
    // This is equivalent to what Linux does. See
    // https://github.com/torvalds/linux/blob/master/net/ipv4/tcp_input.c#L309.
    (window / (2 * usize::from(mss))).clamp(MIN_QUICKACK, MAX_QUICKACK)
}

/// TCP control block variables that are responsible for receiving.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub(crate) struct Recv<I, R> {
    timer: Option<ReceiveTimer<I>>,
    mss: EffectiveMss,
    wnd_scale: WindowScale,
    last_window_update: (SeqNum, WindowSize),
    remaining_quickacks: usize,
    last_segment_at: Option<I>,
    /// True iff the SYN segment from the peer allowed for us to generate
    /// selective acks.
    sack_permitted: bool,

    // Buffer may be closed once receive is shutdown (e.g. with `shutdown(SHUT_RD)`).
    buffer: RecvBufferState<R>,
    ts_opt: TimestampOptionState<I>,
}

impl<I> Recv<I, ()> {
    fn with_buffer<R>(self, buffer: R) -> Recv<I, R> {
        let Self {
            timer,
            mss,
            wnd_scale,
            last_window_update,
            buffer: old_buffer,
            remaining_quickacks,
            last_segment_at,
            sack_permitted,
            ts_opt,
        } = self;
        let nxt = match old_buffer {
            RecvBufferState::Open { assembler, .. } => assembler.nxt(),
            RecvBufferState::Closed { .. } => unreachable!(),
        };
        Recv {
            timer,
            mss,
            wnd_scale,
            last_window_update,
            remaining_quickacks,
            last_segment_at,
            buffer: RecvBufferState::Open { buffer, assembler: Assembler::new(nxt) },
            sack_permitted,
            ts_opt,
        }
    }
}

impl<I, R> Recv<I, R> {
    fn sack_blocks(&self) -> SackBlocks {
        if self.sack_permitted {
            match &self.buffer {
                RecvBufferState::Open { buffer: _, assembler } => {
                    assembler.sack_blocks(SackBlockSizeLimiters {
                        timestamp_enabled: self.ts_opt.is_enabled(),
                    })
                }
                RecvBufferState::Closed { buffer_size: _, nxt: _ } => SackBlocks::default(),
            }
        } else {
            // Peer can't process selective acks.
            SackBlocks::default()
        }
    }
}

impl<I: Instant, R> Recv<I, R> {
    fn timestamp_option_for_ack(&self, now: I) -> Option<TimestampOption> {
        self.ts_opt.make_option_for_ack(now).map(TxTimestampOption::into)
    }
}

/// The calculation returned from [`Recv::calculate_window_size`].
struct WindowSizeCalculation {
    /// The sequence number of the next octet that we expect to receive from the
    /// peer.
    rcv_nxt: SeqNum,
    /// The next window size to advertise.
    window_size: WindowSize,
    /// The current threshold used to move the window or not.
    ///
    /// The threshold is the minimum of the receive MSS and half the receive
    /// buffer capacity to account for the cases where buffer capacity is much
    /// smaller than the MSS.
    threshold: usize,
}

impl<I: Instant, R: ReceiveBuffer> Recv<I, R> {
    /// Calculates the next window size to advertise to the peer.
    fn calculate_window_size(&self) -> WindowSizeCalculation {
        let rcv_nxt = self.nxt();
        let Self {
            buffer,
            timer: _,
            mss,
            wnd_scale: _,
            last_window_update: (rcv_wup, last_wnd),
            remaining_quickacks: _,
            last_segment_at: _,
            sack_permitted: _,
            ts_opt: _,
        } = self;

        // Per RFC 9293 Section 3.8.6.2.2:
        //   The suggested SWS avoidance algorithm for the receiver is to keep
        //   RCV.NXT+RCV.WND fixed until the reduction satisfies:
        //     RCV.BUFF - RCV.USER - RCV.WND  >=
        //            min( Fr * RCV.BUFF, Eff.snd.MSS )
        //   where Fr is a fraction whose recommended value is 1/2, and
        //   Eff.snd.MSS is the effective send MSS for the connection.
        //   When the inequality is satisfied, RCV.WND is set to RCV.BUFF-RCV.USER.

        // `len` and `capacity` are RCV.USER and RCV.BUFF respectively.
        // Note that the window is still kept open even after receiver was shut down.
        let BufferLimits { capacity, len } = buffer.limits();

        // Because the buffer can be updated by bindings without immediately
        // acquiring core locks, it is possible that RCV.NXT has moved forward
        // before we've had a chance to recalculate the window here and update
        // the peer. In this case, simply saturate the subtraction, we should be
        // able to open the window now that the unused window is 0.
        let unused_window = u32::try_from(*rcv_wup + *last_wnd - rcv_nxt).unwrap_or(0);
        // `unused_window` is RCV.WND as described above.
        let unused_window = WindowSize::from_u32(unused_window).unwrap_or(WindowSize::MAX);

        // Note: between the last window update and now, it's possible that we
        // have reduced our receive buffer's capacity, so we need to use
        // saturating arithmetic below.
        let reduction = capacity.saturating_sub(len.saturating_add(usize::from(unused_window)));
        let threshold = usize::min(capacity / 2, usize::from(mss.get()));
        let window_size = if reduction >= threshold {
            // We have enough reduction in the buffer space, advertise more.
            WindowSize::new(capacity - len).unwrap_or(WindowSize::MAX)
        } else {
            // Keep the right edge fixed by only advertise whatever is unused in
            // the last advertisement.
            unused_window
        };
        WindowSizeCalculation { rcv_nxt, window_size, threshold }
    }

    /// Processes data being removed from the receive buffer. Returns a window
    /// update segment to be sent immediately if necessary.
    fn poll_receive_data_dequeued(&mut self, snd_max: SeqNum, now: I) -> Option<Segment<()>> {
        let WindowSizeCalculation { rcv_nxt, window_size: calculated_window_size, threshold } =
            self.calculate_window_size();
        let (rcv_wup, last_window_size) = self.last_window_update;

        // We may have received more segments but have delayed acknowledgements,
        // so the last window size should be seen from the perspective of the
        // sender. Correcting for the difference between the current RCV.NXT and
        // the one that went with the last window update should give us a
        // clearer view.
        let rcv_diff = rcv_nxt - rcv_wup;
        debug_assert!(rcv_diff >= 0, "invalid RCV.NXT change: {rcv_nxt:?} < {rcv_wup:?}");
        let last_window_size =
            last_window_size.saturating_sub(usize::try_from(rcv_diff).unwrap_or(0));

        // Make sure effective_window_size is a multiple of wnd_scale, since
        // that's what the peer will receive. Otherwise, our later MSS
        // comparison might pass, but we'll advertise a window that's less than
        // MSS, which we don't want.
        let effective_window_size = (calculated_window_size >> self.wnd_scale) << self.wnd_scale;
        // NOTE: We lose type information here, but we already know these are
        // both WindowSize from the type annotations above.
        let effective_window_size_usize: usize = effective_window_size.into();
        let last_window_size_usize: usize = last_window_size.into();

        // If the previously-advertised window was less than advertised MSS, we
        // assume the sender is in SWS avoidance.  If we now have enough space
        // to accept at least one MSS worth of data, tell the caller to
        // immediately send a window update to get the sender back into normal
        // operation.
        if last_window_size_usize < threshold && effective_window_size_usize >= threshold {
            let ack = self.nxt();
            // A segment was produced, update the receiver with last info.
            self.last_window_update = (rcv_nxt, calculated_window_size);
            self.ts_opt.process_tx_ack(ack);
            // Discard delayed ack timer if any, we're sending out an
            // acknowledgement now.
            self.timer = None;
            Some(Segment::ack(
                snd_max,
                ack,
                calculated_window_size >> self.wnd_scale,
                SegmentOptions {
                    sack_blocks: self.sack_blocks(),
                    timestamp: self.timestamp_option_for_ack(now),
                },
            ))
        } else {
            None
        }
    }

    pub(crate) fn nxt(&self) -> SeqNum {
        match &self.buffer {
            RecvBufferState::Open { assembler, .. } => assembler.nxt(),
            RecvBufferState::Closed { nxt, .. } => *nxt,
        }
    }

    fn poll_send(&mut self, snd_max: SeqNum, now: I) -> Option<Segment<()>> {
        match self.timer {
            Some(ReceiveTimer::DelayedAck { at }) => (at <= now).then(|| {
                self.timer = None;
                self.make_ack(snd_max, now)
            }),
            None => None,
        }
    }

    /// Handles a FIN, returning the [`RecvParams`]` to use from now on.
    fn handle_fin(&self) -> RecvParams<I> {
        let WindowSizeCalculation { rcv_nxt, window_size, threshold: _ } =
            self.calculate_window_size();
        RecvParams {
            ack: rcv_nxt + 1,
            wnd: window_size.checked_sub(1).unwrap_or(WindowSize::ZERO),
            wnd_scale: self.wnd_scale,
            ts_opt: self.ts_opt.clone(),
            last_ack_recv: self.last_segment_at,
        }
    }

    fn reset_quickacks(&mut self) {
        let Self {
            timer: _,
            mss,
            wnd_scale: _,
            last_window_update: _,
            remaining_quickacks,
            buffer,
            last_segment_at: _,
            sack_permitted: _,
            ts_opt: _,
        } = self;
        let new_remaining = quickack_counter(buffer.limits(), *mss);
        // Update if we increased the number of quick acks.
        *remaining_quickacks = new_remaining.max(*remaining_quickacks);
    }
}

impl<'a, I: Instant, R: ReceiveBuffer> RecvSegmentArgumentsProvider<'a, I> for &'a mut Recv<I, R> {
    fn take_rcv_segment_args(
        self,
    ) -> (SeqNum, UnscaledWindowSize, SackBlocks, &'a TimestampOptionState<I>) {
        let WindowSizeCalculation { rcv_nxt, window_size, threshold: _ } =
            self.calculate_window_size();
        // A segment was produced, update the receiver with last info.
        self.last_window_update = (rcv_nxt, window_size);
        self.ts_opt.process_tx_ack(rcv_nxt);
        (rcv_nxt, window_size >> self.wnd_scale, self.sack_blocks(), &self.ts_opt)
    }
}

/// Cached parameters for states that do not have a full receive state machine.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub(super) struct RecvParams<I> {
    pub(super) ack: SeqNum,
    pub(super) wnd_scale: WindowScale,
    pub(super) wnd: WindowSize,
    ts_opt: TimestampOptionState<I>,
    last_ack_recv: Option<I>,
}

impl<I: Instant> RecvParams<I> {
    fn timestamp_option_for_ack(&self, now: I) -> Option<TimestampOption> {
        self.ts_opt.make_option_for_ack(now).map(TxTimestampOption::into)
    }

    fn timestamp_option_for_non_ack(&self, now: I) -> Option<TimestampOption> {
        self.ts_opt.make_option_for_non_ack(now).map(TxTimestampOption::into)
    }
}

impl<'a, I: Instant> RecvSegmentArgumentsProvider<'a, I> for &'a mut RecvParams<I> {
    fn take_rcv_segment_args(
        self,
    ) -> (SeqNum, UnscaledWindowSize, SackBlocks, &'a TimestampOptionState<I>) {
        // A segment was produced, update the receiver with last info.
        self.ts_opt.process_tx_ack(self.ack);
        (self.ack, self.wnd >> self.wnd_scale, SackBlocks::default(), &self.ts_opt)
    }
}

/// Abstracts over [`Recv`] and [`RecvParams`] with a cached window calculation.
enum CalculatedRecvParams<'a, I, R> {
    Recv { backing_state: &'a mut Recv<I, R>, cached_window: WindowSizeCalculation },
    RecvParams { backing_state: &'a mut RecvParams<I> },
}

impl<'a, I, R> CalculatedRecvParams<'a, I, R> {
    /// Constructs a [`CalculatedRecvParams`] from a [`RecvParams`].
    ///
    /// Note: do not use [`Recv::to_params`] to instantiate this, prefer
    /// [`CalculatedRecvParams::from_recv`] instead.
    fn from_params(params: &'a mut RecvParams<I>) -> Self {
        Self::RecvParams { backing_state: params }
    }

    fn nxt(&self) -> SeqNum {
        match self {
            Self::Recv { backing_state: _, cached_window } => cached_window.rcv_nxt,
            Self::RecvParams { backing_state } => backing_state.ack,
        }
    }

    fn wnd(&self) -> WindowSize {
        match self {
            Self::Recv { backing_state: _, cached_window } => cached_window.window_size,
            Self::RecvParams { backing_state } => backing_state.wnd,
        }
    }

    fn ts_opt(&self) -> &TimestampOptionState<I> {
        match self {
            Self::Recv { backing_state, cached_window: _ } => &backing_state.ts_opt,
            Self::RecvParams { backing_state } => &backing_state.ts_opt,
        }
    }

    fn ts_opt_mut(&mut self) -> &mut TimestampOptionState<I> {
        match self {
            Self::Recv { backing_state, cached_window: _ } => &mut backing_state.ts_opt,
            Self::RecvParams { backing_state } => &mut backing_state.ts_opt,
        }
    }
}

impl<'a, I: Instant, R: ReceiveBuffer> CalculatedRecvParams<'a, I, R> {
    fn reset_quickacks(&mut self) {
        match self {
            CalculatedRecvParams::Recv { backing_state, cached_window: _ } => {
                backing_state.reset_quickacks();
            }
            CalculatedRecvParams::RecvParams { backing_state: _ } => {}
        }
    }
}

impl<'a, I: Instant, R> RecvSegmentArgumentsProvider<'a, I> for CalculatedRecvParams<'a, I, R> {
    fn take_rcv_segment_args(
        self,
    ) -> (SeqNum, UnscaledWindowSize, SackBlocks, &'a TimestampOptionState<I>) {
        let (ack, wnd, wnd_scale, sack_blocks, ts_opt) = match self {
            Self::Recv {
                backing_state,
                cached_window: WindowSizeCalculation { rcv_nxt, window_size, threshold: _ },
            } => {
                // A segment was produced, update the receiver with last info.
                backing_state.last_window_update = (rcv_nxt, window_size);
                backing_state.ts_opt.process_tx_ack(rcv_nxt);
                (
                    rcv_nxt,
                    window_size,
                    backing_state.wnd_scale,
                    backing_state.sack_blocks(),
                    &backing_state.ts_opt,
                )
            }
            Self::RecvParams {
                backing_state: RecvParams { ack, wnd_scale, wnd, ts_opt, last_ack_recv: _ },
            } => {
                // A segment was produced, update the receiver with last info.
                ts_opt.process_tx_ack(*ack);
                (*ack, *wnd, *wnd_scale, SackBlocks::default(), &*ts_opt)
            }
        };
        (ack, wnd >> wnd_scale, sack_blocks, ts_opt)
    }
}

impl<'a, I: Instant, R: ReceiveBuffer> CalculatedRecvParams<'a, I, R> {
    fn from_recv(recv: &'a mut Recv<I, R>) -> Self {
        let cached_window = recv.calculate_window_size();
        Self::Recv { backing_state: recv, cached_window }
    }
}

trait RecvSegmentArgumentsProvider<'a, I: Instant>: Sized {
    /// Returns the receive state needed to construct a segment.
    ///
    /// The implementer assumes that the parameters *will be sent to a peer* and
    /// may cache the yielded values as the last sent receiver information.
    fn take_rcv_segment_args(
        self,
    ) -> (SeqNum, UnscaledWindowSize, SackBlocks, &'a TimestampOptionState<I>);

    /// Consumes this provider and calls `f` with the arguments that should be
    /// put in the segment to be returned by `f`.
    fn make_segment<
        P,
        F: FnOnce(SeqNum, UnscaledWindowSize, SackBlocks, &TimestampOptionState<I>) -> Segment<P>,
    >(
        self,
        f: F,
    ) -> Segment<P> {
        let (ack, wnd, sack, ts_opt) = self.take_rcv_segment_args();
        f(ack, wnd, sack, ts_opt)
    }

    /// Makes an ACK segment with the provided `seq` and `self`'s receiver
    /// state.
    fn make_ack<P: Payload>(self, seq: SeqNum, now: I) -> Segment<P> {
        let (ack, wnd, sack_blocks, ts_opt) = self.take_rcv_segment_args();
        Segment::ack(
            seq,
            ack,
            wnd,
            // Per RFC 7323 section 3.2:
            //   Once TSopt has been successfully negotiated [...] the TSopt
            //   MUST be sent in every non-<RST> segment for the duration of the
            //   connection.
            SegmentOptions {
                sack_blocks,
                timestamp: ts_opt.make_option_for_ack(now).map(TxTimestampOption::into),
            },
        )
    }
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-22):
///
///   ESTABLISHED - represents an open connection, data received can be
///   delivered to the user.  The normal state for the data transfer phase
///   of the connection.
///
/// Allowed operations:
///   - send
///   - recv
///   - shutdown
/// Disallowed operations:
///   - listen
///   - accept
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct Established<I, R, S> {
    pub(crate) snd: Takeable<Send<I, S, { FinQueued::NO }>>,
    pub(crate) rcv: Takeable<Recv<I, R>>,
}

/// Indicates whether at least one byte of data was acknowledged by the remote
/// in an incoming segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DataAcked {
    Yes,
    No,
}

impl<I: Instant, S: SendBuffer, const FIN_QUEUED: bool> Send<I, S, FIN_QUEUED> {
    /// Returns true if the connection should still be alive per the send state.
    fn timed_out(&mut self, now: I, keep_alive: &KeepAlive) -> bool {
        match self.timer {
            Some(SendTimer::KeepAlive(keep_alive_timer)) => {
                if keep_alive.enabled {
                    keep_alive_timer.timed_out(now, keep_alive)
                } else {
                    // Don't check keep alive timer if disabled, and clear it.
                    self.timer = None;
                    false
                }
            }
            Some(SendTimer::Retrans(timer)) | Some(SendTimer::ZeroWindowProbe(timer)) => {
                timer.timed_out(now)
            }
            Some(SendTimer::SWSProbe { at: _ }) | None => false,
        }
    }

    /// Polls for new segments with enabled options.
    ///
    /// `limit` is the maximum bytes wanted in the TCP segment (if any). The
    /// returned segment will have payload size up to the smaller of the given
    /// limit or the calculated MSS for the connection.
    fn poll_send<'a>(
        &mut self,
        id: &impl StateMachineDebugId,
        counters: &TcpCountersRefs<'_>,
        rcv: impl RecvSegmentArgumentsProvider<'a, I>,
        now: I,
        SocketOptions {
            keep_alive,
            nagle_enabled,
            user_timeout,
            delayed_ack: _,
            fin_wait2_timeout: _,
            max_syn_retries: _,
            ip_options: _,
        }: &SocketOptions,
    ) -> Option<Segment<S::Payload<'_>>> {
        let Self {
            nxt: snd_nxt,
            max: snd_max,
            una: snd_una,
            wnd: snd_wnd,
            buffer,
            wl1: _,
            wl2: _,
            last_push,
            rtt_sampler,
            rtt_estimator,
            timer,
            congestion_control,
            wnd_scale: _,
            wnd_max: snd_wnd_max,
            last_data_sent,
        } = self;
        let BufferLimits { capacity: _, len: readable_bytes } = buffer.limits();
        let mss = congestion_control.mss();
        let mut zero_window_probe = false;
        let mut override_sws = false;

        match timer {
            Some(SendTimer::Retrans(retrans_timer)) => {
                if retrans_timer.at <= now {
                    // Per https://tools.ietf.org/html/rfc6298#section-5:
                    //   (5.4) Retransmit the earliest segment that has not
                    //         been acknowledged by the TCP receiver.
                    //   (5.5) The host MUST set RTO <- RTO * 2 ("back off
                    //         the timer").  The maximum value discussed in
                    //         (2.5) above may be used to provide an upper
                    //         bound to this doubling operation.
                    //   (5.6) Start the retransmission timer, such that it
                    //         expires after RTO seconds (for the value of
                    //         RTO after the doubling operation outlined in
                    //         5.5).

                    // NB: congestion control needs to know the value of SND.NXT
                    // before we rewind it.
                    congestion_control.on_retransmission_timeout(*snd_nxt);
                    *snd_nxt = *snd_una;
                    retrans_timer.backoff(now);
                    counters.increment(|c| &c.timeouts);
                }
            }
            Some(SendTimer::ZeroWindowProbe(retrans_timer)) => {
                debug_assert!(readable_bytes > 0 || FIN_QUEUED);
                if retrans_timer.at <= now {
                    zero_window_probe = true;
                    *snd_nxt = *snd_una;
                    // Per RFC 9293 Section 3.8.6.1:
                    //   [...] SHOULD increase exponentially the interval
                    //   between successive probes.
                    retrans_timer.backoff(now);
                }
            }
            Some(SendTimer::KeepAlive(keep_alive_timer)) => {
                // Per RFC 9293 Section 3.8.4:
                //   Keep-alive packets MUST only be sent when no sent data is
                //   outstanding, and no data or acknowledgment packets have
                //   been received for the connection within an interval.
                if keep_alive.enabled && !FIN_QUEUED && readable_bytes == 0 {
                    if keep_alive_timer.poll_send(now, keep_alive, user_timeout) {
                        // Per RFC 9293 Section 3.8.4:
                        //   Such a segment generally contains SEG.SEQ = SND.NXT-1
                        return Some(rcv.make_ack(*snd_max - 1, now));
                    }
                } else {
                    log::debug!("resetting the keep alive timer");
                    *timer = None;
                }
            }
            Some(SendTimer::SWSProbe { at }) => {
                if *at <= now {
                    override_sws = true;
                    *timer = None;
                }
            }
            None => {}
        };

        // If there's an empty advertised window but we want to send data, we
        // need to start Zero Window Probing, overwriting any previous timer.
        if *snd_wnd == WindowSize::ZERO && readable_bytes > 0 {
            match timer {
                Some(SendTimer::ZeroWindowProbe(_)) => {}
                _ => {
                    *timer = Some(SendTimer::ZeroWindowProbe(RetransTimer::new(
                        now,
                        rtt_estimator.rto(),
                        *user_timeout,
                        DEFAULT_MAX_RETRIES,
                    )));

                    // RFC 9293 3.8.6.1:
                    //   The transmitting host SHOULD send the first zero-window
                    //   probe when a zero window has existed for the
                    //   retransmission timeout period (SHLD-29)
                    //
                    // We'll only end up here if the peer sent a zero window
                    // advertisement. In that case, we have not yet waited, and
                    // so should immedaitely return.
                    return None;
                }
            }
        }

        // Find the sequence number for the next segment, we start with snd_nxt
        // unless a fast retransmit is needed.
        //
        // Bail early if congestion control tells us not to send anything.
        let CongestionControlSendOutcome {
            next_seg,
            congestion_limit,
            congestion_window,
            loss_recovery,
        } = congestion_control.poll_send(*snd_una, *snd_nxt, *snd_wnd, readable_bytes)?;

        // First calculate the unused window, note that if our peer has shrank
        // their window (it is strongly discouraged), the following conversion
        // will fail and we return early.
        let snd_limit = *snd_una + *snd_wnd;
        let unused_window = u32::try_from(snd_limit - next_seg).ok_checked::<TryFromIntError>()?;
        let offset =
            usize::try_from(next_seg - *snd_una).unwrap_or_else(|TryFromIntError { .. }| {
                panic!("next_seg({:?}) should never fall behind snd.una({:?})", next_seg, *snd_una);
            });
        let available = u32::try_from(readable_bytes + usize::from(FIN_QUEUED) - offset)
            .unwrap_or_else(|_| WindowSize::MAX.into());
        // We can only send the minimum of the unused or congestion windows and
        // the bytes that are available, additionally, if in zero window probe
        // mode, allow at least one byte past the limit to be sent.
        let can_send = unused_window
            .min(congestion_limit)
            .min(available)
            .min(u32::from(mss))
            .max(u32::from(zero_window_probe));

        if can_send == 0 {
            if available == 0 && offset == 0 && timer.is_none() && keep_alive.enabled {
                log::debug!("starting keep alive idle timer: {:?}", keep_alive.idle.get());
                *timer = Some(SendTimer::KeepAlive(KeepAliveTimer::idle(now, keep_alive)));
            }
            return None;
        }

        let has_fin = FIN_QUEUED && can_send == available;
        let seg = buffer.peek_with(offset, |readable| {
            let bytes_to_send = u32::min(
                can_send - u32::from(has_fin),
                u32::try_from(readable.len()).unwrap_or(u32::MAX),
            );
            let has_fin = has_fin && bytes_to_send == can_send - u32::from(has_fin);

            // Checks if the frame needs to be delayed.
            //
            // Conditions triggering delay:
            //  - we're sending a segment smaller than MSS.
            //  - the segment is not a FIN (a FIN segment never needs to be
            //    delayed).
            //  - this is not a loss-recovery frame.
            let loss_recovery_allow_delay = match loss_recovery {
                LossRecoverySegment::Yes { rearm_retransmit: _, mode: _ } => false,
                LossRecoverySegment::No => true,
            };
            if bytes_to_send < u32::from(mss) && !has_fin && loss_recovery_allow_delay {
                if bytes_to_send == 0 {
                    return None;
                }
                // First check if disallowed by nagle.
                // Per RFC 9293 Section 3.7.4:
                //   If there is unacknowledged data (i.e., SND.NXT > SND.UNA),
                //   then the sending TCP endpoint buffers all user data
                //   (regardless of the PSH bit) until the outstanding data has
                //   been acknowledged or until the TCP endpoint can send a
                //   full-sized segment (Eff.snd.MSS bytes).
                if *nagle_enabled && snd_nxt.after(*snd_una) {
                    return None;
                }
                // Otherwise check if disallowed by SWS avoidance.
                // Per RFC 9293 Section 3.8.6.2.1:
                //   Send data:
                //   (1) if a maximum-sized segment can be sent, i.e., if:
                //       min(D,U) >= Eff.snd.MSS;
                //   (2) or if the data is pushed and all queued data can be
                //       sent now, i.e., if:
                //       [SND.NXT = SND.UNA and] PUSHed and D <= U
                //       (the bracketed condition is imposed by the Nagle algorithm);
                //   (3) or if at least a fraction Fs of the maximum window can
                //       be sent, i.e., if:
                //       [SND.NXT = SND.UNA and] min(D,U) >= Fs * Max(SND.WND);
                //   (4) or if the override timeout occurs.
                //   ... Here Fs is a fraction whose recommended value is 1/2
                // Explanation:
                // To simplify the conditions, we can ignore the brackets since
                // those are controlled by the nagle algorithm and is handled by
                // the block above. Also we consider all data as PUSHed so for
                // example (2) is now simply `D <= U`. Mapping into the code
                // context, `D` is `available` and `U` is `open_window`.
                //
                // The RFC says when to send data, negating it, we will get the
                // condition for when to hold off sending segments, that is:
                // - negate (2) we get D > U,
                // - negate (1) and combine with D > U, we get U < Eff.snd.MSS,
                // - negate (3) and combine with D > U, we get U < Fs * Max(SND.WND).
                // If the overriding timer fired or we are in zero window
                // probing phase, we override it to send data anyways.
                if available > unused_window
                    && unused_window
                        < u32::min(u32::from(mss), u32::from(*snd_wnd_max) / SWS_BUFFER_FACTOR)
                    && !override_sws
                    && !zero_window_probe
                {
                    if timer.is_none() {
                        *timer =
                            Some(SendTimer::SWSProbe { at: now.panicking_add(SWS_PROBE_TIMEOUT) })
                    }
                    return None;
                }
            }

            let seg = rcv.make_segment(|ack, wnd, sack_blocks, ts_opt| {
                let options = SegmentOptions {
                    sack_blocks,
                    // Per RFC 7323 section 3.2:
                    //   Once TSopt has been successfully negotiated [...]
                    //   the TSopt MUST be sent in every non-<RST> segment
                    //   for the duration of the connection.
                    timestamp: ts_opt.make_option_for_ack(now).map(TxTimestampOption::into),
                };
                // We may have to trim bytes_to_send to account for options.
                let bytes_to_send = bytes_to_send.min(u32::from(mss.payload_size(&options).get()));

                // From https://datatracker.ietf.org/doc/html/rfc9293#section-3.9.1.2:
                //
                //  A TCP endpoint MAY implement PUSH flags on SEND calls
                //  (MAY-15). If PUSH flags are not implemented, then the
                //  sending TCP peer: (1) MUST NOT buffer data indefinitely
                //  (MUST-60), and (2) MUST set the PSH bit in the last buffered
                //  segment
                //
                // Given we don't have a well established SEND call boundary in
                // Fuchsia, we can't quite fulfill it, so we apply the PSH bit
                // in 2 situations:
                // - If there's no more data available in the send buffer.
                // - (borrowed from Linux behavior) it's been snd_wnd_max / 2 in
                // sequence space number that we haven't pushed.
                let no_more_data_to_send = u32::try_from(readable_bytes - offset)
                    .is_ok_and(|avail| avail == bytes_to_send);

                let periodic_push =
                    next_seg.after_or_eq(*last_push + snd_wnd_max.halved().max(WindowSize::ONE));
                let push = no_more_data_to_send || periodic_push;
                if bytes_to_send > 0 {
                    *last_data_sent = Some(now);
                }
                let (seg, discarded) = Segment::new(
                    SegmentHeader {
                        seq: next_seg,
                        ack: Some(ack),
                        control: has_fin.then_some(Control::FIN),
                        wnd,
                        options: Options::Segment(options),
                        push,
                    },
                    readable.slice(0..bytes_to_send),
                );
                debug_assert_eq!(discarded, 0);
                seg
            });
            Some(seg)
        })?;
        trace_instant!("tcp::Send::poll_send/segment",
            "id" => id.trace_id(),
            "seq" => u32::from(next_seg),
            "len" => seg.len(),
            "can_send" => can_send,
            "snd_wnd" => u32::from(*snd_wnd),
            "cwnd" => congestion_window,
            "unused_window" => unused_window,
            "available" => available,
        );
        let seq_max = next_seg + seg.len();
        rtt_sampler.on_will_send_segment(now, next_seg..seq_max, *snd_max);
        congestion_control.on_will_send_segment(seg.len());

        if seq_max.after(*snd_nxt) {
            *snd_nxt = seq_max;
        } else {
            // Anything before SND.NXT is possibly a retransmission caused by
            // loss recovery.
            match loss_recovery {
                LossRecoverySegment::Yes { rearm_retransmit: _, ref mode } => match mode {
                    LossRecoveryMode::FastRecovery => counters.increment(|c| &c.fast_retransmits),
                    LossRecoveryMode::SackRecovery => counters.increment(|c| &c.sack_retransmits),
                },
                LossRecoverySegment::No => (),
            }
        }
        if seq_max.after(*snd_max) {
            *snd_max = seq_max;
        } else {
            // Anything before SDN.MAX is considered a retransmitted segment.
            counters.increment(|c| &c.retransmits);
            if congestion_control.in_slow_start() {
                counters.increment(|c| &c.slow_start_retransmits);
            }
        }

        // Record the last pushed segment.
        if seg.header().push {
            *last_push = seg.header().seq;
        }

        // Per https://tools.ietf.org/html/rfc6298#section-5:
        //   (5.1) Every time a packet containing data is sent (including a
        //         retransmission), if the timer is not running, start it
        //         running so that it will expire after RTO seconds (for the
        //         current value of RTO).
        let update_rto = match timer {
            Some(SendTimer::Retrans(_)) | Some(SendTimer::ZeroWindowProbe(_)) => {
                // Loss recovery might have asked us to rearm either way, check
                // with it.
                match loss_recovery {
                    LossRecoverySegment::Yes { rearm_retransmit, mode: _ } => rearm_retransmit,
                    LossRecoverySegment::No => false,
                }
            }
            Some(SendTimer::KeepAlive(_)) | Some(SendTimer::SWSProbe { at: _ }) | None => true,
        };
        if update_rto {
            *timer = Some(SendTimer::Retrans(RetransTimer::new(
                now,
                rtt_estimator.rto(),
                *user_timeout,
                DEFAULT_MAX_RETRIES,
            )))
        }
        Some(seg)
    }

    /// Processes an incoming ACK and returns a segment if one needs to be sent,
    /// along with whether at least one byte of data was ACKed.
    fn process_ack<'a, R: RecvSegmentArgumentsProvider<'a, I>>(
        &mut self,
        id: &impl StateMachineDebugId,
        counters: &TcpCountersRefs<'_>,
        seg_seq: SeqNum,
        seg_ack: SeqNum,
        seg_wnd: UnscaledWindowSize,
        seg_sack_blocks: &SackBlocks,
        rx_ts_opt: Option<RxTimestampOption>,
        pure_ack: bool,
        rcv: R,
        now: I,
        SocketOptions {
            keep_alive,
            nagle_enabled: _,
            user_timeout,
            delayed_ack: _,
            fin_wait2_timeout: _,
            max_syn_retries: _,
            ip_options: _,
        }: &SocketOptions,
    ) -> (Option<Segment<()>>, DataAcked) {
        let Self {
            nxt: snd_nxt,
            max: snd_max,
            una: snd_una,
            wnd: snd_wnd,
            wl1: snd_wl1,
            wl2: snd_wl2,
            last_push: _,
            wnd_max,
            buffer,
            rtt_sampler,
            rtt_estimator,
            timer,
            congestion_control,
            wnd_scale,
            last_data_sent: _,
        } = self;
        let seg_wnd = seg_wnd << *wnd_scale;
        match timer {
            Some(SendTimer::KeepAlive(_)) | None => {
                log::debug!(
                    "restart keep-alive idle timer for receiving an ACK: {:?}",
                    keep_alive.idle.get()
                );
                *timer = keep_alive
                    .enabled
                    .then(|| SendTimer::KeepAlive(KeepAliveTimer::idle(now, keep_alive)));
            }
            Some(SendTimer::Retrans(retrans_timer)) => {
                // Per https://tools.ietf.org/html/rfc6298#section-5:
                //   (5.2) When all outstanding data has been acknowledged,
                //         turn off the retransmission timer.
                //   (5.3) When an ACK is received that acknowledges new
                //         data, restart the retransmission timer so that
                //         it will expire after RTO seconds (for the current
                //         value of RTO).
                if seg_ack == *snd_max {
                    *timer = None;
                } else if seg_ack.before(*snd_max) && seg_ack.after(*snd_una) {
                    *retrans_timer = RetransTimer::new(
                        now,
                        rtt_estimator.rto(),
                        *user_timeout,
                        DEFAULT_MAX_RETRIES,
                    );
                }
            }
            Some(SendTimer::ZeroWindowProbe(_)) | Some(SendTimer::SWSProbe { at: _ }) => {}
        }
        // Note: we rewind SND.NXT to SND.UNA on retransmission; if
        // `seg_ack` is after `snd.max`, it means the segment acks
        // something we never sent.
        if seg_ack.after(*snd_max) {
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-72):
            //   If the ACK acks something not yet sent (SEG.ACK >
            //   SND.NXT) then send an ACK, drop the segment, and
            //   return.
            return (Some(rcv.make_ack(*snd_max, now)), DataAcked::No);
        }

        let bytes_acked = match u32::try_from(seg_ack - *snd_una) {
            Ok(acked) => NonZeroU32::new(acked),
            Err(TryFromIntError { .. }) => {
                // We've received an ACK prior to SND.UNA. This must be an out
                // of order acknowledgement. Ignore it.
                return (None, DataAcked::No);
            }
        };

        let is_dup_ack_by_sack =
            congestion_control.preprocess_ack(seg_ack, *snd_nxt, seg_sack_blocks);
        let (is_dup_ack, data_acked) = if let Some(acked) = bytes_acked {
            let BufferLimits { len, capacity: _ } = buffer.limits();
            let fin_acked = FIN_QUEUED && seg_ack == *snd_una + len + 1;
            // Remove the acked bytes from the send buffer. The following
            // operation should not panic because we are in this branch
            // means seg_ack is before snd.max, thus seg_ack - snd.una
            // cannot exceed the buffer length.
            buffer.mark_read(
                NonZeroUsize::try_from(acked)
                    .unwrap_or_else(|TryFromIntError { .. }| {
                        // we've checked that acked must be smaller than the outstanding
                        // bytes we have in the buffer; plus in Rust, any allocation can
                        // only have a size up to isize::MAX bytes.
                        panic!(
                            "acked({:?}) must be smaller than isize::MAX({:?})",
                            acked,
                            isize::MAX
                        )
                    })
                    .get()
                    - usize::from(fin_acked),
            );
            *snd_una = seg_ack;
            // If the incoming segment acks something that has been sent
            // but not yet retransmitted (`snd.nxt < seg_ack <= snd.max`),
            // bump `snd.nxt` as well.
            if seg_ack.after(*snd_nxt) {
                *snd_nxt = seg_ack;
            }
            // If the incoming ACK can be used as an RTT sample, feed the sample
            // to the estimator.
            if let Some(rtt) = rtt_sampler.on_ack(now, seg_ack, rx_ts_opt) {
                rtt_estimator.sample(
                    rtt,
                    rtt_sampler.samples_per_round_trip(
                        &congestion_control.mss(),
                        congestion_control.flight_size(),
                    ),
                );
            }

            // Note that we may not have an RTT estimation yet, see
            // CongestionControl::on_ack.
            let recovered = congestion_control.on_ack(seg_ack, acked, now, rtt_estimator.srtt());
            if recovered {
                counters.increment(|c| &c.loss_recovered);
            }

            // This can be a duplicate ACK according to the SACK-based algorithm
            // in RFC 6675. Use that if available, otherwise given we've
            // received a new acknowledgement this is not a duplicate ACK.
            let is_dup_ack = is_dup_ack_by_sack.unwrap_or(false);

            // At least one byte of data was ACKed by the peer.
            (is_dup_ack, DataAcked::Yes)
        } else {
            // Check if this is a duplicate ACK according to RFC 5681 if we
            // don't have this information from the SACK blocks.
            let is_dup_ack = is_dup_ack_by_sack.unwrap_or_else(|| {
                // Per RFC 5681
                //   (https://www.rfc-editor.org/rfc/rfc5681#section-2):
                //   DUPLICATE ACKNOWLEDGMENT: An acknowledgment is considered a
                //   "duplicate" in the following algorithms when (a) the
                //   receiver of the ACK has outstanding data, (b) the incoming
                //   acknowledgment carries no data, (c) the SYN and FIN bits
                //   are both off, (d) the acknowledgment number is equal to the
                //   greatest acknowledgment received on the given connection
                //   (TCP.UNA from [RFC793]) and (e) the advertised window in
                //   the incoming acknowledgment equals the advertised window in
                //   the last incoming acknowledgment.
                snd_nxt.after(*snd_una) // (a)
                    && pure_ack // (b) & (c)
                    && seg_ack == *snd_una // (d)
                    && seg_wnd == *snd_wnd // (e)
            });

            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-72):
            //   If the ACK is a duplicate (SEG.ACK < SND.UNA), it can be
            //   ignored.
            (is_dup_ack, DataAcked::No)
        };

        if is_dup_ack {
            counters.increment(|c| &c.dup_acks);
            let new_loss_recovery = congestion_control.on_dup_ack(seg_ack, *snd_nxt);
            match new_loss_recovery {
                Some(LossRecoveryMode::FastRecovery) => counters.increment(|c| &c.fast_recovery),
                Some(LossRecoveryMode::SackRecovery) => counters.increment(|c| &c.sack_recovery),
                None => (),
            }
        }

        // Per RFC 9293
        // (https://datatracker.ietf.org/doc/html/rfc9293#section-3.10.7.4-2.5.2.2.2.3.2.2):
        //   If SND.UNA =< SEG.ACK =< SND.NXT, the send window should be
        //   updated.  If (SND.WL1 < SEG.SEQ or (SND.WL1 = SEG.SEQ and
        //   SND.WL2 =< SEG.ACK)), set SND.WND <- SEG.WND, set
        //   SND.WL1 <- SEG.SEQ, and set SND.WL2 <- SEG.ACK.
        if !snd_una.after(seg_ack)
            && (snd_wl1.before(seg_seq) || (seg_seq == *snd_wl1 && !snd_wl2.after(seg_ack)))
        {
            *snd_wnd = seg_wnd;
            *snd_wl1 = seg_seq;
            *snd_wl2 = seg_ack;
            *wnd_max = seg_wnd.max(*wnd_max);
            if seg_wnd != WindowSize::ZERO && matches!(timer, Some(SendTimer::ZeroWindowProbe(_))) {
                *timer = None;
                // We need to ensure that we're reset when exiting ZWP as if
                // we're going to retransmit. The actual retransmit handling
                // will be performed by poll_send.
                *snd_nxt = *snd_una;
            }
        }

        // Only generate traces for interesting things.
        if data_acked == DataAcked::Yes || is_dup_ack {
            trace_instant!("tcp::Send::process_ack",
                "id" => id.trace_id(),
                "seg_ack" => u32::from(seg_ack),
                "snd_nxt" => u32::from(*snd_nxt),
                "snd_wnd" => u32::from(*snd_wnd),
                "rtt_ms" => u32::try_from(
                    // If we don't have an RTT sample yet use zero to
                    // signal.
                    rtt_estimator.srtt().unwrap_or(Duration::ZERO).as_millis()
                ).unwrap_or(u32::MAX),
                "cwnd" => congestion_control.inspect_cwnd().cwnd(),
                "ssthresh" => congestion_control.slow_start_threshold(),
                "loss_recovery" => congestion_control.inspect_loss_recovery_mode().is_some(),
                "acked" => data_acked == DataAcked::Yes,
            );
        }

        (None, data_acked)
    }

    fn update_mss(&mut self, mss: Mss, seq: SeqNum) -> ShouldRetransmit {
        // Only update the MSS if the provided value is a valid MSS that is less than
        // the current sender MSS. From [RFC 8201 section 5.4]:
        //
        //    A node must not increase its estimate of the Path MTU in response to
        //    the contents of a Packet Too Big message.  A message purporting to
        //    announce an increase in the Path MTU might be a stale packet that has
        //    been floating around in the network, a false packet injected as part
        //    of a denial-of-service (DoS) attack, or the result of having multiple
        //    paths to the destination, each with a different PMTU.
        //
        // [RFC 8201 section 5.4]: https://datatracker.ietf.org/doc/html/rfc8201#section-5.4
        if mss >= *self.congestion_control.mss().mss() {
            return ShouldRetransmit::No;
        }

        // Per [RFC 8201 section 5.4], rewind SND.NXT to the sequence number of the
        // segment that exceeded the MTU, and try to send some more data. This will
        // cause us to retransmit all unacknowledged data starting from that segment in
        // segments that fit into the new path MTU.
        //
        //    Reception of a Packet Too Big message implies that a packet was
        //    dropped by the node that sent the ICMPv6 message.  A reliable upper-
        //    layer protocol will detect this loss by its own means, and recover it
        //    by its normal retransmission methods.  The retransmission could
        //    result in delay, depending on the loss detection method used by the
        //    upper-layer protocol. ...
        //
        //    Alternatively, the retransmission could be done in immediate response
        //    to a notification that the Path MTU was decreased, but only for the
        //    specific connection specified by the Packet Too Big message.  The
        //    packet size used in the retransmission should be no larger than the
        //    new PMTU.
        //
        // [RFC 8201 section 5.4]: https://datatracker.ietf.org/doc/html/rfc8201#section-5.4
        self.nxt = seq;

        // Update the MSS, and let congestion control update the congestion window
        // accordingly.
        self.congestion_control.update_mss(mss, self.una, self.nxt);

        // Have the caller trigger an immediate retransmit of up to the new MSS.
        //
        // We do this to allow PMTUD to continue immediately in case there are further
        // updates to be discovered along the path in use. We could also eagerly
        // retransmit *all* data that was sent after the too-big packet, but if there
        // are further updates to the PMTU, this could cause a packet storm, so we let
        // the retransmission timer take care of any remaining in-flight packets that
        // were too large.
        ShouldRetransmit::Yes
    }

    #[cfg(test)]
    pub(crate) fn congestion_control(&self) -> &CongestionControl<I> {
        &self.congestion_control
    }
}

impl<I: Instant, S: SendBuffer> Send<I, S, { FinQueued::NO }> {
    fn queue_fin(self) -> Send<I, S, { FinQueued::YES }> {
        let Self {
            nxt,
            max,
            una,
            wnd,
            wl1,
            wl2,
            last_push,
            buffer,
            rtt_sampler,
            rtt_estimator,
            timer,
            congestion_control,
            wnd_scale,
            wnd_max,
            last_data_sent,
        } = self;
        Send {
            nxt,
            max,
            una,
            wnd,
            wl1,
            wl2,
            last_push,
            buffer,
            rtt_sampler,
            rtt_estimator,
            timer,
            congestion_control,
            wnd_scale,
            wnd_max,
            last_data_sent,
        }
    }
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
///   CLOSE-WAIT - represents waiting for a connection termination request
///   from the local user.
///
/// Allowed operations:
///   - send
///   - recv (only leftovers and no new data will be accepted from the peer)
///   - shutdown
/// Disallowed operations:
///   - listen
///   - accept
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct CloseWait<I, S> {
    snd: Takeable<Send<I, S, { FinQueued::NO }>>,
    closed_rcv: RecvParams<I>,
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
/// LAST-ACK - represents waiting for an acknowledgment of the
/// connection termination request previously sent to the remote TCP
/// (which includes an acknowledgment of its connection termination
/// request).
///
/// Allowed operations:
///   - recv (only leftovers and no new data will be accepted from the peer)
/// Disallowed operations:
///   - send
///   - shutdown
///   - accept
///   - listen
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct LastAck<I, S> {
    snd: Send<I, S, { FinQueued::YES }>,
    closed_rcv: RecvParams<I>,
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
/// FIN-WAIT-1 - represents waiting for a connection termination request
/// from the remote TCP, or an acknowledgment of the connection
/// termination request previously sent.
///
/// Allowed operations:
///   - recv
/// Disallowed operations:
///   - send
///   - shutdown
///   - accept
///   - listen
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct FinWait1<I, R, S> {
    snd: Takeable<Send<I, S, { FinQueued::YES }>>,
    rcv: Takeable<Recv<I, R>>,
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
/// FIN-WAIT-2 - represents waiting for a connection termination request
/// from the remote TCP.
///
/// Allowed operations:
///   - recv
/// Disallowed operations:
///   - send
///   - shutdown
///   - accept
///   - listen
///   - connect
pub struct FinWait2<I, R> {
    last_seq: SeqNum,
    rcv: Recv<I, R>,
    snd_info: info::SendInfo<I>,
    timeout_at: Option<I>,
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-21):
///
/// CLOSING - represents waiting for a connection termination request
/// acknowledgment from the remote TCP
///
/// Allowed operations:
///   - recv
/// Disallowed operations:
///   - send
///   - shutdown
///   - accept
///   - listen
///   - connect
pub struct Closing<I, S> {
    snd: Send<I, S, { FinQueued::YES }>,
    closed_rcv: RecvParams<I>,
}

/// Per RFC 793 (https://tools.ietf.org/html/rfc793#page-22):
///
/// TIME-WAIT - represents waiting for enough time to pass to be sure
/// the remote TCP received the acknowledgment of its connection
/// termination request.
///
/// Allowed operations:
///   - recv (only leftovers and no new data will be accepted from the peer)
/// Disallowed operations:
///   - send
///   - shutdown
///   - accept
///   - listen
///   - connect
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub struct TimeWait<I> {
    pub(super) last_seq: SeqNum,
    pub(super) expiry: I,
    pub(super) closed_rcv: RecvParams<I>,
    pub(super) snd_info: info::SendInfo<I>,
}

fn new_time_wait_expiry<I: Instant>(now: I) -> I {
    now.panicking_add(MSL * 2)
}

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq, Eq))]
pub enum State<I, R, S, ActiveOpen> {
    Closed(Closed<Option<ConnectionError>>),
    Listen(Listen),
    SynRcvd(SynRcvd<I, ActiveOpen>),
    SynSent(SynSent<I, ActiveOpen>),
    Established(Established<I, R, S>),
    CloseWait(CloseWait<I, S>),
    LastAck(LastAck<I, S>),
    FinWait1(FinWait1<I, R, S>),
    FinWait2(FinWait2<I, R>),
    Closing(Closing<I, S>),
    TimeWait(TimeWait<I>),
}

impl<I, R, S, ActiveOpen> core::fmt::Display for State<I, R, S, ActiveOpen> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        let name = match self {
            State::Closed(_) => "Closed",
            State::Listen(_) => "Listen",
            State::SynRcvd(_) => "SynRcvd",
            State::SynSent(_) => "SynSent",
            State::Established(_) => "Established",
            State::CloseWait(_) => "CloseWait",
            State::LastAck(_) => "LastAck",
            State::FinWait1(_) => "FinWait1",
            State::FinWait2(_) => "FinWait2",
            State::Closing(_) => "Closing",
            State::TimeWait(_) => "TimeWait",
        };
        write!(f, "{name}")
    }
}

#[derive(Debug, PartialEq, Eq)]
/// Possible errors for closing a connection
pub(super) enum CloseError {
    /// The connection is already being closed.
    Closing,
    /// There is no connection to be closed.
    NoConnection,
}

/// A provider that creates receive and send buffers when the connection
/// becomes established.
pub(crate) trait BufferProvider<R: ReceiveBuffer, S: SendBuffer> {
    /// An object that is returned when a passive open connection is
    /// established.
    type PassiveOpen;

    /// An object that is needed to initiate a connection which will be
    /// later used to create buffers once the connection is established.
    type ActiveOpen: IntoBuffers<R, S>;

    /// Creates new send and receive buffers and an object that needs to be
    /// vended to the application.
    fn new_passive_open_buffers(buffer_sizes: BufferSizes) -> (R, S, Self::PassiveOpen);
}

/// Provides a helper for data that needs to be moved out of a reference.
///
/// `Takeable` helps implement the TCP protocol and socket state machines by
/// providing a wrapper around a type that can be moved out of its placement.
///
/// Once moved, `Takeable` will always panic when the value is accessed.
///
/// Moving a `Takeable` always happens through a [`TakeableRef`] which can be
/// transformed either into the inner value or a new `Takeable`. This is useful
/// because [`TakeableRef::take`] moves out of `self``, which makes it a bit
/// more difficult to accidentally call `take`` twice on the same `Takeable`.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct Takeable<T>(Option<T>);

impl<T> From<T> for Takeable<T> {
    fn from(value: T) -> Self {
        Self(Some(value))
    }
}

impl<T> Takeable<T> {
    pub(crate) fn get(&self) -> &T {
        let Self(i) = self;
        i.as_ref().expect("accessed taken takeable")
    }

    pub(crate) fn get_mut(&mut self) -> &mut T {
        let Self(i) = self;
        i.as_mut().expect("accessed taken takeable")
    }

    pub(crate) fn new(v: T) -> Self {
        Self(Some(v))
    }

    pub(crate) fn to_ref(&mut self) -> TakeableRef<'_, T> {
        TakeableRef(self)
    }

    pub(crate) fn from_ref(t: TakeableRef<'_, T>) -> Self {
        let TakeableRef(Self(t)) = t;
        Self(Some(t.take().expect("accessed taken takeable")))
    }

    pub(crate) fn into_inner(self) -> T {
        let Self(i) = self;
        i.expect("accessed taken takeable")
    }

    pub(crate) fn map<R, F: FnOnce(T) -> R>(self, f: F) -> Takeable<R> {
        Takeable(Some(f(self.into_inner())))
    }
}

impl<T> Deref for Takeable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.get()
    }
}

impl<T> DerefMut for Takeable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.get_mut()
    }
}

/// A mutable reference to a [`Takeable`].
pub(crate) struct TakeableRef<'a, T>(&'a mut Takeable<T>);

impl<'a, T> TakeableRef<'a, T> {
    pub(crate) fn take(self) -> T {
        let Self(Takeable(t)) = self;
        t.take().expect("accessed taken takeable")
    }

    pub(crate) fn to_takeable(self) -> Takeable<T> {
        Takeable::new(self.take())
    }
}

#[must_use = "must check to determine if the socket needs to be removed from the demux state"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewlyClosed {
    No,
    Yes,
}

#[derive(Debug)]
pub(crate) enum ShouldRetransmit {
    No,
    Yes,
}

impl<I: Instant + 'static, R: ReceiveBuffer, S: SendBuffer, ActiveOpen: Debug>
    State<I, R, S, ActiveOpen>
{
    /// Updates this state to the provided new state.
    fn transition_to_state(
        &mut self,
        counters: &TcpCountersRefs<'_>,
        new_state: State<I, R, S, ActiveOpen>,
    ) -> NewlyClosed {
        log::debug!("transition to state {} => {}", self, new_state);
        let newly_closed = if let State::Closed(Closed { reason }) = &new_state {
            let (was_established, was_closed) = match self {
                State::Closed(_) => (false, true),
                State::Listen(_) | State::SynRcvd(_) | State::SynSent(_) => (false, false),
                State::Established(_)
                | State::CloseWait(_)
                | State::LastAck(_)
                | State::FinWait1(_)
                | State::FinWait2(_)
                | State::Closing(_)
                | State::TimeWait(_) => (true, false),
            };
            if was_established {
                counters.increment(|c| &c.established_closed);
                match reason {
                    Some(ConnectionError::ConnectionRefused)
                    | Some(ConnectionError::ConnectionReset) => {
                        counters.increment(|c| &c.established_resets);
                    }
                    Some(ConnectionError::TimedOut) => {
                        counters.increment(|c| &c.established_timedout);
                    }
                    _ => {}
                }
            }
            (!was_closed).then_some(NewlyClosed::Yes).unwrap_or(NewlyClosed::No)
        } else {
            NewlyClosed::No
        };
        *self = new_state;
        newly_closed
    }
    /// Processes an incoming segment and advances the state machine.
    ///
    /// Returns a segment if one needs to be sent; if a passive open connection
    /// is newly established, the corresponding object that our client needs
    /// will be returned. Also returns whether an at least one byte of data was
    /// ACKed by the incoming segment.
    pub(crate) fn on_segment<P: Payload, BP: BufferProvider<R, S, ActiveOpen = ActiveOpen>>(
        &mut self,
        id: &impl StateMachineDebugId,
        counters: &TcpCountersRefs<'_>,
        incoming: Segment<P>,
        now: I,
        options @ SocketOptions {
            keep_alive: _,
            nagle_enabled: _,
            user_timeout: _,
            delayed_ack,
            fin_wait2_timeout,
            max_syn_retries: _,
            ip_options: _,
        }: &SocketOptions,
        defunct: bool,
    ) -> (Option<Segment<()>>, Option<BP::PassiveOpen>, DataAcked, NewlyClosed)
    where
        BP::PassiveOpen: Debug,
        ActiveOpen: IntoBuffers<R, S>,
    {
        let mut passive_open = None;
        let mut data_acked = DataAcked::No;
        let (seg, newly_closed) = (|| {
            let (mut rcv, snd_max, rst_on_new_data) = match self {
                State::Closed(closed) => return (closed.on_segment(&incoming), NewlyClosed::No),
                State::Listen(listen) => {
                    return (
                        match listen.on_segment(incoming, now) {
                            ListenOnSegmentDisposition::SendSynAckAndEnterSynRcvd(
                                syn_ack,
                                SynRcvd {
                                    iss,
                                    irs,
                                    rtt_sampler,
                                    retrans_timer,
                                    simultaneous_open,
                                    buffer_sizes,
                                    smss,
                                    rcv_wnd_scale,
                                    snd_wnd_scale,
                                    sack_permitted,
                                    rcv,
                                },
                            ) => {
                                match simultaneous_open {
                                    None => {
                                        assert_eq!(
                                            self.transition_to_state(
                                                counters,
                                                State::SynRcvd(SynRcvd {
                                                    iss,
                                                    irs,
                                                    rtt_sampler,
                                                    retrans_timer,
                                                    simultaneous_open: None,
                                                    buffer_sizes,
                                                    smss,
                                                    rcv_wnd_scale,
                                                    snd_wnd_scale,
                                                    sack_permitted,
                                                    rcv,
                                                }),
                                            ),
                                            NewlyClosed::No
                                        )
                                    }
                                }
                                Some(syn_ack)
                            }
                            ListenOnSegmentDisposition::SendRst(rst) => Some(rst),
                            ListenOnSegmentDisposition::Ignore => None,
                        },
                        NewlyClosed::No,
                    );
                }
                State::SynSent(synsent) => {
                    return match synsent.on_segment(incoming, now) {
                        SynSentOnSegmentDisposition::SendAckAndEnterEstablished(established) => (
                            replace_with_and(self, |this| {
                                assert_matches!(this, State::SynSent(SynSent {
                                    active_open,
                                    buffer_sizes,
                                    ..
                                }) => {
                                    log::debug!("transition to state SynSent => Established");
                                    let Established {snd, rcv} = established;
                                    let (rcv_buffer, snd_buffer) =
                                        active_open.into_buffers(buffer_sizes);
                                    let mut established = Established {
                                        snd: snd.map(|s| s.with_buffer(snd_buffer)),
                                        rcv: rcv.map(|s| s.with_buffer(rcv_buffer)),
                                    };
                                    let ack = Some(
                                        established.rcv.make_ack(established.snd.max, now)
                                    );
                                    (State::Established(established), ack)
                                })
                            }),
                            NewlyClosed::No,
                        ),
                        SynSentOnSegmentDisposition::SendSynAckAndEnterSynRcvd(
                            syn_ack,
                            mut syn_rcvd,
                        ) => {
                            log::debug!("transition to state SynSent => SynRcvd");
                            replace_with(self, |this| {
                                assert_matches!(this, State::SynSent(SynSent {
                                    active_open,
                                    ..
                                }) => {
                                    assert_matches!(syn_rcvd.simultaneous_open.replace(active_open), None);
                                    State::SynRcvd(syn_rcvd)
                                })
                            });
                            (Some(syn_ack), NewlyClosed::No)
                        }
                        SynSentOnSegmentDisposition::SendRst(rst) => (Some(rst), NewlyClosed::No),
                        SynSentOnSegmentDisposition::EnterClosed(closed) => {
                            assert_eq!(
                                self.transition_to_state(counters, State::Closed(closed)),
                                NewlyClosed::Yes,
                            );
                            (None, NewlyClosed::Yes)
                        }
                        SynSentOnSegmentDisposition::Ignore => (None, NewlyClosed::No),
                    };
                }
                State::SynRcvd(SynRcvd {
                    iss,
                    irs: _,
                    rtt_sampler: _,
                    retrans_timer: _,
                    simultaneous_open: _,
                    buffer_sizes: _,
                    smss: _,
                    rcv_wnd_scale: _,
                    snd_wnd_scale: _,
                    sack_permitted: _,
                    rcv,
                }) => (CalculatedRecvParams::from_params(rcv), *iss + 1, false),
                State::Established(Established { rcv, snd }) => {
                    (CalculatedRecvParams::from_recv(rcv.get_mut()), snd.max, false)
                }
                State::CloseWait(CloseWait { snd, closed_rcv }) => {
                    (CalculatedRecvParams::from_params(closed_rcv), snd.max, true)
                }
                State::LastAck(LastAck { snd, closed_rcv })
                | State::Closing(Closing { snd, closed_rcv }) => {
                    (CalculatedRecvParams::from_params(closed_rcv), snd.max, true)
                }
                State::FinWait1(FinWait1 { rcv, snd }) => {
                    let closed = rcv.buffer.is_closed();
                    (CalculatedRecvParams::from_recv(rcv.get_mut()), snd.max, closed)
                }
                State::FinWait2(FinWait2 { last_seq, rcv, timeout_at: _, snd_info: _ }) => {
                    let closed = rcv.buffer.is_closed();
                    (CalculatedRecvParams::from_recv(rcv), *last_seq, closed)
                }
                State::TimeWait(TimeWait { last_seq, expiry: _, closed_rcv, snd_info: _ }) => {
                    (CalculatedRecvParams::from_params(closed_rcv), *last_seq, true)
                }
            };

            // Unreachable note(1): The above match returns early for states CLOSED,
            // SYN_SENT and LISTEN, so it is impossible to have the above states
            // past this line.

            // Reset the connection if we receive new data while the socket is being closed
            // and the receiver has been shut down.
            if rst_on_new_data && (incoming.header().seq + incoming.data().len()).after(rcv.nxt()) {
                return (
                    Some(Segment::rst(
                        snd_max,
                        ResetOptions {
                            timestamp: rcv
                                .ts_opt()
                                .make_option_for_non_ack(now)
                                .map(TxTimestampOption::into),
                        },
                    )),
                    self.transition_to_state(
                        counters,
                        State::Closed(Closed { reason: Some(ConnectionError::ConnectionReset) }),
                    ),
                );
            }

            // NOTE: RFC 7323, section 3.2 states that:
            //   If a non-<RST> segment is received without a TSopt, a TCP
            //   SHOULD silently drop the segment.
            //
            // This recommendation is relevant for implementations that support
            // PAWS, as it's impossible for the PAWS algorithm to verify a
            // segment that lacks the timestamp option.
            //
            // We choose not to abide by the recommendation for two reasons:
            //   1) We don't yet support PAWS, and
            //   2) Other platforms in the industry do not abide by it. Linux,
            //      for example, does not drop such segments (even though it
            //      does implement PAWS).
            //
            // Future readers may find this email thread from the RFC's authors
            // useful for understanding the rationale behind this RFC text:
            // https://mailarchive.ietf.org/arch/msg/tcpm/a9WVwj7s8jR0zCOFgKWQLzHZpGw/
            // TODO(https://fxbug.dev/438961717): Revisit this decision when
            // implementing PAWS.

            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-69):
            //   first check sequence number
            let is_rst = incoming.header().control == Some(Control::RST);
            // pure ACKs (empty segments) don't need to be ack'ed.
            let pure_ack = incoming.len() == 0;
            let needs_ack = !pure_ack;
            let segment = match incoming.overlap(rcv.nxt(), rcv.wnd()) {
                Some(incoming) => incoming,
                None => {
                    // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-69):
                    //   If an incoming segment is not acceptable, an acknowledgment
                    //   should be sent in reply (unless the RST bit is set, if so drop
                    //   the segment and return):
                    //     <SEQ=SND.NXT><ACK=RCV.NXT><CTL=ACK>
                    //   After sending the acknowledgment, drop the unacceptable segment
                    //   and return.
                    let segment = if is_rst {
                        None
                    } else {
                        // Enter quickacks when we receive a segment out of
                        // the window.
                        rcv.reset_quickacks();
                        Some(rcv.make_ack(snd_max, now))
                    };

                    return (segment, NewlyClosed::No);
                }
            };
            let (
                SegmentHeader {
                    seq: seg_seq,
                    ack: seg_ack,
                    wnd: seg_wnd,
                    control,
                    options: seg_options,
                    push: _,
                },
                data,
            ) = segment.into_parts();

            // Per RFC 7323, section 5.3, updating the cached "recent" timestamp
            // from our peer should only occur after we've validated that the
            // segment is within the receive window.
            rcv.ts_opt_mut().update_recent_timestamp(
                seg_seq,
                seg_options.timestamp().map(RxTimestampOption::from),
            );

            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-70):
            //   second check the RST bit
            //   If the RST bit is set then, any outstanding RECEIVEs and SEND
            //   should receive "reset" responses.  All segment queues should be
            //   flushed.  Users should also receive an unsolicited general
            //   "connection reset" signal.  Enter the CLOSED state, delete the
            //   TCB, and return.
            if control == Some(Control::RST) {
                return (
                    None,
                    self.transition_to_state(
                        counters,
                        State::Closed(Closed { reason: Some(ConnectionError::ConnectionReset) }),
                    ),
                );
            }
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-70):
            //   fourth, check the SYN bit
            //   If the SYN is in the window it is an error, send a reset, any
            //   outstanding RECEIVEs and SEND should receive "reset" responses,
            //   all segment queues should be flushed, the user should also
            //   receive an unsolicited general "connection reset" signal, enter
            //   the CLOSED state, delete the TCB, and return.
            //   If the SYN is not in the window this step would not be reached
            //   and an ack would have been sent in the first step (sequence
            //   number check).
            if control == Some(Control::SYN) {
                return (
                    Some(Segment::rst(
                        snd_max,
                        ResetOptions {
                            timestamp: rcv
                                .ts_opt()
                                .make_option_for_non_ack(now)
                                .map(TxTimestampOption::into),
                        },
                    )),
                    self.transition_to_state(
                        counters,
                        State::Closed(Closed { reason: Some(ConnectionError::ConnectionReset) }),
                    ),
                );
            }
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-72):
            //   fifth check the ACK field
            match seg_ack {
                Some(seg_ack) => match self {
                    State::Closed(_) | State::Listen(_) | State::SynSent(_) => {
                        // This unreachable assert is justified by note (1).
                        unreachable!("encountered an already-handled state: {:?}", self)
                    }
                    State::SynRcvd(SynRcvd {
                        iss,
                        irs,
                        rtt_sampler,
                        retrans_timer: _,
                        simultaneous_open,
                        buffer_sizes,
                        smss,
                        rcv_wnd_scale,
                        snd_wnd_scale,
                        sack_permitted,
                        rcv,
                    }) => {
                        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-72):
                        //    if the ACK bit is on
                        //    SYN-RECEIVED STATE
                        //    If SND.UNA =< SEG.ACK =< SND.NXT then enter ESTABLISHED state
                        //    and continue processing.
                        //    If the segment acknowledgment is not acceptable, form a
                        //    reset segment,
                        //      <SEQ=SEG.ACK><CTL=RST>
                        //    and send it.
                        // Note: We don't support sending data with SYN, so we don't
                        // store the `SND` variables because they can be easily derived
                        // from ISS: SND.UNA=ISS and SND.NXT=ISS+1.
                        let snd_next = *iss + 1;
                        let rcv_next = *irs + 1;
                        if seg_ack != snd_next {
                            return (
                                Some(Segment::rst(
                                    seg_ack,
                                    ResetOptions {
                                        timestamp: rcv.timestamp_option_for_non_ack(now),
                                    },
                                )),
                                NewlyClosed::No,
                            );
                        } else {
                            let congestion_control = CongestionControl::cubic_with_mss(*smss);
                            let mut rtt_estimator = Estimator::default();
                            let mut rtt_sampler = SamplingStrategy::new(&rcv.ts_opt, &rtt_sampler);
                            let rx_timestamp_opt =
                                seg_options.timestamp().map(RxTimestampOption::from);
                            if let Some(rtt) = rtt_sampler.on_ack(now, seg_ack, rx_timestamp_opt) {
                                rtt_estimator.sample(
                                    rtt,
                                    rtt_sampler.samples_per_round_trip(
                                        &congestion_control.mss(),
                                        congestion_control.flight_size(),
                                    ),
                                );
                            }
                            let (rcv_buffer, snd_buffer) = match simultaneous_open.take() {
                                None => {
                                    let (rcv_buffer, snd_buffer, client) =
                                        BP::new_passive_open_buffers(*buffer_sizes);
                                    assert_matches!(passive_open.replace(client), None);
                                    (rcv_buffer, snd_buffer)
                                }
                                Some(active_open) => active_open.into_buffers(*buffer_sizes),
                            };
                            let (snd_wnd_scale, rcv_wnd_scale) = snd_wnd_scale
                                .map(|snd_wnd_scale| (snd_wnd_scale, *rcv_wnd_scale))
                                .unwrap_or_default();
                            let established = Established {
                                snd: Send {
                                    nxt: snd_next,
                                    max: snd_next,
                                    una: seg_ack,
                                    wnd: seg_wnd << snd_wnd_scale,
                                    wl1: seg_seq,
                                    wl2: seg_ack,
                                    last_push: snd_next,
                                    buffer: snd_buffer,
                                    rtt_sampler,
                                    rtt_estimator,
                                    timer: None,
                                    congestion_control,
                                    wnd_scale: snd_wnd_scale,
                                    wnd_max: seg_wnd << snd_wnd_scale,
                                    last_data_sent: None,
                                }
                                .into(),
                                rcv: Recv {
                                    buffer: RecvBufferState::Open {
                                        buffer: rcv_buffer,
                                        assembler: Assembler::new(rcv_next),
                                    },
                                    timer: None,
                                    mss: *smss,
                                    wnd_scale: rcv_wnd_scale,
                                    last_segment_at: None,
                                    remaining_quickacks: quickack_counter(
                                        buffer_sizes.rcv_limits(),
                                        *smss,
                                    ),
                                    last_window_update: (rcv_next, buffer_sizes.rwnd()),
                                    sack_permitted: *sack_permitted,
                                    ts_opt: rcv.ts_opt.clone(),
                                }
                                .into(),
                            };
                            assert_eq!(
                                self.transition_to_state(counters, State::Established(established)),
                                NewlyClosed::No
                            );
                        }
                        // Unreachable note(2): Because we either return early or
                        // transition to Established for the ack processing, it is
                        // impossible for SYN_RCVD to appear past this line.
                    }
                    State::Established(Established { snd, rcv }) => {
                        let (ack, segment_acked_data) = snd.process_ack(
                            id,
                            counters,
                            seg_seq,
                            seg_ack,
                            seg_wnd,
                            seg_options.sack_blocks(),
                            seg_options.timestamp().map(RxTimestampOption::from),
                            pure_ack,
                            rcv.get_mut(),
                            now,
                            options,
                        );
                        data_acked = segment_acked_data;
                        if let Some(ack) = ack {
                            return (Some(ack), NewlyClosed::No);
                        }
                    }
                    State::CloseWait(CloseWait { snd, closed_rcv }) => {
                        let (ack, segment_acked_data) = snd.process_ack(
                            id,
                            counters,
                            seg_seq,
                            seg_ack,
                            seg_wnd,
                            seg_options.sack_blocks(),
                            seg_options.timestamp().map(RxTimestampOption::from),
                            pure_ack,
                            closed_rcv,
                            now,
                            options,
                        );
                        data_acked = segment_acked_data;
                        if let Some(ack) = ack {
                            return (Some(ack), NewlyClosed::No);
                        }
                    }
                    State::LastAck(LastAck { snd, closed_rcv }) => {
                        let BufferLimits { len, capacity: _ } = snd.buffer.limits();
                        let fin_seq = snd.una + len + 1;
                        let (ack, segment_acked_data) = snd.process_ack(
                            id,
                            counters,
                            seg_seq,
                            seg_ack,
                            seg_wnd,
                            seg_options.sack_blocks(),
                            seg_options.timestamp().map(RxTimestampOption::from),
                            pure_ack,
                            closed_rcv,
                            now,
                            options,
                        );
                        data_acked = segment_acked_data;
                        if let Some(ack) = ack {
                            return (Some(ack), NewlyClosed::No);
                        } else if seg_ack == fin_seq {
                            return (
                                None,
                                self.transition_to_state(
                                    counters,
                                    State::Closed(Closed { reason: None }),
                                ),
                            );
                        }
                    }
                    State::FinWait1(FinWait1 { snd, rcv }) => {
                        let BufferLimits { len, capacity: _ } = snd.buffer.limits();
                        let fin_seq = snd.una + len + 1;
                        let (ack, segment_acked_data) = snd.process_ack(
                            id,
                            counters,
                            seg_seq,
                            seg_ack,
                            seg_wnd,
                            seg_options.sack_blocks(),
                            seg_options.timestamp().map(RxTimestampOption::from),
                            pure_ack,
                            rcv.get_mut(),
                            now,
                            options,
                        );
                        data_acked = segment_acked_data;
                        if let Some(ack) = ack {
                            return (Some(ack), NewlyClosed::No);
                        } else if seg_ack == fin_seq {
                            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-73):
                            //   In addition to the processing for the ESTABLISHED
                            //   state, if the FIN segment is now acknowledged then
                            //   enter FIN-WAIT-2 and continue processing in that
                            //   state
                            let last_seq = snd.nxt;
                            let finwait2 = FinWait2 {
                                last_seq,
                                rcv: rcv.to_ref().take(),
                                // If the connection is already defunct, we set
                                // a timeout to reclaim, but otherwise, a later
                                // `close` call should set the timer.
                                timeout_at: fin_wait2_timeout.and_then(|timeout| {
                                    defunct.then_some(now.saturating_add(timeout))
                                }),
                                snd_info: info::SendInfo::from_send(&snd),
                            };
                            assert_eq!(
                                self.transition_to_state(counters, State::FinWait2(finwait2)),
                                NewlyClosed::No
                            );
                        }
                    }
                    State::Closing(Closing { snd, closed_rcv }) => {
                        let BufferLimits { len, capacity: _ } = snd.buffer.limits();
                        let fin_seq = snd.una + len + 1;
                        let (ack, segment_acked_data) = {
                            // NB: The compiler gets confused about "reborrow
                            // coercion". Add a type annotation to help it. See
                            // https://github.com/rust-lang/rust/issues/35919.
                            let closed_rcv: &mut RecvParams<I> = closed_rcv;
                            snd.process_ack(
                                id,
                                counters,
                                seg_seq,
                                seg_ack,
                                seg_wnd,
                                seg_options.sack_blocks(),
                                seg_options.timestamp().map(RxTimestampOption::from),
                                pure_ack,
                                closed_rcv,
                                now,
                                options,
                            )
                        };
                        data_acked = segment_acked_data;
                        if let Some(ack) = ack {
                            data_acked = segment_acked_data;
                            return (Some(ack), NewlyClosed::No);
                        } else if seg_ack == fin_seq {
                            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-73):
                            //   In addition to the processing for the ESTABLISHED state, if
                            //   the ACK acknowledges our FIN then enter the TIME-WAIT state,
                            //   otherwise ignore the segment.
                            let timewait = TimeWait {
                                last_seq: snd.nxt,
                                expiry: new_time_wait_expiry(now),
                                closed_rcv: closed_rcv.clone(),
                                snd_info: info::SendInfo::from_send(&snd),
                            };
                            assert_eq!(
                                self.transition_to_state(counters, State::TimeWait(timewait)),
                                NewlyClosed::No
                            );
                        }
                    }
                    State::FinWait2(_) | State::TimeWait(_) => {}
                },
                // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-72):
                //   if the ACK bit is off drop the segment and return
                None => return (None, NewlyClosed::No),
            }
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-74):
            //   seventh, process the segment text
            //   Once in the ESTABLISHED state, it is possible to deliver segment
            //   text to user RECEIVE buffers.  Text from segments can be moved
            //   into buffers until either the buffer is full or the segment is
            //   empty.  If the segment empties and carries an PUSH flag, then
            //   the user is informed, when the buffer is returned, that a PUSH
            //   has been received.
            //
            //   When the TCP takes responsibility for delivering the data to the
            //   user it must also acknowledge the receipt of the data.
            //   Once the TCP takes responsibility for the data it advances
            //   RCV.NXT over the data accepted, and adjusts RCV.WND as
            //   apporopriate to the current buffer availability.  The total of
            //   RCV.NXT and RCV.WND should not be reduced.
            //
            //   Please note the window management suggestions in section 3.7.
            //   Send an acknowledgment of the form:
            //     <SEQ=SND.NXT><ACK=RCV.NXT><CTL=ACK>
            //   This acknowledgment should be piggybacked on a segment being
            //   transmitted if possible without incurring undue delay.
            let maybe_ack_to_text = |rcv: &mut Recv<I, R>, rto: Rto| {
                if !needs_ack {
                    return (None, rcv.nxt());
                }

                // Record timestamp of last data segment and reset quickacks if
                // it's above RTO in case the sender has entered slow start
                // again.
                if let Some(last) = rcv.last_segment_at.replace(now) {
                    if now.saturating_duration_since(last) >= rto.get() {
                        rcv.reset_quickacks();
                    }
                }

                // Write the segment data in the buffer and keep track if it fills
                // any hole in the assembler.
                let had_out_of_order = rcv.buffer.has_out_of_order();
                if data.len() > 0 {
                    let offset = usize::try_from(seg_seq - rcv.nxt()).unwrap_or_else(|TryFromIntError {..}| {
                                panic!("The segment was trimmed to fit the window, thus seg.seq({:?}) must not come before rcv.nxt({:?})", seg_seq, rcv.nxt());
                            });
                    match &mut rcv.buffer {
                        RecvBufferState::Open { buffer, assembler } => {
                            let nwritten = buffer.write_at(offset, &data);
                            let readable = assembler.insert(seg_seq..seg_seq + nwritten);
                            buffer.make_readable(readable, assembler.has_outstanding());
                        }
                        RecvBufferState::Closed { nxt, .. } => *nxt = seg_seq + data.len(),
                    }
                }
                // Per RFC 5681 Section 4.2:
                //  Out-of-order data segments SHOULD be acknowledged
                //  immediately, ...  the receiver SHOULD send an
                //  immediate ACK when it receives a data segment that
                //  fills in all or part of a gap in the sequence space.
                let immediate_ack = !*delayed_ack
                    || had_out_of_order
                    || rcv.buffer.has_out_of_order()
                    // Always send immediate ACKS if we're in a quickack period.
                    || rcv.remaining_quickacks != 0
                    // Per RFC 5681 Section 4.2:
                    //  An implementation is deemed to comply with this
                    //  requirement if it sends at least one acknowledgment
                    //  every time it receives 2*RMSS bytes of new data from the
                    //  sender. [...]
                    //  The sender may be forced to use a segment size less than
                    //  RMSS due to the maximum transmission unit (MTU), the
                    //  path MTU discovery algorithm or other factors.  For
                    //  instance, consider the case when the receiver announces
                    //  an RMSS of X bytes but the sender ends up using a
                    //  segment size of Y bytes (Y < X) due to path MTU
                    //  discovery (or the sender's MTU size).  The receiver will
                    //  generate stretch ACKs if it waits for 2*X bytes to
                    //  arrive before an ACK is sent.  Clearly this will take
                    //  more than 2 segments of size Y bytes. Therefore, while a
                    //  specific algorithm is not defined, it is desirable for
                    //  receivers to attempt to prevent this situation, for
                    //  example, by acknowledging at least every second segment,
                    //  regardless of size.
                    //
                    // To avoid having to keep track of an estimated rmss from
                    // the sender's perspective, we follow the simplified
                    // guidance from the RFC and always acknowledge every other
                    // segment, giving up on the timer if it is set.
                    || rcv.timer.is_some();

                if immediate_ack {
                    rcv.timer = None;
                } else {
                    rcv.timer = Some(ReceiveTimer::DelayedAck {
                        at: now.panicking_add(ACK_DELAY_THRESHOLD),
                    });
                }
                let segment =
                    (!matches!(rcv.timer, Some(ReceiveTimer::DelayedAck { .. }))).then(|| {
                        rcv.remaining_quickacks = rcv.remaining_quickacks.saturating_sub(1);
                        rcv.make_ack(snd_max, now)
                    });
                (segment, rcv.nxt())
            };

            let (ack_to_text, rcv_nxt) = match self {
                State::Closed(_) | State::Listen(_) | State::SynRcvd(_) | State::SynSent(_) => {
                    // This unreachable assert is justified by note (1) and (2).
                    unreachable!("encountered an already-handled state: {:?}", self)
                }
                State::Established(Established { snd, rcv }) => {
                    maybe_ack_to_text(rcv.get_mut(), snd.rtt_estimator.rto())
                }
                State::FinWait1(FinWait1 { snd, rcv }) => {
                    maybe_ack_to_text(rcv.get_mut(), snd.rtt_estimator.rto())
                }
                State::FinWait2(FinWait2 { last_seq: _, rcv, timeout_at: _, snd_info: _ }) => {
                    maybe_ack_to_text(rcv, Rto::DEFAULT)
                }
                State::CloseWait(CloseWait { closed_rcv, .. })
                | State::LastAck(LastAck { closed_rcv, .. })
                | State::Closing(Closing { closed_rcv, .. })
                | State::TimeWait(TimeWait { closed_rcv, .. }) => {
                    // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-75):
                    //   This should not occur, since a FIN has been received from the
                    //   remote side.  Ignore the segment text.
                    (None, closed_rcv.ack)
                }
            };
            // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-75):
            //   eighth, check the FIN bit
            let ack_to_fin = if control == Some(Control::FIN) && rcv_nxt == seg_seq + data.len() {
                // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-75):
                //   If the FIN bit is set, signal the user "connection closing" and
                //   return any pending RECEIVEs with same message, advance RCV.NXT
                //   over the FIN, and send an acknowledgment for the FIN.
                match self {
                    State::Closed(_) | State::Listen(_) | State::SynRcvd(_) | State::SynSent(_) => {
                        // This unreachable assert is justified by note (1) and (2).
                        unreachable!("encountered an already-handled state: {:?}", self)
                    }
                    State::Established(Established { snd, rcv }) => {
                        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-75):
                        //   Enter the CLOSE-WAIT state.
                        let mut params = rcv.handle_fin();
                        let segment = params.make_ack(snd_max, now);
                        let closewait =
                            CloseWait { snd: snd.to_ref().to_takeable(), closed_rcv: params };
                        assert_eq!(
                            self.transition_to_state(counters, State::CloseWait(closewait)),
                            NewlyClosed::No
                        );
                        Some(segment)
                    }
                    State::CloseWait(_) | State::LastAck(_) | State::Closing(_) => {
                        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-75):
                        //   CLOSE-WAIT STATE
                        //     Remain in the CLOSE-WAIT state.
                        //   CLOSING STATE
                        //     Remain in the CLOSING state.
                        //   LAST-ACK STATE
                        //     Remain in the LAST-ACK state.
                        None
                    }
                    State::FinWait1(FinWait1 { snd, rcv }) => {
                        let mut params = rcv.handle_fin();
                        let segment = params.make_ack(snd_max, now);
                        let closing = Closing { snd: snd.to_ref().take(), closed_rcv: params };
                        assert_eq!(
                            self.transition_to_state(counters, State::Closing(closing)),
                            NewlyClosed::No
                        );
                        Some(segment)
                    }
                    State::FinWait2(FinWait2 { last_seq, rcv, timeout_at: _, snd_info }) => {
                        let mut params = rcv.handle_fin();
                        let segment = params.make_ack(snd_max, now);
                        let timewait = TimeWait {
                            last_seq: *last_seq,
                            expiry: new_time_wait_expiry(now),
                            closed_rcv: params,
                            snd_info: snd_info.clone(),
                        };
                        assert_eq!(
                            self.transition_to_state(counters, State::TimeWait(timewait)),
                            NewlyClosed::No,
                        );
                        Some(segment)
                    }
                    State::TimeWait(TimeWait { last_seq, expiry, closed_rcv, snd_info: _ }) => {
                        // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-76):
                        //   TIME-WAIT STATE
                        //     Remain in the TIME-WAIT state.  Restart the 2 MSL time-wait
                        //     timeout.
                        *expiry = new_time_wait_expiry(now);
                        Some(closed_rcv.make_ack(*last_seq, now))
                    }
                }
            } else {
                None
            };
            // If we generated an ACK to FIN, then because of the cumulative nature
            // of ACKs, the ACK generated to text (if any) can be safely overridden.
            (ack_to_fin.or(ack_to_text), NewlyClosed::No)
        })();
        (seg, passive_open, data_acked, newly_closed)
    }

    /// To be called when bytes have been dequeued from the receive buffer.
    ///
    /// Returns a segment to send.
    pub(crate) fn poll_receive_data_dequeued(&mut self, now: I) -> Option<Segment<()>> {
        let (rcv, snd_max) = match self {
            State::Closed(_)
            | State::Listen(_)
            | State::SynRcvd(_)
            | State::SynSent(_)
            | State::CloseWait(_)
            | State::LastAck(_)
            | State::Closing(_)
            | State::TimeWait(_) => return None,
            State::Established(Established { snd, rcv }) => (rcv.get_mut(), snd.max),
            State::FinWait1(FinWait1 { snd, rcv }) => (rcv.get_mut(), snd.max),
            State::FinWait2(FinWait2 { last_seq, rcv, timeout_at: _, snd_info: _ }) => {
                (rcv, *last_seq)
            }
        };

        rcv.poll_receive_data_dequeued(snd_max, now)
    }

    /// Polls if there are any bytes available to send in the buffer.
    ///
    /// Forms one segment of at most `limit` available bytes, as long as the
    /// receiver window allows.
    ///
    /// Returns `Ok` if a segment is available, otherwise whether the state has
    /// become closed will be returned in `Err`.
    pub(crate) fn poll_send(
        &mut self,
        id: &impl StateMachineDebugId,
        counters: &TcpCountersRefs<'_>,
        now: I,
        socket_options: &SocketOptions,
    ) -> Result<Segment<S::Payload<'_>>, NewlyClosed> {
        let newly_closed = self.poll_close(counters, now, socket_options);
        if matches!(self, State::Closed(_)) {
            return Err(newly_closed);
        }
        fn poll_rcv_then_snd<
            'a,
            I: Instant,
            R: ReceiveBuffer,
            S: SendBuffer,
            M: StateMachineDebugId,
            const FIN_QUEUED: bool,
        >(
            id: &M,
            counters: &TcpCountersRefs<'_>,
            snd: &'a mut Send<I, S, FIN_QUEUED>,
            rcv: &'a mut Recv<I, R>,
            now: I,
            socket_options: &SocketOptions,
        ) -> Option<Segment<S::Payload<'a>>> {
            // We favor `rcv` over `snd` if both are present. The alternative
            // will also work (sending whatever is ready at the same time of
            // sending the delayed ACK) but we need special treatment for
            // FIN_WAIT_1 if we choose the alternative, because otherwise we
            // will have a spurious retransmitted FIN.
            let seg = rcv
                .poll_send(snd.max, now)
                .map(|seg| seg.into_empty())
                .or_else(|| snd.poll_send(id, counters, &mut *rcv, now, socket_options));
            // We must have piggybacked an ACK so we can cancel the timer now.
            if seg.is_some() && matches!(rcv.timer, Some(ReceiveTimer::DelayedAck { .. })) {
                rcv.timer = None;
            }
            seg
        }
        let seg = match self {
            State::SynSent(SynSent {
                iss,
                rtt_sampler,
                retrans_timer,
                active_open: _,
                buffer_sizes: _,
                device_mss,
                default_mss: _,
                rcv_wnd_scale,
                ts_opt,
            }) => (retrans_timer.at <= now).then(|| {
                rtt_sampler.on_will_send_segment(now, *iss..*iss + 1, *iss);
                retrans_timer.backoff(now);
                Segment::syn(
                    *iss,
                    UnscaledWindowSize::from(u16::MAX),
                    HandshakeOptions {
                        mss: Some(*device_mss),
                        window_scale: Some(*rcv_wnd_scale),
                        sack_permitted: SACK_PERMITTED,
                        timestamp: ts_opt.make_option_for_syn(now).map(TxTimestampOption::into),
                    },
                )
            }),
            State::SynRcvd(SynRcvd {
                iss,
                irs,
                rtt_sampler,
                retrans_timer,
                simultaneous_open: _,
                buffer_sizes: _,
                smss,
                rcv_wnd_scale,
                snd_wnd_scale,
                sack_permitted: _,
                rcv,
            }) => (retrans_timer.at <= now).then(|| {
                rtt_sampler.on_will_send_segment(now, *iss..*iss + 1, *iss);
                retrans_timer.backoff(now);
                Segment::syn_ack(
                    *iss,
                    *irs + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    HandshakeOptions {
                        mss: Some(*smss.mss()),
                        window_scale: snd_wnd_scale.map(|_| *rcv_wnd_scale),
                        sack_permitted: SACK_PERMITTED,
                        // Per RFC 7323 section 3.2:
                        //   A TCP MAY send a TSopt in <SYN,ACK> only if it
                        //   received a TSopt in the initial <SYN> segment for
                        //   the connection.
                        timestamp: rcv.timestamp_option_for_ack(now),
                    },
                )
            }),
            State::Established(Established { snd, rcv }) => {
                poll_rcv_then_snd(id, counters, snd, rcv, now, socket_options)
            }
            State::CloseWait(CloseWait { snd, closed_rcv }) => {
                snd.poll_send(id, counters, closed_rcv, now, socket_options)
            }
            State::LastAck(LastAck { snd, closed_rcv })
            | State::Closing(Closing { snd, closed_rcv }) => {
                snd.poll_send(id, counters, closed_rcv, now, socket_options)
            }
            State::FinWait1(FinWait1 { snd, rcv }) => {
                poll_rcv_then_snd(id, counters, snd, rcv, now, socket_options)
            }
            State::FinWait2(FinWait2 { last_seq, rcv, timeout_at: _, snd_info: _ }) => {
                rcv.poll_send(*last_seq, now).map(|seg| seg.into_empty())
            }
            State::Closed(_) | State::Listen(_) | State::TimeWait(_) => None,
        };
        seg.ok_or(NewlyClosed::No)
    }

    /// Polls the state machine to check if the connection should be closed.
    ///
    /// Returns whether the connection has been closed.
    fn poll_close(
        &mut self,
        counters: &TcpCountersRefs<'_>,
        now: I,
        SocketOptions {
            keep_alive,
            nagle_enabled: _,
            user_timeout: _,
            delayed_ack: _,
            fin_wait2_timeout: _,
            max_syn_retries: _,
            ip_options: _,
        }: &SocketOptions,
    ) -> NewlyClosed {
        let timed_out = match self {
            State::Established(Established { snd, rcv: _ }) => snd.timed_out(now, keep_alive),
            State::CloseWait(CloseWait { snd, closed_rcv: _ }) => snd.timed_out(now, keep_alive),
            State::LastAck(LastAck { snd, closed_rcv: _ })
            | State::Closing(Closing { snd, closed_rcv: _ }) => snd.timed_out(now, keep_alive),
            State::FinWait1(FinWait1 { snd, rcv: _ }) => snd.timed_out(now, keep_alive),
            State::SynSent(SynSent { retrans_timer, .. })
            | State::SynRcvd(SynRcvd { retrans_timer, .. }) => retrans_timer.timed_out(now),

            State::Closed(_) | State::Listen(_) | State::TimeWait(_) => false,
            State::FinWait2(FinWait2 { last_seq: _, rcv: _, timeout_at, snd_info: _ }) => {
                timeout_at.map(|at| now >= at).unwrap_or(false)
            }
        };
        if timed_out {
            return self.transition_to_state(
                counters,
                State::Closed(Closed { reason: Some(ConnectionError::TimedOut) }),
            );
        } else if let State::TimeWait(tw) = self {
            if tw.expiry <= now {
                return self.transition_to_state(counters, State::Closed(Closed { reason: None }));
            }
        }
        NewlyClosed::No
    }

    /// Returns an instant at which the caller SHOULD make their best effort to
    /// call [`poll_send`].
    ///
    /// An example synchronous protocol loop would look like:
    ///
    /// ```ignore
    /// loop {
    ///     let now = Instant::now();
    ///     output(state.poll_send(now));
    ///     let incoming = wait_until(state.poll_send_at())
    ///     output(state.on_segment(incoming, Instant::now()));
    /// }
    /// ```
    ///
    /// Note: When integrating asynchronously, the caller needs to install
    /// timers (for example, by using `TimerContext`), then calls to
    /// `poll_send_at` and to `install_timer`/`cancel_timer` should not
    /// interleave, otherwise timers may be lost.
    pub(crate) fn poll_send_at(&self) -> Option<I> {
        let combine_expiry = |e1: Option<I>, e2: Option<I>| match (e1, e2) {
            (None, None) => None,
            (None, Some(e2)) => Some(e2),
            (Some(e1), None) => Some(e1),
            (Some(e1), Some(e2)) => Some(e1.min(e2)),
        };
        match self {
            State::Established(Established { snd, rcv }) => combine_expiry(
                snd.timer.as_ref().map(SendTimer::expiry),
                rcv.timer.as_ref().map(ReceiveTimer::expiry),
            ),
            State::CloseWait(CloseWait { snd, closed_rcv: _ }) => Some(snd.timer?.expiry()),
            State::LastAck(LastAck { snd, closed_rcv: _ })
            | State::Closing(Closing { snd, closed_rcv: _ }) => Some(snd.timer?.expiry()),
            State::FinWait1(FinWait1 { snd, rcv }) => combine_expiry(
                snd.timer.as_ref().map(SendTimer::expiry),
                rcv.timer.as_ref().map(ReceiveTimer::expiry),
            ),
            State::FinWait2(FinWait2 { last_seq: _, rcv, timeout_at, snd_info: _ }) => {
                combine_expiry(*timeout_at, rcv.timer.as_ref().map(ReceiveTimer::expiry))
            }
            State::SynRcvd(syn_rcvd) => Some(syn_rcvd.retrans_timer.at),
            State::SynSent(syn_sent) => Some(syn_sent.retrans_timer.at),
            State::Closed(_) | State::Listen(_) => None,
            State::TimeWait(TimeWait { last_seq: _, expiry, closed_rcv: _, snd_info: _ }) => {
                Some(*expiry)
            }
        }
    }

    /// Corresponds to the [CLOSE](https://tools.ietf.org/html/rfc793#page-60)
    /// user call.
    ///
    /// The caller should provide the current time if this close call would make
    /// the connection defunct, so that we can reclaim defunct connections based
    /// on timeouts.
    pub(super) fn close(
        &mut self,
        counters: &TcpCountersRefs<'_>,
        close_reason: CloseReason<I>,
        socket_options: &SocketOptions,
    ) -> Result<NewlyClosed, CloseError>
    where
        ActiveOpen: IntoBuffers<R, S>,
    {
        match self {
            State::Closed(_) => Err(CloseError::NoConnection),
            State::Listen(_) | State::SynSent(_) => {
                Ok(self.transition_to_state(counters, State::Closed(Closed { reason: None })))
            }
            State::SynRcvd(SynRcvd {
                iss,
                irs,
                rtt_sampler: _,
                retrans_timer: _,
                simultaneous_open,
                buffer_sizes,
                smss,
                rcv_wnd_scale,
                snd_wnd_scale,
                sack_permitted,
                rcv: RecvParams { ack: _, wnd_scale: _, wnd: _, ts_opt, last_ack_recv: _ },
            }) => {
                // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-60):
                //   SYN-RECEIVED STATE
                //     If no SENDs have been issued and there is no pending data
                //     to send, then form a FIN segment and send it, and enter
                //     FIN-WAIT-1 state; otherwise queue for processing after
                //     entering ESTABLISHED state.
                // Note: Per RFC, we should transition into FIN-WAIT-1, however
                // popular implementations deviate from it - Freebsd resets the
                // connection instead of a normal shutdown:
                // https://github.com/freebsd/freebsd-src/blob/8fc80638496e620519b2585d9fab409494ea4b43/sys/netinet/tcp_subr.c#L2344-L2346
                // while Linux simply does not send anything:
                // https://github.com/torvalds/linux/blob/68e77ffbfd06ae3ef8f2abf1c3b971383c866983/net/ipv4/inet_connection_sock.c#L1180-L1187
                // Here we choose the Linux's behavior, because it is more
                // popular and it is still correct from the protocol's point of
                // view: the peer will find out eventually when it retransmits
                // its SYN - it will get a RST back because now the listener no
                // longer exists - it is as if the initial SYN is lost. The
                // following check makes sure we only proceed if we were
                // actively opened, i.e., initiated by `connect`.
                let (rcv_buffer, snd_buffer) = simultaneous_open
                    .take()
                    .expect(
                        "a SYN-RCVD state that is in the pending queue \
                        should call abort instead of close",
                    )
                    .into_buffers(*buffer_sizes);
                // Note: `Send` in `FinWait1` always has a FIN queued.
                // Since we don't support sending data when connection
                // isn't established, so enter FIN-WAIT-1 immediately.
                let (snd_wnd_scale, rcv_wnd_scale) = snd_wnd_scale
                    .map(|snd_wnd_scale| (snd_wnd_scale, *rcv_wnd_scale))
                    .unwrap_or_default();
                let next = *iss + 1;
                let finwait1 = FinWait1 {
                    snd: Send {
                        nxt: next,
                        max: next,
                        una: next,
                        wnd: WindowSize::DEFAULT,
                        wl1: *iss,
                        wl2: *irs,
                        last_push: next,
                        buffer: snd_buffer,
                        rtt_sampler: SamplingStrategy::new(&ts_opt, &MeasuredSampler::default()),
                        rtt_estimator: Estimator::NoSample,
                        timer: None,
                        congestion_control: CongestionControl::cubic_with_mss(*smss),
                        wnd_scale: snd_wnd_scale,
                        wnd_max: WindowSize::DEFAULT,
                        last_data_sent: None,
                    }
                    .into(),
                    rcv: Recv {
                        buffer: RecvBufferState::Open {
                            buffer: rcv_buffer,
                            assembler: Assembler::new(*irs + 1),
                        },
                        timer: None,
                        mss: *smss,
                        remaining_quickacks: quickack_counter(buffer_sizes.rcv_limits(), *smss),
                        last_segment_at: None,
                        wnd_scale: rcv_wnd_scale,
                        last_window_update: (*irs + 1, buffer_sizes.rwnd()),
                        sack_permitted: *sack_permitted,
                        ts_opt: ts_opt.clone(),
                    }
                    .into(),
                };
                Ok(self.transition_to_state(counters, State::FinWait1(finwait1)))
            }
            State::Established(Established { snd, rcv }) => {
                // Per RFC 793 (https://tools.ietf.org/html/rfc793#page-60):
                //   ESTABLISHED STATE
                //     Queue this until all preceding SENDs have been segmentized,
                //     then form a FIN segment and send it.  In any case, enter
                //     FIN-WAIT-1 state.
                let finwait1 = FinWait1 {
                    snd: snd.to_ref().take().queue_fin().into(),
                    rcv: rcv.to_ref().to_takeable(),
                };
                Ok(self.transition_to_state(counters, State::FinWait1(finwait1)))
            }
            State::CloseWait(CloseWait { snd, closed_rcv }) => {
                let lastack = LastAck {
                    snd: snd.to_ref().take().queue_fin(),
                    closed_rcv: closed_rcv.clone(),
                };
                Ok(self.transition_to_state(counters, State::LastAck(lastack)))
            }
            State::LastAck(_) | State::FinWait1(_) | State::Closing(_) | State::TimeWait(_) => {
                Err(CloseError::Closing)
            }
            State::FinWait2(FinWait2 { last_seq: _, rcv: _, timeout_at, snd_info: _ }) => {
                if let (CloseReason::Close { now }, Some(fin_wait2_timeout)) =
                    (close_reason, socket_options.fin_wait2_timeout)
                {
                    assert_eq!(timeout_at.replace(now.saturating_add(fin_wait2_timeout)), None);
                }
                Err(CloseError::Closing)
            }
        }
    }

    pub(super) fn shutdown_recv(&mut self) -> Result<(), CloseError> {
        match self {
            State::Closed(_) => Err(CloseError::NoConnection),

            State::Listen(_)
            | State::SynSent(_)
            | State::SynRcvd(_)
            | State::CloseWait(_)
            | State::LastAck(_)
            | State::Closing(_)
            | State::TimeWait(_) => Ok(()),

            // Shutdown receive by closing the receive buffer.
            State::Established(Established { rcv, .. }) | State::FinWait1(FinWait1 { rcv, .. }) => {
                rcv.buffer.close();
                Ok(())
            }
            State::FinWait2(FinWait2 { rcv, .. }) => {
                rcv.buffer.close();
                Ok(())
            }
        }
    }

    /// Corresponds to [ABORT](https://tools.ietf.org/html/rfc9293#section-3.10.5)
    /// user call.
    pub(crate) fn abort(
        &mut self,
        counters: &TcpCountersRefs<'_>,
        now: I,
        reason: ConnectionError,
    ) -> (Option<Segment<()>>, NewlyClosed) {
        let reply = match self {
            //   LISTEN STATE
            //      *  Any outstanding RECEIVEs should be returned with "error:
            //      connection reset" responses.  Delete TCB, enter CLOSED state, and
            //      return.
            //   SYN-SENT STATE
            //   *  All queued SENDs and RECEIVEs should be given "connection reset"
            //      notification.  Delete the TCB, enter CLOSED state, and return.
            //   CLOSING STATE
            //   LAST-ACK STATE
            //   TIME-WAIT STATE
            //   *  Respond with "ok" and delete the TCB, enter CLOSED state, and
            //      return.
            State::Closed(_)
            | State::Listen(_)
            | State::SynSent(_)
            | State::Closing(_)
            | State::LastAck(_)
            | State::TimeWait(_) => None,
            //   SYN-RECEIVED STATE
            //   ESTABLISHED STATE
            //   FIN-WAIT-1 STATE
            //   FIN-WAIT-2 STATE
            //   CLOSE-WAIT STATE
            //   *  Send a reset segment:
            //      <SEQ=SND.NXT><CTL=RST>
            //   *  All queued SENDs and RECEIVEs should be given "connection reset"
            //      notification; all segments queued for transmission (except for the
            //      RST formed above) or retransmission should be flushed.  Delete the
            //      TCB, enter CLOSED state, and return.
            State::SynRcvd(SynRcvd {
                iss,
                irs,
                rtt_sampler: _,
                retrans_timer: _,
                simultaneous_open: _,
                buffer_sizes: _,
                smss: _,
                rcv_wnd_scale: _,
                snd_wnd_scale: _,
                sack_permitted: _,
                rcv,
            }) => {
                // When we're in SynRcvd we already sent out SYN-ACK with iss,
                // so a RST must have iss+1.
                Some(Segment::rst_ack(
                    *iss + 1,
                    *irs + 1,
                    ResetOptions { timestamp: rcv.timestamp_option_for_ack(now) },
                ))
            }
            State::Established(Established { snd, rcv }) => Some(Segment::rst_ack(
                snd.nxt,
                rcv.nxt(),
                ResetOptions { timestamp: rcv.timestamp_option_for_ack(now) },
            )),
            State::FinWait1(FinWait1 { snd, rcv }) => Some(Segment::rst_ack(
                snd.nxt,
                rcv.nxt(),
                ResetOptions { timestamp: rcv.timestamp_option_for_ack(now) },
            )),
            State::FinWait2(FinWait2 { rcv, last_seq, timeout_at: _, snd_info: _ }) => {
                Some(Segment::rst_ack(
                    *last_seq,
                    rcv.nxt(),
                    ResetOptions { timestamp: rcv.timestamp_option_for_ack(now) },
                ))
            }
            State::CloseWait(CloseWait { snd, closed_rcv }) => Some(Segment::rst_ack(
                snd.nxt,
                closed_rcv.ack,
                ResetOptions { timestamp: closed_rcv.timestamp_option_for_ack(now) },
            )),
        };
        (reply, self.transition_to_state(counters, State::Closed(Closed { reason: Some(reason) })))
    }

    pub(crate) fn buffers_mut(&mut self) -> BuffersRefMut<'_, R, S> {
        match self {
            State::TimeWait(_) | State::Closed(_) => BuffersRefMut::NoBuffers,
            State::Listen(Listen { buffer_sizes, .. })
            | State::SynRcvd(SynRcvd { buffer_sizes, .. })
            | State::SynSent(SynSent { buffer_sizes, .. }) => BuffersRefMut::Sizes(buffer_sizes),
            State::Established(Established { snd, rcv }) => match &mut rcv.buffer {
                RecvBufferState::Open { buffer: recv_buf, .. } => {
                    BuffersRefMut::Both { send: &mut snd.buffer, recv: recv_buf }
                }
                RecvBufferState::Closed { .. } => BuffersRefMut::SendOnly(&mut snd.buffer),
            },
            State::FinWait1(FinWait1 { snd, rcv }) => match &mut rcv.buffer {
                RecvBufferState::Open { buffer: recv_buf, .. } => {
                    BuffersRefMut::Both { send: &mut snd.buffer, recv: recv_buf }
                }
                RecvBufferState::Closed { .. } => BuffersRefMut::SendOnly(&mut snd.buffer),
            },
            State::FinWait2(FinWait2::<I, R> { rcv, .. }) => match &mut rcv.buffer {
                RecvBufferState::Open { buffer: recv_buf, .. } => BuffersRefMut::RecvOnly(recv_buf),
                RecvBufferState::Closed { .. } => BuffersRefMut::NoBuffers,
            },
            State::Closing(Closing::<I, S> { snd, .. })
            | State::LastAck(LastAck::<I, S> { snd, .. }) => {
                BuffersRefMut::SendOnly(&mut snd.buffer)
            }
            State::CloseWait(CloseWait::<I, S> { snd, .. }) => {
                BuffersRefMut::SendOnly(&mut snd.buffer)
            }
        }
    }

    /// Processes an incoming ICMP error, returns an soft error that needs to be
    /// recorded in the containing socket.
    pub(super) fn on_icmp_error(
        &mut self,
        counters: &TcpCountersRefs<'_>,
        err: IcmpErrorCode,
        seq: SeqNum,
    ) -> (Option<ConnectionError>, NewlyClosed, ShouldRetransmit) {
        let Some(result) = IcmpErrorResult::try_from_icmp_error(err) else {
            return (None, NewlyClosed::No, ShouldRetransmit::No);
        };
        let err = match result {
            IcmpErrorResult::ConnectionError(err) => err,
            IcmpErrorResult::PmtuUpdate(mms) => {
                // Clamp the PMTU Update to the minimum allowed MSS.
                let mss = Mss::from_mms(mms).unwrap_or(Mss::MIN);
                let should_send = self.on_pmtu_update(mss, seq);
                return (None, NewlyClosed::No, should_send);
            }
        };
        // We consider the following RFC quotes when implementing this function.
        // Per RFC 5927 Section 4.1:
        //  Many TCP implementations have incorporated a validation check such
        //  that they react only to those ICMP error messages that appear to
        //  relate to segments currently "in flight" to the destination system.
        //  These implementations check that the TCP sequence number contained
        //  in the payload of the ICMP error message is within the range
        //  SND.UNA =< SEG.SEQ < SND.NXT.
        // Per RFC 5927 Section 5.2:
        //  Based on this analysis, most popular TCP implementations treat all
        //  ICMP "hard errors" received for connections in any of the
        //  synchronized states (ESTABLISHED, FIN-WAIT-1, FIN-WAIT-2, CLOSE-WAIT,
        //  CLOSING, LAST-ACK, or TIME-WAIT) as "soft errors".  That is, they do
        //  not abort the corresponding connection upon receipt of them.
        // Per RFC 5461 Section 4.1:
        //  A number of TCP implementations have modified their reaction to all
        //  ICMP soft errors and treat them as hard errors when they are received
        //  for connections in the SYN-SENT or SYN-RECEIVED states. For example,
        //  this workaround has been implemented in the Linux kernel since
        //  version 2.0.0 (released in 1996) [Linux]
        let connect_error = match self {
            State::Closed(_) => None,
            State::Listen(listen) => unreachable!(
                "ICMP errors should not be delivered on a listener, received code {:?} on {:?}",
                err, listen
            ),
            State::SynRcvd(SynRcvd {
                iss,
                irs: _,
                rtt_sampler: _,
                retrans_timer: _,
                simultaneous_open: _,
                buffer_sizes: _,
                smss: _,
                rcv_wnd_scale: _,
                snd_wnd_scale: _,
                sack_permitted: _,
                rcv: _,
            })
            | State::SynSent(SynSent {
                iss,
                rtt_sampler: _,
                retrans_timer: _,
                active_open: _,
                buffer_sizes: _,
                device_mss: _,
                default_mss: _,
                rcv_wnd_scale: _,
                ts_opt: _,
            }) => {
                if *iss == seq {
                    return (
                        None,
                        self.transition_to_state(
                            counters,
                            State::Closed(Closed { reason: Some(err) }),
                        ),
                        ShouldRetransmit::No,
                    );
                }
                None
            }
            State::Established(Established { snd, rcv: _ })
            | State::CloseWait(CloseWait { snd, closed_rcv: _ }) => {
                (!snd.una.after(seq) && seq.before(snd.nxt)).then_some(err)
            }
            State::LastAck(LastAck { snd, closed_rcv: _ })
            | State::Closing(Closing { snd, closed_rcv: _ }) => {
                (!snd.una.after(seq) && seq.before(snd.nxt)).then_some(err)
            }
            State::FinWait1(FinWait1 { snd, rcv: _ }) => {
                (!snd.una.after(seq) && seq.before(snd.nxt)).then_some(err)
            }
            // The following states does not have any outstanding segments, so
            // they don't expect any incoming ICMP error.
            State::FinWait2(_) | State::TimeWait(_) => None,
        };
        (connect_error, NewlyClosed::No, ShouldRetransmit::No)
    }

    fn on_pmtu_update(&mut self, mss: Mss, seq: SeqNum) -> ShouldRetransmit {
        // If the sequence number of the segment that was too large does not correspond
        // to an in-flight segment, ignore the error.
        match self {
            State::Listen(listen) => unreachable!(
                "PMTU updates should not be delivered to a listener, received {mss:?} on {listen:?}"
            ),
            State::Closed(_)
            | State::SynRcvd(_)
            | State::SynSent(_)
            | State::FinWait2(_)
            | State::TimeWait(_) => {}
            State::Established(Established { snd, .. })
            | State::CloseWait(CloseWait { snd, .. }) => {
                if !snd.una.after(seq) && seq.before(snd.nxt) {
                    return snd.update_mss(mss, seq);
                }
            }
            State::LastAck(LastAck { snd, .. }) | State::Closing(Closing { snd, .. }) => {
                if !snd.una.after(seq) && seq.before(snd.nxt) {
                    return snd.update_mss(mss, seq);
                }
            }
            State::FinWait1(FinWait1 { snd, .. }) => {
                if !snd.una.after(seq) && seq.before(snd.nxt) {
                    return snd.update_mss(mss, seq);
                }
            }
        }
        ShouldRetransmit::No
    }
}

/// From the socket layer, both `close` and `shutdown` will result in a state
/// machine level `close` call. We need to differentiate between the two
/// because we may need to do extra work if it is a socket `close`.
pub(super) enum CloseReason<I: Instant> {
    Shutdown,
    Close { now: I },
}

#[cfg(test)]
mod test {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::fmt::Debug;
    use core::time::Duration;

    use assert_matches::assert_matches;
    use netstack3_base::sync::ResourceTokenValue;
    use netstack3_base::testutil::{FakeInstant, FakeInstantCtx};
    use netstack3_base::{
        CounterCollection, FragmentedPayload, InstantContext as _, Milliseconds, Options,
        SackBlock, Timestamp, TimestampOption, Unitless,
    };
    use packet::InnerPacketBuilder as _;
    use test_case::{test_case, test_matrix};

    use super::*;
    use crate::internal::base::DEFAULT_FIN_WAIT2_TIMEOUT;
    use crate::internal::buffer::Buffer;
    use crate::internal::buffer::testutil::{InfiniteSendBuffer, RepeatingSendBuffer, RingBuffer};
    use crate::internal::congestion::DUP_ACK_THRESHOLD;
    use crate::internal::counters::TcpCountersWithSocketInner;
    use crate::internal::counters::testutil::CounterExpectations;
    use crate::internal::rtt::{MeasuredSampler, TimestampSampler};
    use crate::internal::state::info::SendInfo;
    use crate::internal::timestamp::{TS_ECHO_REPLY_FOR_NON_ACKS, TimestampValueState};

    const TEST_IRS: SeqNum = SeqNum::new(100);
    const TEST_ISS: SeqNum = SeqNum::new(300);

    const ISS_1: SeqNum = SeqNum::new(500);
    const ISS_2: SeqNum = SeqNum::new(700);

    const RTT: Duration = Duration::from_millis(500);

    const DEVICE_MAXIMUM_SEGMENT_SIZE: Mss = Mss::new(1400).unwrap();

    const TEST_MSS: EffectiveMss =
        EffectiveMss::from_mss(Mss::new(256).unwrap(), MssSizeLimiters { timestamp_enabled: true });
    // Support Buffering up to three MSS.
    const BUFFER_SIZE: usize = (TEST_MSS.get() as usize) * 3;
    // One MSS worth of test data.
    const TEST_BYTES: &[u8] = &[0xab; TEST_MSS.get() as usize];

    // The TCP Timestamp Option uses randomized offsets per connection for
    // security. Simplify tests by using a fixed value.
    const TIMESTAMP_OFFSET: Timestamp<Milliseconds> = Timestamp::new(12345);

    // The timestamp we'll send if the system clock has not advanced.
    const DEFAULT_TIMESTAMP: Timestamp<Unitless> = TIMESTAMP_OFFSET.discard_unit();
    // The default Timestamp Option we'll send in a Non-ACK segment.
    const DEFAULT_NON_ACK_TS_OPT: TimestampOption =
        TimestampOption::new(DEFAULT_TIMESTAMP, TS_ECHO_REPLY_FOR_NON_ACKS);
    // The default Timestamp Option we'll send in an ACK segment
    const DEFAULT_ACK_TS_OPT: TimestampOption = TimestampOption::new(
        // NB: Most of the tests establish a connection with ourself. As such,
        // we end up echoing back our own timestamp.
        DEFAULT_TIMESTAMP,
        DEFAULT_TIMESTAMP,
    );

    // The timestamp we'll send if the system clock has advance by one RTT.
    // TODO(https://github.com/rust-lang/rust/issues/143802): Directly add `RTT`
    // to `TIMESTAMP_OFFSET` once const `Add` implementations are supported.
    const RTT_TIMESTAMP: Timestamp<Unitless> =
        Timestamp::new(TIMESTAMP_OFFSET.get() + RTT.as_millis() as u32);
    // The Timestamp Option we'll send in a Non-ACK segment after a single RTT
    // has elapsed.
    const NON_ACK_TS_OPT_AFTER_RTT: TimestampOption =
        TimestampOption::new(RTT_TIMESTAMP, TS_ECHO_REPLY_FOR_NON_ACKS);
    // The Timestamp Option we'll send in an ACK segment after a single RTT has
    // elapsed.
    const ACK_TS_OPT_AFTER_RTT: TimestampOption =
        TimestampOption::new(RTT_TIMESTAMP, DEFAULT_TIMESTAMP);

    const fn default_ts_opt_state(last_ack: SeqNum) -> TimestampOptionState<FakeInstant> {
        TimestampOptionState::Enabled {
            // Most tests negotiate the Timestamp Option with ourself.
            // As such the most recent timestamp value we've received from the
            // peer is our own timestamp.
            ts_recent: DEFAULT_TIMESTAMP,
            last_ack_sent: last_ack,
            ts_val: TimestampValueState {
                offset: TIMESTAMP_OFFSET,
                initialized_at: FakeInstant { offset: Duration::ZERO },
            },
        }
    }

    const fn default_ts_opt_negotiation_state() -> TimestampOptionNegotiationState<FakeInstant> {
        TimestampOptionNegotiationState::Negotiating(TimestampValueState {
            offset: TIMESTAMP_OFFSET,
            initialized_at: FakeInstant { offset: Duration::ZERO },
        })
    }

    const fn default_segment_options(
        ts_val: Timestamp<Unitless>,
        ts_echo_reply: Timestamp<Unitless>,
    ) -> SegmentOptions {
        SegmentOptions {
            sack_blocks: SackBlocks::EMPTY,
            timestamp: Some(TimestampOption::new(ts_val, ts_echo_reply)),
        }
    }

    const DEFAULT_SEGMENT_OPTIONS: SegmentOptions =
        default_segment_options(DEFAULT_TIMESTAMP, DEFAULT_TIMESTAMP);

    fn timestamp_now(clock: &FakeInstantCtx) -> Timestamp<Unitless> {
        (TIMESTAMP_OFFSET + clock.now().offset).discard_unit()
    }

    fn default_quickack_counter() -> usize {
        quickack_counter(
            BufferLimits { capacity: WindowSize::DEFAULT.into(), len: 0 },
            EffectiveMss::from_mss(
                DEVICE_MAXIMUM_SEGMENT_SIZE,
                MssSizeLimiters { timestamp_enabled: true },
            ),
        )
    }

    impl SocketOptions {
        fn default_for_state_tests() -> Self {
            // In testing, it is convenient to disable delayed ack and nagle by
            // default.
            Self { delayed_ack: false, nagle_enabled: false, ..Default::default() }
        }
    }

    /// A buffer provider that doesn't need extra information to construct
    /// buffers as this is only used in unit tests for the state machine only.
    enum ClientlessBufferProvider {}

    impl<R: ReceiveBuffer + Default, S: SendBuffer + Default> BufferProvider<R, S>
        for ClientlessBufferProvider
    {
        type PassiveOpen = ();
        type ActiveOpen = ();

        fn new_passive_open_buffers(_buffer_sizes: BufferSizes) -> (R, S, Self::PassiveOpen) {
            (R::default(), S::default(), ())
        }
    }

    impl RingBuffer {
        fn with_data<'a>(cap: usize, data: &'a [u8]) -> Self {
            let mut buffer = RingBuffer::new(cap);
            let nwritten = buffer.write_at(0, &data);
            assert_eq!(nwritten, data.len());
            buffer.make_readable(nwritten, false);
            buffer
        }
    }

    /// A buffer that can't read or write for test purpose.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct NullBuffer;

    impl Buffer for NullBuffer {
        fn limits(&self) -> BufferLimits {
            BufferLimits { len: 0, capacity: 0 }
        }

        fn target_capacity(&self) -> usize {
            0
        }

        fn request_capacity(&mut self, _size: usize) {}
    }

    impl ReceiveBuffer for NullBuffer {
        fn write_at<P: Payload>(&mut self, _offset: usize, _data: &P) -> usize {
            0
        }

        fn make_readable(&mut self, count: usize, has_outstanding: bool) {
            assert_eq!(count, 0);
            assert_eq!(has_outstanding, false);
        }
    }

    impl SendBuffer for NullBuffer {
        type Payload<'a> = &'a [u8];

        fn mark_read(&mut self, count: usize) {
            assert_eq!(count, 0);
        }

        fn peek_with<'a, F, R>(&'a mut self, offset: usize, f: F) -> R
        where
            F: FnOnce(Self::Payload<'a>) -> R,
        {
            assert_eq!(offset, 0);
            f(&[])
        }
    }

    #[derive(Debug, Default)]
    struct FakeStateMachineDebugId {
        resource_token: ResourceTokenValue,
    }

    impl StateMachineDebugId for FakeStateMachineDebugId {
        fn trace_id(&self) -> TraceResourceId<'_> {
            TraceResourceId::new(self.resource_token.token())
        }
    }

    impl<R: ReceiveBuffer, S: SendBuffer> State<FakeInstant, R, S, ()> {
        fn poll_send_with_default_options(
            &mut self,
            now: FakeInstant,
            counters: &TcpCountersRefs<'_>,
        ) -> Option<Segment<S::Payload<'_>>> {
            self.poll_send(
                &FakeStateMachineDebugId::default(),
                counters,
                now,
                &SocketOptions::default_for_state_tests(),
            )
            .ok()
        }

        fn on_segment_with_default_options<P: Payload, BP: BufferProvider<R, S, ActiveOpen = ()>>(
            &mut self,
            incoming: Segment<P>,
            now: FakeInstant,
            counters: &TcpCountersRefs<'_>,
        ) -> (Option<Segment<()>>, Option<BP::PassiveOpen>)
        where
            BP::PassiveOpen: Debug,
            R: Default,
            S: Default,
        {
            self.on_segment_with_options::<_, BP>(
                incoming,
                now,
                counters,
                &SocketOptions::default_for_state_tests(),
            )
        }

        fn on_segment_with_options<P: Payload, BP: BufferProvider<R, S, ActiveOpen = ()>>(
            &mut self,
            incoming: Segment<P>,
            now: FakeInstant,
            counters: &TcpCountersRefs<'_>,
            options: &SocketOptions,
        ) -> (Option<Segment<()>>, Option<BP::PassiveOpen>)
        where
            BP::PassiveOpen: Debug,
            R: Default,
            S: Default,
        {
            let (segment, passive_open, _data_acked, _newly_closed) = self.on_segment::<P, BP>(
                &FakeStateMachineDebugId::default(),
                counters,
                incoming,
                now,
                options,
                false, /* defunct */
            );
            (segment, passive_open)
        }

        fn recv_mut(&mut self) -> Option<&mut Recv<FakeInstant, R>> {
            match self {
                State::Closed(_)
                | State::Listen(_)
                | State::SynRcvd(_)
                | State::SynSent(_)
                | State::CloseWait(_)
                | State::LastAck(_)
                | State::Closing(_)
                | State::TimeWait(_) => None,
                State::Established(Established { rcv, .. })
                | State::FinWait1(FinWait1 { rcv, .. }) => Some(rcv.get_mut()),
                State::FinWait2(FinWait2 { rcv, .. }) => Some(rcv),
            }
        }

        #[track_caller]
        fn assert_established(&mut self) -> &mut Established<FakeInstant, R, S> {
            assert_matches!(self, State::Established(e) => e)
        }
    }

    impl<S: SendBuffer + Debug> State<FakeInstant, RingBuffer, S, ()> {
        fn read_with(&mut self, f: impl for<'b> FnOnce(&'b [&'_ [u8]]) -> usize) -> usize {
            match self {
                State::Closed(_)
                | State::Listen(_)
                | State::SynRcvd(_)
                | State::SynSent(_)
                | State::CloseWait(_)
                | State::LastAck(_)
                | State::Closing(_)
                | State::TimeWait(_) => {
                    panic!("No receive state in {:?}", self);
                }
                State::Established(Established { snd: _, rcv })
                | State::FinWait1(FinWait1 { snd: _, rcv }) => {
                    assert_matches!(&mut rcv.buffer, RecvBufferState::Open{ buffer, .. } => buffer.read_with(f))
                }
                State::FinWait2(FinWait2 { last_seq: _, rcv, timeout_at: _, snd_info: _ }) => {
                    assert_matches!(&mut rcv.buffer, RecvBufferState::Open{ buffer, .. } => buffer.read_with(f))
                }
            }
        }
    }

    impl State<FakeInstant, RingBuffer, NullBuffer, ()> {
        fn new_syn_rcvd(instant: FakeInstant) -> Self {
            State::SynRcvd(SynRcvd {
                iss: TEST_ISS,
                irs: TEST_IRS,
                rtt_sampler: MeasuredSampler::Tracking {
                    range: TEST_ISS..TEST_ISS + 1,
                    timestamp: instant,
                },
                retrans_timer: RetransTimer::new(instant, Rto::DEFAULT, None, DEFAULT_MAX_RETRIES),
                simultaneous_open: Some(()),
                buffer_sizes: Default::default(),
                smss: EffectiveMss::from_mss(
                    DEVICE_MAXIMUM_SEGMENT_SIZE,
                    MssSizeLimiters { timestamp_enabled: true },
                ),
                rcv_wnd_scale: WindowScale::default(),
                snd_wnd_scale: Some(WindowScale::default()),
                sack_permitted: SACK_PERMITTED,
                rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::default(),
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
            })
        }
    }

    impl<S, const FIN_QUEUED: bool> Send<FakeInstant, S, FIN_QUEUED> {
        fn default_for_test_at(seq: SeqNum, buffer: S) -> Self {
            Self {
                nxt: seq,
                max: seq,
                una: seq,
                wnd: WindowSize::DEFAULT,
                wnd_max: WindowSize::DEFAULT,
                buffer,
                wl1: TEST_IRS + 1,
                wl2: seq,
                last_push: seq,
                rtt_estimator: Estimator::default(),
                rtt_sampler: SamplingStrategy::default(),
                timer: None,
                congestion_control: CongestionControl::cubic_with_mss(EffectiveMss::from_mss(
                    DEVICE_MAXIMUM_SEGMENT_SIZE,
                    MssSizeLimiters { timestamp_enabled: true },
                )),
                wnd_scale: WindowScale::default(),
                last_data_sent: None,
            }
        }

        fn default_for_test(buffer: S) -> Self {
            Self::default_for_test_at(TEST_ISS + 1, buffer)
        }
    }

    impl<R: ReceiveBuffer> Recv<FakeInstant, R> {
        fn default_for_test_at(seq: SeqNum, buffer: R) -> Self {
            let BufferLimits { capacity, len } = buffer.limits();
            let avail_buffer = capacity - len;
            Self {
                buffer: RecvBufferState::Open { buffer, assembler: Assembler::new(seq) },
                timer: None,
                mss: EffectiveMss::from_mss(
                    DEVICE_MAXIMUM_SEGMENT_SIZE,
                    MssSizeLimiters { timestamp_enabled: true },
                ),
                remaining_quickacks: 0,
                last_segment_at: None,
                wnd_scale: WindowScale::default(),
                last_window_update: (
                    seq,
                    WindowSize::from_u32(avail_buffer.try_into().unwrap()).unwrap(),
                ),
                sack_permitted: SACK_PERMITTED,
                ts_opt: default_ts_opt_state(seq),
            }
        }

        fn default_for_test(buffer: R) -> Self {
            Self::default_for_test_at(TEST_IRS + 1, buffer)
        }
    }

    #[derive(Default)]
    struct FakeTcpCounters {
        stack_wide: TcpCountersWithSocketInner,
        per_socket: TcpCountersWithSocketInner,
    }

    impl FakeTcpCounters {
        fn refs<'a>(&'a self) -> TcpCountersRefs<'a> {
            let Self { stack_wide, per_socket } = self;
            TcpCountersRefs { stack_wide, per_socket }
        }
    }

    impl CounterExpectations {
        #[track_caller]
        fn assert_counters(&self, FakeTcpCounters { stack_wide, per_socket }: &FakeTcpCounters) {
            assert_eq!(self, &stack_wide.cast(), "stack-wide counter mismatch");
            assert_eq!(self, &per_socket.cast(), "per-socket counter mismatch");
        }
    }

    #[test_case(Segment::rst(TEST_IRS, ResetOptions::default()) => None; "drop RST")]
    #[test_case(Segment::rst_ack(TEST_IRS, TEST_ISS, ResetOptions::default()) => None; "drop RST|ACK")]
    #[test_case(Segment::syn(TEST_IRS, UnscaledWindowSize::from(0), HandshakeOptions::default()) => Some(Segment::rst_ack(SeqNum::new(0), TEST_IRS + 1, ResetOptions::default())); "reset SYN")]
    #[test_case(Segment::syn_ack(TEST_IRS, TEST_ISS, UnscaledWindowSize::from(0), HandshakeOptions::default()) => Some(Segment::rst(TEST_ISS, ResetOptions::default())); "reset SYN|ACK")]
    #[test_case(
        Segment::with_data(
            TEST_IRS,
            TEST_ISS,
            UnscaledWindowSize::from(0),
            SegmentOptions::default(),
            &[0, 1, 2][..]
        ) => Some(Segment::rst(TEST_ISS, ResetOptions::default())); "reset data segment")]
    fn segment_arrives_when_closed(
        incoming: impl Into<Segment<&'static [u8]>>,
    ) -> Option<Segment<()>> {
        let closed = Closed { reason: () };
        closed.on_segment(&incoming.into())
    }

    #[test_case(
        Segment::rst_ack(TEST_ISS, TEST_IRS - 1, ResetOptions::default()), RTT
    => SynSentOnSegmentDisposition::Ignore; "unacceptable ACK with RST")]
    #[test_case(
        Segment::ack(
            TEST_ISS, TEST_IRS - 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        RTT
    => SynSentOnSegmentDisposition::SendRst(
        Segment::rst(TEST_IRS-1, ResetOptions::default()),
    ); "unacceptable ACK without RST")]
    #[test_case(
        Segment::rst_ack(TEST_ISS, TEST_IRS, ResetOptions::default()), RTT
    => SynSentOnSegmentDisposition::EnterClosed(
        Closed { reason: Some(ConnectionError::ConnectionRefused) },
    ); "acceptable ACK(ISS) with RST")]
    #[test_case(
        Segment::rst_ack(TEST_ISS, TEST_IRS + 1, ResetOptions::default()), RTT
    => SynSentOnSegmentDisposition::EnterClosed(
        Closed { reason: Some(ConnectionError::ConnectionRefused) },
    ); "acceptable ACK(ISS+1) with RST")]
    #[test_case(
        Segment::rst(TEST_ISS, ResetOptions::default()), RTT
    => SynSentOnSegmentDisposition::Ignore; "RST without ack")]
    #[test_case(
        Segment::syn(
            TEST_ISS,
            UnscaledWindowSize::from(u16::MAX),
            HandshakeOptions {
                window_scale: Some(WindowScale::default()),
                timestamp: Some(DEFAULT_NON_ACK_TS_OPT),
                ..Default::default() }
        ), RTT
    => SynSentOnSegmentDisposition::SendSynAckAndEnterSynRcvd(
        Segment::syn_ack(
            TEST_IRS,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            HandshakeOptions {
                mss: Some(Mss::DEFAULT_IPV4),
                window_scale: Some(WindowScale::default()),
                sack_permitted: SACK_PERMITTED,
                timestamp: Some(ACK_TS_OPT_AFTER_RTT),
                ..Default::default()
            }),
        SynRcvd {
            iss: TEST_IRS,
            irs: TEST_ISS,
            rtt_sampler: MeasuredSampler::Tracking {
                range: TEST_IRS..TEST_IRS + 1,
                timestamp: FakeInstant::default(),
            },
            retrans_timer: RetransTimer::new(
                FakeInstant::from(RTT),
                Rto::DEFAULT,
                NonZeroDuration::new(TEST_USER_TIMEOUT.get() - RTT),
                DEFAULT_MAX_SYNACK_RETRIES
            ),
            simultaneous_open: None,
            buffer_sizes: BufferSizes::default(),
            smss: EffectiveMss::from_mss(
                Mss::DEFAULT_IPV4, MssSizeLimiters { timestamp_enabled: true }
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted: false,
            rcv: RecvParams {
                ack: TEST_ISS + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(TEST_ISS + 1),
                last_ack_recv: None,
            }
        }
    ); "SYN only")]
    #[test_case(
        Segment::fin(
            TEST_ISS, TEST_IRS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        RTT
    => SynSentOnSegmentDisposition::Ignore; "acceptable ACK with FIN")]
    #[test_case(
        Segment::ack(
            TEST_ISS, TEST_IRS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        RTT
    => SynSentOnSegmentDisposition::Ignore; "acceptable ACK(ISS+1) with nothing")]
    #[test_case(
        Segment::ack(
            TEST_ISS, TEST_IRS, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        RTT
    => SynSentOnSegmentDisposition::Ignore; "acceptable ACK(ISS) without RST")]
    #[test_case(
        Segment::syn(TEST_ISS, UnscaledWindowSize::from(u16::MAX), HandshakeOptions::default()),
        TEST_USER_TIMEOUT.get()
    => SynSentOnSegmentDisposition::EnterClosed(Closed {
        reason: None
    }); "syn but timed out")]
    fn segment_arrives_when_syn_sent(
        incoming: Segment<()>,
        delay: Duration,
    ) -> SynSentOnSegmentDisposition<FakeInstant, ()> {
        let syn_sent = SynSent {
            iss: TEST_IRS,
            rtt_sampler: MeasuredSampler::Tracking {
                range: TEST_IRS..TEST_IRS + 1,
                timestamp: FakeInstant::default(),
            },
            retrans_timer: RetransTimer::new(
                FakeInstant::default(),
                Rto::DEFAULT,
                Some(TEST_USER_TIMEOUT),
                DEFAULT_MAX_RETRIES,
            ),
            active_open: (),
            buffer_sizes: BufferSizes::default(),
            default_mss: Mss::DEFAULT_IPV4,
            device_mss: DEVICE_MAXIMUM_SEGMENT_SIZE,
            rcv_wnd_scale: WindowScale::default(),
            ts_opt: default_ts_opt_negotiation_state(),
        };
        syn_sent.on_segment(incoming, FakeInstant::from(delay))
    }

    #[test_case(Segment::rst(TEST_ISS, ResetOptions::default()) => ListenOnSegmentDisposition::Ignore; "ignore RST")]
    #[test_case(
        Segment::ack(
            TEST_ISS, TEST_IRS, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ) => ListenOnSegmentDisposition::SendRst(Segment::rst(TEST_IRS, ResetOptions::default()));
        "reject ACK")]
    #[test_case(Segment::syn(TEST_ISS, UnscaledWindowSize::from(u16::MAX),
        HandshakeOptions {
            window_scale: Some(WindowScale::default()),
            timestamp: Some(DEFAULT_NON_ACK_TS_OPT),
            ..Default::default()
        }) =>
        ListenOnSegmentDisposition::SendSynAckAndEnterSynRcvd(
            Segment::syn_ack(
                TEST_IRS,
                TEST_ISS + 1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(Mss::DEFAULT_IPV4),
                    window_scale: Some(WindowScale::default()),
                    timestamp: Some(DEFAULT_ACK_TS_OPT),
                    sack_permitted: SACK_PERMITTED,
                    ..Default::default()
                }),
            SynRcvd {
                iss: TEST_IRS,
                irs: TEST_ISS,
                rtt_sampler: MeasuredSampler::Tracking {
                    range: TEST_IRS..TEST_IRS + 1,
                    timestamp: FakeInstant::default(),
                },
                retrans_timer: RetransTimer::new(
                    FakeInstant::default(),
                    Rto::DEFAULT,
                    None,
                    DEFAULT_MAX_SYNACK_RETRIES,
                ),
                simultaneous_open: None,
                buffer_sizes: BufferSizes::default(),
                smss: EffectiveMss::from_mss(
                    Mss::DEFAULT_IPV4, MssSizeLimiters{timestamp_enabled: true}
                ),
                sack_permitted: false,
                rcv_wnd_scale: WindowScale::default(),
                snd_wnd_scale: Some(WindowScale::default()),
                rcv: RecvParams {
                    ack: TEST_ISS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::default(),
                    ts_opt: default_ts_opt_state(TEST_ISS + 1),
                    last_ack_recv: None,
                }
            }); "accept syn")]
    fn segment_arrives_when_listen(
        incoming: Segment<()>,
    ) -> ListenOnSegmentDisposition<FakeInstant> {
        let listen = Closed::<Initial>::listen(
            TEST_IRS,
            TIMESTAMP_OFFSET,
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            None,
        );
        listen.on_segment(incoming, FakeInstant::default())
    }

    #[test_case(
        Segment::ack(
            TEST_IRS, TEST_ISS, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => Some(Segment::ack(
        TEST_ISS + 1,
        TEST_IRS + 1,
        UnscaledWindowSize::from(u16::MAX),
        default_segment_options(RTT_TIMESTAMP, DEFAULT_TIMESTAMP),
    )) ; "OTW segment")]
    #[test_case(
        Segment::rst_ack(TEST_IRS, TEST_ISS, ResetOptions::default()),
        None
    => None; "OTW RST")]
    #[test_case(
        Segment::rst_ack(TEST_IRS + 1, TEST_ISS, ResetOptions::default()),
        Some(State::Closed(Closed { reason: Some(ConnectionError::ConnectionReset) }))
    => None; "acceptable RST")]
    #[test_case(
        Segment::syn(TEST_IRS + 1, UnscaledWindowSize::from(u16::MAX), HandshakeOptions {
            window_scale: Some(WindowScale::default()),
            timestamp: Some(DEFAULT_NON_ACK_TS_OPT),
            ..Default::default()
        }),
        Some(State::Closed(Closed { reason: Some(ConnectionError::ConnectionReset) }))
    => Some(
        Segment::rst(TEST_ISS + 1, ResetOptions { timestamp: Some(NON_ACK_TS_OPT_AFTER_RTT) }),
    ); "duplicate syn")]
    #[test_case(
        Segment::ack(
            TEST_IRS + 1, TEST_ISS, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => Some(
        Segment::rst(TEST_ISS, ResetOptions { timestamp: Some(NON_ACK_TS_OPT_AFTER_RTT) }),
    ); "unacceptable ack (ISS)")]
    #[test_case(
        Segment::ack(
            TEST_IRS + 1, TEST_ISS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        Some(State::Established(
            Established {
                snd: Send {
                    rtt_estimator: Estimator::Measured {
                        srtt: RTT,
                        rtt_var: RTT / 2,
                    },
                    rtt_sampler: SamplingStrategy::Timestamp(TimestampSampler{
                        ts_state: TimestampValueState {
                            offset: TIMESTAMP_OFFSET,
                            initialized_at: FakeInstant::default(),
                        },
                    }),
                    ..Send::default_for_test(NullBuffer)
                }.into(),
                rcv: Recv {
                    remaining_quickacks: quickack_counter(BufferLimits {
                        capacity: WindowSize::DEFAULT.into(),
                        len: 0,
                    }, EffectiveMss::from_mss(
                        DEVICE_MAXIMUM_SEGMENT_SIZE, MssSizeLimiters { timestamp_enabled: true }
                    )),
                    ..Recv::default_for_test(RingBuffer::default())
                }.into(),
            }
        ))
    => None; "acceptable ack (ISS + 1)")]
    #[test_case(
        Segment::ack(
            TEST_IRS + 1, TEST_ISS + 2, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => Some(
        Segment::rst(TEST_ISS + 2, ResetOptions { timestamp: Some(NON_ACK_TS_OPT_AFTER_RTT) })
    ); "unacceptable ack (ISS + 2)")]
    #[test_case(
        Segment::ack(
            TEST_IRS + 1, TEST_ISS - 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => Some(
        Segment::rst(TEST_ISS - 1, ResetOptions { timestamp: Some(NON_ACK_TS_OPT_AFTER_RTT) })
    ); "unacceptable ack (ISS - 1)")]
    #[test_case(
        Segment::new_empty(
            SegmentHeader {
                seq: TEST_IRS + 1,
                wnd: UnscaledWindowSize::from(u16::MAX),
                options: DEFAULT_SEGMENT_OPTIONS.into(),
                ..Default::default()
            }
        ),
        None
    => None; "no ack")]
    #[test_case(
        Segment::fin(
            TEST_IRS + 1, TEST_ISS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        Some(State::CloseWait(CloseWait {
            snd: Send {
                rtt_estimator: Estimator::Measured {
                    srtt: RTT,
                    rtt_var: RTT / 2,
                },
                rtt_sampler: SamplingStrategy::Timestamp(TimestampSampler{
                    ts_state: TimestampValueState {
                        offset: TIMESTAMP_OFFSET,
                        initialized_at: FakeInstant::default(),
                    },
                }),
                ..Send::default_for_test(NullBuffer)
            }.into(),
            closed_rcv: RecvParams {
                ack: TEST_IRS + 2,
                wnd: WindowSize::from_u32(u32::from(u16::MAX - 1)).unwrap(),
                wnd_scale: WindowScale::ZERO,
                ts_opt: default_ts_opt_state(TEST_IRS + 2),
                last_ack_recv: Some(FakeInstant::from(RTT)),
            },
        }))
    => Some(Segment::ack(
        TEST_ISS + 1,
        TEST_IRS + 2,
        UnscaledWindowSize::from(u16::MAX - 1),
        default_segment_options(RTT_TIMESTAMP, DEFAULT_TIMESTAMP),
    )); "fin")]
    fn segment_arrives_when_syn_rcvd(
        incoming: Segment<()>,
        expected: Option<State<FakeInstant, RingBuffer, NullBuffer, ()>>,
    ) -> Option<Segment<()>> {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut state = State::new_syn_rcvd(clock.now());
        clock.sleep(RTT);
        let (seg, _passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                incoming,
                clock.now(),
                &counters.refs(),
            );
        match expected {
            Some(new_state) => assert_eq!(state, new_state),
            None => assert_matches!(state, State::SynRcvd(_)),
        };
        seg
    }

    #[test]
    fn abort_when_syn_rcvd() {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut state = State::new_syn_rcvd(clock.now());
        let segment = assert_matches!(
            state.abort(&counters.refs(), clock.now(), ConnectionError::ConnectionReset),
            (Some(seg), NewlyClosed::Yes) => seg
        );
        assert_eq!(segment.header().control, Some(Control::RST));
        assert_eq!(segment.header().seq, TEST_ISS + 1);
        assert_eq!(segment.header().ack, Some(TEST_IRS + 1));
    }

    #[test_case(
        Segment::syn(TEST_IRS + 1, UnscaledWindowSize::from(u16::MAX), HandshakeOptions {
            timestamp: Some(DEFAULT_NON_ACK_TS_OPT), ..Default::default()
        }),
        Some(State::Closed (
            Closed { reason: Some(ConnectionError::ConnectionReset) },
        ))
    => Some(Segment::rst(TEST_ISS + 1, ResetOptions {timestamp: Some(DEFAULT_NON_ACK_TS_OPT)}));
    "duplicate syn")]
    #[test_case(
        Segment::rst(TEST_IRS + 1, ResetOptions::default()),
        Some(State::Closed (
            Closed { reason: Some(ConnectionError::ConnectionReset) },
        ))
    => None; "accepatable rst")]
    #[test_case(
        Segment::ack(
            TEST_ISS + 1, TEST_IRS + 2, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => Some(Segment::ack(
            TEST_ISS + 1, TEST_IRS + 1, UnscaledWindowSize::from(2), DEFAULT_SEGMENT_OPTIONS
    )); "unacceptable ack")]
    #[test_case(
        Segment::ack(
            TEST_IRS + 1, TEST_ISS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => None; "pure ack")]
    #[test_case(
        Segment::fin(
            TEST_IRS + 1, TEST_ISS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        Some(State::CloseWait(CloseWait {
            snd: Send::default_for_test(NullBuffer).into(),
            closed_rcv: RecvParams {
                ack: TEST_IRS + 2,
                wnd: WindowSize::new(1).unwrap(),
                wnd_scale: WindowScale::ZERO,
                ts_opt: default_ts_opt_state(TEST_IRS + 2),
                last_ack_recv: Some(FakeInstant::default()),
            },
        }))
    => Some(
        Segment::ack(
            TEST_ISS + 1, TEST_IRS + 2, UnscaledWindowSize::from(1), DEFAULT_SEGMENT_OPTIONS
        )
    ); "pure fin")]
    #[test_case(
        Segment::piggybacked_fin(
            TEST_IRS + 1,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            "A".as_bytes()
        ),
        Some(State::CloseWait(CloseWait {
            snd: Send::default_for_test(NullBuffer).into(),
            closed_rcv: RecvParams {
                ack: TEST_IRS + 3,
                wnd: WindowSize::ZERO,
                wnd_scale: WindowScale::ZERO,
                ts_opt: default_ts_opt_state(TEST_IRS + 3),
                last_ack_recv: Some(FakeInstant::default()),
            },
        }))
    => Some(
        Segment::ack(
            TEST_ISS + 1, TEST_IRS + 3, UnscaledWindowSize::from(0), DEFAULT_SEGMENT_OPTIONS
    )); "fin with 1 byte")]
    #[test_case(
        Segment::piggybacked_fin(
            TEST_IRS + 1,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            "AB".as_bytes()
        ),
        None
    => Some(Segment::ack(
            TEST_ISS + 1, TEST_IRS + 3, UnscaledWindowSize::from(0), DEFAULT_SEGMENT_OPTIONS
    )); "fin with 2 bytes")]
    fn segment_arrives_when_established(
        incoming: Segment<&[u8]>,
        expected: Option<State<FakeInstant, RingBuffer, NullBuffer, ()>>,
    ) -> Option<Segment<()>> {
        let counters = FakeTcpCounters::default();
        let mut state = State::Established(Established {
            snd: Send::default_for_test(NullBuffer).into(),
            rcv: Recv::default_for_test(RingBuffer::new(2)).into(),
        });
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                incoming,
                FakeInstant::default(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        match expected {
            Some(new_state) => assert_eq!(new_state, state),
            None => assert_matches!(state, State::Established(_)),
        };
        seg
    }

    #[test]
    fn common_rcv_data_segment_arrives() {
        let counters = FakeTcpCounters::default();
        // Tests the common behavior when data segment arrives in states that
        // have a receive state.
        let new_snd = || Send::default_for_test(NullBuffer);
        let new_rcv = || Recv::default_for_test(RingBuffer::new(TEST_BYTES.len()));
        for mut state in [
            State::Established(Established { snd: new_snd().into(), rcv: new_rcv().into() }),
            State::FinWait1(FinWait1 { snd: new_snd().queue_fin().into(), rcv: new_rcv().into() }),
            State::FinWait2(FinWait2 {
                last_seq: TEST_ISS + 1,
                rcv: new_rcv(),
                timeout_at: None,
                snd_info: SendInfo::default(),
            }),
        ] {
            assert_eq!(
                state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                    Segment::with_data(
                        TEST_IRS + 1,
                        TEST_ISS + 1,
                        UnscaledWindowSize::from(u16::MAX),
                        DEFAULT_SEGMENT_OPTIONS,
                        TEST_BYTES
                    ),
                    FakeInstant::default(),
                    &counters.refs(),
                ),
                (
                    Some(Segment::ack(
                        TEST_ISS + 1,
                        TEST_IRS + 1 + TEST_BYTES.len(),
                        UnscaledWindowSize::from(0),
                        DEFAULT_SEGMENT_OPTIONS
                    )),
                    None
                )
            );
            assert_eq!(
                state.read_with(|bytes| {
                    assert_eq!(bytes.concat(), TEST_BYTES);
                    TEST_BYTES.len()
                }),
                TEST_BYTES.len()
            );
        }
    }

    #[test]
    fn common_snd_ack_segment_arrives() {
        let counters = FakeTcpCounters::default();
        // Tests the common behavior when ack segment arrives in states that
        // have a send state.
        let new_snd = || Send {
            congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
            ..Send::default_for_test(RingBuffer::with_data(TEST_BYTES.len(), TEST_BYTES))
        };
        let new_rcv = || Recv::default_for_test(NullBuffer);
        for mut state in [
            State::Established(Established { snd: new_snd().into(), rcv: new_rcv().into() }),
            State::FinWait1(FinWait1 { snd: new_snd().queue_fin().into(), rcv: new_rcv().into() }),
            State::Closing(Closing {
                snd: new_snd().queue_fin(),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::ZERO,
                    wnd_scale: WindowScale::default(),
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            }),
            State::CloseWait(CloseWait {
                snd: new_snd().into(),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::ZERO,
                    wnd_scale: WindowScale::default(),
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            }),
            State::LastAck(LastAck {
                snd: new_snd().queue_fin(),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::ZERO,
                    wnd_scale: WindowScale::default(),
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            }),
        ] {
            assert_eq!(
                state.poll_send_with_default_options(FakeInstant::default(), &counters.refs(),),
                Some(Segment::new_assert_no_discard(
                    SegmentHeader {
                        seq: TEST_ISS + 1,
                        ack: Some(TEST_IRS + 1),
                        control: None,
                        wnd: UnscaledWindowSize::from(0),
                        push: true,
                        options: DEFAULT_SEGMENT_OPTIONS.into(),
                    },
                    FragmentedPayload::new_contiguous(TEST_BYTES)
                ))
            );
            assert_eq!(
                state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                    Segment::<()>::ack(
                        TEST_IRS + 1,
                        TEST_ISS + 1 + TEST_BYTES.len(),
                        UnscaledWindowSize::from(u16::MAX),
                        DEFAULT_SEGMENT_OPTIONS
                    ),
                    FakeInstant::default(),
                    &counters.refs(),
                ),
                (None, None),
            );
            assert_eq!(state.poll_send_at(), None);
            let snd = match state {
                State::Closed(_)
                | State::Listen(_)
                | State::SynRcvd(_)
                | State::SynSent(_)
                | State::FinWait2(_)
                | State::TimeWait(_) => unreachable!("Unexpected state {:?}", state),
                State::Established(e) => e.snd.into_inner().queue_fin(),
                State::CloseWait(c) => c.snd.into_inner().queue_fin(),
                State::LastAck(l) => l.snd,
                State::FinWait1(f) => f.snd.into_inner(),
                State::Closing(c) => c.snd,
            };
            assert_eq!(snd.nxt, TEST_ISS + 1 + TEST_BYTES.len());
            assert_eq!(snd.max, TEST_ISS + 1 + TEST_BYTES.len());
            assert_eq!(snd.una, TEST_ISS + 1 + TEST_BYTES.len());
            assert_eq!(snd.buffer.limits().len, 0);
        }
    }

    #[test_case(
        Segment::syn(TEST_IRS + 2, UnscaledWindowSize::from(u16::MAX),
            HandshakeOptions {timestamp: Some(DEFAULT_NON_ACK_TS_OPT), ..Default::default()}),
        Some(State::Closed (
            Closed { reason: Some(ConnectionError::ConnectionReset) },
        ))
    => Some(Segment::rst(TEST_ISS + 1, ResetOptions {timestamp: Some(DEFAULT_NON_ACK_TS_OPT)})); "syn")]
    #[test_case(
        Segment::rst(TEST_IRS + 2, ResetOptions::default()),
        Some(State::Closed (
            Closed { reason: Some(ConnectionError::ConnectionReset) },
        ))
    => None; "rst")]
    #[test_case(
        Segment::fin(
            TEST_IRS + 2, TEST_ISS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ),
        None
    => None; "ignore fin")]
    #[test_case(
        Segment::with_data(
            TEST_IRS,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            "a".as_bytes()
        ),
        None => Some(Segment::ack(
            TEST_ISS + 1, TEST_IRS + 2, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        ));
        "ack old data")]
    #[test_case(
        Segment::with_data(
            TEST_IRS + 2,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            "Hello".as_bytes()
        ),
        Some(State::Closed (
            Closed { reason: Some(ConnectionError::ConnectionReset) },
        ))
    => Some(Segment::rst(TEST_ISS + 1, ResetOptions {timestamp: Some(DEFAULT_NON_ACK_TS_OPT)}));
    "reset on new data")]
    fn segment_arrives_when_close_wait(
        incoming: Segment<&[u8]>,
        expected: Option<State<FakeInstant, RingBuffer, NullBuffer, ()>>,
    ) -> Option<Segment<()>> {
        let counters = FakeTcpCounters::default();
        let mut state = State::CloseWait(CloseWait {
            snd: Send::default_for_test(NullBuffer).into(),
            closed_rcv: RecvParams {
                ack: TEST_IRS + 2,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::ZERO,
                ts_opt: default_ts_opt_state(TEST_IRS + 1),
                last_ack_recv: None,
            },
        });
        let (seg, _passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                incoming,
                FakeInstant::default(),
                &counters.refs(),
            );
        match expected {
            Some(new_state) => assert_eq!(new_state, state),
            None => assert_matches!(state, State::CloseWait(_)),
        };
        seg
    }

    #[test_case(true; "sack")]
    #[test_case(false; "no sack")]
    fn active_passive_open(sack_permitted: bool) {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let passive_iss = ISS_2;
        let active_iss = ISS_1;
        let (syn_sent, syn_seg) = Closed::<Initial>::connect(
            active_iss,
            TIMESTAMP_OFFSET,
            clock.now(),
            (),
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        assert_eq!(
            syn_seg,
            Segment::syn(
                active_iss,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    // Matches the stack-wide constant.
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(DEFAULT_NON_ACK_TS_OPT),
                },
            )
        );
        assert_eq!(
            syn_sent,
            SynSent {
                iss: active_iss,
                rtt_sampler: MeasuredSampler::Tracking {
                    range: active_iss..active_iss + 1,
                    timestamp: clock.now(),
                },
                retrans_timer: RetransTimer::new(
                    clock.now(),
                    Rto::DEFAULT,
                    None,
                    DEFAULT_MAX_SYN_RETRIES,
                ),
                active_open: (),
                buffer_sizes: BufferSizes::default(),
                default_mss: Mss::DEFAULT_IPV4,
                device_mss: DEVICE_MAXIMUM_SEGMENT_SIZE,
                rcv_wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_negotiation_state(),
            }
        );
        let mut active = State::SynSent(syn_sent);
        let mut passive = State::Listen(Closed::<Initial>::listen(
            passive_iss,
            TIMESTAMP_OFFSET,
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            None,
        ));
        clock.sleep(RTT / 2);

        let initialized_at = clock.now();

        // Update the SYN segment to match what the test wants.
        let syn_seg = {
            let (mut header, data) = syn_seg.into_parts();
            let opt = assert_matches!(&mut header.options, Options::Handshake(o) => o);
            opt.sack_permitted = sack_permitted;
            Segment::new_assert_no_discard(header, data)
        };

        let (seg, passive_open) = passive
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn_seg,
                clock.now(),
                &counters.refs(),
            );
        let syn_ack = seg.expect("failed to generate a syn-ack segment");
        assert_eq!(passive_open, None);
        assert_eq!(
            syn_ack,
            Segment::syn_ack(
                passive_iss,
                active_iss + 1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    // Matches the stack-wide constant.
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(DEFAULT_ACK_TS_OPT),
                },
            )
        );
        assert_matches!(passive, State::SynRcvd(ref syn_rcvd) if syn_rcvd == &SynRcvd {
            iss: passive_iss,
            irs: active_iss,
            rtt_sampler: MeasuredSampler::Tracking {
                range: passive_iss..passive_iss + 1,
                timestamp: clock.now(),
            },
            retrans_timer: RetransTimer::new(
                clock.now(),
                Rto::DEFAULT,
                None,
                DEFAULT_MAX_SYNACK_RETRIES,
            ),
            simultaneous_open: None,
            buffer_sizes: Default::default(),
            smss: EffectiveMss::from_mss(
                DEVICE_MAXIMUM_SEGMENT_SIZE,
                MssSizeLimiters {timestamp_enabled: true}
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted,
            rcv: RecvParams {
                ack: active_iss + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: TimestampOptionState::Enabled {
                    ts_recent: DEFAULT_TIMESTAMP,
                    last_ack_sent: active_iss + 1,
                    ts_val: TimestampValueState {
                        offset: TIMESTAMP_OFFSET,
                        initialized_at,
                    },
                },
                last_ack_recv: None,
            },
        });
        clock.sleep(RTT / 2);

        // Update the SYN ACK segment to match what the test wants.
        let syn_ack = {
            let (mut header, data) = syn_ack.into_parts();
            let opt = assert_matches!(&mut header.options, Options::Handshake(o) => o);
            opt.sack_permitted = sack_permitted;
            Segment::new_assert_no_discard(header, data)
        };

        let (seg, passive_open) = active
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn_ack,
                clock.now(),
                &counters.refs(),
            );
        let ack_seg = seg.expect("failed to generate a ack segment");
        assert_eq!(passive_open, None);
        let ts_val = timestamp_now(&clock);
        assert_eq!(
            ack_seg,
            Segment::ack(
                active_iss + 1,
                passive_iss + 1,
                UnscaledWindowSize::from(u16::MAX),
                default_segment_options(ts_val, DEFAULT_TIMESTAMP),
            )
        );
        let established = assert_matches!(&active, State::Established(e) => e);
        assert_eq!(
            established,
            &Established {
                snd: Send {
                    wl1: passive_iss,
                    rtt_estimator: Estimator::Measured { srtt: RTT, rtt_var: RTT / 2 },
                    rtt_sampler: SamplingStrategy::Timestamp(TimestampSampler {
                        ts_state: TimestampValueState {
                            offset: TIMESTAMP_OFFSET,
                            initialized_at: FakeInstant::default(),
                        }
                    }),
                    ..Send::default_for_test_at(active_iss + 1, RingBuffer::default())
                }
                .into(),
                rcv: Recv {
                    remaining_quickacks: default_quickack_counter(),
                    sack_permitted,
                    ..Recv::default_for_test_at(passive_iss + 1, RingBuffer::default())
                }
                .into()
            }
        );
        clock.sleep(RTT / 2);
        assert_eq!(
            passive.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                ack_seg,
                clock.now(),
                &counters.refs(),
            ),
            (None, Some(())),
        );
        let established = assert_matches!(&passive, State::Established(e) => e);
        assert_eq!(
            established,
            &Established {
                snd: Send {
                    wl1: active_iss + 1,
                    rtt_estimator: Estimator::Measured { srtt: RTT, rtt_var: RTT / 2 },
                    rtt_sampler: SamplingStrategy::Timestamp(TimestampSampler {
                        ts_state: TimestampValueState { offset: TIMESTAMP_OFFSET, initialized_at }
                    }),
                    ..Send::default_for_test_at(passive_iss + 1, RingBuffer::default())
                }
                .into(),
                rcv: Recv {
                    remaining_quickacks: default_quickack_counter(),
                    sack_permitted,
                    ts_opt: TimestampOptionState::Enabled {
                        ts_recent: ts_val,
                        last_ack_sent: active_iss + 1,
                        ts_val: TimestampValueState { offset: TIMESTAMP_OFFSET, initialized_at },
                    },
                    ..Recv::default_for_test_at(active_iss + 1, RingBuffer::default())
                }
                .into()
            }
        )
    }

    #[test]
    fn simultaneous_open() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let time1 = clock.now();
        let timestamp1 = timestamp_now(&clock);
        let (syn_sent1, syn1) = Closed::<Initial>::connect(
            ISS_1,
            TIMESTAMP_OFFSET,
            time1,
            (),
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        let (syn_sent2, syn2) = Closed::<Initial>::connect(
            ISS_2,
            TIMESTAMP_OFFSET,
            time1,
            (),
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );

        assert_eq!(
            syn1,
            Segment::syn(
                ISS_1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(TimestampOption::new(timestamp1, TS_ECHO_REPLY_FOR_NON_ACKS)),
                },
            )
        );
        assert_eq!(
            syn2,
            Segment::syn(
                ISS_2,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(TimestampOption::new(timestamp1, TS_ECHO_REPLY_FOR_NON_ACKS)),
                },
            )
        );

        let mut state1 = State::SynSent(syn_sent1);
        let mut state2 = State::SynSent(syn_sent2);

        // NB: Because we've only sent a one-way message thus far, progress the
        // clock by a half of an RTT.
        clock.sleep(RTT / 2);
        let timestamp2 = timestamp_now(&clock);
        let (seg, passive_open) = state1
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn2,
                clock.now(),
                &counters.refs(),
            );
        let syn_ack1 = seg.expect("failed to generate syn ack");
        assert_eq!(passive_open, None);
        let (seg, passive_open) = state2
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn1,
                clock.now(),
                &counters.refs(),
            );
        let syn_ack2 = seg.expect("failed to generate syn ack");
        assert_eq!(passive_open, None);

        assert_eq!(
            syn_ack1,
            Segment::syn_ack(
                ISS_1,
                ISS_2 + 1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(TimestampOption::new(timestamp2, timestamp1)),
                },
            )
        );
        assert_eq!(
            syn_ack2,
            Segment::syn_ack(
                ISS_2,
                ISS_1 + 1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(TimestampOption::new(timestamp2, timestamp1)),
                },
            )
        );

        assert_matches!(state1, State::SynRcvd(ref syn_rcvd) if syn_rcvd == &SynRcvd {
            iss: ISS_1,
            irs: ISS_2,
            rtt_sampler: MeasuredSampler::Tracking {
                range: ISS_1..ISS_1 + 1,
                timestamp: time1
            },
            retrans_timer: RetransTimer::new(
                clock.now(),
                Rto::DEFAULT,
                None,
                DEFAULT_MAX_SYNACK_RETRIES,
            ),
            simultaneous_open: Some(()),
            buffer_sizes: BufferSizes::default(),
            smss: EffectiveMss::from_mss(
                DEVICE_MAXIMUM_SEGMENT_SIZE,
                MssSizeLimiters{timestamp_enabled: true}
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted: SACK_PERMITTED,
            rcv: RecvParams {
                ack: ISS_2 + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(ISS_2 + 1),
                last_ack_recv: None,
            },
        });
        assert_matches!(state2, State::SynRcvd(ref syn_rcvd) if syn_rcvd == &SynRcvd {
            iss: ISS_2,
            irs: ISS_1,
            rtt_sampler: MeasuredSampler::Tracking {
                range: ISS_2..ISS_2 + 1,
                timestamp: time1
            },
            retrans_timer: RetransTimer::new(
                clock.now(),
                Rto::DEFAULT,
                None,
                DEFAULT_MAX_SYNACK_RETRIES,
            ),
            simultaneous_open: Some(()),
            buffer_sizes: BufferSizes::default(),
            smss: EffectiveMss::from_mss(
                DEVICE_MAXIMUM_SEGMENT_SIZE,
                MssSizeLimiters{timestamp_enabled: true}
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted: SACK_PERMITTED,
            rcv: RecvParams {
                ack: ISS_1 + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(ISS_1 + 1),
                last_ack_recv: None,
            },
        });

        // NB: Now that we've done SYN + SYN_ACK, a full round trip has occurred.
        // Progress the clock by another half of an RTT.
        clock.sleep(RTT / 2);
        let timestamp3 = timestamp_now(&clock);
        assert_eq!(
            state1.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn_ack2,
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    ISS_1 + 1,
                    ISS_2 + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(timestamp3, timestamp2),
                )),
                None
            )
        );
        assert_eq!(
            state2.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn_ack1,
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    ISS_2 + 1,
                    ISS_1 + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(timestamp3, timestamp2),
                )),
                None
            )
        );

        let established = assert_matches!(state1, State::Established(e) => e);
        assert_eq!(
            established,
            Established {
                snd: Send {
                    wl1: ISS_2 + 1,
                    rtt_estimator: Estimator::Measured { srtt: RTT, rtt_var: RTT / 2 },
                    congestion_control: CongestionControl::cubic_with_mss(EffectiveMss::from_mss(
                        Mss::DEFAULT_IPV4,
                        MssSizeLimiters { timestamp_enabled: true },
                    )),
                    rtt_sampler: SamplingStrategy::Timestamp(TimestampSampler {
                        ts_state: TimestampValueState {
                            offset: TIMESTAMP_OFFSET,
                            initialized_at: FakeInstant::default(),
                        }
                    }),
                    ..Send::default_for_test_at(ISS_1 + 1, RingBuffer::default())
                }
                .into(),
                rcv: Recv {
                    remaining_quickacks: default_quickack_counter() - 1,
                    last_segment_at: Some(clock.now()),
                    ts_opt: TimestampOptionState::Enabled {
                        ts_recent: timestamp2,
                        last_ack_sent: ISS_2 + 1,
                        ts_val: TimestampValueState {
                            offset: TIMESTAMP_OFFSET,
                            initialized_at: FakeInstant::default(),
                        }
                    },
                    ..Recv::default_for_test_at(ISS_2 + 1, RingBuffer::default())
                }
                .into()
            }
        );

        let established = assert_matches!(state2, State::Established(e) => e);
        assert_eq!(
            established,
            Established {
                snd: Send {
                    wl1: ISS_1 + 1,
                    rtt_estimator: Estimator::Measured { srtt: RTT, rtt_var: RTT / 2 },
                    congestion_control: CongestionControl::cubic_with_mss(EffectiveMss::from_mss(
                        Mss::DEFAULT_IPV4,
                        MssSizeLimiters { timestamp_enabled: true },
                    )),
                    rtt_sampler: SamplingStrategy::Timestamp(TimestampSampler {
                        ts_state: TimestampValueState {
                            offset: TIMESTAMP_OFFSET,
                            initialized_at: FakeInstant::default(),
                        }
                    }),
                    ..Send::default_for_test_at(ISS_2 + 1, RingBuffer::default())
                }
                .into(),
                rcv: Recv {
                    remaining_quickacks: default_quickack_counter() - 1,
                    last_segment_at: Some(clock.now()),
                    ts_opt: TimestampOptionState::Enabled {
                        ts_recent: timestamp2,
                        last_ack_sent: ISS_1 + 1,
                        ts_val: TimestampValueState {
                            offset: TIMESTAMP_OFFSET,
                            initialized_at: FakeInstant::default(),
                        }
                    },
                    ..Recv::default_for_test_at(ISS_1 + 1, RingBuffer::default())
                }
                .into()
            }
        );
    }

    #[test_case(true; "sack permitted")]
    #[test_case(false; "sack not permitted")]
    fn established_receive(sack_permitted: bool) {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut established = State::Established(Established {
            snd: Send {
                wnd: WindowSize::ZERO,
                wnd_max: WindowSize::ZERO,
                buffer: NullBuffer,
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test(NullBuffer)
            }
            .into(),
            rcv: Recv {
                mss: TEST_MSS,
                sack_permitted,
                ..Recv::default_for_test(RingBuffer::new(BUFFER_SIZE))
            }
            .into(),
        });

        // Received an expected segment at rcv.nxt.
        assert_eq!(
            established.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1 + TEST_BYTES.len(),
                    UnscaledWindowSize::from((BUFFER_SIZE - TEST_BYTES.len()) as u16),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            ),
        );
        assert_eq!(
            established.read_with(|available| {
                assert_eq!(available, &[TEST_BYTES]);
                available[0].len()
            }),
            TEST_BYTES.len()
        );

        // Receive an out-of-order segment.
        let segment_start = TEST_IRS + 1 + TEST_BYTES.len() * 2;
        assert_eq!(
            established.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    segment_start,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs()
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1 + TEST_BYTES.len(),
                    UnscaledWindowSize::from(u16::try_from(BUFFER_SIZE).unwrap()),
                    SegmentOptions {
                        sack_blocks: if sack_permitted {
                            [SackBlock::try_new(
                                segment_start,
                                segment_start + u32::try_from(TEST_BYTES.len()).unwrap(),
                            )
                            .unwrap()]
                            .into_iter()
                            .collect()
                        } else {
                            SackBlocks::default()
                        },
                        timestamp: Some(DEFAULT_ACK_TS_OPT),
                    },
                )),
                None
            ),
        );
        assert_eq!(
            established.read_with(|available| {
                let empty: &[u8] = &[];
                assert_eq!(available, &[empty]);
                0
            }),
            0
        );

        // Receive the next segment that fills the hole.
        assert_eq!(
            established.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    TEST_IRS + 1 + TEST_BYTES.len(),
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs()
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1 + 3 * TEST_BYTES.len(),
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - 2 * TEST_BYTES.len()),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            ),
        );
        assert_eq!(
            established.read_with(|available| {
                assert_eq!(available, &[[TEST_BYTES, TEST_BYTES].concat()]);
                available[0].len()
            }),
            TEST_BYTES.len() * 2
        );
    }

    #[test]
    fn established_send() {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        // NB: Queue enough data for a full segment and a partial segment.
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        const PARTIAL_LEN: usize = 10;
        assert_eq!(send_buffer.enqueue_data(&TEST_BYTES[..PARTIAL_LEN]), PARTIAL_LEN);
        let mut established = State::Established(Established {
            snd: Send {
                una: TEST_ISS,
                wl2: TEST_ISS,
                wnd: WindowSize::ZERO,
                wnd_max: WindowSize::ZERO,
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test_at(TEST_ISS + 1, send_buffer)
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        // Data queued but the window is not opened, nothing to send.
        assert_eq!(established.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        let open_window = |established: &mut State<FakeInstant, RingBuffer, RingBuffer, ()>,
                           ack: SeqNum,
                           win: usize,
                           now: FakeInstant,
                           counters: &TcpCountersRefs<'_>| {
            assert_eq!(
                established.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                    Segment::ack(
                        TEST_IRS + 1,
                        ack,
                        UnscaledWindowSize::from_usize(win),
                        DEFAULT_SEGMENT_OPTIONS
                    ),
                    now,
                    counters
                ),
                (None, None),
            );
        };
        // Open up the window by 1 byte.
        open_window(&mut established, TEST_ISS + 1, 1, clock.now(), &counters.refs());
        assert_eq!(
            established.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from_usize(BUFFER_SIZE),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(&TEST_BYTES[1..2]),
            ))
        );

        // Open up the window, and expect a full segment.
        open_window(
            &mut established,
            TEST_ISS + 2,
            TEST_BYTES.len() + PARTIAL_LEN,
            clock.now(),
            &counters.refs(),
        );
        assert_eq!(
            established.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 2,
                TEST_IRS + 1,
                UnscaledWindowSize::from_usize(BUFFER_SIZE),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES),
            ))
        );

        assert_eq!(
            established.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &SocketOptions { nagle_enabled: false, ..SocketOptions::default_for_state_tests() }
            ),
            Ok(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: TEST_ISS + TEST_BYTES.len() + 2,
                    ack: Some(TEST_IRS + 1),
                    control: None,
                    wnd: UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    push: true,
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[..PARTIAL_LEN - 2]),
            ))
        );

        // We've exhausted our send buffer.
        assert_eq!(established.poll_send_with_default_options(clock.now(), &counters.refs()), None);
    }

    // A helper to get the state machines estimation of the current RTO.
    fn get_rto_estimate<I, R, S, A>(state: &State<I, R, S, A>) -> Duration {
        let rto = match state {
            State::Closed(_)
            | State::Listen(_)
            | State::SynRcvd(_)
            | State::SynSent(_)
            | State::FinWait2(_)
            | State::TimeWait(_) => Rto::DEFAULT,
            State::Established(Established { snd, .. })
            | State::CloseWait(CloseWait { snd, .. }) => snd.rtt_estimator.rto(),
            State::FinWait1(FinWait1 { snd, .. }) => snd.rtt_estimator.rto(),
            State::LastAck(LastAck { snd, .. }) | State::Closing(Closing { snd, .. }) => {
                snd.rtt_estimator.rto()
            }
        };
        rto.get()
    }

    #[test]
    fn self_connect_retransmission() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let (syn_sent, syn) = Closed::<Initial>::connect(
            ISS_1,
            TIMESTAMP_OFFSET,
            clock.now(),
            (),
            Default::default(),
            *TEST_MSS.mss(),
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );

        let expected_syn = |ts_val| {
            Segment::syn(
                ISS_1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(*TEST_MSS.mss()),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(TimestampOption::new(ts_val, TS_ECHO_REPLY_FOR_NON_ACKS)),
                },
            )
        };

        let time1 = timestamp_now(&clock);
        assert_eq!(syn, expected_syn(time1));
        let mut state = State::<_, RingBuffer, RingBuffer, ()>::SynSent(syn_sent);

        // Retransmission timer should be installed.
        assert_eq!(state.poll_send_at(), Some(FakeInstant::from(Rto::DEFAULT.get())));
        clock.sleep(get_rto_estimate(&state));
        let time2 = timestamp_now(&clock);
        // The SYN segment should be retransmitted.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(expected_syn(time2).into_empty())
        );

        // Bring the state to SYNRCVD.
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn,
                clock.now(),
                &counters.refs(),
            );
        let syn_ack = seg.expect("expected SYN-ACK");

        let expected_syn_ack = |ts_val| {
            Segment::syn_ack(
                ISS_1,
                ISS_1 + 1,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(*TEST_MSS.mss()),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(TimestampOption::new(ts_val, time1)),
                },
            )
        };

        assert_eq!(syn_ack, expected_syn_ack(time2));
        assert_eq!(passive_open, None);
        // Retransmission timer should be installed.
        assert_eq!(state.poll_send_at(), Some(clock.now() + Rto::DEFAULT.get()));
        clock.sleep(get_rto_estimate(&state));
        let time3 = timestamp_now(&clock);
        // The SYN-ACK segment should be retransmitted.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(expected_syn_ack(time3).into_empty())
        );

        // Bring the state to ESTABLISHED and write some data.
        assert_eq!(
            state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn_ack,
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    ISS_1 + 1,
                    ISS_1 + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(time3, time2),
                )),
                None
            )
        );
        match state {
            State::Closed(_)
            | State::Listen(_)
            | State::SynRcvd(_)
            | State::SynSent(_)
            | State::LastAck(_)
            | State::FinWait1(_)
            | State::FinWait2(_)
            | State::Closing(_)
            | State::TimeWait(_) => {
                panic!("expected that we have entered established state, but got {:?}", state)
            }
            State::Established(Established { ref mut snd, rcv: _ })
            | State::CloseWait(CloseWait { ref mut snd, closed_rcv: _ }) => {
                assert_eq!(snd.buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
            }
        }
        // We have no outstanding segments, so there is no retransmission timer.
        assert_eq!(state.poll_send_at(), None);
        // The retransmission timer should backoff exponentially.
        for i in 0..3 {
            assert_eq!(
                state.poll_send_with_default_options(clock.now(), &counters.refs()),
                Some(Segment::new_assert_no_discard(
                    SegmentHeader {
                        seq: ISS_1 + 1,
                        ack: Some(ISS_1 + 1),
                        control: None,
                        wnd: UnscaledWindowSize::from(u16::MAX),
                        push: true,
                        options: default_segment_options(timestamp_now(&clock), time2).into(),
                    },
                    FragmentedPayload::new_contiguous(TEST_BYTES),
                ))
            );
            let rto = get_rto_estimate(&state);
            assert_eq!(state.poll_send_at(), Some(clock.now() + (1 << i) * rto));
            clock.sleep((1 << i) * rto);
            CounterExpectations {
                retransmits: i,
                slow_start_retransmits: i,
                timeouts: i,
                ..Default::default()
            }
            .assert_counters(&counters);
        }
        let time4 = timestamp_now(&clock);
        let rto_before_ack = get_rto_estimate(&state);
        // The receiver acks the first byte of the payload.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    ISS_1 + 1 + TEST_BYTES.len(),
                    ISS_1 + 1 + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(time4, time4),
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None),
        );
        // The timer is rearmed with the current RTO estimate.
        assert_eq!(state.poll_send_at(), Some(clock.now() + rto_before_ack));
        clock.sleep(rto_before_ack);
        let time5 = timestamp_now(&clock);
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs(),),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: ISS_1 + 1 + 1,
                    ack: Some(ISS_1 + 1),
                    wnd: UnscaledWindowSize::from(u16::MAX),
                    push: true,
                    options: default_segment_options(time5, time2).into(),
                    ..Default::default()
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[1..]),
            ))
        );

        // Reduce the MTU. The next transmission should be reduced.
        assert_matches!(
            state.on_pmtu_update(Mss::MIN, ISS_1 + TEST_BYTES.len()),
            ShouldRetransmit::Yes
        );
        let new_mss = usize::from(EffectiveMss::from_mss(
            Mss::MIN,
            MssSizeLimiters { timestamp_enabled: true },
        ));
        clock.sleep(2 * get_rto_estimate(&state));
        let time6 = timestamp_now(&clock);
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs(),),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: ISS_1 + 1 + 1,
                    ack: Some(ISS_1 + 1),
                    wnd: UnscaledWindowSize::from(u16::MAX),
                    push: false,
                    options: default_segment_options(time6, time2).into(),
                    ..Default::default()
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[1..new_mss + 1]),
            ))
        );

        let rto_before_ack = get_rto_estimate(&state);
        // The MTU update should have reduced `snd.nxt`. Currently,
        //   snd.nxt = ISS_1 + Mss::MIN + 1,
        //   snd.max = ISS_1 + TEST_BYTES.len() + 1.
        // A segment with ack number ISS_1 + Mss::MIN + 2 should bump snd.nxt
        // immediately.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    ISS_1 + 1 + TEST_BYTES.len(),
                    ISS_1 + new_mss + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(time6, time6),
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );

        CounterExpectations {
            retransmits: 4,
            slow_start_retransmits: 4,
            timeouts: 4,
            ..Default::default()
        }
        .assert_counters(&counters);

        assert_eq!(state.poll_send_at(), Some(clock.now() + rto_before_ack));
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: ISS_1 + new_mss + 2,
                    ack: Some(ISS_1 + 1),
                    wnd: UnscaledWindowSize::from(u16::MAX),
                    push: true,
                    options: default_segment_options(time6, time2).into(),
                    ..Default::default()
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[new_mss + 1..]),
            ))
        );

        // Finally the receiver ACKs all the outstanding data.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    ISS_1 + 1 + TEST_BYTES.len(),
                    ISS_1 + 1 + TEST_BYTES.len(),
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(time6, time6),
                ),
                clock.now(),
                &counters.refs()
            ),
            (None, None)
        );
        // The retransmission timer should be removed.
        assert_eq!(state.poll_send_at(), None);
    }

    #[test]
    fn passive_close() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        // NB: Queue enough data for a full segment and a partial segment.
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        const PARTIAL_LEN: usize = 10;
        assert_eq!(send_buffer.enqueue_data(&TEST_BYTES[..PARTIAL_LEN]), PARTIAL_LEN);
        // Set up the state machine to start with Established.
        let mut state = State::Established(Established {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test(send_buffer.clone())
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        let last_wnd = WindowSize::new(BUFFER_SIZE - 1).unwrap();
        let last_wnd_scale = WindowScale::default();
        // Transition the state machine to CloseWait by sending a FIN.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::fin(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 2,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - 1),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            )
        );
        // Then call CLOSE to transition the state machine to LastAck.
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Shutdown,
                &SocketOptions::default_for_state_tests()
            ),
            Ok(NewlyClosed::No)
        );
        assert_eq!(
            state,
            State::LastAck(LastAck {
                snd: Send::default_for_test(send_buffer),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 2,
                    wnd: last_wnd,
                    wnd_scale: last_wnd_scale,
                    ts_opt: default_ts_opt_state(TEST_IRS + 2),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            })
        );
        // When the send window is not big enough, there should be no FIN.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 2,
                last_wnd >> WindowScale::default(),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES),
            ))
        );
        // We should be able to send out all remaining bytes together with a FIN.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: TEST_ISS + TEST_BYTES.len() + 1,
                    ack: Some(TEST_IRS + 2),
                    control: Some(Control::FIN),
                    wnd: last_wnd >> WindowScale::default(),
                    push: true,
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[..PARTIAL_LEN]),
            ))
        );
        // Now let's test we retransmit correctly by only acking the data.
        clock.sleep(RTT);
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + 2,
                    TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );
        assert_eq!(state.poll_send_at(), Some(clock.now() + Rto::DEFAULT.get()));
        clock.sleep(Rto::DEFAULT.get());
        // The FIN should be retransmitted.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::fin(
                TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 1,
                TEST_IRS + 2,
                last_wnd >> WindowScale::default(),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
            ))
        );

        // Finally, our FIN is acked.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + 2,
                    TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );
        // The connection is closed.
        assert_eq!(state, State::Closed(Closed { reason: None }));
        CounterExpectations {
            retransmits: 1,
            slow_start_retransmits: 1,
            timeouts: 1,
            established_closed: 1,
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    #[test]
    fn syn_rcvd_active_close() {
        let counters = FakeTcpCounters::default();
        let mut state: State<_, RingBuffer, NullBuffer, ()> = State::SynRcvd(SynRcvd {
            iss: TEST_ISS,
            irs: TEST_IRS,
            rtt_sampler: MeasuredSampler::default(),
            retrans_timer: RetransTimer {
                at: FakeInstant::default(),
                rto: Rto::MIN,
                user_timeout_until: None,
                remaining_retries: Some(DEFAULT_MAX_RETRIES),
            },
            simultaneous_open: Some(()),
            buffer_sizes: Default::default(),
            smss: EffectiveMss::from_mss(
                Mss::DEFAULT_IPV4,
                MssSizeLimiters { timestamp_enabled: true },
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted: SACK_PERMITTED,
            rcv: RecvParams {
                ack: TEST_IRS + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(TEST_IRS + 1),
                last_ack_recv: None,
            },
        });
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Shutdown,
                &SocketOptions::default_for_state_tests()
            ),
            Ok(NewlyClosed::No)
        );
        assert_matches!(state, State::FinWait1(_));
        assert_eq!(
            state.poll_send_with_default_options(FakeInstant::default(), &counters.refs()),
            Some(Segment::fin(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from(u16::MAX),
                DEFAULT_SEGMENT_OPTIONS
            ))
        );
    }

    #[test]
    fn established_active_close() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        // NB: Queue enough data for a full segment and a partial segment.
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        const PARTIAL_LEN: usize = 10;
        assert_eq!(send_buffer.enqueue_data(&TEST_BYTES[..PARTIAL_LEN]), PARTIAL_LEN);
        // Set up the state machine to start with Established.
        let mut state = State::Established(Established {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test(send_buffer.clone())
            }
            .into(),
            rcv: Recv { mss: TEST_MSS, ..Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)) }
                .into(),
        });
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Shutdown,
                &SocketOptions::default_for_state_tests()
            ),
            Ok(NewlyClosed::No)
        );
        assert_matches!(state, State::FinWait1(_));
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Shutdown,
                &SocketOptions::default_for_state_tests()
            ),
            Err(CloseError::Closing)
        );

        // Poll bytes to send the first segment.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from_usize(BUFFER_SIZE),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES)
            ))
        );

        // Then send the second segment together with the FIN.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: TEST_ISS + 1 + TEST_BYTES.len(),
                    ack: Some(TEST_IRS + 1),
                    control: Some(Control::FIN),
                    wnd: UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    push: true,
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[..PARTIAL_LEN])
            ))
        );

        // Test that the recv state works in FIN_WAIT_1.
        assert_eq!(
            state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    TEST_IRS + 1,
                    TEST_ISS + TEST_BYTES.len() + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 2,
                    TEST_IRS + TEST_BYTES.len() + 1,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - TEST_BYTES.len()),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            )
        );

        assert_eq!(
            state.read_with(|avail| {
                let got = avail.concat();
                assert_eq!(got, TEST_BYTES);
                got.len()
            }),
            TEST_BYTES.len()
        );

        // The retrans timer should be installed correctly.
        assert_eq!(state.poll_send_at(), Some(clock.now() + Rto::DEFAULT.get()));

        // Because only the first segment was acked, we need to retransmit the
        // second.
        clock.sleep(Rto::DEFAULT.get());
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: TEST_ISS + TEST_BYTES.len() + 1,
                    ack: Some(TEST_IRS + TEST_BYTES.len() + 1),
                    control: Some(Control::FIN),
                    wnd: UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    push: true,
                    options: default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP)
                        .into(),
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[..PARTIAL_LEN]),
            ))
        );

        // Now our FIN is acked, we should transition to FinWait2.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + TEST_BYTES.len() + 1,
                    TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );
        assert_matches!(state, State::FinWait2(_));

        // Test that the recv state works in FIN_WAIT_2.
        assert_eq!(
            state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    TEST_IRS + TEST_BYTES.len() + 1,
                    TEST_ISS + TEST_BYTES.len() + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 2,
                    TEST_IRS + 2 * TEST_BYTES.len() + 1,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - TEST_BYTES.len()),
                    default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                )),
                None
            )
        );

        assert_eq!(
            state.read_with(|avail| {
                let got = avail.concat();
                assert_eq!(got, TEST_BYTES);
                got.len()
            }),
            TEST_BYTES.len()
        );

        // Should ack the FIN and transition to TIME_WAIT.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::fin(
                    TEST_IRS + 2 * TEST_BYTES.len() + 1,
                    TEST_ISS + TEST_BYTES.len() + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + TEST_BYTES.len() + PARTIAL_LEN + 2,
                    TEST_IRS + 2 * TEST_BYTES.len() + 2,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - 1),
                    default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                )),
                None
            )
        );

        assert_matches!(state, State::TimeWait(_));

        const SMALLEST_DURATION: Duration = Duration::from_secs(1);
        assert_eq!(state.poll_send_at(), Some(clock.now() + MSL * 2));
        clock.sleep(MSL * 2 - SMALLEST_DURATION);
        // The state should still be in time wait before the time out.
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_matches!(state, State::TimeWait(_));
        clock.sleep(SMALLEST_DURATION);
        // The state should become closed.
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_eq!(state, State::Closed(Closed { reason: None }));
        CounterExpectations {
            retransmits: 1,
            slow_start_retransmits: 1,
            timeouts: 1,
            established_closed: 1,
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    #[test]
    fn fin_wait_1_fin_ack_to_time_wait() {
        let counters = FakeTcpCounters::default();
        // Test that we can transition from FIN-WAIT-2 to TIME-WAIT directly
        // with one FIN-ACK segment.
        let mut state = State::FinWait1(FinWait1 {
            snd: Send { una: TEST_ISS + 1, ..Send::default_for_test_at(TEST_ISS + 2, NullBuffer) }
                .into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::fin(
                    TEST_IRS + 1,
                    TEST_ISS + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                FakeInstant::default(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 2,
                    TEST_IRS + 2,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - 1),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            ),
        );
        assert_matches!(state, State::TimeWait(_));
    }

    #[test]
    fn simultaneous_close() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());

        let iss = ISS_1;
        // Set up the state machine to start with Established.
        let mut state = State::Established(Established {
            snd: Send::default_for_test_at(iss + 1, send_buffer.clone()).into(),
            rcv: Recv::default_for_test_at(iss + 1, RingBuffer::new(BUFFER_SIZE)).into(),
        });
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Shutdown,
                &SocketOptions::default_for_state_tests()
            ),
            Ok(NewlyClosed::No)
        );
        assert_matches!(state, State::FinWait1(_));
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Shutdown,
                &SocketOptions::default_for_state_tests()
            ),
            Err(CloseError::Closing)
        );

        let fin = state.poll_send_with_default_options(clock.now(), &counters.refs());
        assert_eq!(
            fin,
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: iss + 1,
                    ack: Some(iss + 1),
                    control: Some(Control::FIN),
                    wnd: UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    push: true,
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                },
                FragmentedPayload::new_contiguous(TEST_BYTES),
            ))
        );
        assert_eq!(
            state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::piggybacked_fin(
                    iss + 1,
                    iss + 1,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    iss + TEST_BYTES.len() + 2,
                    iss + TEST_BYTES.len() + 2,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - TEST_BYTES.len() - 1),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            )
        );

        // We have a self connection, feeding the FIN packet we generated should
        // make us transition to CLOSING.
        assert_matches!(state, State::Closing(_));
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    iss + TEST_BYTES.len() + 2,
                    iss + TEST_BYTES.len() + 2,
                    UnscaledWindowSize::from_usize(BUFFER_SIZE - TEST_BYTES.len() - 1),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );

        // And feeding the ACK we produced for FIN should make us transition to
        // TIME-WAIT.
        assert_matches!(state, State::TimeWait(_));

        const SMALLEST_DURATION: Duration = Duration::from_secs(1);
        assert_eq!(state.poll_send_at(), Some(clock.now() + MSL * 2));
        clock.sleep(MSL * 2 - SMALLEST_DURATION);
        // The state should still be in time wait before the time out.
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_matches!(state, State::TimeWait(_));
        clock.sleep(SMALLEST_DURATION);
        // The state should become closed.
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_eq!(state, State::Closed(Closed { reason: None }));
        CounterExpectations { established_closed: 1, ..Default::default() }
            .assert_counters(&counters);
    }

    #[test]
    fn time_wait_restarts_timer() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut time_wait = State::<_, NullBuffer, NullBuffer, ()>::TimeWait(TimeWait {
            last_seq: TEST_ISS + 2,
            closed_rcv: RecvParams {
                ack: TEST_IRS + 2,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(TEST_IRS + 1),
                last_ack_recv: None,
            },
            expiry: new_time_wait_expiry(clock.now()),
            snd_info: SendInfo::default(),
        });

        assert_eq!(time_wait.poll_send_at(), Some(clock.now() + MSL * 2));
        clock.sleep(Duration::from_secs(1));
        assert_eq!(
            time_wait.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::fin(
                    TEST_IRS + 2,
                    TEST_ISS + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 2,
                    TEST_IRS + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                )),
                None
            ),
        );
        assert_eq!(time_wait.poll_send_at(), Some(clock.now() + MSL * 2));
    }

    #[test_case(
        State::Established(Established {
            snd: Send::default_for_test_at(TEST_ISS, NullBuffer).into(),
            rcv: Recv::default_for_test_at(TEST_IRS + u32::from(TEST_MSS), RingBuffer::default()).into(),
        }),
        Segment::with_data(
            TEST_IRS,
            TEST_ISS,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            TEST_BYTES
        ) => Some(Segment::ack(
            TEST_ISS,
            TEST_IRS + u32::from(TEST_MSS),
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS
        ));
        "retransmit data"
    )]
    #[test_case(
        State::SynRcvd(SynRcvd {
            iss: TEST_ISS,
            irs: TEST_IRS,
            rtt_sampler: MeasuredSampler::default(),
            retrans_timer: RetransTimer {
                at: FakeInstant::default(),
                rto: Rto::MIN,
                user_timeout_until: None,
                remaining_retries: Some(DEFAULT_MAX_RETRIES),
            },
            simultaneous_open: None,
            buffer_sizes: BufferSizes::default(),
            smss: EffectiveMss::from_mss(
                Mss::DEFAULT_IPV4, MssSizeLimiters {timestamp_enabled: true }
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted: SACK_PERMITTED,
            rcv: RecvParams {
                ack: TEST_IRS + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(TEST_IRS+1),
                last_ack_recv: None
            },
        }),
        Segment::syn_ack(
            TEST_IRS,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            HandshakeOptions {
                window_scale: Some(WindowScale::default()),
                timestamp: Some(DEFAULT_ACK_TS_OPT),
                ..Default::default()
            }) =>
        Some(Segment::ack(
            TEST_ISS + 1, TEST_IRS + 1, UnscaledWindowSize::from(u16::MAX), DEFAULT_SEGMENT_OPTIONS
        )); "retransmit syn_ack"
    )]
    // Regression test for https://fxbug.dev/42058963
    fn ack_to_retransmitted_segment(
        mut state: State<FakeInstant, RingBuffer, NullBuffer, ()>,
        seg: Segment<&[u8]>,
    ) -> Option<Segment<()>> {
        let counters = FakeTcpCounters::default();
        let (reply, _): (_, Option<()>) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                seg,
                FakeInstant::default(),
                &counters.refs(),
            );
        reply
    }

    #[test]
    fn fast_retransmit() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::default();
        let mss =
            EffectiveMss::from_mss(Mss::DEFAULT_IPV4, MssSizeLimiters { timestamp_enabled: true });

        let first_payload_byte = b'A';
        let last_payload_byte = b'D';

        for b in first_payload_byte..=last_payload_byte {
            assert_eq!(send_buffer.enqueue_data(&vec![b; usize::from(mss)]), usize::from(mss));
        }
        let mut state: State<_, _, _, ()> = State::Established(Established {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(EffectiveMss::from_mss(
                    Mss::DEFAULT_IPV4,
                    MssSizeLimiters { timestamp_enabled: true },
                )),
                ..Send::default_for_test_at(TEST_ISS, send_buffer.clone())
            }
            .into(),
            rcv: Recv::default_for_test_at(TEST_IRS, RingBuffer::default()).into(),
        });

        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs(),),
            Some(Segment::with_data(
                TEST_ISS,
                TEST_IRS,
                UnscaledWindowSize::from(u16::MAX),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(&vec![b'A'; usize::from(mss)])
            ))
        );

        let mut dup_ack = |expected_byte: u8, counters: &TcpCountersRefs<'_>| {
            clock.sleep(Duration::from_millis(10));
            assert_eq!(
                state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                    Segment::ack(
                        TEST_IRS,
                        TEST_ISS,
                        UnscaledWindowSize::from(u16::MAX),
                        DEFAULT_SEGMENT_OPTIONS
                    ),
                    clock.now(),
                    counters,
                ),
                (None, None)
            );

            assert_eq!(
                state.poll_send_with_default_options(clock.now(), counters,),
                Some(Segment::new_assert_no_discard(
                    SegmentHeader {
                        seq: TEST_ISS
                            + u32::from(expected_byte - first_payload_byte) * u32::from(mss),
                        ack: Some(TEST_IRS),
                        control: None,
                        wnd: UnscaledWindowSize::from(u16::MAX),
                        push: expected_byte == last_payload_byte,
                        options: default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP)
                            .into(),
                    },
                    FragmentedPayload::new_contiguous(&vec![expected_byte; usize::from(mss)])
                ))
            );
        };

        // The first two dup acks should allow two previously unsent segments
        // into the network.
        CounterExpectations::default().assert_counters(&counters);
        dup_ack(b'B', &counters.refs());
        CounterExpectations { fast_recovery: 0, dup_acks: 1, ..Default::default() }
            .assert_counters(&counters);
        dup_ack(b'C', &counters.refs());
        CounterExpectations { fast_recovery: 0, dup_acks: 2, ..Default::default() }
            .assert_counters(&counters);
        // The third dup ack will cause a fast retransmit of the first segment
        // at snd.una.
        dup_ack(b'A', &counters.refs());
        CounterExpectations {
            retransmits: 1,
            fast_recovery: 1,
            fast_retransmits: 1,
            dup_acks: 3,
            ..Default::default()
        }
        .assert_counters(&counters);
        // Afterwards, we continue to send previously unsent data if allowed.
        dup_ack(b'D', &counters.refs());
        CounterExpectations {
            retransmits: 1,
            fast_recovery: 1,
            fast_retransmits: 1,
            dup_acks: 4,
            ..Default::default()
        }
        .assert_counters(&counters);

        // Make sure the window size is deflated after loss is recovered.
        clock.sleep(Duration::from_millis(10));
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS,
                    TEST_ISS + u32::from(Mss::DEFAULT_IPV4),
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );
        let established = assert_matches!(state, State::Established(established) => established);
        assert_eq!(established.snd.congestion_control.inspect_cwnd().cwnd(), 2 * u32::from(mss));
        assert_eq!(established.snd.congestion_control.inspect_loss_recovery_mode(), None);
        CounterExpectations {
            retransmits: 1,
            fast_recovery: 1,
            fast_retransmits: 1,
            dup_acks: 4,
            loss_recovered: 1,
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    #[test_case(true; "user timeout")]
    #[test_case(false; "no user timeout")]
    fn keep_alive(with_user_timeout: bool) {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut state: State<_, _, _, ()> = State::Established(Established {
            snd: Send::default_for_test_at(TEST_ISS, RingBuffer::default()).into(),
            rcv: Recv::default_for_test_at(TEST_IRS, RingBuffer::default()).into(),
        });

        let keep_alive = KeepAlive { enabled: true, ..Default::default() };
        let num_probes_user_timeout = 3;
        // Use a number slightly deviated from the interval period.
        let user_timeout_duration = keep_alive
            .interval
            .saturating_mul(NonZeroU32::new(num_probes_user_timeout - 1).unwrap())
            + NonZeroDuration::new(keep_alive.interval.get() / 2).unwrap();
        let socket_options = SocketOptions {
            keep_alive,
            user_timeout: with_user_timeout.then_some(user_timeout_duration),
            ..SocketOptions::default_for_state_tests()
        };
        let socket_options = &socket_options;
        let keep_alive = &socket_options.keep_alive;

        // Currently we have nothing to send,
        assert_eq!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                socket_options,
            ),
            Err(NewlyClosed::No),
        );
        // so the above poll_send call will install a timer, which will fire
        // after `keep_alive.idle`.
        assert_eq!(state.poll_send_at(), Some(clock.now().panicking_add(keep_alive.idle.into())));

        // Now we receive an ACK after half the idle period.
        clock.sleep(keep_alive.idle.get() / 2);
        assert_eq!(
            state.on_segment::<&[u8], ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::ack(
                    TEST_IRS,
                    TEST_ISS,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                socket_options,
                false, /* defunct */
            ),
            (None, None, DataAcked::No, NewlyClosed::No)
        );
        // the timer is reset to fire after the new keep alive interval.
        assert_eq!(state.poll_send_at(), Some(clock.now().panicking_add(keep_alive.idle.into())),);
        clock.sleep(keep_alive.idle.into());

        // Then there should be `count` probes being sent out after `count`
        // `interval` seconds.
        let (expect_probes, max_deadline) = if with_user_timeout {
            // Prove we're actually testing a user timeout.
            assert!(num_probes_user_timeout < keep_alive.count.get().into());
            (num_probes_user_timeout, clock.time + user_timeout_duration.get())
        } else {
            (u32::from(keep_alive.count.get()), FakeInstant::LATEST)
        };
        for _ in 0..expect_probes {
            assert_eq!(
                state.poll_send(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    clock.now(),
                    socket_options,
                ),
                Ok(Segment::ack(
                    TEST_ISS - 1,
                    TEST_IRS,
                    UnscaledWindowSize::from(u16::MAX),
                    default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                ))
            );
            assert_matches!(state, State::Established(_));
            let next = state.poll_send_at().expect("timer must be set");
            assert_eq!(next, (clock.time + keep_alive.interval.get()).min(max_deadline));
            clock.sleep_until(next);
        }

        // At this time the connection is closed and we don't have anything to
        // send.
        assert_eq!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                socket_options,
            ),
            Err(NewlyClosed::Yes),
        );
        assert_eq!(state, State::Closed(Closed { reason: Some(ConnectionError::TimedOut) }));
        CounterExpectations {
            established_closed: 1,
            established_timedout: 1,
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    /// A `SendBuffer` that doesn't allow peeking some number of bytes.
    #[derive(Debug)]
    struct ReservingBuffer<B> {
        buffer: B,
        reserved_bytes: usize,
    }

    impl<B: Buffer> Buffer for ReservingBuffer<B> {
        fn limits(&self) -> BufferLimits {
            self.buffer.limits()
        }

        fn target_capacity(&self) -> usize {
            self.buffer.target_capacity()
        }

        fn request_capacity(&mut self, size: usize) {
            self.buffer.request_capacity(size)
        }
    }

    impl<B: SendBuffer> SendBuffer for ReservingBuffer<B> {
        type Payload<'a> = B::Payload<'a>;

        fn mark_read(&mut self, count: usize) {
            self.buffer.mark_read(count)
        }

        fn peek_with<'a, F, R>(&'a mut self, offset: usize, f: F) -> R
        where
            F: FnOnce(B::Payload<'a>) -> R,
        {
            let Self { buffer, reserved_bytes } = self;
            buffer.peek_with(offset, |payload| {
                let len = payload.len();
                let new_len = len.saturating_sub(*reserved_bytes);
                f(payload.slice(0..new_len.try_into().unwrap_or(u32::MAX)))
            })
        }
    }

    #[test_case(true, 0)]
    #[test_case(false, 0)]
    #[test_case(true, 1)]
    #[test_case(false, 1)]
    fn poll_send_len(has_fin: bool, reserved_bytes: usize) {
        const VALUE: u8 = 0xaa;

        fn with_poll_send_result<const HAS_FIN: bool>(
            f: impl FnOnce(Segment<FragmentedPayload<'_, 2>>),
            reserved_bytes: usize,
        ) {
            const DATA_LEN: usize = 40;
            let buffer = ReservingBuffer {
                buffer: RingBuffer::with_data(DATA_LEN, &vec![VALUE; DATA_LEN]),
                reserved_bytes,
            };
            assert_eq!(buffer.limits().len, DATA_LEN);

            let mut snd = Send::<FakeInstant, _, HAS_FIN>::default_for_test_at(TEST_ISS, buffer);
            let counters = FakeTcpCounters::default();

            f(snd
                .poll_send(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    &mut RecvParams {
                        ack: TEST_ISS,
                        wnd: WindowSize::DEFAULT,
                        wnd_scale: WindowScale::ZERO,
                        ts_opt: default_ts_opt_state(TEST_IRS + 1),
                        last_ack_recv: None,
                    },
                    FakeInstant::default(),
                    &SocketOptions::default_for_state_tests(),
                )
                .expect("has data"))
        }

        let f = |segment: Segment<FragmentedPayload<'_, 2>>| {
            let segment_len = segment.len();
            let (SegmentHeader { .. }, data) = segment.into_parts();
            let data_len = data.len();

            if has_fin && reserved_bytes == 0 {
                assert_eq!(
                    segment_len,
                    u32::try_from(data_len + 1).unwrap(),
                    "FIN not accounted for"
                );
            } else {
                assert_eq!(segment_len, u32::try_from(data_len).unwrap());
            }

            let mut target = vec![0; data_len];
            data.partial_copy(0, target.as_mut_slice());
            assert_eq!(target, vec![VALUE; data_len]);
        };
        match has_fin {
            true => with_poll_send_result::<true>(f, reserved_bytes),
            false => with_poll_send_result::<false>(f, reserved_bytes),
        }
    }

    #[test]
    fn zero_window_probe() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        // Set up the state machine to start with Established.
        let mut state = State::Established(Established {
            snd: Send {
                wnd: WindowSize::ZERO,
                wnd_max: WindowSize::ZERO,
                ..Send::default_for_test(send_buffer.clone())
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_eq!(state.poll_send_at(), Some(clock.now().panicking_add(Rto::DEFAULT.get())));

        // Send the first probe after first RTO.
        clock.sleep(Rto::DEFAULT.get());
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from_usize(BUFFER_SIZE),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                FragmentedPayload::new_contiguous(&TEST_BYTES[0..1])
            ))
        );

        // The receiver still has a zero window.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );
        // The timer should backoff exponentially.
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_eq!(state.poll_send_at(), Some(clock.now().panicking_add(Rto::DEFAULT.get() * 2)));

        // No probe should be sent before the timeout.
        clock.sleep(Rto::DEFAULT.get());
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);

        // Probe sent after the timeout.
        clock.sleep(Rto::DEFAULT.get());
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from_usize(BUFFER_SIZE),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                FragmentedPayload::new_contiguous(&TEST_BYTES[0..1])
            ))
        );

        // The receiver now opens its receive window.
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + 1,
                    TEST_ISS + 2,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs(),
            ),
            (None, None)
        );
        assert_eq!(state.poll_send_at(), None);
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: TEST_ISS + 2,
                    ack: Some(TEST_IRS + 1),
                    control: None,
                    wnd: UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    push: true,
                    options: default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP)
                        .into(),
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[1..])
            ))
        );
    }

    #[test]
    fn nagle() {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        // NB: Queue enough data for a full segment and a partial segment.
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        const PARTIAL_LEN: usize = 10;
        assert_eq!(send_buffer.enqueue_data(&TEST_BYTES[..PARTIAL_LEN]), PARTIAL_LEN);
        // Set up the state machine to start with Established.
        let mut state: State<_, _, _, ()> = State::Established(Established {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test(send_buffer.clone())
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        let mut socket_options =
            SocketOptions { nagle_enabled: true, ..SocketOptions::default_for_state_tests() };

        // Send the first segment
        assert_eq!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options
            ),
            Ok(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from_usize(BUFFER_SIZE),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES)
            ))
        );
        // The second segment shouldn't get sent until we disable nagle.
        assert_eq!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options
            ),
            Err(NewlyClosed::No)
        );
        socket_options.nagle_enabled = false;
        assert_eq!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options
            ),
            Ok(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: TEST_ISS + TEST_BYTES.len() + 1,
                    ack: Some(TEST_IRS + 1),
                    control: None,
                    wnd: UnscaledWindowSize::from_usize(BUFFER_SIZE),
                    push: true,
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                },
                FragmentedPayload::new_contiguous(&TEST_BYTES[..PARTIAL_LEN])
            ))
        );
    }

    #[test]
    fn mss_option() {
        // Ensure we setup an MSS smaller than the amount of data being sent.
        let mss = Mss::MIN;
        assert!(usize::from(mss) < TEST_BYTES.len());

        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let (syn_sent, syn) = Closed::<Initial>::connect(
            TEST_ISS,
            TIMESTAMP_OFFSET,
            clock.now(),
            (),
            Default::default(),
            mss,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        let mut state = State::<_, RingBuffer, RingBuffer, ()>::SynSent(syn_sent);

        // Bring the state to SYNRCVD.
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn,
                clock.now(),
                &counters.refs(),
            );
        let syn_ack = seg.expect("expected SYN-ACK");
        assert_eq!(passive_open, None);

        // Bring the state to ESTABLISHED and write some data.
        assert_eq!(
            state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                syn_ack,
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            )
        );
        match state {
            State::Closed(_)
            | State::Listen(_)
            | State::SynRcvd(_)
            | State::SynSent(_)
            | State::LastAck(_)
            | State::FinWait1(_)
            | State::FinWait2(_)
            | State::Closing(_)
            | State::TimeWait(_) => {
                panic!("expected that we have entered established state, but got {:?}", state)
            }
            State::Established(Established { ref mut snd, rcv: _ })
            | State::CloseWait(CloseWait { ref mut snd, closed_rcv: _ }) => {
                assert_eq!(snd.buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
            }
        }
        // Expect to only receive MSS bytes.
        let effective_mss =
            EffectiveMss::from_mss(mss, MssSizeLimiters { timestamp_enabled: true });
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_ISS + 1,
                UnscaledWindowSize::from(u16::MAX),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(&TEST_BYTES[..usize::from(effective_mss)]),
            ))
        );
    }

    const TEST_USER_TIMEOUT: NonZeroDuration = NonZeroDuration::from_secs(2 * 60).unwrap();

    // We can use a smaller and a larger RTT so that when using the smaller one,
    // we can reach maximum retransmit retires and when using the larger one, we
    // timeout before reaching maximum retries.
    #[test_case(Duration::from_millis(1), false, true; "retrans_max_retries")]
    #[test_case(Duration::from_secs(1), false, false; "retrans_no_max_retries")]
    #[test_case(Duration::from_millis(1), true, true; "zwp_max_retries")]
    #[test_case(Duration::from_secs(1), true, false; "zwp_no_max_retires")]
    fn user_timeout(rtt: Duration, zero_window_probe: bool, max_retries: bool) {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut send_buffer = RingBuffer::new(BUFFER_SIZE);
        assert_eq!(send_buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        // Set up the state machine to start with Established.
        let mut state: State<_, _, _, ()> = State::Established(Established {
            snd: Send {
                rtt_estimator: Estimator::Measured { srtt: rtt, rtt_var: Duration::ZERO },
                ..Send::default_for_test(send_buffer.clone())
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        let mut times = 0;
        let start = clock.now();
        let socket_options = SocketOptions {
            user_timeout: (!max_retries).then_some(TEST_USER_TIMEOUT),
            ..SocketOptions::default_for_state_tests()
        };
        while let Ok(seg) = state.poll_send(
            &FakeStateMachineDebugId::default(),
            &counters.refs(),
            clock.now(),
            &socket_options,
        ) {
            if zero_window_probe {
                let zero_window_ack = Segment::ack(
                    seg.header().ack.unwrap(),
                    seg.header().seq,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS,
                );
                assert_matches!(
                    state.on_segment::<(), ClientlessBufferProvider>(
                        &FakeStateMachineDebugId::default(),
                        &counters.refs(),
                        zero_window_ack,
                        clock.now(),
                        &socket_options,
                        false,
                    ),
                    (None, None, DataAcked::No, _newly_closed)
                );

                // In non-test code, calling poll_send is done automatically in
                // try_handle_incoming_for_connection.
                //
                // This is when the ZWP timer gets set.
                assert_matches!(
                    state.poll_send(
                        &FakeStateMachineDebugId::default(),
                        &counters.refs(),
                        clock.now(),
                        &socket_options,
                    ),
                    Err(NewlyClosed::No)
                );
                let inner_state = assert_matches!(state, State::Established(ref e) => e);
                assert_matches!(inner_state.snd.timer, Some(SendTimer::ZeroWindowProbe(_)));
            }

            let deadline = state.poll_send_at().expect("must have a retransmission timer");
            clock.sleep(deadline.checked_duration_since(clock.now()).unwrap());
            times += 1;
        }
        let elapsed = clock.now().checked_duration_since(start).unwrap();
        if max_retries {
            assert_eq!(times, 1 + DEFAULT_MAX_RETRIES.get());
        } else {
            assert_eq!(elapsed, TEST_USER_TIMEOUT.get());
            assert!(times < DEFAULT_MAX_RETRIES.get());
        }
        assert_eq!(state, State::Closed(Closed { reason: Some(ConnectionError::TimedOut) }));
        CounterExpectations {
            established_closed: 1,
            established_timedout: 1,
            fast_recovery: if zero_window_probe { 1 } else { 0 },
            // Note: We don't want to assert on these counters having a specific
            // value, so copy in their actual value.
            timeouts: counters.stack_wide.timeouts.get(),
            retransmits: counters.stack_wide.retransmits.get(),
            slow_start_retransmits: counters.stack_wide.slow_start_retransmits.get(),
            dup_acks: counters.stack_wide.dup_acks.get(),
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    #[test]
    fn retrans_timer_backoff() {
        let mut clock = FakeInstantCtx::default();
        let mut timer = RetransTimer::new(
            clock.now(),
            Rto::DEFAULT,
            Some(TEST_USER_TIMEOUT),
            DEFAULT_MAX_RETRIES,
        );
        assert_eq!(timer.at, FakeInstant::from(Rto::DEFAULT.get()));
        clock.sleep(TEST_USER_TIMEOUT.get());
        timer.backoff(clock.now());
        assert_eq!(timer.at, FakeInstant::from(TEST_USER_TIMEOUT.get()));
        clock.sleep(Duration::from_secs(1));
        // The current time is now later than the timeout deadline,
        timer.backoff(clock.now());
        // Firing time does not change.
        assert_eq!(timer.at, FakeInstant::from(TEST_USER_TIMEOUT.get()));
    }

    #[test_case(
        State::Established(Established {
            snd: Send {
                rtt_estimator: Estimator::Measured {
                    srtt: Rto::DEFAULT.get(),
                    rtt_var: Duration::ZERO,
                },
                ..Send::default_for_test(RingBuffer::new(BUFFER_SIZE))
            }.into(),
            rcv: Recv::default_for_test(RingBuffer::new(
                TEST_BYTES.len() + 2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE) as usize,
            )).into(),
        }); "established")]
    #[test_case(
        State::FinWait1(FinWait1 {
            snd: Send {
                rtt_estimator: Estimator::Measured {
                    srtt: Rto::DEFAULT.get(),
                    rtt_var: Duration::ZERO,
                },
                ..Send::default_for_test(RingBuffer::new(BUFFER_SIZE))
            }.into(),
            rcv: Recv::default_for_test(RingBuffer::new(
                TEST_BYTES.len() + 2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE) as usize,
            )).into(),
        }); "fin_wait_1")]
    #[test_case(
        State::FinWait2(FinWait2 {
            last_seq: TEST_ISS + 1,
            rcv: Recv::default_for_test(RingBuffer::new(
                TEST_BYTES.len() + 2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE) as usize,
            )),
            timeout_at: None,
            snd_info: SendInfo::default(),
        }); "fin_wait_2")]
    fn delayed_ack(mut state: State<FakeInstant, RingBuffer, RingBuffer, ()>) {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let socket_options =
            SocketOptions { delayed_ack: true, ..SocketOptions::default_for_state_tests() };
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::with_data(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &socket_options,
                false, /* defunct */
            ),
            (None, None, DataAcked::No, NewlyClosed::No)
        );
        assert_eq!(state.poll_send_at(), Some(clock.now().panicking_add(ACK_DELAY_THRESHOLD)));
        clock.sleep(ACK_DELAY_THRESHOLD);
        assert_eq!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options
            ),
            Ok(Segment::ack(
                TEST_ISS + 1,
                TEST_IRS + 1 + TEST_BYTES.len(),
                UnscaledWindowSize::from_u32(2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE)),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
            ))
        );
        let full_segment_sized_payload =
            vec![b'0'; u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE) as usize];

        let expect_last_window_update = (
            TEST_IRS + 1 + TEST_BYTES.len(),
            WindowSize::from_u32(2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE)).unwrap(),
        );
        assert_eq!(state.recv_mut().unwrap().last_window_update, expect_last_window_update);
        // The first full sized segment should not trigger an immediate ACK,
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::with_data(
                    TEST_IRS + 1 + TEST_BYTES.len(),
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    &full_segment_sized_payload[..],
                ),
                clock.now(),
                &socket_options,
                false, /* defunct */
            ),
            (None, None, DataAcked::No, NewlyClosed::No)
        );
        // ... but just a timer.
        assert_eq!(state.poll_send_at(), Some(clock.now().panicking_add(ACK_DELAY_THRESHOLD)));
        // The last reported window should not have been updated since we
        // didn't send an ACK:
        assert_eq!(state.recv_mut().unwrap().last_window_update, expect_last_window_update);

        // Now the second full sized segment arrives, an ACK should be sent
        // immediately.
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::with_data(
                    TEST_IRS + 1 + TEST_BYTES.len() + full_segment_sized_payload.len(),
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    &full_segment_sized_payload[..],
                ),
                clock.now(),
                &socket_options,
                false, /* defunct */
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1 + TEST_BYTES.len() + 2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    UnscaledWindowSize::from(0),
                    default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                )),
                None,
                DataAcked::No,
                NewlyClosed::No,
            )
        );
        // Last window update moves forward now.
        assert_eq!(
            state.recv_mut().unwrap().last_window_update,
            (
                TEST_IRS + 1 + TEST_BYTES.len() + 2 * u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE),
                WindowSize::ZERO,
            )
        );
        assert_eq!(state.poll_send_at(), None);
    }

    #[test_case(true; "sack permitted")]
    #[test_case(false; "sack not permitted")]
    fn immediate_ack_if_out_of_order_or_fin(sack_permitted: bool) {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let socket_options =
            SocketOptions { delayed_ack: true, ..SocketOptions::default_for_state_tests() };
        let mut state: State<_, _, _, ()> = State::Established(Established {
            snd: Send::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
            rcv: Recv {
                sack_permitted,
                ..Recv::default_for_test(RingBuffer::new(TEST_BYTES.len() + 1))
            }
            .into(),
        });
        // Upon receiving an out-of-order segment, we should send an ACK
        // immediately.
        let segment_start = TEST_IRS + 2;
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::with_data(
                    segment_start,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    &TEST_BYTES[1..]
                ),
                clock.now(),
                &socket_options,
                false, /* defunct */
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1,
                    UnscaledWindowSize::from(u16::try_from(TEST_BYTES.len() + 1).unwrap()),
                    SegmentOptions {
                        sack_blocks: if sack_permitted {
                            [SackBlock::try_new(
                                segment_start,
                                segment_start + u32::try_from(TEST_BYTES.len()).unwrap() - 1,
                            )
                            .unwrap()]
                            .into_iter()
                            .collect()
                        } else {
                            SackBlocks::default()
                        },
                        timestamp: Some(DEFAULT_ACK_TS_OPT),
                    },
                )),
                None,
                DataAcked::No,
                NewlyClosed::No,
            )
        );
        assert_eq!(state.poll_send_at(), None);
        // The next segment fills a gap, so it should trigger an immediate
        // ACK.
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::with_data(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS,
                    &TEST_BYTES[..1]
                ),
                clock.now(),
                &socket_options,
                false, /* defunct */
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1 + TEST_BYTES.len(),
                    UnscaledWindowSize::from(1),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None,
                DataAcked::No,
                NewlyClosed::No
            )
        );
        assert_eq!(state.poll_send_at(), None);
        // We should also respond immediately with an ACK to a FIN.
        assert_eq!(
            state.on_segment::<(), ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                Segment::fin(
                    TEST_IRS + 1 + TEST_BYTES.len(),
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &socket_options,
                false, /* defunct */
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1 + TEST_BYTES.len() + 1,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None,
                DataAcked::No,
                NewlyClosed::No,
            )
        );
        assert_eq!(state.poll_send_at(), None);
    }

    #[test]
    fn fin_wait2_timeout() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut state: State<_, _, NullBuffer, ()> = State::FinWait2(FinWait2 {
            last_seq: TEST_ISS,
            rcv: Recv::default_for_test_at(TEST_IRS, NullBuffer),
            timeout_at: None,
            snd_info: SendInfo::default(),
        });
        assert_eq!(
            state.close(
                &counters.refs(),
                CloseReason::Close { now: clock.now() },
                &SocketOptions::default_for_state_tests()
            ),
            Err(CloseError::Closing)
        );
        assert_eq!(
            state.poll_send_at(),
            Some(clock.now().panicking_add(DEFAULT_FIN_WAIT2_TIMEOUT))
        );
        clock.sleep(DEFAULT_FIN_WAIT2_TIMEOUT);
        assert_eq!(state.poll_send_with_default_options(clock.now(), &counters.refs()), None);
        assert_eq!(state, State::Closed(Closed { reason: Some(ConnectionError::TimedOut) }));
        CounterExpectations {
            established_closed: 1,
            established_timedout: 1,
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    #[test_case(RetransTimer {
        user_timeout_until: Some(FakeInstant::from(Duration::from_secs(100))),
        remaining_retries: None,
        at: FakeInstant::from(Duration::from_secs(1)),
        rto: Rto::new(Duration::from_secs(1)),
    }, FakeInstant::from(Duration::from_secs(1)) => true)]
    #[test_case(RetransTimer {
        user_timeout_until: Some(FakeInstant::from(Duration::from_secs(100))),
        remaining_retries: None,
        at: FakeInstant::from(Duration::from_secs(2)),
        rto: Rto::new(Duration::from_secs(1)),
    }, FakeInstant::from(Duration::from_secs(1)) => false)]
    #[test_case(RetransTimer {
        user_timeout_until: Some(FakeInstant::from(Duration::from_secs(100))),
        remaining_retries: Some(NonZeroU8::new(1).unwrap()),
        at: FakeInstant::from(Duration::from_secs(2)),
        rto: Rto::new(Duration::from_secs(1)),
    }, FakeInstant::from(Duration::from_secs(1)) => false)]
    #[test_case(RetransTimer {
        user_timeout_until: Some(FakeInstant::from(Duration::from_secs(1))),
        remaining_retries: Some(NonZeroU8::new(1).unwrap()),
        at: FakeInstant::from(Duration::from_secs(1)),
        rto: Rto::new(Duration::from_secs(1)),
    }, FakeInstant::from(Duration::from_secs(1)) => true)]
    fn send_timed_out(timer: RetransTimer<FakeInstant>, now: FakeInstant) -> bool {
        timer.timed_out(now)
    }

    #[test_case(
        State::SynSent(SynSent{
            iss: TEST_ISS,
            rtt_sampler: MeasuredSampler::Tracking {
                range: TEST_ISS..TEST_ISS + 1,
                timestamp: FakeInstant::default(),
            },
            retrans_timer: RetransTimer::new(
                FakeInstant::default(),
                Rto::MIN,
                NonZeroDuration::from_secs(60),
                DEFAULT_MAX_SYN_RETRIES,
            ),
            active_open: (),
            buffer_sizes: Default::default(),
            device_mss: Mss::DEFAULT_IPV4,
            default_mss: Mss::DEFAULT_IPV4,
            rcv_wnd_scale: WindowScale::default(),
            ts_opt: default_ts_opt_negotiation_state(),
        })
    => DEFAULT_MAX_SYN_RETRIES.get(); "syn_sent")]
    #[test_case(
        State::SynRcvd(SynRcvd{
            iss: TEST_ISS,
            irs: TEST_IRS,
            rtt_sampler: MeasuredSampler::Tracking {
                range: TEST_ISS..TEST_ISS + 1,
                timestamp: FakeInstant::default(),
            },
            retrans_timer: RetransTimer::new(
                FakeInstant::default(),
                Rto::MIN,
                NonZeroDuration::from_secs(60),
                DEFAULT_MAX_SYNACK_RETRIES,
            ),
            simultaneous_open: None,
            buffer_sizes: BufferSizes::default(),
            smss: EffectiveMss::from_mss(
                Mss::DEFAULT_IPV4, MssSizeLimiters { timestamp_enabled: true }
            ),
            rcv_wnd_scale: WindowScale::default(),
            snd_wnd_scale: Some(WindowScale::default()),
            sack_permitted: SACK_PERMITTED,
            rcv: RecvParams {
                ack: TEST_IRS + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(TEST_IRS+1),
                last_ack_recv: None,
            },
        })
    => DEFAULT_MAX_SYNACK_RETRIES.get(); "syn_rcvd")]
    fn handshake_timeout(mut state: State<FakeInstant, RingBuffer, RingBuffer, ()>) -> u8 {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let mut retransmissions = 0;
        clock.sleep_until(state.poll_send_at().expect("must have a retransmission timer"));
        while let Some(_seg) = state.poll_send_with_default_options(clock.now(), &counters.refs()) {
            let deadline = state.poll_send_at().expect("must have a retransmission timer");
            clock.sleep_until(deadline);
            retransmissions += 1;
        }
        assert_eq!(state, State::Closed(Closed { reason: Some(ConnectionError::TimedOut) }));
        CounterExpectations::default().assert_counters(&counters);
        retransmissions
    }

    #[test_case(
        u16::MAX as usize, WindowScale::default(), Some(WindowScale::default())
    => (WindowScale::default(), WindowScale::default()))]
    #[test_case(
        u16::MAX as usize + 1, WindowScale::new(1).unwrap(), Some(WindowScale::default())
    => (WindowScale::new(1).unwrap(), WindowScale::default()))]
    #[test_case(
        u16::MAX as usize + 1, WindowScale::new(1).unwrap(), None
    => (WindowScale::default(), WindowScale::default()))]
    #[test_case(
        u16::MAX as usize, WindowScale::default(), Some(WindowScale::new(1).unwrap())
    => (WindowScale::default(), WindowScale::new(1).unwrap()))]
    fn window_scale(
        buffer_size: usize,
        syn_window_scale: WindowScale,
        syn_ack_window_scale: Option<WindowScale>,
    ) -> (WindowScale, WindowScale) {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let (syn_sent, syn_seg) = Closed::<Initial>::connect(
            TEST_ISS,
            TIMESTAMP_OFFSET,
            clock.now(),
            (),
            BufferSizes { receive: buffer_size, ..Default::default() },
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        assert_eq!(
            syn_seg,
            Segment::syn(
                TEST_ISS,
                UnscaledWindowSize::from(u16::try_from(buffer_size).unwrap_or(u16::MAX)),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(syn_window_scale),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(DEFAULT_NON_ACK_TS_OPT),
                },
            )
        );
        let mut active = State::SynSent(syn_sent);
        clock.sleep(RTT / 2);
        let (seg, passive_open) = active
            .on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::syn_ack(
                    TEST_IRS,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    HandshakeOptions {
                        mss: Some(Mss::DEFAULT_IPV4),
                        window_scale: syn_ack_window_scale,
                        sack_permitted: SACK_PERMITTED,
                        timestamp: Some(DEFAULT_ACK_TS_OPT),
                    },
                ),
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        assert_matches!(seg, Some(_));

        let established: Established<FakeInstant, RingBuffer, NullBuffer> =
            assert_matches!(active, State::Established(established) => established);

        assert_eq!(established.snd.wnd, WindowSize::DEFAULT);

        (established.rcv.wnd_scale, established.snd.wnd_scale)
    }

    #[test_case(
        u16::MAX as usize,
        Segment::syn_ack(
            TEST_IRS + 1 + u16::MAX as usize,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            HandshakeOptions{timestamp: Some(DEFAULT_NON_ACK_TS_OPT), .. Default::default()},
        )
    )]
    #[test_case(
        u16::MAX as usize + 1,
        Segment::syn_ack(
            TEST_IRS + 1 + u16::MAX as usize,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            HandshakeOptions{timestamp: Some(DEFAULT_NON_ACK_TS_OPT), .. Default::default()},
        )
    )]
    #[test_case(
        u16::MAX as usize,
        Segment::with_data(
            TEST_IRS + 1 + u16::MAX as usize,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            &TEST_BYTES[..],
        )
    )]
    #[test_case(
        u16::MAX as usize + 1,
        Segment::with_data(
            TEST_IRS + 1 + u16::MAX as usize,
            TEST_ISS + 1,
            UnscaledWindowSize::from(u16::MAX),
            DEFAULT_SEGMENT_OPTIONS,
            &TEST_BYTES[..],
        )
    )]
    fn window_scale_otw_seq(receive_buf_size: usize, otw_seg: impl Into<Segment<&'static [u8]>>) {
        let counters = FakeTcpCounters::default();
        let buffer_sizes = BufferSizes { send: 0, receive: receive_buf_size };
        let rcv_wnd_scale = buffer_sizes.rwnd().scale();
        let rcv = RecvParams {
            ack: TEST_IRS + 1,
            wnd_scale: WindowScale::default(),
            wnd: buffer_sizes.rwnd_unscaled() << WindowScale::default(),
            ts_opt: default_ts_opt_state(TEST_IRS + 1),
            last_ack_recv: None,
        };
        let mut syn_rcvd: State<_, RingBuffer, RingBuffer, ()> = State::SynRcvd(SynRcvd {
            iss: TEST_ISS,
            irs: TEST_IRS,
            rtt_sampler: MeasuredSampler::default(),
            retrans_timer: RetransTimer::new(
                FakeInstant::default(),
                Rto::DEFAULT,
                NonZeroDuration::from_secs(10),
                DEFAULT_MAX_SYNACK_RETRIES,
            ),
            simultaneous_open: None,
            buffer_sizes: BufferSizes { send: 0, receive: receive_buf_size },
            smss: EffectiveMss::from_mss(
                Mss::DEFAULT_IPV4,
                MssSizeLimiters { timestamp_enabled: true },
            ),
            rcv_wnd_scale,
            snd_wnd_scale: WindowScale::new(1),
            sack_permitted: SACK_PERMITTED,
            rcv,
        });

        assert_eq!(
            syn_rcvd.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                otw_seg.into(),
                FakeInstant::default(),
                &counters.refs()
            ),
            (
                Some(Segment::ack(
                    TEST_ISS + 1,
                    TEST_IRS + 1,
                    buffer_sizes.rwnd_unscaled(),
                    DEFAULT_SEGMENT_OPTIONS
                )),
                None
            ),
        )
    }

    #[test]
    fn poll_send_reserving_buffer() {
        const RESERVED_BYTES: usize = 3;
        let mut snd: Send<FakeInstant, _, false> = Send::default_for_test(ReservingBuffer {
            buffer: RingBuffer::with_data(TEST_BYTES.len(), TEST_BYTES),
            reserved_bytes: RESERVED_BYTES,
        });

        let counters = FakeTcpCounters::default();

        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                FakeInstant::default(),
                &SocketOptions::default_for_state_tests(),
            ),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                WindowSize::DEFAULT >> WindowScale::default(),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(&TEST_BYTES[..TEST_BYTES.len() - RESERVED_BYTES])
            ))
        );

        assert_eq!(snd.nxt, TEST_ISS + 1 + (TEST_BYTES.len() - RESERVED_BYTES));
    }

    #[test]
    fn rcv_silly_window_avoidance() {
        const MULTIPLE: usize = 3;
        const CAP: usize = TEST_BYTES.len() * MULTIPLE;
        let mut rcv: Recv<FakeInstant, RingBuffer> =
            Recv { mss: TEST_MSS, ..Recv::default_for_test_at(TEST_IRS, RingBuffer::new(CAP)) };

        fn get_buffer(rcv: &mut Recv<FakeInstant, RingBuffer>) -> &mut RingBuffer {
            assert_matches!(
                &mut rcv.buffer,
                RecvBufferState::Open {buffer, .. } => buffer
            )
        }

        // Initially the entire buffer is advertised.
        assert_eq!(rcv.calculate_window_size().window_size, WindowSize::new(CAP).unwrap());

        for _ in 0..MULTIPLE {
            assert_eq!(get_buffer(&mut rcv).enqueue_data(TEST_BYTES), TEST_BYTES.len());
        }
        let assembler = assert_matches!(&mut rcv.buffer,
            RecvBufferState::Open { assembler, .. } => assembler);
        assert_eq!(assembler.insert(TEST_IRS..TEST_IRS + CAP), CAP);
        // Since the buffer is full, we now get a zero window.
        assert_eq!(rcv.calculate_window_size().window_size, WindowSize::ZERO);

        // The user reads 1 byte, but our implementation should not advertise
        // a new window because it is too small;
        assert_eq!(get_buffer(&mut rcv).read_with(|_| 1), 1);
        assert_eq!(rcv.calculate_window_size().window_size, WindowSize::ZERO);

        // Now at least there is at least 1 MSS worth of free space in the
        // buffer, advertise it.
        assert_eq!(get_buffer(&mut rcv).read_with(|_| TEST_BYTES.len()), TEST_BYTES.len());
        assert_eq!(
            rcv.calculate_window_size().window_size,
            WindowSize::new(TEST_BYTES.len() + 1).unwrap()
        );
    }

    #[test]
    // Regression test for https://fxbug.dev/376061162.
    fn correct_window_scale_during_send() {
        let snd_wnd_scale = WindowScale::new(4).unwrap();
        let rcv_wnd_scale = WindowScale::new(8).unwrap();
        let wnd_size = WindowSize::new(1024).unwrap();

        let counters = FakeTcpCounters::default();
        // Tests the common behavior when ack segment arrives in states that
        // have a send state.
        let new_snd = || Send {
            wnd: wnd_size,
            wnd_scale: snd_wnd_scale,
            congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
            ..Send::default_for_test(RingBuffer::with_data(TEST_BYTES.len(), TEST_BYTES))
        };
        let new_rcv = || Recv {
            wnd_scale: rcv_wnd_scale,
            ..Recv::default_for_test(RingBuffer::new(wnd_size.into()))
        };
        for mut state in [
            State::Established(Established { snd: new_snd().into(), rcv: new_rcv().into() }),
            State::FinWait1(FinWait1 { snd: new_snd().queue_fin().into(), rcv: new_rcv().into() }),
            State::Closing(Closing {
                snd: new_snd().queue_fin(),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: wnd_size,
                    wnd_scale: rcv_wnd_scale,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            }),
            State::CloseWait(CloseWait {
                snd: new_snd().into(),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: wnd_size,
                    wnd_scale: rcv_wnd_scale,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            }),
            State::LastAck(LastAck {
                snd: new_snd().queue_fin(),
                closed_rcv: RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: wnd_size,
                    wnd_scale: rcv_wnd_scale,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: Some(FakeInstant::default()),
                },
            }),
        ] {
            assert_eq!(
                state.poll_send_with_default_options(FakeInstant::default(), &counters.refs(),),
                Some(Segment::new_assert_no_discard(
                    SegmentHeader {
                        seq: TEST_ISS + 1,
                        ack: Some(TEST_IRS + 1),
                        control: None,
                        // We expect this to be wnd_size >> rcv_wnd_scale, which
                        // equals 1024 >> 8 == 4
                        wnd: UnscaledWindowSize::from(4),
                        push: true,
                        options: DEFAULT_SEGMENT_OPTIONS.into(),
                    },
                    FragmentedPayload::new_contiguous(TEST_BYTES)
                ))
            );
        }
    }

    #[test_case(true; "prompted window update")]
    #[test_case(false; "unprompted window update")]
    fn snd_silly_window_avoidance(prompted_window_update: bool) {
        const CAP: usize = TEST_BYTES.len() * 2;
        let mut snd: Send<FakeInstant, RingBuffer, false> = Send {
            wl1: TEST_IRS,
            wl2: TEST_ISS,
            wnd: WindowSize::new(CAP).unwrap(),
            congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
            ..Send::default_for_test(RingBuffer::new(CAP))
        };

        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();

        // We enqueue two copies of TEST_BYTES.
        assert_eq!(snd.buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        assert_eq!(snd.buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());

        // The first copy should be sent out since the receiver has the space.
        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from(u16::MAX),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES),
            )),
        );

        assert_eq!(
            snd.process_ack(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                TEST_IRS + 1,
                TEST_ISS + 1 + TEST_BYTES.len(),
                UnscaledWindowSize::from(0),
                &SackBlocks::EMPTY,
                None,
                true,
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd_scale: WindowScale::default(),
                    wnd: WindowSize::DEFAULT,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default(),
            ),
            (None, DataAcked::Yes)
        );

        // Now that we received a zero window, we should start probing for the
        // window reopening.
        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            None
        );

        assert_eq!(
            snd.timer,
            Some(SendTimer::ZeroWindowProbe(RetransTimer::new(
                clock.now(),
                snd.rtt_estimator.rto(),
                None,
                DEFAULT_MAX_RETRIES,
            )))
        );

        clock.sleep_until(snd.timer.as_ref().unwrap().expiry());

        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            Some(Segment::with_data(
                TEST_ISS + 1 + TEST_BYTES.len(),
                TEST_IRS + 1,
                UnscaledWindowSize::from(u16::MAX),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                FragmentedPayload::new_contiguous(&TEST_BYTES[..1]),
            ))
        );

        if prompted_window_update {
            // Now the receiver sends back a window update, but not enough for a
            // full MSS.
            assert_eq!(
                snd.process_ack(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    TEST_IRS + 1,
                    TEST_ISS + 1 + TEST_BYTES.len() + 1,
                    UnscaledWindowSize::from(3),
                    &SackBlocks::EMPTY,
                    None,
                    true,
                    &mut RecvParams {
                        ack: TEST_IRS + 1,
                        wnd_scale: WindowScale::default(),
                        wnd: WindowSize::DEFAULT,
                        ts_opt: default_ts_opt_state(TEST_IRS + 1),
                        last_ack_recv: None,
                    },
                    clock.now(),
                    &SocketOptions::default(),
                ),
                (None, DataAcked::Yes)
            );
        } else {
            // First probe sees the same empty window.
            assert_eq!(
                snd.process_ack(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    TEST_IRS + 1,
                    TEST_ISS + 1 + TEST_BYTES.len(),
                    UnscaledWindowSize::from(0),
                    &SackBlocks::EMPTY,
                    None,
                    true,
                    &mut RecvParams {
                        ack: TEST_IRS + 1,
                        wnd_scale: WindowScale::default(),
                        wnd: WindowSize::DEFAULT,
                        ts_opt: default_ts_opt_state(TEST_IRS + 1),
                        last_ack_recv: None,
                    },
                    clock.now(),
                    &SocketOptions::default(),
                ),
                (None, DataAcked::No)
            );

            // The receiver freed up buffer space and decided to send a sub-MSS
            // window update (likely a bug).
            assert_eq!(
                snd.process_ack(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    TEST_IRS + 1,
                    TEST_ISS + 1 + TEST_BYTES.len(),
                    UnscaledWindowSize::from(3),
                    &SackBlocks::EMPTY,
                    None,
                    true,
                    &mut RecvParams {
                        ack: TEST_IRS + 1,
                        wnd_scale: WindowScale::default(),
                        wnd: WindowSize::DEFAULT,
                        ts_opt: default_ts_opt_state(TEST_IRS + 1),
                        last_ack_recv: None,
                    },
                    clock.now(),
                    &SocketOptions::default(),
                ),
                (None, DataAcked::No)
            );
        }

        // We would then transition into SWS avoidance.
        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            None,
        );
        assert_eq!(
            snd.timer,
            Some(SendTimer::SWSProbe { at: clock.now().panicking_add(SWS_PROBE_TIMEOUT) })
        );
        clock.sleep(SWS_PROBE_TIMEOUT);

        // After the overriding timeout, we should push out whatever the
        // receiver is willing to receive.
        let seq_index = usize::from(prompted_window_update);
        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            Some(Segment::with_data(
                TEST_ISS + 1 + TEST_BYTES.len() + seq_index,
                TEST_IRS + 1,
                UnscaledWindowSize::from(u16::MAX),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                FragmentedPayload::new_contiguous(&TEST_BYTES[seq_index..3 + seq_index]),
            ))
        );
    }

    #[test]
    fn snd_enter_zwp_on_negative_window_update() {
        const CAP: usize = TEST_BYTES.len() * 2;
        let mut snd: Send<FakeInstant, RingBuffer, false> = Send {
            wnd: WindowSize::new(CAP).unwrap(),
            wl1: TEST_IRS,
            wl2: TEST_ISS,
            congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
            ..Send::default_for_test(RingBuffer::new(CAP))
        };

        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();

        // We enqueue two copies of TEST_BYTES.
        assert_eq!(snd.buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        assert_eq!(snd.buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());

        // The first copy should be sent out since the receiver has the space.
        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            Some(Segment::with_data(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from(u16::MAX),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES),
            )),
        );

        // We've sent some data, but haven't received an ACK, so the retransmit
        // timer should be set.
        assert_matches!(snd.timer, Some(SendTimer::Retrans(_)));

        // Send an ACK segment that doesn't ACK any data, but does close the
        // window. This is strongly discouraged, but technically allowed by RFC
        // 9293:
        //   A TCP receiver SHOULD NOT shrink the window, i.e., move the right
        //   window edge to the left (SHLD-14). However, a sending TCP peer MUST
        //   be robust against window shrinking, which may cause the "usable
        //   window" (see Section 3.8.6.2.1) to become negative (MUST-34).
        assert_eq!(
            snd.process_ack(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                TEST_IRS + 1,
                TEST_ISS + 1 + TEST_BYTES.len(),
                UnscaledWindowSize::from(0),
                &SackBlocks::EMPTY,
                None,
                true,
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd_scale: WindowScale::default(),
                    wnd: WindowSize::DEFAULT,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default(),
            ),
            (None, DataAcked::Yes)
        );

        // Trying to send more data should result in no segment being sent, but
        // instead setting up the ZWP timr.
        assert_eq!(
            snd.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                &mut RecvParams {
                    ack: TEST_IRS + 1,
                    wnd: WindowSize::DEFAULT,
                    wnd_scale: WindowScale::ZERO,
                    ts_opt: default_ts_opt_state(TEST_IRS + 1),
                    last_ack_recv: None,
                },
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            ),
            None,
        );
        assert_matches!(snd.timer, Some(SendTimer::ZeroWindowProbe(_)));
    }

    #[test]
    // Regression test for https://fxbug.dev/334926865.
    fn ack_uses_snd_max() {
        let counters = FakeTcpCounters::default();
        let mut clock = FakeInstantCtx::default();
        let mut buffer = RingBuffer::new(BUFFER_SIZE);
        assert_eq!(buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());
        assert_eq!(buffer.enqueue_data(TEST_BYTES), TEST_BYTES.len());

        // This connection has the same send and receive seqnum space.
        let iss = ISS_1 + 1;
        let mut state: State<_, _, _, ()> = State::Established(Established {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test_at(iss, buffer)
            }
            .into(),
            rcv: Recv {
                mss: TEST_MSS,
                ..Recv::default_for_test_at(iss, RingBuffer::new(BUFFER_SIZE))
            }
            .into(),
        });

        // Send the first two data segments.
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                iss,
                iss,
                UnscaledWindowSize::from(u16::try_from(BUFFER_SIZE).unwrap()),
                DEFAULT_SEGMENT_OPTIONS,
                FragmentedPayload::new_contiguous(TEST_BYTES),
            )),
        );
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::new_assert_no_discard(
                SegmentHeader {
                    seq: iss + TEST_BYTES.len(),
                    ack: Some(iss),
                    control: None,
                    wnd: UnscaledWindowSize::from(u16::try_from(BUFFER_SIZE).unwrap()),
                    push: true,
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                },
                FragmentedPayload::new_contiguous(TEST_BYTES),
            )),
        );

        // Retransmit, now snd.nxt = TEST.BYTES.len() + 1.
        clock.sleep(Rto::DEFAULT.get());
        assert_eq!(
            state.poll_send_with_default_options(clock.now(), &counters.refs()),
            Some(Segment::with_data(
                iss,
                iss,
                UnscaledWindowSize::from(u16::try_from(BUFFER_SIZE).unwrap()),
                default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                FragmentedPayload::new_contiguous(TEST_BYTES),
            )),
        );

        // the ACK sent should have seq = snd.max (2 * TEST_BYTES.len() + 1) to
        // avoid getting stuck in an ACK cycle.
        assert_eq!(
            state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    iss,
                    iss,
                    UnscaledWindowSize::from(u16::try_from(BUFFER_SIZE).unwrap()),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs(),
            ),
            (
                Some(Segment::ack(
                    iss + 2 * TEST_BYTES.len(),
                    iss + TEST_BYTES.len(),
                    UnscaledWindowSize::from(
                        u16::try_from(BUFFER_SIZE - TEST_BYTES.len()).unwrap()
                    ),
                    default_segment_options(timestamp_now(&clock), DEFAULT_TIMESTAMP),
                )),
                None,
            )
        );
    }

    #[test_case(
        State::Closed(Closed { reason: None }),
        State::Closed(Closed { reason: None }) => NewlyClosed::No; "closed to closed")]
    #[test_case(
        State::SynSent(SynSent {
            iss: TEST_ISS,
            rtt_sampler: MeasuredSampler::Tracking {
                range: TEST_ISS..TEST_ISS + 1,
                timestamp: FakeInstant::default(),
            },
            retrans_timer: RetransTimer::new(
                FakeInstant::default(),
                Rto::DEFAULT,
                None,
                DEFAULT_MAX_SYN_RETRIES,
            ),
            active_open: (),
            buffer_sizes: BufferSizes::default(),
            default_mss: Mss::DEFAULT_IPV4,
            device_mss: DEVICE_MAXIMUM_SEGMENT_SIZE,
            rcv_wnd_scale: WindowScale::default(),
            ts_opt: default_ts_opt_negotiation_state(),
        }),
        State::Closed(Closed { reason: None }) => NewlyClosed::Yes; "non-closed to closed")]
    #[test_case(
        State::SynSent(SynSent {
                iss: TEST_ISS,
                rtt_sampler: MeasuredSampler::Tracking {
                    range: TEST_ISS..TEST_ISS + 1,
                    timestamp: FakeInstant::default(),
                },
                retrans_timer: RetransTimer::new(
                    FakeInstant::default(),
                    Rto::DEFAULT,
                    None,
                    DEFAULT_MAX_SYN_RETRIES,
                ),
                active_open: (),
                buffer_sizes: BufferSizes::default(),
                default_mss: Mss::DEFAULT_IPV4,
                device_mss: DEVICE_MAXIMUM_SEGMENT_SIZE,
                rcv_wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_negotiation_state(),
            },
        ),
        State::SynRcvd(SynRcvd {
            iss: TEST_ISS,
            irs: TEST_IRS,
            rtt_sampler: MeasuredSampler::default(),
            retrans_timer: RetransTimer::new(
                FakeInstant::default(),
                Rto::DEFAULT,
                NonZeroDuration::from_secs(10),
                DEFAULT_MAX_SYNACK_RETRIES,
            ),
            simultaneous_open: None,
            buffer_sizes: BufferSizes { send: 0, receive: 0 },
            smss: EffectiveMss::from_mss(
                Mss::DEFAULT_IPV4, MssSizeLimiters{timestamp_enabled: true}
            ),
            rcv_wnd_scale: WindowScale::new(0).unwrap(),
            snd_wnd_scale: WindowScale::new(0),
            sack_permitted: SACK_PERMITTED,
            rcv: RecvParams{
                ack: TEST_IRS + 1,
                wnd: WindowSize::DEFAULT,
                wnd_scale: WindowScale::default(),
                ts_opt: default_ts_opt_state(TEST_IRS+1),
                last_ack_recv: None,
            }
        }) => NewlyClosed::No; "non-closed to non-closed")]
    fn transition_to_state(
        mut old_state: State<FakeInstant, RingBuffer, RingBuffer, ()>,
        new_state: State<FakeInstant, RingBuffer, RingBuffer, ()>,
    ) -> NewlyClosed {
        let counters = FakeTcpCounters::default();
        old_state.transition_to_state(&counters.refs(), new_state)
    }

    #[test_case(true, false, false; "more than mss dequeued")]
    #[test_case(false, false, false; "less than mss dequeued")]
    #[test_case(true, true, false; "more than mss dequeued and delack")]
    #[test_case(false, true, false; "less than mss dequeued and delack")]
    #[test_case(true, false, true; "more than mss dequeued with sack")]
    fn poll_receive_data_dequeued_state(
        dequeue_more_than_mss: bool,
        delayed_ack: bool,
        sack: bool,
    ) {
        // NOTE: Just enough room to hold TEST_BYTES, but will end up below
        // TEST_MSS when full.
        const BUFFER_SIZE: usize = TEST_BYTES.len();

        let new_snd = || Send {
            congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
            ..Send::default_for_test(NullBuffer)
        };
        let new_rcv =
            || Recv { mss: TEST_MSS, ..Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)) };

        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        for mut state in [
            State::Established(Established { snd: new_snd().into(), rcv: new_rcv().into() }),
            State::FinWait1(FinWait1 { snd: new_snd().queue_fin().into(), rcv: new_rcv().into() }),
            State::FinWait2(FinWait2 {
                last_seq: TEST_ISS + 1,
                rcv: new_rcv(),
                timeout_at: None,
                snd_info: SendInfo::default(),
            }),
        ] {
            // At the beginning, our last window update was greater than MSS, so there's no
            // need to send an update.
            assert_matches!(state.poll_receive_data_dequeued(clock.now()), None);
            let expect_window_update = (TEST_IRS + 1, WindowSize::new(BUFFER_SIZE).unwrap());
            assert_eq!(state.recv_mut().unwrap().last_window_update, expect_window_update);

            // Receive a segment that almost entirely fills the buffer.  We now have less
            // than MSS of available window, which means we won't send an update.

            let seg = state.on_segment_with_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(0),
                    DEFAULT_SEGMENT_OPTIONS,
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs(),
                &SocketOptions { delayed_ack, ..SocketOptions::default_for_state_tests() },
            );

            let expect_ack = (!delayed_ack).then_some(Segment::ack(
                TEST_ISS + 1,
                TEST_IRS + 1 + TEST_BYTES.len(),
                UnscaledWindowSize::from((BUFFER_SIZE - TEST_BYTES.len()) as u16),
                DEFAULT_SEGMENT_OPTIONS,
            ));
            assert_eq!(seg, (expect_ack, None));
            assert_matches!(state.poll_receive_data_dequeued(clock.now()), None);

            let expect_window_update = if delayed_ack {
                expect_window_update
            } else {
                (TEST_IRS + 1 + TEST_BYTES.len(), WindowSize::new(0).unwrap())
            };
            assert_eq!(state.recv_mut().unwrap().last_window_update, expect_window_update);

            if dequeue_more_than_mss {
                // Dequeue the data we just received, which frees up enough space in the buffer
                // to receive another MSS worth of data. Since we went from under to over MSS,
                // we expect to immediately send a window update.
                assert_eq!(
                    state.read_with(|available| {
                        assert_eq!(available, &[TEST_BYTES]);
                        available[0].len()
                    }),
                    TEST_BYTES.len()
                );

                // Verify that if the connection has received segments out of
                // order, it will include the SackBlocks option in the window
                // update.
                let expected_sack_blocks = if sack {
                    let assembler = assert_matches!(
                        &mut state.recv_mut().unwrap().buffer,
                        RecvBufferState::Open { buffer: _, assembler } => assembler
                    );
                    let gap_start = TEST_IRS + u32::from(TEST_MSS) + 2;
                    // Setup the assembler with a gap to simulate having
                    // received data out of order.
                    assert_eq!(assembler.insert(gap_start..gap_start + 1), 0);
                    let expected_sack_blocks =
                        assembler.sack_blocks(SackBlockSizeLimiters { timestamp_enabled: true });
                    assert!(!expected_sack_blocks.is_empty());
                    expected_sack_blocks
                } else {
                    SackBlocks::EMPTY
                };

                assert_eq!(
                    state.poll_receive_data_dequeued(clock.now()),
                    Some(Segment::ack(
                        TEST_ISS + 1,
                        TEST_IRS + 1 + TEST_BYTES.len(),
                        UnscaledWindowSize::from(BUFFER_SIZE as u16),
                        SegmentOptions {
                            sack_blocks: expected_sack_blocks,
                            timestamp: Some(DEFAULT_ACK_TS_OPT),
                        },
                    ))
                );
                assert_eq!(
                    state.recv_mut().unwrap().last_window_update,
                    (TEST_IRS + 1 + TEST_BYTES.len(), WindowSize::new(BUFFER_SIZE).unwrap())
                );
                // Delack timer must've been reset if it was set.
                assert!(state.recv_mut().unwrap().timer.is_none());
            } else {
                // Dequeue data we just received, but not enough that will cause
                // the window to be half open (given we're using a very small
                // buffer size).
                assert_eq!(
                    state.read_with(|available| {
                        assert_eq!(available, &[TEST_BYTES]);
                        TEST_BYTES.len() / 2 - 1
                    }),
                    TEST_BYTES.len() / 2 - 1
                );
                assert_eq!(state.poll_receive_data_dequeued(clock.now()), None,);
                assert_eq!(
                    state.recv_mut().unwrap().last_window_update,
                    // No change, no ACK was sent.
                    expect_window_update,
                );
                assert_eq!(state.recv_mut().unwrap().timer.is_some(), delayed_ack);
            }
        }
    }

    #[test]
    fn poll_receive_data_dequeued_small_window() {
        const MSS: Mss = Mss::new(65000).unwrap();
        const WINDOW: WindowSize = WindowSize::from_u32(500).unwrap();
        let mut recv = Recv::<FakeInstant, _> {
            mss: EffectiveMss::from_mss(MSS, MssSizeLimiters { timestamp_enabled: true }),
            last_window_update: (TEST_IRS + 1, WindowSize::from_u32(1).unwrap()),
            ..Recv::default_for_test(RingBuffer::new(WINDOW.into()))
        };
        let seg = recv
            .poll_receive_data_dequeued(TEST_ISS, FakeInstant::default())
            .expect("generates segment");
        assert_eq!(seg.header().ack, Some(recv.nxt()));
        assert_eq!(seg.header().wnd << recv.wnd_scale, WINDOW);
    }

    #[test]
    fn quickack_period() {
        let mut quickack = default_quickack_counter();
        let mut state = State::Established(Established::<FakeInstant, _, _> {
            snd: Send::default_for_test(NullBuffer).into(),
            rcv: Recv {
                remaining_quickacks: quickack,
                ..Recv::default_for_test(RingBuffer::default())
            }
            .into(),
        });
        let socket_options = SocketOptions { delayed_ack: true, ..Default::default() };
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let data = vec![0u8; usize::from(DEVICE_MAXIMUM_SEGMENT_SIZE)];
        while quickack != 0 {
            let seq = state.recv_mut().unwrap().nxt();
            let segment = Segment::new_assert_no_discard(
                SegmentHeader {
                    seq,
                    ack: Some(TEST_ISS + 1),
                    options: DEFAULT_SEGMENT_OPTIONS.into(),
                    ..Default::default()
                },
                &data[..],
            );
            let (seg, passive_open) = state.on_segment_with_options::<_, ClientlessBufferProvider>(
                segment,
                clock.now(),
                &counters.refs(),
                &socket_options,
            );
            let recv = state.recv_mut().unwrap();
            assert_matches!(recv.timer, None);

            assert_eq!(passive_open, None);
            let seg = seg.expect("no segment generated");
            assert_eq!(seg.header().ack, Some(seq + u32::try_from(data.len()).unwrap()));
            assert_eq!(recv.remaining_quickacks, quickack - 1);
            quickack -= 1;
            state.buffers_mut().into_receive_buffer().unwrap().reset();
        }

        // The next segment will be delayed.
        let segment = Segment::new_assert_no_discard(
            SegmentHeader {
                seq: state.recv_mut().unwrap().nxt(),
                ack: Some(TEST_ISS + 1),
                options: DEFAULT_SEGMENT_OPTIONS.into(),
                ..Default::default()
            },
            &data[..],
        );
        let (seg, passive_open) = state.on_segment_with_options::<_, ClientlessBufferProvider>(
            segment,
            clock.now(),
            &counters.refs(),
            &socket_options,
        );
        assert_eq!(passive_open, None);
        assert_eq!(seg, None);
        assert_matches!(state.recv_mut().unwrap().timer, Some(ReceiveTimer::DelayedAck { .. }));
    }

    #[test]
    fn quickack_reset_out_of_window() {
        let mut state = State::Established(Established::<FakeInstant, _, _> {
            snd: Send::default_for_test(NullBuffer).into(),
            rcv: Recv::default_for_test(RingBuffer::default()).into(),
        });
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let data = vec![0u8; usize::from(DEVICE_MAXIMUM_SEGMENT_SIZE)];

        let segment = Segment::new_assert_no_discard(
            SegmentHeader {
                // Generate a segment that is out of the receiving window.
                seq: state.recv_mut().unwrap().nxt() - i32::try_from(data.len() + 1).unwrap(),
                ack: Some(TEST_ISS + 1),
                options: DEFAULT_SEGMENT_OPTIONS.into(),
                ..Default::default()
            },
            &data[..],
        );
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                segment,
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        let recv = state.recv_mut().unwrap();
        let seg = seg.expect("expected segment");
        assert_eq!(seg.header().ack, Some(recv.nxt()));
        assert_eq!(recv.remaining_quickacks, default_quickack_counter());
    }

    #[test]
    fn quickack_reset_rto() {
        let mut clock = FakeInstantCtx::default();
        let mut state = State::Established(Established::<FakeInstant, _, _> {
            snd: Send::default_for_test(NullBuffer).into(),
            rcv: Recv {
                last_segment_at: Some(clock.now()),
                ..Recv::default_for_test(RingBuffer::default())
            }
            .into(),
        });
        let counters = FakeTcpCounters::default();
        let data = vec![0u8; usize::from(DEVICE_MAXIMUM_SEGMENT_SIZE)];

        let segment = Segment::new_assert_no_discard(
            SegmentHeader {
                seq: state.recv_mut().unwrap().nxt(),
                ack: Some(TEST_ISS + 1),
                options: DEFAULT_SEGMENT_OPTIONS.into(),
                ..Default::default()
            },
            &data[..],
        );
        clock.sleep(Rto::DEFAULT.get());
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                segment,
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        let recv = state.recv_mut().unwrap();
        let seg = seg.expect("expected segment");
        assert_eq!(seg.header().ack, Some(recv.nxt()));
        // This just sent an ack back to us, so it burns one off the default
        // counter's value.
        assert_eq!(recv.remaining_quickacks, default_quickack_counter() - 1);
        assert_eq!(recv.last_segment_at, Some(clock.now()));
    }

    #[test_case(true; "sack permitted")]
    #[test_case(false; "sack not permitted")]
    fn receiver_selective_acks(sack_permitted: bool) {
        let mut state = State::Established(Established::<FakeInstant, _, _> {
            snd: Send::default_for_test(RingBuffer::default()).into(),
            rcv: Recv { sack_permitted, ..Recv::default_for_test(RingBuffer::default()) }.into(),
        });
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let data = vec![0u8; usize::from(DEVICE_MAXIMUM_SEGMENT_SIZE)];
        let mss = u32::from(DEVICE_MAXIMUM_SEGMENT_SIZE);
        // Send an out of order segment.
        let seg_start = TEST_IRS + 1 + mss;
        let segment = Segment::new_assert_no_discard(
            SegmentHeader {
                seq: seg_start,
                ack: Some(TEST_ISS + 1),
                wnd: WindowSize::DEFAULT >> WindowScale::default(),
                options: DEFAULT_SEGMENT_OPTIONS.into(),
                ..Default::default()
            },
            &data[..],
        );
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                segment,
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        let seg = seg.expect("expected segment");
        assert_eq!(seg.header().ack, Some(TEST_IRS + 1));
        let expect = if sack_permitted {
            SackBlocks::from_iter([SackBlock::try_new(seg_start, seg_start + mss).unwrap()])
        } else {
            SackBlocks::default()
        };
        let sack_blocks =
            assert_matches!(&seg.header().options, Options::Segment(o) => &o.sack_blocks);
        assert_eq!(sack_blocks, &expect);

        // If we need to send data now, sack blocks should be present.
        assert_eq!(
            state.buffers_mut().into_send_buffer().unwrap().enqueue_data(&data[..]),
            data.len()
        );
        let seg = state
            .poll_send_with_default_options(clock.now(), &counters.refs())
            .expect("generates segment");
        assert_eq!(seg.header().ack, Some(TEST_IRS + 1));

        // The exact same sack blocks are present there.
        let sack_blocks =
            assert_matches!(&seg.header().options, Options::Segment(o) => &o.sack_blocks);
        assert_eq!(sack_blocks, &expect);
        // If there are sack blocks, the segment length should have been
        // restricted.
        let expect_len = mss - u32::try_from(seg.header().options.builder().bytes_len()).unwrap();
        assert_eq!(seg.len(), expect_len);

        // Now receive the out of order block, no SACK should be present
        // anymore.
        let segment = Segment::new_assert_no_discard(
            SegmentHeader {
                seq: TEST_IRS + 1,
                ack: Some(TEST_ISS + 1),
                options: DEFAULT_SEGMENT_OPTIONS.into(),
                ..Default::default()
            },
            &data[..],
        );
        let (seg, passive_open) = state
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                segment,
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        let seg = seg.expect("expected segment");
        assert_eq!(seg.header().ack, Some(TEST_IRS + (2 * mss) + 1));
        let sack_blocks =
            assert_matches!(&seg.header().options, Options::Segment(o) => &o.sack_blocks);
        assert_eq!(sack_blocks, &SackBlocks::default());
    }

    #[derive(Debug)]
    enum RttTestScenario {
        AckOne,
        AckTwo,
        Retransmit,
        AckPartial,
    }

    // Tests integration of the RTT sampler with the TCP state machine.
    #[test_case(RttTestScenario::AckOne)]
    #[test_case(RttTestScenario::AckTwo)]
    #[test_case(RttTestScenario::Retransmit)]
    #[test_case(RttTestScenario::AckPartial)]
    fn rtt(scenario: RttTestScenario) {
        let mut state = State::Established(Established::<FakeInstant, _, _> {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(TEST_MSS),
                ..Send::default_for_test(RingBuffer::default())
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::default()).into(),
        });

        const CLOCK_STEP: Duration = Duration::from_millis(1);

        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        for _ in 0..3 {
            assert_eq!(
                state.buffers_mut().into_send_buffer().unwrap().enqueue_data(TEST_BYTES),
                TEST_BYTES.len()
            );
        }

        assert_eq!(
            state.assert_established().snd.rtt_sampler,
            SamplingStrategy::Measured(MeasuredSampler::NotTracking)
        );
        let seg = state
            .poll_send_with_default_options(clock.now(), &counters.refs())
            .expect("generate segment");
        assert_eq!(seg.header().seq, TEST_ISS + 1);
        assert_eq!(seg.len(), u32::try_from(TEST_BYTES.len()).unwrap());
        let expect_sampler = SamplingStrategy::Measured(MeasuredSampler::Tracking {
            range: (TEST_ISS + 1)..(TEST_ISS + 1 + seg.len()),
            timestamp: clock.now(),
        });
        assert_eq!(state.assert_established().snd.rtt_sampler, expect_sampler);
        // Send more data that is present in the buffer.
        clock.sleep(CLOCK_STEP);
        let seg = state
            .poll_send_with_default_options(clock.now(), &counters.refs())
            .expect("generate segment");
        assert_eq!(seg.header().seq, TEST_ISS + 1 + TEST_BYTES.len());
        assert_eq!(seg.len(), u32::try_from(TEST_BYTES.len()).unwrap());
        // The probe for RTT doesn't change.
        let established = state.assert_established();
        assert_eq!(established.snd.rtt_sampler, expect_sampler);

        // No RTT estimation yet.
        assert_eq!(established.snd.rtt_estimator.srtt(), None);

        let (retransmit, ack_number) = match scenario {
            RttTestScenario::AckPartial => (false, TEST_ISS + 1 + 1),
            RttTestScenario::AckOne => (false, TEST_ISS + 1 + TEST_BYTES.len()),
            RttTestScenario::AckTwo => (false, TEST_ISS + 1 + TEST_BYTES.len() * 2),
            RttTestScenario::Retransmit => (true, TEST_ISS + 1 + TEST_BYTES.len() * 2),
        };

        if retransmit {
            // No ack, retransmit should clear things.
            let timeout = state.poll_send_at().expect("timeout should be present");
            clock.time = timeout;
            let seg = state
                .poll_send_with_default_options(clock.now(), &counters.refs())
                .expect("generate segment");
            // First segment is retransmitted.
            assert_eq!(seg.header().seq, TEST_ISS + 1);
            // Retransmit should've cleared the mark.
            assert_eq!(
                state.assert_established().snd.rtt_sampler,
                SamplingStrategy::Measured(MeasuredSampler::NotTracking)
            );
        } else {
            clock.sleep(CLOCK_STEP);
        }

        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + 1,
                    ack_number,
                    WindowSize::DEFAULT >> WindowScale::default(),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs()
            ),
            (None, None)
        );
        let established = state.assert_established();
        // No outstanding RTT estimate remains.
        assert_eq!(
            established.snd.rtt_sampler,
            SamplingStrategy::Measured(MeasuredSampler::NotTracking)
        );
        if retransmit {
            // No estimation was generated on retransmission.
            assert_eq!(established.snd.rtt_estimator.srtt(), None);
        } else {
            // Receiving a segment that acks any of the sent data should
            // generate an RTT calculation.
            assert_eq!(established.snd.rtt_estimator.srtt(), Some(CLOCK_STEP * 2));
        }

        clock.sleep(CLOCK_STEP);
        let seg = state
            .poll_send_with_default_options(clock.now(), &counters.refs())
            .expect("generate segment");
        let seq = seg.header().seq;
        assert_eq!(seq, TEST_ISS + 1 + TEST_BYTES.len() * 2);
        let expect_sampler = SamplingStrategy::Measured(MeasuredSampler::Tracking {
            range: seq..(seq + seg.len()),
            timestamp: clock.now(),
        });
        assert_eq!(state.assert_established().snd.rtt_sampler, expect_sampler);

        // Ack the second segment again now (which may be a new ACK in the one
        // ACK scenario), in all cases this should not move the target seq for
        // RTT.
        clock.sleep(CLOCK_STEP);
        assert_eq!(
            state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::ack(
                    TEST_IRS + 1,
                    TEST_ISS + 1 + TEST_BYTES.len() * 2,
                    WindowSize::DEFAULT >> WindowScale::default(),
                    DEFAULT_SEGMENT_OPTIONS
                ),
                clock.now(),
                &counters.refs()
            ),
            (None, None)
        );
        assert_eq!(state.assert_established().snd.rtt_sampler, expect_sampler);
    }

    #[test]
    fn loss_recovery_skips_nagle() {
        let mut buffer = RingBuffer::default();
        let payload = "Hello World".as_bytes();
        assert_eq!(buffer.enqueue_data(payload), payload.len());
        let mut state = State::<_, _, _, ()>::Established(Established {
            snd: Send::default_for_test(buffer).into(),
            rcv: Recv::default_for_test(RingBuffer::default()).into(),
        });

        let socket_options =
            SocketOptions { nagle_enabled: true, ..SocketOptions::default_for_state_tests() };
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let seg = state
            .poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options,
            )
            .expect("should not close");
        assert_eq!(seg.len(), u32::try_from(payload.len()).unwrap());
        // Receive duplicate acks repeatedly before this payload until we hit
        // fast retransmit.
        let ack = Segment::ack(
            TEST_IRS + 1,
            seg.header().seq,
            WindowSize::DEFAULT >> WindowScale::default(),
            DEFAULT_SEGMENT_OPTIONS,
        );

        let mut dup_acks = 0;
        let seg = loop {
            let (seg, passive_open, data_acked, newly_closed) = state
                .on_segment::<(), ClientlessBufferProvider>(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    ack.clone(),
                    clock.now(),
                    &socket_options,
                    false,
                );
            assert_eq!(seg, None);
            assert_eq!(passive_open, None);
            assert_eq!(data_acked, DataAcked::No);
            assert_eq!(newly_closed, NewlyClosed::No);
            dup_acks += 1;

            match state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options,
            ) {
                Ok(seg) => break seg,
                Err(newly_closed) => {
                    assert_eq!(newly_closed, NewlyClosed::No);
                    assert!(
                        dup_acks < DUP_ACK_THRESHOLD,
                        "failed to retransmit after {dup_acks} dup acks"
                    );
                }
            }
        };
        assert_eq!(seg.len(), u32::try_from(payload.len()).unwrap());
        assert_eq!(seg.header().seq, ack.header().ack.unwrap());
    }

    #[test]
    fn sack_recovery_rearms_rto() {
        let mss = EffectiveMss::from_mss(
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            MssSizeLimiters { timestamp_enabled: true },
        );
        let una = TEST_ISS + 1;
        let nxt = una + (u32::from(DUP_ACK_THRESHOLD) + 1) * u32::from(mss);

        let mut congestion_control = CongestionControl::cubic_with_mss(mss);
        // Inflate the congestion window so we won't hit congestion during test.
        congestion_control.inflate_cwnd(20 * u32::from(mss));
        let mut state = State::<_, _, _, ()>::Established(Established {
            snd: Send {
                nxt,
                max: nxt,
                una,
                wl1: TEST_IRS + 1,
                wl2: una,
                congestion_control,
                ..Send::default_for_test(InfiniteSendBuffer::default())
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::default()).into(),
        });

        let socket_options = SocketOptions::default_for_state_tests();
        const RTT: Duration = Duration::from_millis(1);
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        // Send a single segment, we should've set RTO.
        let seg = state
            .poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options,
            )
            .expect("should not close");
        assert_eq!(seg.len(), u32::from(mss));
        let start_rto = assert_matches!(
            state.assert_established().snd.timer,
            Some(SendTimer::Retrans(RetransTimer{at, ..})) => at
        );
        clock.sleep(RTT);

        // Receive an ACK with SACK blocks that put us in SACK recovery.
        let ack = Segment::ack(
            TEST_IRS + 1,
            una,
            WindowSize::DEFAULT >> WindowScale::default(),
            SegmentOptions {
                sack_blocks: [SackBlock::try_new(
                    nxt - u32::from(DUP_ACK_THRESHOLD) * u32::from(mss),
                    nxt,
                )
                .unwrap()]
                .into_iter()
                .collect(),
                timestamp: Some(DEFAULT_ACK_TS_OPT),
            },
        );
        let (seg, passive_open, data_acked, newly_closed) = state
            .on_segment::<(), ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                ack,
                clock.now(),
                &socket_options,
                false,
            );
        assert_eq!(seg, None);
        assert_eq!(passive_open, None);
        assert_eq!(data_acked, DataAcked::No);
        assert_eq!(newly_closed, NewlyClosed::No);
        assert_eq!(
            state.assert_established().snd.congestion_control.inspect_loss_recovery_mode(),
            Some(LossRecoveryMode::SackRecovery)
        );

        let seg = state
            .poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options,
            )
            .expect("should not close");
        assert_eq!(seg.len(), u32::from(mss));
        // SACK retransmission.
        assert_eq!(seg.header().seq, una);
        // RTO is rearmed.
        let new_rto = assert_matches!(
            state.assert_established().snd.timer,
            Some(SendTimer::Retrans(RetransTimer { at, .. })) => at
        );
        assert!(new_rto > start_rto, "{new_rto:?} > {start_rto:?}");

        CounterExpectations {
            retransmits: 1,
            sack_recovery: 1,
            sack_retransmits: 1,
            dup_acks: 1,
            ..Default::default()
        }
        .assert_counters(&counters);
    }

    // Define an enum here so test case name generation with test matrix works
    // better.
    enum SackPermitted {
        Yes,
        No,
    }

    // Test a connection that is limited only by a theoretical congestion
    // window, provided in multiples of MSS.
    //
    // This is a bit large for a unit test, but the set up to prove that traffic
    // is only limited by the congestion window is not achievable when the
    // entire stack is hooked up. Namely this test is proving 2 things:
    //
    // - When provided with infinite bytes to send and a receiver with an
    //   impossibly large window, a TCP sender still paces the connection based
    //   on the congestion window estimate. It is very hard to provide an
    //   infinite data source and boundless receiver in a full integration test.
    // - After sending enough bytes and going through enough congestion events,
    //   the TCP sender has an _acceptable_ estimate of what the congestion
    //   window is.
    #[test_matrix(
        [1, 2, 3, 5, 20, 50],
        [SackPermitted::Yes, SackPermitted::No]
    )]
    fn congestion_window_limiting(theoretical_window: u32, sack_permitted: SackPermitted) {
        netstack3_base::testutil::set_logger_for_test();

        let mss = EffectiveMss::from_mss(
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            MssSizeLimiters { timestamp_enabled: true },
        );
        let generate_sack = match sack_permitted {
            SackPermitted::Yes => true,
            SackPermitted::No => false,
        };

        // The test argument is the theoretical window in terms of multiples
        // of mss.
        let theoretical_window = theoretical_window * u32::from(mss);

        // Ensure we're not going to ever be blocked by the receiver window.
        let snd_wnd = WindowSize::MAX;
        let wnd_scale = snd_wnd.scale();
        // Instantiate with a buffer that always reports it has data to send.
        let buffer = InfiniteSendBuffer::default();

        let mut state = State::<FakeInstant, _, _, ()>::Established(Established {
            snd: Send {
                wnd: snd_wnd,
                wnd_max: snd_wnd,
                wnd_scale,
                congestion_control: CongestionControl::cubic_with_mss(mss),
                ..Send::default_for_test(buffer)
            }
            .into(),
            rcv: Recv { mss, ..Recv::default_for_test(RingBuffer::default()) }.into(),
        });

        let socket_options = SocketOptions::default_for_state_tests();
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();

        assert!(state.assert_established().snd.congestion_control.in_slow_start());

        let poll_until_empty =
            |state: &mut State<_, _, _, _>, segments: &mut Vec<(SeqNum, u32)>, now| loop {
                match state.poll_send(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    now,
                    &socket_options,
                ) {
                    Ok(seg) => {
                        // All sent segments controlled only by congestion
                        // window should send MSS.
                        assert_eq!(seg.len(), u32::from(mss));
                        segments.push((seg.header().seq, seg.len()));
                    }
                    Err(closed) => {
                        assert_eq!(closed, NewlyClosed::No);
                        break;
                    }
                }
            };

        let mut pending_segments = Vec::new();
        let mut pending_acks = Vec::new();
        let mut receiver = Assembler::new(TEST_ISS + 1);
        let mut total_sent = 0;
        let mut total_sent_rounds = 0;

        // Ensure the test doesn't run for too long.
        let mut loops = 1000;
        let mut continue_running = |state: &mut State<_, _, _, _>| {
            loops -= 1;
            assert!(loops > 0, "test seems to have stalled");

            // Run the connection until we have crossed a number of interesting
            // congestion events.
            const CONGESTION_EVENTS: u64 = 20;
            let event_count =
                counters.stack_wide.timeouts.get() + counters.stack_wide.loss_recovered.get();
            let congestion_control = &state.assert_established().snd.congestion_control;
            event_count <= CONGESTION_EVENTS
                // Continue running until we've left loss recovery or slow
                // start.
                || congestion_control.inspect_loss_recovery_mode().is_some() || congestion_control.in_slow_start()
        };

        while continue_running(&mut state) {
            // Use a somewhat large RTT. When SACK recovery is in use, the
            // infinite amount of available data combined with the RTO rearms on
            // retransmission causes it to always have more data to send, so the
            // RTT is effectively what's driving the clock in that case.
            clock.sleep(Duration::from_millis(10));
            if pending_acks.is_empty() {
                poll_until_empty(&mut state, &mut pending_segments, clock.now());
            } else {
                for (ack, sack_blocks) in pending_acks.drain(..) {
                    let seg: Segment<()> = Segment::ack(
                        TEST_IRS + 1,
                        ack,
                        snd_wnd >> wnd_scale,
                        SegmentOptions { sack_blocks, timestamp: Some(DEFAULT_ACK_TS_OPT) },
                    );
                    let (seg, passive_open, data_acked, newly_closed) = state
                        .on_segment::<_, ClientlessBufferProvider>(
                        &FakeStateMachineDebugId::default(),
                        &counters.refs(),
                        seg,
                        clock.now(),
                        &socket_options,
                        false,
                    );

                    assert_eq!(seg, None);
                    assert_eq!(passive_open, None);
                    assert_eq!(newly_closed, NewlyClosed::No);
                    // When we hit the congestion window duplicate acks will
                    // start to be sent out so we can't really assert on
                    // data_acked.
                    let _: DataAcked = data_acked;

                    poll_until_empty(&mut state, &mut pending_segments, clock.now());
                }
            }

            let established = state.assert_established();
            let congestion_control = &established.snd.congestion_control;
            let ssthresh = congestion_control.slow_start_threshold();
            let cwnd = congestion_control.inspect_cwnd().cwnd();
            let in_slow_start = congestion_control.in_slow_start();
            let in_loss_recovery = congestion_control.inspect_loss_recovery_mode().is_some();
            let pipe = congestion_control.pipe();
            let sent = u32::try_from(pending_segments.len()).unwrap() * u32::from(mss);
            let recovery_counters = if generate_sack {
                (
                    counters.stack_wide.sack_retransmits.get(),
                    counters.stack_wide.sack_recovery.get(),
                )
            } else {
                (
                    counters.stack_wide.fast_retransmits.get(),
                    counters.stack_wide.fast_recovery.get(),
                )
            };

            if !in_slow_start {
                total_sent += sent;
                total_sent_rounds += 1;
            }

            // This test is a bit hard to debug when things go wrong, so emit
            // the steps in logs.
            log::debug!(
                "ssthresh={ssthresh}, \
                    cwnd={cwnd}, \
                    sent={sent}, \
                    pipe={pipe}, \
                    in_slow_start={in_slow_start}, \
                    in_loss_recovery={in_loss_recovery}, \
                    total_retransmits={}, \
                    (retransmits,recovery)={:?}",
                counters.stack_wide.retransmits.get(),
                recovery_counters,
            );

            if pending_segments.is_empty() {
                assert_matches!(established.snd.timer, Some(SendTimer::Retrans(_)));
                // Nothing to do. Move the clock to hit RTO.
                clock.sleep_until(state.poll_send_at().expect("must have timeout"));
                log::debug!("RTO");
                continue;
            }

            let mut available = theoretical_window;
            for (seq, len) in pending_segments.drain(..) {
                // Drop segments that don't fit the congestion window.
                if available < len {
                    break;
                }
                available -= len;
                // Generate all the acks.
                if seq.after_or_eq(receiver.nxt()) {
                    let _: usize = receiver.insert(seq..(seq + len));
                }
                let sack_blocks = if generate_sack {
                    receiver.sack_blocks(SackBlockSizeLimiters { timestamp_enabled: true })
                } else {
                    SackBlocks::default()
                };
                pending_acks.push((receiver.nxt(), sack_blocks));
            }
        }

        // The effective average utilization of the congestion window at every
        // RTT, not including the slow start periods.
        let avg_sent = total_sent / total_sent_rounds;
        // Define the tolerance as a range based on the theoretical window. This
        // is simply the tightest round integer value that we could find that
        // makes the test pass. Given we're not closely tracking how many
        // congestion events happening the estimate varies a lot, even with a
        // big average over the non slow-start periods.
        let tolerance = (theoretical_window / 3).max(u32::from(mss));
        let low_range = theoretical_window - tolerance;
        let high_range = theoretical_window + tolerance;
        assert!(
            avg_sent >= low_range && avg_sent <= high_range,
            "{low_range} <= {avg_sent} <= {high_range}"
        );
    }

    // Tests that out of order ACKs including SACK blocks are discarded. This is
    // a regression test for https://fxbug.dev/409599338.
    #[test]
    fn out_of_order_ack_with_sack_blocks() {
        let send_segments = u32::from(DUP_ACK_THRESHOLD + 2);
        let mss = EffectiveMss::from_mss(
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            MssSizeLimiters { timestamp_enabled: true },
        );

        let send_bytes = send_segments * u32::from(mss);
        // Ensure we're not going to ever be blocked by the receiver window.
        let snd_wnd = WindowSize::from_u32(send_bytes).unwrap();
        let wnd_scale = snd_wnd.scale();
        let mut congestion_control = CongestionControl::cubic_with_mss(mss);
        congestion_control.inflate_cwnd(snd_wnd.into());

        let start = TEST_ISS + 1;

        let mut state = State::<FakeInstant, _, _, ()>::Established(Established {
            snd: Send {
                congestion_control,
                wnd_scale,
                wnd: snd_wnd,
                wnd_max: snd_wnd,
                ..Send::default_for_test_at(
                    start,
                    RepeatingSendBuffer::new(usize::try_from(send_bytes).unwrap()),
                )
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::default()).into(),
        });
        let socket_options = SocketOptions::default_for_state_tests();
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let sent = core::iter::from_fn(|| {
            match state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options,
            ) {
                Ok(seg) => Some(seg.len()),
                Err(newly_closed) => {
                    assert_eq!(newly_closed, NewlyClosed::No);
                    None
                }
            }
        })
        .sum::<u32>();
        assert_eq!(sent, send_segments * u32::from(mss));

        // First receive a cumulative ACK for the entire transfer.
        let end = state.assert_established().snd.nxt;
        let seg =
            Segment::<()>::ack(TEST_IRS + 1, end, snd_wnd >> wnd_scale, DEFAULT_SEGMENT_OPTIONS);
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                seg,
                clock.now(),
                &socket_options,
                false
            ),
            (None, None, DataAcked::Yes, NewlyClosed::No)
        );
        assert_eq!(state.assert_established().snd.congestion_control.pipe(), 0);
        assert_matches!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options
            ),
            Err(NewlyClosed::No)
        );

        // Then receive an out of order ACK that only acknowledges the
        // DUP_ACK_THRESHOLD segments after the first one.
        let sack_block = SackBlock::try_new(
            start + u32::from(mss),
            start + u32::from(DUP_ACK_THRESHOLD + 1) * u32::from(mss),
        )
        .unwrap();
        assert!(sack_block.right().before(end));
        let seg = Segment::<()>::ack(
            TEST_IRS + 1,
            start,
            snd_wnd >> wnd_scale,
            SegmentOptions {
                sack_blocks: [sack_block].into_iter().collect(),
                timestamp: Some(DEFAULT_ACK_TS_OPT),
            },
        );
        assert_eq!(
            state.on_segment::<_, ClientlessBufferProvider>(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                seg,
                clock.now(),
                &socket_options,
                false
            ),
            (None, None, DataAcked::No, NewlyClosed::No)
        );
        // The out of order ACK should not be considered in the scoreboard for
        // bytes in flight and we should not be in recovery mode.
        let congestion_control = &state.assert_established().snd.congestion_control;
        assert_eq!(congestion_control.pipe(), 0);
        assert_eq!(congestion_control.inspect_loss_recovery_mode(), None);
        // No more segments should be generated.
        assert_matches!(
            state.poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &socket_options
            ),
            Err(NewlyClosed::No)
        );
    }

    #[test]
    fn push_segments() {
        let send_segments = 16;
        let mss = EffectiveMss::from_mss(
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            MssSizeLimiters { timestamp_enabled: true },
        );
        let send_bytes = send_segments * usize::from(mss);
        // Use a receiver window that can take 4 Mss at a time, so we should set
        // the PSH bit every 2 MSS.
        let snd_wnd = WindowSize::from_u32(4 * u32::from(mss)).unwrap();
        let wnd_scale = snd_wnd.scale();
        let mut state = State::<FakeInstant, _, _, ()>::Established(Established {
            snd: Send {
                congestion_control: CongestionControl::cubic_with_mss(mss),
                wnd_scale,
                wnd: snd_wnd,
                wnd_max: snd_wnd,
                ..Send::default_for_test(RepeatingSendBuffer::new(send_bytes))
            }
            .into(),
            rcv: Recv::default_for_test(RingBuffer::default()).into(),
        });
        let socket_options = SocketOptions::default_for_state_tests();
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();

        for i in 0..send_segments {
            let seg = state
                .poll_send(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    clock.now(),
                    &socket_options,
                )
                .expect("produces segment");
            let is_last = i == (send_segments - 1);
            // Given the we set the max window to 4 MSS, SND.NXT must advance at
            // least 2 MSS for us to set the PSH bit, which means it's observed
            // every 2nd segment generated after the first one.
            let is_periodic = i != 0 && i % 2 == 0;
            // Ensure that our math is correct and we're checking both
            // conditions with this test.
            assert!(!(is_last && is_periodic));
            assert_eq!(seg.header().push, is_last || is_periodic, "at {i}");
            let ack = Segment::ack(
                TEST_IRS + 1,
                seg.header().seq + seg.len(),
                snd_wnd >> wnd_scale,
                DEFAULT_SEGMENT_OPTIONS,
            );
            assert_eq!(
                state.on_segment::<(), ClientlessBufferProvider>(
                    &FakeStateMachineDebugId::default(),
                    &counters.refs(),
                    ack,
                    clock.now(),
                    &socket_options,
                    false
                ),
                (None, None, DataAcked::Yes, NewlyClosed::No)
            );
        }
    }

    #[test]
    fn connect_with_timestamp_option() {
        let mut alice_clock = FakeInstantCtx::default();
        let mut bob_clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();

        // Use different timestamp offsets on both sides to make distinguishing
        // the two sets of timestamps easier.
        const ALICE_OFFSET: Timestamp<Milliseconds> = Timestamp::new(1000);
        const BOB_OFFSET: Timestamp<Milliseconds> = Timestamp::new(2000);
        let alice_time1 = (ALICE_OFFSET + alice_clock.now().offset).discard_unit();
        let bob_time1 = (BOB_OFFSET + bob_clock.now().offset).discard_unit();
        assert_ne!(alice_time1, bob_time1);

        // Call connect on Alice's side and expect a SYN with an appropriate
        // timestamp option.
        let (alice_state, syn_seg) = Closed::<Initial>::connect(
            ISS_1,
            ALICE_OFFSET,
            alice_clock.now(),
            (),
            Default::default(),
            *TEST_MSS.mss(),
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        assert_eq!(syn_seg.header().control, Some(Control::SYN));
        assert_eq!(syn_seg.header().ack, None);
        assert_eq!(
            *syn_seg.header().options.timestamp(),
            Some(TimestampOption::new(alice_time1, TS_ECHO_REPLY_FOR_NON_ACKS))
        );
        let mut alice_state = State::<_, RingBuffer, RingBuffer, _>::SynSent(alice_state);

        // Call listen on Bob's side and expect a SYN-ACK in response to Alice's
        // initial SYN. The SYN-ACK's timestamp should echo back Alice's
        // timestamp.
        let mut bob_state =
            State::<_, RingBuffer, RingBuffer, _>::Listen(Closed::<Initial>::listen(
                ISS_2,
                BOB_OFFSET,
                Default::default(),
                DEVICE_MAXIMUM_SEGMENT_SIZE,
                Mss::DEFAULT_IPV4,
                None,
            ));
        let syn_ack_seg = assert_matches!(
            bob_state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                syn_seg,
                bob_clock.now(),
                &counters.refs(),
            ),
            (Some(syn_ack_seg), _) => syn_ack_seg
        );
        assert_eq!(syn_ack_seg.header().control, Some(Control::SYN));
        assert_eq!(syn_ack_seg.header().ack, Some(ISS_1 + 1));
        assert_eq!(
            *syn_ack_seg.header().options.timestamp(),
            Some(TimestampOption::new(bob_time1, alice_time1))
        );

        // Receive the SYN-ACK on Alice's side and expect an ACK in response. It
        // should echo back Bob's timestamp.
        let ack_seg = assert_matches!(
            alice_state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                syn_ack_seg,
                alice_clock.now(),
                &counters.refs(),
            ),
            (Some(ack_seg), _) => ack_seg
        );
        assert_eq!(ack_seg.header().control, None);
        assert_eq!(ack_seg.header().ack, Some(ISS_2 + 1));
        assert_eq!(
            *ack_seg.header().options.timestamp(),
            Some(TimestampOption::new(alice_time1, bob_time1))
        );
        assert_matches!(
            bob_state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                ack_seg,
                bob_clock.now(),
                &counters.refs(),
            ),
            (None, _)
        );

        // Advance time so that both sides update timestamps.
        // Note: The timestamp option doesn't place any requirements on both
        // sides having a synchronized view of time. Advance the two clocks
        // differently to exercise this allowed asymmetry.
        alice_clock.sleep(RTT);
        bob_clock.sleep(2 * RTT);

        let alice_time2 = (ALICE_OFFSET + alice_clock.now().offset).discard_unit();
        let bob_time2 = (BOB_OFFSET + bob_clock.now().offset).discard_unit();
        assert_ne!(alice_time2, bob_time2);

        // Send data from Alice's side. The timestamp option should include
        // Alice's new time and Bob's old time.
        assert_eq!(
            alice_state
                .buffers_mut()
                .into_send_buffer()
                .expect("should have a send buffer")
                .enqueue_data(TEST_BYTES),
            TEST_BYTES.len()
        );
        let data_seg = alice_state
            .poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                alice_clock.now(),
                &SocketOptions::default_for_state_tests(),
            )
            .expect("poll_send should succeed");
        assert_eq!(data_seg.header().control, None);
        assert_eq!(data_seg.header().ack, Some(ISS_2 + 1));
        assert_eq!(
            *data_seg.header().options.timestamp(),
            Some(TimestampOption::new(alice_time2, bob_time1))
        );

        // Upon receiving Alice's data, Bob should respond with an ACK. The
        // timestamp option should include Bob's new time and echo back Alice's
        // new time.
        let ack_seg = assert_matches!(
            bob_state.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                data_seg,
                bob_clock.now(),
                &counters.refs(),
            ),
            (Some(ack_seg), _) => ack_seg
        );
        assert_eq!(ack_seg.header().control, None);
        assert_eq!(ack_seg.header().ack, Some(ISS_1 + TEST_BYTES.len() + 1));
        assert_eq!(
            *ack_seg.header().options.timestamp(),
            Some(TimestampOption::new(bob_time2, alice_time2))
        );
        assert_matches!(
            alice_state.on_segment_with_default_options::<(), ClientlessBufferProvider>(
                ack_seg,
                alice_clock.now(),
                &counters.refs(),
            ),
            (None, _)
        );
    }

    #[test]
    fn connect_without_timestamp_option() {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let (state, seg) = Closed::<Initial>::connect(
            TEST_ISS,
            TIMESTAMP_OFFSET,
            clock.now(),
            (),
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        // Expect to have sent a SYN advertising support for timestamps.
        assert_eq!(
            seg,
            Segment::syn(
                TEST_ISS,
                UnscaledWindowSize::from(u16::MAX),
                HandshakeOptions {
                    mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                    window_scale: Some(WindowScale::default()),
                    sack_permitted: SACK_PERMITTED,
                    timestamp: Some(DEFAULT_NON_ACK_TS_OPT),
                },
            )
        );

        let mut state = State::SynSent::<_, RingBuffer, RingBuffer, _>(state);

        // Simulate a SYN-ACK from a peer that does not support timestamps.
        // Expect an ACK to be generated that omits the timestamp.
        let (seg, passive_open) = state
            .on_segment_with_default_options::<(), ClientlessBufferProvider>(
                Segment::syn_ack(
                    TEST_IRS,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(u16::MAX),
                    HandshakeOptions {
                        mss: Some(DEVICE_MAXIMUM_SEGMENT_SIZE),
                        window_scale: Some(WindowScale::default()),
                        sack_permitted: SACK_PERMITTED,
                        timestamp: None,
                    },
                ),
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        assert_eq!(
            seg,
            Some(Segment::ack(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from(u16::MAX),
                SegmentOptions { sack_blocks: SackBlocks::EMPTY, timestamp: None }
            ))
        );

        // The connection should now be established, with timestamps disabled.
        let (wl1, wl2, ts_opt, mss) = assert_matches!(
            &state,
            State::Established(Established {
                snd: Takeable(Some(Send { wl1, wl2, .. })),
                rcv: Takeable(Some(Recv { ts_opt, mss, ..})),
            }) => (wl1, wl2, ts_opt, mss)
        );
        assert_eq!(*wl1, TEST_IRS);
        assert_eq!(*wl2, TEST_ISS + 1);
        assert_eq!(*ts_opt, TimestampOptionState::Disabled);
        // NB: The EffectiveMss should account for the lack of Timestamps.
        assert_eq!(
            *mss,
            EffectiveMss::from_mss(
                DEVICE_MAXIMUM_SEGMENT_SIZE,
                MssSizeLimiters { timestamp_enabled: false }
            )
        );
    }

    #[test]
    fn segments_with_missing_timestamp_option() {
        let clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        // An established connection that has negotiated the use of timestamps.
        let mut established = State::Established(Established {
            snd: Send::default_for_test(NullBuffer).into(),
            rcv: Recv {
                ts_opt: default_ts_opt_state(TEST_IRS + 1),
                ..Recv::default_for_test(RingBuffer::new(BUFFER_SIZE))
            }
            .into(),
        });

        // Receive a segment that lacks the timestamp option. Although the RFC
        // states we SHOULD drop such segments, we ignore the RFC recommendation
        // for conformance with Linux. Expect the segment to be processed.
        let (seg, passive_open) = established
            .on_segment_with_default_options::<_, ClientlessBufferProvider>(
                Segment::with_data(
                    TEST_IRS + 1,
                    TEST_ISS + 1,
                    UnscaledWindowSize::from(0),
                    SegmentOptions { sack_blocks: SackBlocks::EMPTY, timestamp: None },
                    TEST_BYTES,
                ),
                clock.now(),
                &counters.refs(),
            );
        assert_eq!(passive_open, None);
        assert_eq!(
            seg,
            Some(Segment::ack(
                TEST_ISS + 1,
                TEST_IRS + 1 + TEST_BYTES.len(),
                UnscaledWindowSize::from((BUFFER_SIZE - TEST_BYTES.len()) as u16),
                DEFAULT_SEGMENT_OPTIONS,
            )),
        );
        assert_eq!(
            established.read_with(|available| {
                assert_eq!(available, &[TEST_BYTES]);
                available[0].len()
            }),
            TEST_BYTES.len()
        );
    }

    #[test]
    fn last_data_sent_update() {
        let get_last_data_sent = |state: &State<_, _, _, _>| {
            assert_matches!(state, State::Established(Established {
                snd: Takeable(Some(Send { last_data_sent, .. })),
                ..
            }) => *last_data_sent)
        };

        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();
        let start_time = clock.now();

        let mut connection = State::Established(Established {
            snd: Send::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
            rcv: Recv::default_for_test(RingBuffer::new(BUFFER_SIZE)).into(),
        });
        assert_eq!(get_last_data_sent(&connection), None);

        // A plain ACK shouldn't update last_data_sent...
        let (_seg, _) = connection.on_segment_with_default_options::<_, ClientlessBufferProvider>(
            Segment::<()>::ack(
                TEST_ISS + 1,
                TEST_IRS + 1,
                UnscaledWindowSize::from(1000u16),
                DEFAULT_SEGMENT_OPTIONS,
            ),
            clock.now(),
            &counters.refs(),
        );
        assert_eq!(get_last_data_sent(&connection), None);

        // ... But a real send will.
        assert_eq!(
            connection
                .buffers_mut()
                .into_send_buffer()
                .expect("should have a send buffer")
                .enqueue_data(TEST_BYTES),
            TEST_BYTES.len()
        );
        let _seg = connection
            .poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            )
            .expect("should send segment");

        let first_send_time = get_last_data_sent(&connection);
        assert_eq!(first_send_time, Some(start_time));

        // Advance time and send again.
        clock.sleep(Duration::from_secs(1));
        let time_after_sleep = clock.now();

        assert_eq!(
            connection
                .buffers_mut()
                .into_send_buffer()
                .expect("should have a send buffer")
                .enqueue_data(TEST_BYTES),
            TEST_BYTES.len()
        );
        let _seg = connection
            .poll_send(
                &FakeStateMachineDebugId::default(),
                &counters.refs(),
                clock.now(),
                &SocketOptions::default_for_state_tests(),
            )
            .expect("should send segment");

        let second_send_time = get_last_data_sent(&connection);
        assert_eq!(second_send_time, Some(time_after_sleep));
    }

    // Verify that a connection using the RTT `TimestampSampler` adheres to the
    // RTM rule. Per RFC 7323, section 4.2:
    //     RTTM Rule: A TSecr value received in a segment MAY be used to update
    //     the averaged RTT measurement only if the segment advances the left
    //     edge of the send window, i.e., SND.UNA is increased.
    #[test]
    fn rttm_rule() {
        let mut clock = FakeInstantCtx::default();
        let counters = FakeTcpCounters::default();

        // Complete the Handshake.
        let (syn_sent, syn_seg) = Closed::<Initial>::connect(
            ISS_1,
            TIMESTAMP_OFFSET,
            clock.now(),
            (),
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            &SocketOptions::default_for_state_tests(),
        );
        let mut alice = State::<_, RingBuffer, RingBuffer, _>::SynSent(syn_sent);
        let mut bob = State::<_, RingBuffer, RingBuffer, _>::Listen(Closed::<Initial>::listen(
            ISS_2,
            TIMESTAMP_OFFSET,
            Default::default(),
            DEVICE_MAXIMUM_SEGMENT_SIZE,
            Mss::DEFAULT_IPV4,
            None,
        ));
        clock.sleep(RTT / 2);
        let (seg, _) = bob.on_segment_with_default_options::<_, ClientlessBufferProvider>(
            syn_seg,
            clock.now(),
            &counters.refs(),
        );
        let syn_ack = seg.expect("failed to generate a syn-ack segment");
        clock.sleep(RTT / 2);
        let (seg, _) = alice.on_segment_with_default_options::<_, ClientlessBufferProvider>(
            syn_ack,
            clock.now(),
            &counters.refs(),
        );
        let ack_seg = seg.expect("failed to generate a ack segment");
        clock.sleep(RTT / 2);
        assert_eq!(
            bob.on_segment_with_default_options::<_, ClientlessBufferProvider>(
                ack_seg,
                clock.now(),
                &counters.refs(),
            ),
            (None, Some(())),
        );

        // Verify both are established and have RTT estimates.
        let get_rtt_estimate = |state: &State<_, _, _, _>| {
            let est = assert_matches!(state, State::Established(e) => e);
            assert_matches!(est.snd.rtt_estimator, Estimator::Measured { srtt, .. } => srtt)
        };
        let alice_first_estimate = get_rtt_estimate(&alice);
        let bob_first_estimate = get_rtt_estimate(&bob);
        assert_eq!(alice_first_estimate, RTT);
        assert_eq!(bob_first_estimate, RTT);

        // Change the RTT of the network, then send data from Alice to Bob.
        let new_rtt = RTT * 10;
        let data_to_send = &[1, 2, 3, 4];
        assert_eq!(
            alice.assert_established().snd.buffer.enqueue_data(data_to_send),
            data_to_send.len()
        );
        let data_seg = alice
            .poll_send_with_default_options(clock.now(), &counters.refs())
            .expect("expected segment to send");
        clock.sleep(new_rtt / 2);
        let (seg, _) = bob.on_segment_with_default_options::<_, ClientlessBufferProvider>(
            data_seg,
            clock.now(),
            &counters.refs(),
        );
        let ack = seg.expect("expected ACK from bob");
        clock.sleep(new_rtt / 2);
        let (seg, _) = alice.on_segment_with_default_options::<_, ClientlessBufferProvider>(
            ack.clone(),
            clock.now(),
            &counters.refs(),
        );
        assert_eq!(seg, None);

        // Verify Alices's RTT estimate is updated, while Bob's is unchanged.
        let alice_second_estimate = get_rtt_estimate(&alice);
        let bob_second_estimate = get_rtt_estimate(&bob);
        assert_ne!(alice_first_estimate, alice_second_estimate);
        assert_eq!(bob_first_estimate, bob_second_estimate);

        // Change the RTT again, and send the same ACK from Bob to Alice.
        // Alice's estimate should not be updated, since this ACK doesn't
        // advance SND.UNA.
        let new_rtt = new_rtt * 10;
        clock.sleep(new_rtt / 2);
        let (seg, _) = alice.on_segment_with_default_options::<_, ClientlessBufferProvider>(
            ack,
            clock.now(),
            &counters.refs(),
        );
        assert_eq!(seg, None);
        let alice_third_estimate = get_rtt_estimate(&alice);
        assert_eq!(alice_second_estimate, alice_third_estimate);
    }
}
