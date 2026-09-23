// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! ResendSP Go-Back-N sliding-window transport state machines.
//!
//! Provides [`ResendSender`] and [`ResendReceiver`] to encapsulate sliding-window
//! sequence allocation, in-flight frame buffering, cumulative ACK processing,
//! round-trip time (RTT) tracking, and retransmission timeout recovery for
//! reliable transport over noisy or high-latency serial links.

use std::time::{Duration, Instant};

use crate::error::{FrameError, RetransmissionLimitExceeded, UnexpectedSeqError};
use crate::frame::{FrameType, encode_frame};
use crate::window::is_seq_in_window;

/// Maximum allowable sliding window size (half the 8-bit sequence space to avoid ambiguity).
pub const MAX_WINDOW_SIZE: u8 = 128;

/// Total 8-bit sequence space size.
const SEQUENCE_SPACE: usize = (u8::MAX as usize) + 1;

/// Default sliding window size for ResendSP (number of frames in flight).
pub const DEFAULT_WINDOW_SIZE: u8 = 64;

/// Default retransmission timeout duration for Go-Back-N recovery (1,000 ms).
pub const DEFAULT_RETRANSMISSION_TIMEOUT: Duration = Duration::from_millis(1000);

/// Maximum consecutive retransmission timeouts before the transport session fails.
pub const DEFAULT_MAX_RETRANSMISSION_ATTEMPTS: usize = 60;

/// Outcome of processing an acknowledgment sequence number in the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckOutcome {
    /// The ACK fell within the active window `[base, next_seq)` and advanced `base`.
    Advanced {
        /// The new base sequence number (one past the acknowledged sequence).
        new_base: u8,
        /// The round-trip time measured for the acknowledged frame, if available.
        rtt: Option<Duration>,
        /// Whether all outstanding frames in the window have now been acknowledged.
        all_acked: bool,
    },
    /// The ACK sequence fell outside the active window and was ignored.
    OutOfWindow {
        /// Current base sequence number.
        base: u8,
        /// Current next sequence number.
        next_seq: u8,
    },
}

#[derive(Debug, Clone)]
struct Slot {
    frame: Vec<u8>,
    tx_time: Instant,
    retransmitted: bool,
}

/// Go-Back-N sliding-window sender state machine.
///
/// Encapsulates sequence allocation, in-flight frame buffering, sliding-window
/// bounds checks, cumulative ACK processing, round-trip time measurement, and
/// timeout retransmission gathering.
#[derive(Debug)]
pub struct ResendSender {
    base: u8,
    next_seq: u8,
    window_size: u8,
    max_retransmission_attempts: usize,
    retransmission_attempts: usize,
    slots: [Option<Slot>; SEQUENCE_SPACE],
}

impl Default for ResendSender {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_SIZE, DEFAULT_MAX_RETRANSMISSION_ATTEMPTS)
    }
}

impl ResendSender {
    /// Creates a new sender state machine with the specified window size and retransmission limit.
    ///
    /// # Panics
    ///
    /// Panics if `window_size == 0` or `window_size > 128`, or if `max_retransmission_attempts == 0`.
    pub fn new(window_size: u8, max_retransmission_attempts: usize) -> Self {
        assert!(
            window_size > 0 && window_size <= MAX_WINDOW_SIZE,
            "window_size must be in range 1..=128, got {window_size}"
        );
        assert!(
            max_retransmission_attempts > 0,
            "max_retransmission_attempts must be greater than 0, got {max_retransmission_attempts}"
        );
        const INIT_SLOT: Option<Slot> = None;
        Self {
            base: 0,
            next_seq: 0,
            window_size,
            max_retransmission_attempts,
            retransmission_attempts: 0,
            slots: [INIT_SLOT; SEQUENCE_SPACE],
        }
    }

    /// Returns the current base sequence number (oldest unacknowledged frame).
    pub fn base(&self) -> u8 {
        self.base
    }

    /// Returns the next sequence number to be assigned.
    pub fn next_seq(&self) -> u8 {
        self.next_seq
    }

    /// Returns the number of frames currently in flight.
    pub fn in_flight(&self) -> u8 {
        self.next_seq.wrapping_sub(self.base)
    }

    /// Returns true if another frame can be transmitted within the sliding window.
    pub fn can_send(&self) -> bool {
        self.in_flight() < self.window_size
    }

    /// Returns true if there are no in-flight unacknowledged frames.
    pub fn is_idle(&self) -> bool {
        self.base == self.next_seq
    }

