// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TCP Timestamp Option as defined in RFC 7323.

use core::time::Duration;

use netstack3_base::{
    Instant, Milliseconds, RxTimestampOption, SeqNum, Timestamp, TxTimestampOption, Unitless,
};

/// Whether the TCP timestamp option should be used.
///
/// Note this choice should be made only once for each TCP connection, at the
/// time of the handshake, and retained for the lifetime of the connection.
// TODO(https://fxbug.dev/436529062): Allow users to configure whether the
// timestamp option should be used. For now, assume it always enabled.
pub const IS_TS_OPT_LOCALLY_ENABLED: bool = true;

/// The echo reply value to include in the timestamp option for non-ACK.
///
/// Per RFC 7323 section 3.2:
///   The TSecr field is valid if the ACK bit is set in the TCP header.  If
///   the ACK bit is not set in the outgoing TCP header, the sender of that
///   segment SHOULD set the TSecr field to zero.
pub(super) const TS_ECHO_REPLY_FOR_NON_ACKS: Timestamp<Unitless> = Timestamp::new(0);

/// State used to calculate the `ts_val` to populate in timestamp options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TimestampValueState<I> {
    /// A randomized offset to apply to our timestamps.
    pub(super) offset: Timestamp<Milliseconds>,
    /// The time at which this connection was initialized.
    pub(super) initialized_at: I,
}

impl<I: Instant> TimestampValueState<I> {
    pub(super) fn ts_val(&self, now: I) -> Timestamp<Milliseconds> {
        let Self { offset, initialized_at } = self;
        *offset + now.checked_duration_since(*initialized_at).unwrap_or(Duration::ZERO)
    }
}

/// State held for a TCP connection that is in the process of negotiating
/// the timestamp option (e.g. is undergoing the TCP handshake).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TimestampOptionNegotiationState<I> {
    /// The timestamp option is not being negotiated for this connection.
    Disabled,
    /// The timestamp option is being negotiated for this connection.
    ///
    /// This connection is currently waiting for the peer to confirm that
    /// the timestamp option is supported.
    Negotiating(TimestampValueState<I>),
}

impl<I> TimestampOptionNegotiationState<I> {
    /// Constructs a new [`TimestampOptionNegotiationState`].
    pub(super) fn new(now: I, offset: Timestamp<Milliseconds>) -> Self {
        if IS_TS_OPT_LOCALLY_ENABLED {
            Self::Negotiating(TimestampValueState { offset, initialized_at: now })
        } else {
            Self::Disabled
        }
    }
}

impl<I: Instant> TimestampOptionNegotiationState<I> {
    /// Converts this state into the appropriate [`TimestampOption`] to send
    /// as part of a SYN segment.
    ///
    /// Note: Note this function is explicitly for SYNs (rather than SYN-ACKs),
    /// and as such, we populate `ts_echo_reply` in the resulting option with
    /// `TS_ECHO_REPLY_FOR_NON_ACKS` (i.e. zeroing it).
    pub(super) fn make_option_for_syn(&self, now: I) -> Option<TxTimestampOption> {
        match self {
            TimestampOptionNegotiationState::Disabled => None,
            TimestampOptionNegotiationState::Negotiating(ts_val) => Some(TxTimestampOption {
                ts_val: ts_val.ts_val(now),
                ts_echo_reply: TS_ECHO_REPLY_FOR_NON_ACKS,
            }),
        }
    }
}

/// State held for each TCP connection regarding the timestamp option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TimestampOptionState<I> {
    /// The timestamp option is not in use for this connection.
    Disabled,
    /// The timestamp option is in use for this connection.
    Enabled {
        /// A recent timestamp value received from the peer. This corresponds to
        /// `TS.Recent` as defined in RFC 7323 section 4.3.
        ts_recent: Timestamp<Unitless>,
        /// The most recent ACK we've sent. This corresponds to `Last.ACK.sent`
        /// as defined in RFC 7323 section 4.3.
        last_ack_sent: SeqNum,
        /// State used to calculate the `ts_val` field.
        ts_val: TimestampValueState<I>,
    },
}

impl<I> TimestampOptionState<I> {
    pub(super) fn is_enabled(&self) -> bool {
        match self {
            Self::Disabled => false,
            Self::Enabled { .. } => true,
        }
    }