    /// Returns the number of consecutive retransmission attempts made for the current window.
    pub fn retransmission_attempts(&self) -> usize {
        self.retransmission_attempts
    }

    /// Enqueues and encodes a new frame to be transmitted.
    ///
    /// # Note
    /// This method is intended exclusively for sequenced outbound frames (such as
    /// [`FrameType::Data`]). Pure cumulative acknowledgments ([`FrameType::Ack`]) must
    /// not be enqueued into the sliding window; emit them directly using [`encode_frame`]
    /// or coordinate them via [`AckTracker`].
    ///
    /// # Errors
    /// Returns [`FrameError::WindowFull`] if `in_flight() >= window_size`.
    /// Returns [`FrameError::PayloadTooLarge`] if `payload.len() > MAX_PAYLOAD_SIZE`.
    pub fn enqueue_frame(
        &mut self,
        session_id: u32,
        channel_id: u16,
        frame_type: FrameType,
        payload: &[u8],
    ) -> Result<(u8, Vec<u8>), FrameError> {
        if !self.can_send() {
            return Err(FrameError::WindowFull {
                window_size: self.window_size,
                in_flight: self.in_flight(),
            });
        }
        let seq = self.next_seq;
        let wire_bytes = encode_frame(session_id, channel_id, seq, frame_type, payload)?;
        self.slots[seq as usize] =
            Some(Slot { frame: wire_bytes.clone(), tx_time: Instant::now(), retransmitted: false });
        self.next_seq = self.next_seq.wrapping_add(1);
        Ok((seq, wire_bytes))
    }

    /// Processes an incoming cumulative acknowledgment.
    ///
    /// If `ack_seq` is within `[base, next_seq)`, all frames from `base` through
    /// `ack_seq` are removed from the buffer, `base` is advanced to `ack_seq + 1`,
    /// retransmission attempts are reset to 0, and the elapsed RTT for `ack_seq`
    /// is returned in `AckOutcome::Advanced`.
    pub fn handle_ack(&mut self, ack_seq: u8) -> AckOutcome {
        if is_seq_in_window(self.base, self.next_seq, ack_seq) {
            let mut curr = self.base;
            loop {
                if curr == ack_seq {
                    break;
                }
                self.slots[curr as usize] = None;
                curr = curr.wrapping_add(1);
            }

            let rtt = self.slots[ack_seq as usize]
                .take()
                .and_then(|s| if !s.retransmitted { Some(s.tx_time.elapsed()) } else { None });
            self.base = ack_seq.wrapping_add(1);
            self.retransmission_attempts = 0;

            AckOutcome::Advanced { new_base: self.base, rtt, all_acked: self.base == self.next_seq }
        } else {
            AckOutcome::OutOfWindow { base: self.base, next_seq: self.next_seq }
        }
    }

    /// Handles a retransmission timer timeout.
    ///
    /// Increments `retransmission_attempts`. If the limit is reached, returns
    /// `Err(RetransmissionLimitExceeded)`. Otherwise, gathers and returns clones of all
    /// unacknowledged frames from `base` to `next_seq` and updates their transmission
    /// timestamps.
    ///
    /// # Errors
    /// Returns [`RetransmissionLimitExceeded`] if `retransmission_attempts >= max_retransmission_attempts`.
    /// Callers should call [`reset()`](Self::reset) before reusing the sender after exceeding the limit.
    pub fn handle_timeout(&mut self) -> Result<Vec<Vec<u8>>, RetransmissionLimitExceeded> {
        if self.is_idle() {
            return Ok(Vec::new());
        }
        self.retransmission_attempts += 1;
        if self.retransmission_attempts >= self.max_retransmission_attempts {
            return Err(RetransmissionLimitExceeded {
                base: self.base,
                next_seq: self.next_seq,
                attempts: self.retransmission_attempts,
            });
        }

        let mut frames = Vec::new();
        let mut curr = self.base;
        let now = Instant::now();
        while curr != self.next_seq {
            if let Some(slot) = &mut self.slots[curr as usize] {
                slot.tx_time = now;
                slot.retransmitted = true;
                frames.push(slot.frame.clone());
            }
            curr = curr.wrapping_add(1);
        }
        Ok(frames)
    }

    /// Resets the sender to an empty initial state.
    pub fn reset(&mut self) {
        self.base = 0;
        self.next_seq = 0;
        self.retransmission_attempts = 0;
        for slot in &mut self.slots {
            *slot = None;
        }
    }
}

/// Status of an incoming frame evaluated against the receiver window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStatus {
    /// The frame is in order (`seq == expected_seq`).
    InOrder {
        /// Sequence number of the received in-order frame.
        seq: u8,
    },
    /// The frame is a duplicate or arrived out of order.
    OutOfOrder {
        /// Sequence number of the unexpected frame.
        seq: u8,
        /// Sequence number currently expected by the receiver.
        expected_seq: u8,
        /// Cumulative ACK to send in response, if any.
        ack_seq: Option<u8>,
    },
}

/// Go-Back-N receiver state machine.
///
/// In Go-Back-N ARQ, frames arriving out of order are not queued or reordered; they are
/// immediately dropped and prompt a cumulative ACK of the latest contiguous in-order frame.
///
/// This state machine provides two-phase sequence progression ([`inspect`](Self::inspect) and
/// [`advance_in_order`](Self::advance_in_order)) to allow transport drivers to apply backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ResendReceiver {
    expected_seq: u8,
    last_acked: Option<u8>,
}

impl ResendReceiver {
    /// Creates a new receiver state machine with `expected_seq = 0`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the sequence number currently expected by the receiver.
    pub fn expected_seq(&self) -> u8 {
        self.expected_seq
    }

    /// Evaluates an incoming frame sequence number without advancing state.
    ///
    /// Returns [`FrameStatus::InOrder`] if `seq == expected_seq`. Otherwise, returns
    /// [`FrameStatus::OutOfOrder`] with the cumulative ACK sequence to transmit.
    pub fn inspect(&self, seq: u8) -> FrameStatus {
        if seq == self.expected_seq {
            FrameStatus::InOrder { seq }
        } else {
            FrameStatus::OutOfOrder {
                seq,
                expected_seq: self.expected_seq,
                ack_seq: self.current_ack_seq(),
            }
        }
    }

    /// Advances `expected_seq` by 1 and returns the cumulative ACK sequence to transmit.
    ///
    /// # Errors
    /// Returns [`UnexpectedSeqError`] if `seq != expected_seq`.
    pub fn advance_in_order(&mut self, seq: u8) -> Result<u8, UnexpectedSeqError> {
        if seq != self.expected_seq {
            return Err(UnexpectedSeqError { expected: self.expected_seq, received: seq });
        }
        let ack = self.expected_seq;
        self.last_acked = Some(ack);
        self.expected_seq = self.expected_seq.wrapping_add(1);
        Ok(ack)
    }

    /// Processes an incoming frame sequence number in a single step.
    ///
    /// If `seq == expected_seq`, advances the sequence and returns [`FrameStatus::InOrder`].
    /// Otherwise, returns [`FrameStatus::OutOfOrder`] with the cumulative ACK to send in response.
    ///
    /// Callers implementing two-phase flow control (e.g. attempting non-blocking writes to bounded
    /// channels before committing sequence progression) should use [`inspect`](Self::inspect)
    /// followed by [`advance_in_order`](Self::advance_in_order) instead.
    pub fn accept_seq(&mut self, seq: u8) -> FrameStatus {
        match self.inspect(seq) {
            FrameStatus::InOrder { seq } => {
                let _ = self.advance_in_order(seq);
                FrameStatus::InOrder { seq }
            }
            out_of_order => out_of_order,
        }
    }

    /// Returns the current cumulative ACK sequence for out-of-order/duplicate frames.
    ///
    /// If no frames have been received yet, returns `None` to suppress spurious `ACK 255` emission
    /// on startup packet loss (`BUG-PROT-02`).
    pub fn current_ack_seq(&self) -> Option<u8> {
        self.last_acked
    }