    /// Negotiate whether the timestamp option should be used for a given
    /// connection, based on whether it's locally enabled, and whether the
    /// peer requested it.
    pub(super) fn negotiate(
        locally_enabled: TimestampOptionNegotiationState<I>,
        received_option: Option<RxTimestampOption>,
        initial_ack_sent: SeqNum,
    ) -> Self {
        match (locally_enabled, received_option) {
            // Disabled by us.
            (TimestampOptionNegotiationState::Disabled, _) => TimestampOptionState::Disabled,
            // Disabled by the peer.
            (_, None) => TimestampOptionState::Disabled,
            // Successfully negotiated.
            (
                TimestampOptionNegotiationState::Negotiating(our_ts_val),
                Some(RxTimestampOption { ts_val: their_ts_val, ts_echo_reply: _ }),
            ) => {
                // Per RFC 7323 section 3.2, a
                //   TSopt has been successfully negotiated, that is both <SYN>
                //   and <SYN,ACK> contain TSopt.
                TimestampOptionState::Enabled {
                    ts_recent: their_ts_val,
                    last_ack_sent: initial_ack_sent,
                    ts_val: our_ts_val,
                }
            }
        }
    }

    /// On reception of a new segment, update our internal cached `ts_recent`.
    pub(super) fn update_recent_timestamp(
        &mut self,
        seq: SeqNum,
        received_option: Option<RxTimestampOption>,
    ) {
        match (self, received_option) {
            // Either TS options are disabled, or the peer choose not to include
            // the option on this segment (i.e. it's a RST segment).
            (TimestampOptionState::Disabled, _) | (_, None) => {}
            (
                TimestampOptionState::Enabled { ts_recent, last_ack_sent, ts_val: _ },
                Some(RxTimestampOption { ts_val, ts_echo_reply: _ }),
            ) => {
                // Per RFC 7323 section 4.3:
                //   If SEG.TSval >= TS.Recent and SEG.SEQ <= Last.ACK.sent,
                //   then SEG.TSval is copied to TS.Recent; otherwise, it is
                //   ignored.
                if ts_val >= *ts_recent && seq.before_or_eq(*last_ack_sent) {
                    *ts_recent = ts_val;
                }
            }
        }
    }

    /// Updates internal state based on a sent ACK.
    ///
    /// Note: `seq` is the sequence number being ACKed, NOT the sequence
    /// number of the segment.
    pub(super) fn process_tx_ack(&mut self, ack: SeqNum) {
        match self {
            TimestampOptionState::Disabled => {}
            TimestampOptionState::Enabled { ts_recent: _, last_ack_sent, ts_val: _ } => {
                // Per RFC 7323 section 4.3:
                //   Last.ACK.sent holds the ACK field from the last segment
                //   sent. Last.ACK.sent will equal RCV.NXT except when <ACK>s
                //   have been delayed.
                *last_ack_sent = ack;
            }
        }
    }
}

impl<I: Instant> TimestampOptionState<I> {
    /// Constructs the [`TimestampOption`] to send as part of an ACK segment.
    pub(super) fn make_option_for_ack(&self, now: I) -> Option<TxTimestampOption> {
        match self {
            TimestampOptionState::Disabled => None,
            TimestampOptionState::Enabled { ts_recent, last_ack_sent: _, ts_val } => {
                let ts_echo_reply = *ts_recent;
                let ts_val = ts_val.ts_val(now);
                Some(TxTimestampOption { ts_val, ts_echo_reply })
            }
        }
    }

    /// Constructs the [`TimestampOption`] to send as part of a non-ACK segment.
    pub(super) fn make_option_for_non_ack(&self, now: I) -> Option<TxTimestampOption> {
        match self {
            TimestampOptionState::Disabled => None,
            TimestampOptionState::Enabled { ts_recent: _, last_ack_sent: _, ts_val } => {
                let ts_echo_reply = TS_ECHO_REPLY_FOR_NON_ACKS;
                let ts_val = ts_val.ts_val(now);
                Some(TxTimestampOption { ts_val, ts_echo_reply })
            }
        }
    }
}

#[cfg(test)]
mod test {
    use assert_matches::assert_matches;
    use test_case::test_case;

    use super::*;
    use netstack3_base::testutil::FakeInstant;