    /// Resets the receiver to expect sequence number 0.
    pub fn reset(&mut self) {
        self.expected_seq = 0;
        self.last_acked = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FrameType;

    #[fuchsia::test]
    fn test_sender_window_full_and_advance() {
        let mut sender = ResendSender::new(4, 10);
        assert!(sender.can_send());
        assert!(sender.is_idle());
        assert_eq!(sender.in_flight(), 0);

        for i in 0..4 {
            let (seq, _) = sender
                .enqueue_frame(1, 10, FrameType::Data, format!("msg{}", i).as_bytes())
                .unwrap();
            assert_eq!(seq, i);
        }
        assert_eq!(sender.in_flight(), 4);
        assert!(!sender.can_send());
        assert!(!sender.is_idle());

        // Attempting to send when window is full fails
        let err = sender.enqueue_frame(1, 10, FrameType::Data, b"overflow").unwrap_err();
        assert_eq!(err, FrameError::WindowFull { window_size: 4, in_flight: 4 });

        // Cumulative ACK for packet 1 acknowledges 0 and 1
        let outcome = sender.handle_ack(1);
        assert!(matches!(outcome, AckOutcome::Advanced { new_base: 2, all_acked: false, .. }));
        assert_eq!(sender.base(), 2);
        assert_eq!(sender.next_seq(), 4);
        assert_eq!(sender.in_flight(), 2);
        assert!(sender.can_send());

        // Acknowledge remaining 2 and 3
        let outcome = sender.handle_ack(3);
        assert!(matches!(outcome, AckOutcome::Advanced { new_base: 4, all_acked: true, .. }));
        assert_eq!(sender.in_flight(), 0);
        assert!(sender.is_idle());
    }

    #[fuchsia::test]
    fn test_sender_out_of_window_ack_ignored() {
        let mut sender = ResendSender::new(10, 10);
        sender.enqueue_frame(1, 1, FrameType::Data, b"test").unwrap();
        assert_eq!(sender.base(), 0);
        assert_eq!(sender.next_seq(), 1);

        // ACK for frame 5 is out of window
        let outcome = sender.handle_ack(5);
        assert_eq!(outcome, AckOutcome::OutOfWindow { base: 0, next_seq: 1 });
        assert_eq!(sender.base(), 0);
    }

    #[fuchsia::test]
    fn test_sender_timeout_and_max_attempts() {
        let mut sender = ResendSender::new(10, 3);
        sender.enqueue_frame(1, 1, FrameType::Data, b"pkt0").unwrap();
        sender.enqueue_frame(1, 1, FrameType::Data, b"pkt1").unwrap();

        // Timeout 1
        let retrans = sender.handle_timeout().unwrap();
        assert_eq!(retrans.len(), 2);
        assert_eq!(sender.retransmission_attempts(), 1);

        // Timeout 2
        let retrans = sender.handle_timeout().unwrap();
        assert_eq!(retrans.len(), 2);
        assert_eq!(sender.retransmission_attempts(), 2);

        // Timeout 3 -> exceeds max_attempts (3)
        let err = sender.handle_timeout().unwrap_err();
        assert_eq!(err, RetransmissionLimitExceeded { base: 0, next_seq: 2, attempts: 3 });
    }

    #[fuchsia::test]
    fn test_sender_sequence_wrap() {
        let mut sender = ResendSender::new(10, 10);
        // Advance sender near sequence boundary
        sender.base = 254;
        sender.next_seq = 254;

        let (seq0, _) = sender.enqueue_frame(1, 1, FrameType::Data, b"a").unwrap();
        assert_eq!(seq0, 254);
        let (seq1, _) = sender.enqueue_frame(1, 1, FrameType::Data, b"b").unwrap();
        assert_eq!(seq1, 255);
        let (seq2, _) = sender.enqueue_frame(1, 1, FrameType::Data, b"c").unwrap();
        assert_eq!(seq2, 0);
        let (seq3, _) = sender.enqueue_frame(1, 1, FrameType::Data, b"d").unwrap();
        assert_eq!(seq3, 1);

        assert_eq!(sender.in_flight(), 4);

        // Cumulative ACK for 0 acknowledges 254, 255, and 0
        let outcome = sender.handle_ack(0);
        assert!(matches!(outcome, AckOutcome::Advanced { new_base: 1, all_acked: false, .. }));
        assert_eq!(sender.base(), 1);
        assert_eq!(sender.in_flight(), 1);

        let outcome = sender.handle_ack(1);
        assert!(matches!(outcome, AckOutcome::Advanced { new_base: 2, all_acked: true, .. }));
        assert_eq!(sender.in_flight(), 0);
        assert!(sender.is_idle());
    }

    #[fuchsia::test]
    fn test_receiver_in_order_and_duplicates() {
        let mut receiver = ResendReceiver::new();
        assert_eq!(receiver.expected_seq(), 0);

        // Packet 0 arrives in order
        let status = receiver.inspect(0);
        assert_eq!(status, FrameStatus::InOrder { seq: 0 });
        let ack = receiver.advance_in_order(0).unwrap();
        assert_eq!(ack, 0);
        assert_eq!(receiver.expected_seq(), 1);

        // Advance with wrong sequence fails
        assert_eq!(
            receiver.advance_in_order(99),
            Err(UnexpectedSeqError { expected: 1, received: 99 })
        );

        // Packet 1 arrives in order
        let status = receiver.inspect(1);
        assert_eq!(status, FrameStatus::InOrder { seq: 1 });
        let ack = receiver.advance_in_order(1).unwrap();
        assert_eq!(ack, 1);
        assert_eq!(receiver.expected_seq(), 2);

        // Duplicate packet 1 arrives -> returns cumulative ACK for 1
        let status = receiver.inspect(1);
        assert_eq!(status, FrameStatus::OutOfOrder { seq: 1, expected_seq: 2, ack_seq: Some(1) });
        assert_eq!(receiver.expected_seq(), 2);

        // Out-of-order packet 5 arrives -> returns cumulative ACK for 1
        let status = receiver.inspect(5);
        assert_eq!(status, FrameStatus::OutOfOrder { seq: 5, expected_seq: 2, ack_seq: Some(1) });
    }

    #[fuchsia::test]
    fn test_receiver_accept_seq() {
        let mut receiver = ResendReceiver::new();
        assert_eq!(receiver.accept_seq(0), FrameStatus::InOrder { seq: 0 });
        assert_eq!(receiver.expected_seq(), 1);
        assert_eq!(receiver.current_ack_seq(), Some(0));

        // Out of order
        assert_eq!(
            receiver.accept_seq(5),
            FrameStatus::OutOfOrder { seq: 5, expected_seq: 1, ack_seq: Some(0) }
        );
        assert_eq!(receiver.expected_seq(), 1);
    }

    #[fuchsia::test]
    fn test_receiver_suppresses_ack_255_on_startup_packet_loss() {
        let receiver = ResendReceiver::new();
        // Packet 0 was dropped, packet 1 arrives out of order
        let status = receiver.inspect(1);
        assert_eq!(status, FrameStatus::OutOfOrder { seq: 1, expected_seq: 0, ack_seq: None });
        // ack_seq is None, suppressing spurious ACK 255 emission
        assert_eq!(receiver.current_ack_seq(), None);
    }

    #[fuchsia::test]
    fn test_receiver_sequence_wrap_duplicate_ack() {
        let mut receiver = ResendReceiver::new();
        for i in 0..256 {
            receiver.advance_in_order(i as u8).unwrap();
        }
        // After 256 in-order packets, expected_seq wraps to 0
        assert_eq!(receiver.expected_seq(), 0);
        assert_eq!(receiver.current_ack_seq(), Some(255));

        // A duplicate of packet 255 arrives -> must return cumulative ACK Some(255), not None
        let status = receiver.inspect(255);
        assert_eq!(
            status,
            FrameStatus::OutOfOrder { seq: 255, expected_seq: 0, ack_seq: Some(255) }
        );
    }

    #[fuchsia::test]
    fn test_sender_timeout_idle_ignored() {
        let mut sender = ResendSender::new(10, 3);
        assert!(sender.is_idle());
        // Spurious timeout while idle should not increment attempts or error
        let retrans = sender.handle_timeout().unwrap();
        assert!(retrans.is_empty());
        assert_eq!(sender.retransmission_attempts(), 0);
    }

    #[fuchsia::test]
    fn test_karns_algorithm_rtt_ignored_on_retransmission() {
        let mut sender = ResendSender::new(10, 5);
        sender.enqueue_frame(1, 1, FrameType::Data, b"pkt0").unwrap();

        // Timeout retransmits pkt0
        let retrans = sender.handle_timeout().unwrap();
        assert_eq!(retrans.len(), 1);

        // ACK arrives for retransmitted pkt0: rtt must be None under Karn's algorithm
        let outcome = sender.handle_ack(0);
        match outcome {
            AckOutcome::Advanced { rtt, .. } => {
                assert_eq!(rtt, None, "Karn's algorithm must ignore RTT for retransmitted frame");
            }
            other => panic!("Unexpected outcome: {other:?}"),
        }

        // Send a fresh frame without timeout
        sender.enqueue_frame(1, 1, FrameType::Data, b"pkt1").unwrap();
        let outcome = sender.handle_ack(1);
        match outcome {
            AckOutcome::Advanced { rtt, .. } => {
                assert!(rtt.is_some(), "RTT must be present for non-retransmitted frame");
            }
            other => panic!("Unexpected outcome: {other:?}"),
        }
    }

    #[fuchsia::test]
    #[should_panic(expected = "window_size must be in range 1..=128")]
    fn test_sender_zero_window_panics() {
        let _ = ResendSender::new(0, 10);
    }

    #[fuchsia::test]
    #[should_panic(expected = "window_size must be in range 1..=128")]
    fn test_sender_excessive_window_panics() {
        let _ = ResendSender::new(129, 10);
    }

    #[fuchsia::test]
    #[should_panic(expected = "max_retransmission_attempts must be greater than 0")]
    fn test_sender_zero_max_attempts_panics() {
        let _ = ResendSender::new(10, 0);
    }
}