    #[test_case(Timestamp::new(0), Duration::ZERO, Duration::ZERO
        => Timestamp::new(0); "zero")]
    #[test_case(Timestamp::new(20), Duration::ZERO, Duration::ZERO
        => Timestamp::new(20); "offset")]
    #[test_case(Timestamp::new(0), Duration::from_millis(10), Duration::from_millis(15)
        => Timestamp::new(5); "elapsed_time")]
    #[test_case(Timestamp::new(20), Duration::from_millis(10), Duration::from_millis(15)
        => Timestamp::new(25); "offset_and_elapsed_time")]
    fn ts_val(
        offset: Timestamp<Milliseconds>,
        initialized_at: Duration,
        now: Duration,
    ) -> Timestamp<Milliseconds> {
        let initialized_at = FakeInstant::from(initialized_at);
        let now = FakeInstant::from(now);
        TimestampValueState { offset, initialized_at }.ts_val(now)
    }

    const INITIAL_ACK: SeqNum = SeqNum::new(1);
    const TS_VAL: TimestampValueState<FakeInstant> = TimestampValueState {
        offset: Timestamp::new(2),
        initialized_at: FakeInstant { offset: Duration::from_secs(3) },
    };
    const THEIR_TS_VAL: Timestamp<Unitless> = Timestamp::new(4);

    #[test_case(
        TimestampOptionNegotiationState::Disabled,
        None
        => TimestampOptionState::Disabled;
        "both_disabled"
    )]
    #[test_case(
        TimestampOptionNegotiationState::Disabled,
        Some(RxTimestampOption {ts_val: THEIR_TS_VAL, ts_echo_reply: Timestamp::new(0)})
        => TimestampOptionState::Disabled;
        "locally_disabled"
    )]
    #[test_case(
        TimestampOptionNegotiationState::Negotiating(TS_VAL),
        None
        => TimestampOptionState::Disabled;
        "remotely_disabled"
    )]
    #[test_case(
        TimestampOptionNegotiationState::Negotiating(TS_VAL),
        Some(RxTimestampOption {ts_val: THEIR_TS_VAL, ts_echo_reply: Timestamp::new(0)})
        => TimestampOptionState::Enabled {
            ts_recent: THEIR_TS_VAL,
            last_ack_sent: INITIAL_ACK,
            ts_val: TS_VAL
        };
        "enabled"
    )]
    fn negotiation(
        locally_enabled: TimestampOptionNegotiationState<FakeInstant>,
        received_option: Option<RxTimestampOption>,
    ) -> TimestampOptionState<FakeInstant> {
        TimestampOptionState::negotiate(locally_enabled, received_option, INITIAL_ACK)
    }

    #[test_case(SeqNum::new(1), SeqNum::new(2), Timestamp::new(1), Timestamp::new(2)
        => Timestamp::new(1); "ignore_update_if_seq_num_is_greater")]
    #[test_case(SeqNum::new(1), SeqNum::new(0), Timestamp::new(1), Timestamp::new(0)
        => Timestamp::new(1); "ignore_update_if_timestamp_is_earlier")]
    #[test_case(SeqNum::new(1), SeqNum::new(0), Timestamp::new(1), Timestamp::new(2)
        => Timestamp::new(2); "update_if_seq_num_is_less_than")]
    #[test_case(SeqNum::new(1), SeqNum::new(1), Timestamp::new(1), Timestamp::new(2)
        => Timestamp::new(2); "update_if_seq_num_is_equal")]
    fn update_ts_recent(
        last_ack_sent: SeqNum,
        new_seq: SeqNum,
        ts_recent: Timestamp<Unitless>,
        new_ts: Timestamp<Unitless>,
    ) -> Timestamp<Unitless> {
        let mut state = TimestampOptionState::Enabled { last_ack_sent, ts_recent, ts_val: TS_VAL };
        state.update_recent_timestamp(
            new_seq,
            Some(RxTimestampOption { ts_val: new_ts, ts_echo_reply: Timestamp::new(0) }),
        );
        assert_matches!(
            state,
            TimestampOptionState::Enabled { ts_recent, .. } => ts_recent
        )
    }

    #[test]
    fn make_option_when_disabled() {
        assert_eq!(
            TimestampOptionNegotiationState::Disabled.make_option_for_syn(FakeInstant::default()),
            None
        );
        assert_eq!(
            TimestampOptionState::Disabled.make_option_for_ack(FakeInstant::default()),
            None
        );
        assert_eq!(
            TimestampOptionState::Disabled.make_option_for_non_ack(FakeInstant::default()),
            None
        );
    }

    #[test]
    fn make_option_when_enabled() {
        let ts_val = TimestampValueState {
            offset: Timestamp::new(10),
            initialized_at: FakeInstant { offset: Duration::from_millis(5) },
        };
        let now = FakeInstant { offset: Duration::from_millis(10) };

        assert_eq!(
            TimestampOptionNegotiationState::Negotiating(ts_val.clone()).make_option_for_syn(now),
            Some(TxTimestampOption {
                ts_val: Timestamp::new(15),
                ts_echo_reply: TS_ECHO_REPLY_FOR_NON_ACKS
            })
        );

        let state = TimestampOptionState::Enabled {
            ts_recent: THEIR_TS_VAL,
            last_ack_sent: SeqNum::new(0),
            ts_val: ts_val,
        };
        assert_eq!(
            state.make_option_for_ack(now),
            Some(TxTimestampOption { ts_val: Timestamp::new(15), ts_echo_reply: THEIR_TS_VAL })
        );
        assert_eq!(
            state.make_option_for_non_ack(now),
            Some(TxTimestampOption {
                ts_val: Timestamp::new(15),
                ts_echo_reply: TS_ECHO_REPLY_FOR_NON_ACKS
            })
        );
    }
}
