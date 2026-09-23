// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! TCP state tracking.

use core::time::Duration;

use netstack3_base::{Control, SegmentHeader, SeqNum, UnscaledWindowSize, WindowScale, WindowSize};
use replace_with::replace_with_and;

use super::{
    ConnectionDirection, ConnectionUpdateAction, ConnectionUpdateError, EstablishmentLifecycle,
};

/// A struct that completely encapsulates tracking a bidirectional TCP
/// connection.
#[derive(Debug, Clone)]
pub(crate) struct Connection {
    /// The current state of the TCP connection.
    state: State,
}

impl Connection {
    pub fn new(segment: &SegmentHeader, payload_len: usize, self_connected: bool) -> Option<Self> {
        Some(Self {
            // TODO(https://fxbug.dev/355699182): Properly support self-connected
            // connections.
            state: if self_connected {
                State::Untracked
            } else {
                State::new(segment, payload_len)?
            },
        })
    }

    pub fn expiry_duration(&self, establishment_lifecycle: EstablishmentLifecycle) -> Duration {
        self.state.expiry_duration(establishment_lifecycle)
    }

    pub fn update(
        &mut self,
        segment: &SegmentHeader,
        payload_len: usize,
        dir: ConnectionDirection,
    ) -> Result<ConnectionUpdateAction, ConnectionUpdateError> {
        let valid =
            replace_with_and(&mut self.state, |state| state.update(segment, payload_len, dir));

        if !valid {
            return Err(ConnectionUpdateError::InvalidPacket);
        }

        match self.state {
            State::Closed => Ok(ConnectionUpdateAction::RemoveEntry),
            State::Untracked
            | State::SynSent(_)
            | State::WaitingOnOpeningAcks(_)
            | State::Established(_) => Ok(ConnectionUpdateAction::NoAction),
        }
    }
}

/// States for a TCP connection as a whole.
///
/// These vaguely correspond to states from RFC 9293, but since they apply to
/// the whole connection, and we're just snooping in the middle, they can't line
/// up perfectly 1:1. They exist to encode assumptions about the state of the
/// connection that would otherwise be unwieldy to manage if they were all
/// stuffed into [`Peer`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// The connection has properties that break standard state tracking. This
    /// state does a good-enough job tracking the connection.
    ///
    /// This is a short-circuit state that can never be left.
    Untracked,

    /// The connection has been closed, either by the FIN handshake or valid
    /// RST.
    ///
    /// This is a short-circuit state that can never be left.
    Closed,

    /// The initial SYN for this connection has been sent. State contained
    /// within is everything that can be gleaned from the initial SYN packet
    /// sent in the original direction.
    ///
    /// Expected peer states:
    /// - Original: `SYN_SENT`
    /// - Reply: `SYN_RECEIVED` (upon receipt)
    SynSent(SynSent),

    /// We've seen SYNs in both directions and are just waiting for them to be
    /// ACKed.
    ///
    /// Peer states are SYN_SENT, SYN_RECEIVED, or ESTABLISHED.
    WaitingOnOpeningAcks(PeerPair<WaitingOnOpeningAcksPeer>),

    /// The handshake has completed, save for the final ACK (i.e. the last `ACK`
    /// of `SYN`, `SYN/ACK`, `ACK`).
    ///
    /// The connection stays in this state until it is completely torn down
    /// by the closing handshake or a valid RST.
    Established(PeerPair<Peer>),
}

impl State {
    fn new(segment: &SegmentHeader, payload_len: usize) -> Option<Self> {
        // We explicitly don't want to track any connections that we haven't
        // seen from the beginning because:
        //
        // a) This shouldn't happen, since we run from boot
        // b) Window scale is only negotiated during the initial handshake.
        if segment.control != Some(Control::SYN) || segment.ack.is_some() {
            return None;
        }

        Some(Self::SynSent(SynSent {
            iss: segment.seq,
            logical_len: segment.len(payload_len),
            advertised_window_scale: segment.options.window_scale(),
            // This unwrap cannot fail because WindowSize::MAX is 2^30-1,
            // which is larger than the largest possible unscaled window
            // size (2^16).
            window_size: WindowSize::from_u32(u16::from(segment.wnd).into()).unwrap(),
            dir: ConnectionDirection::Original,
        }))
    }

    fn expiry_duration(&self, establishment_lifecycle: EstablishmentLifecycle) -> Duration {
        const MAXIMUM_SEGMENT_LIFETIME: Duration = Duration::from_secs(120);

        // These are all picked to optimize purging connections from the table
        // as soon as is reasonable. Unlike Linux, we are choosing to be more
        // conservative with our timeouts and setting the most aggressive one to
        // the standard MSL of 120 seconds.
        match self {
            State::Untracked => {
                match establishment_lifecycle {
                    // This is small because it's just meant to be the time for
                    // the initial handshake.
                    EstablishmentLifecycle::SeenOriginal | EstablishmentLifecycle::SeenReply => {
                        MAXIMUM_SEGMENT_LIFETIME
                    }
                    EstablishmentLifecycle::Established => Duration::from_secs(6 * 60 * 60),
                }
            }
            State::Closed => Duration::ZERO,
            State::SynSent(_) | State::WaitingOnOpeningAcks(_) => MAXIMUM_SEGMENT_LIFETIME,
            State::Established(PeerPair { original, reply }) => {
                // If there is data outstanding, make the timeout small so we
                // can purge the connection quickly if one of the endpoints
                // disappears. If we believe the connection may have been reset,
                // use the small timeout as well because we can't be sure if
                // that segment was valid.  See the comment on
                // `Peer::unreplied_rst` for the nitty gritty.
                //
                // We treat a connection that's ever had a valid FIN the same
                // way. This pessimizes things for half-closed connections, but
                // that's a very uncommon case.
                if original.unacked_data
                    || reply.unacked_data
                    || original.fin_state.sent()
                    || reply.fin_state.sent()
                    || original.unreplied_rst
                    || reply.unreplied_rst
                {
                    MAXIMUM_SEGMENT_LIFETIME
                } else {
                    Duration::from_secs(5 * 60 * 60 * 24)
                }
            }
        }
    }

    /// Returns a new state that unconditionally replaces the previous one. The
    /// boolean represents whether the segment was valid or not.
    ///
    /// In the case where the segment was invalid, the returned state will be
    /// equivalent to the one that `update` was called on.
    fn update(
        self,
        segment: &SegmentHeader,
        payload_len: usize,
        dir: ConnectionDirection,
    ) -> (State, bool) {
        match self {
            State::Untracked => (State::Untracked, true),
            State::Closed => (State::Closed, true),
            State::SynSent(state) => update_for_syn_sent(state, segment, payload_len, dir),
            State::WaitingOnOpeningAcks(peers) => update_for_waiting_on_opening_acks(
                peers.into_update_peers(dir),
                segment,
                payload_len,
            ),
            State::Established(peers) => {
                update_for_established(peers.into_update_peers(dir), segment, payload_len)
            }
        }
    }
}

/// Contains all of the information required for a single peer in an established
/// TCP connection.
///
/// Packets are valid with the following equations (taken from ["Real Stateful
/// TCP Packet Filtering in IP Filter"][paper] and updated to allow for segments
/// intersecting the valid ranges, rather than needing to be entirely contained
/// within them).
///
/// Definitions:
/// - s: The sequence number of the first octet of the segment
/// - n: The (virtual) length of the segment in octets
/// - a: The ACK number of the segment
///
/// I:   Data upper bound: s   <= receiver.max_wnd_seq
/// II:  Data lower bound: s+n >= sender.max_next_seq - receiver.max_wnd
/// III: ACK  upper bound: a   <= receiver.max_next_seq
/// IV:  ACK  lower bound: a   >= receiver.max_next_seq - MAXACKWINDOW
///
/// MAXACKWINDOW is defined in the paper to be 66000, which is larger than
/// the largest possible window (without scaling). We scale by
/// sender.window_scale to ensure this property remains true.
///
/// [paper]: https://www.usenix.org/legacy/events/sec01/invitedtalks/rooij.pdf
#[derive(Debug, Clone, PartialEq, Eq)]
struct Peer {
    /// How much to scale window updates from this peer.
    window_scale: WindowScale,

    /// The maximum window size ever sent by this peer.
    ///
    /// On every packet sent by this peer, the larger of itself and the
    /// (scaled) window from the packet is taken.
    max_wnd: WindowSize,

    /// The maximum sequence number that is in the window for this peer.
    ///
    /// On every packet sent by this peer, this is updated to be the larger of
    /// itself or the current advertised window (scaled) plus the ACK number in
    /// the packet.
    max_wnd_seq: SeqNum,

    /// The largest "next octet" ever sent by this peer. This is equivalent to
    /// one more than the largest sequence number ever sent by this peer.
    ///
    /// On every packet sent by this peer, this is updated to be the larger of
    /// itself or the sequence number plus the length of the packet.
    max_next_seq: SeqNum,

    /// Whether this peer has sent unACKed sequence numbers, accounting for SYN
    /// and FIN.
    ///
    /// Set when max_next_seq is increased. Unset when a reply segment is seen
    /// that has an ACK number equal to `max_next_seq` (larger would mean an
    /// invalid packet).
    unacked_data: bool,

    /// The state of the first FIN segment sent by this peer.
    fin_state: FinState,

    /// Whether this peer has sent an RST that has not been responded to.
    ///
    /// We can never be sure whether the receiver will consider an RST segment
    /// valid. Instead, we use "this peer has sent an RST but the other peer
    /// hasn't sent anything in response" as a signal that the connection might
    /// be dead. If the receiver side sends a segment in response, it'll provoke
    /// another RST if the connection should truly be reset.
    ///
    /// This should only be reset on receiving a segment, never sending one. If
    /// it was cleared in that case, a valid RST could be reordered with a data
    /// segment, which would leave a stale conntrack entry.
    unreplied_rst: bool,
}

impl Peer {
    /// Checks that the sequence numbers of a segment are within the windows
    /// defined in the comment on [`Peer`].
    fn seq_valid(peers: UpdatePeers<&Self>, seq: SeqNum, len: u32) -> bool {
        let UpdatePeers { sender, receiver, dir: _ } = peers;

        // I: Segment sequence numbers upper bound.
        if seq.after(receiver.max_wnd_seq) {
            return false;
        }

        // II: Segment sequence numbers lower bound.
        if (seq + len).before(sender.max_next_seq - receiver.max_wnd) {
            return false;
        }

        true
    }

    /// Checks that an ACK sequence number of a segment is within the windows
    /// defined in the comment on [`Peer`].
    fn ack_valid(peers: UpdatePeers<&Self>, ack: SeqNum) -> bool {
        let UpdatePeers { sender: _, receiver, dir: _ } = peers;
        // III: ACK upper bound.
        if ack.after(receiver.max_next_seq) {
            return false;
        }

        // IV: ACK lower bound.
        if ack.before(receiver.max_next_seq - receiver.max_ack_window()) {
            return false;
        }

        true
    }

    /// Returns a new `Peer` updated using the provided information from a
    /// segment which it has sent.
    fn update_sender(
        self,
        seq: SeqNum,
        len: u32,
        ack: SeqNum,
        wnd: UnscaledWindowSize,
        control: Option<Control>,
    ) -> Self {
        let Self {
            window_scale,
            max_wnd,
            max_wnd_seq,
            max_next_seq,
            unacked_data,
            fin_state,
            unreplied_rst,
        } = self;

        // The minimum window size is assumed to be 1. From the paper:
        //   On BSD systems, a window probing is always done with a packet
        //   containing one octet of data.
        let window_size = {
            // RFC 7323 2.2:
            //    The window field in a segment where the SYN bit is set (i.e., a <SYN>
            //    or <SYN,ACK>) MUST NOT be scaled.
            let window_scale =
                if control == Some(Control::SYN) { WindowScale::ZERO } else { window_scale };
            let window_size = wnd << window_scale;
            // The unwrap below won't fail because 1 is less than
            // WindowSize::MAX.
            core::cmp::max(window_size, WindowSize::from_u32(1).unwrap())
        };

        // The largest sequence number allowed by the current window.
        let wnd_seq = ack + window_size;
        // The octet one past the last one in the segment.
        let end = seq + len;
        // The largest `end` value sent by this peer.
        let sender_max_next_seq = if max_next_seq.before(end) { end } else { max_next_seq };

        Peer {
            window_scale,
            max_wnd: core::cmp::max(max_wnd, window_size),
            max_wnd_seq: if max_wnd_seq.before(wnd_seq) { wnd_seq } else { max_wnd_seq },
            max_next_seq: sender_max_next_seq,
            unacked_data: if sender_max_next_seq.after(max_next_seq) { true } else { unacked_data },
            fin_state: if control == Some(Control::FIN) {
                fin_state.update_fin_sent(end - 1)
            } else {
                fin_state
            },
            unreplied_rst,
        }
    }

    /// Returns a new `Peer` updated using the provided information from a
    /// segment which it received.
    fn update_receiver(self, ack: SeqNum) -> Self {
        let Self {
            window_scale,
            max_wnd,
            max_wnd_seq,
            max_next_seq,
            unacked_data,
            fin_state,
            unreplied_rst: _,
        } = self;

        Peer {
            window_scale,
            max_wnd,
            max_wnd_seq,
            max_next_seq,
            // It's not possible for ack to be > self.max_next_seq due to
            // equation III, which is checked on every segment.
            unacked_data: if ack == max_next_seq { false } else { unacked_data },
            fin_state: fin_state.update_ack_received(ack),
            // See the comment on this field's definition.
            unreplied_rst: false,
        }
    }

    fn max_ack_window(&self) -> u32 {
        // The paper gives 66000 as the MAXACKWINDOW constant because it's a
        // little larger than the largest possible TCP window. With winow
        // scaling in effect, this constant is no longer valid, so we scale it
        // up by that scaling factor.
        //
        // This shift is guaranteed to never overflow because the maximum value
        // for self.window_scale is 14 and 66000 << 14 < u32::MAX.
        66000u32 << (self.window_scale.get() as u32)
    }
}

/// Tracks the state of the FIN process for a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FinState {
    /// This peer has not sent a FIN yet.
    NotSent,

    /// This peer has sent a FIN with the provided sequence number.
    ///
    /// Updated to ensure it is the sequence number of the first FIN sent.
    Sent(SeqNum),

    /// The FIN sent by this peer has been ACKed.
    Acked,
}

impl FinState {
    /// To be called when the peer has sent a FIN segment with the sequence
    /// number of the FIN.
    ///
    /// Returns an updated `FinState`.
    fn update_fin_sent(self, seq: SeqNum) -> Self {
        match self {
            FinState::NotSent => FinState::Sent(seq),
            FinState::Sent(s) => {
                // NOTE: We want to track the first FIN in the sequence
                // space, not the first one we saw.
                if s.before(seq) { FinState::Sent(s) } else { FinState::Sent(seq) }
            }
            FinState::Acked => FinState::Acked,
        }
    }

    /// To be called when the peer has received an ACK.
    ///
    /// Returns an updated `FinState`.
    fn update_ack_received(self, ack: SeqNum) -> Self {
        match self {
            FinState::NotSent => FinState::NotSent,
            FinState::Sent(seq) => {
                if ack.after(seq) {
                    FinState::Acked
                } else {
                    FinState::Sent(seq)
                }
            }
            FinState::Acked => FinState::Acked,
        }
    }

    /// Has this FIN been acked?
    fn acked(&self) -> bool {
        match self {
            FinState::NotSent => false,
            FinState::Sent(_) => false,
            FinState::Acked => true,
        }
    }

    /// Has this FIN been sent?
    fn sent(&self) -> bool {
        match self {
            FinState::NotSent => false,
            FinState::Sent(_) => true,
            FinState::Acked => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WaitingOnOpeningAcksPeer {
    peer: Peer,
    iss: SeqNum,
    saw_ack: bool,
    advertised_window_scale: Option<WindowScale>,
}

impl WaitingOnOpeningAcksPeer {
    fn seq_valid(peers: UpdatePeers<&Self>, seq: SeqNum, len: u32) -> bool {
        Peer::seq_valid(peers.map_both(|peer| &peer.peer), seq, len)
    }

    fn ack_valid(peers: UpdatePeers<&Self>, ack: SeqNum) -> bool {
        Peer::ack_valid(peers.map_both(|peer| &peer.peer), ack)
    }

    /// Returns a new [`WaitingOnOpeningAcksPeer`] updated using the provided information
    /// from a segment that it has sent.
    fn update_sender(
        self,
        seq: SeqNum,
        len: u32,
        ack: SeqNum,
        wnd: UnscaledWindowSize,
        control: Option<Control>,
    ) -> Self {
        let Self { peer, iss, saw_ack, advertised_window_scale } = self;

        Self {
            peer: peer.update_sender(seq, len, ack, wnd, control),
            iss,
            saw_ack,
            advertised_window_scale,
        }
    }

    /// Returns a new [`WaitingOnOpeningAcksPeer`] updated using the provided information from a
    /// segment that it received.
    fn update_receiver(self, ack: SeqNum) -> Self {
        let Self { peer, iss, saw_ack, advertised_window_scale } = self;

        Self {
            peer: peer.update_receiver(ack),
            iss,
            saw_ack: saw_ack || ack.after(iss),
            advertised_window_scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerPair<T> {
    original: T,
    reply: T,
}

impl<T> PeerPair<T> {
    fn into_update_peers(self, dir: ConnectionDirection) -> UpdatePeers<T> {
        let Self { original, reply } = self;
        match dir {
            ConnectionDirection::Original => UpdatePeers { sender: original, receiver: reply, dir },
            ConnectionDirection::Reply => UpdatePeers { sender: reply, receiver: original, dir },
        }
    }
}

/// Centralizes and names the peers for performing updates. Avoids ambiguity
/// about which peer is the sender/receiver.
struct UpdatePeers<T> {
    sender: T,
    receiver: T,
    dir: ConnectionDirection,
}

impl<T> UpdatePeers<T> {
    fn into_peer_pair(self) -> PeerPair<T> {
        let Self { sender, receiver, dir } = self;
        match dir {
            ConnectionDirection::Original => PeerPair { original: sender, reply: receiver },
            ConnectionDirection::Reply => PeerPair { original: receiver, reply: sender },
        }
    }

    fn as_ref(&self) -> UpdatePeers<&T> {
        let Self { sender, receiver, dir } = self;
        UpdatePeers { sender: &sender, receiver: &receiver, dir: *dir }
    }

    fn map_both<F, U>(self, f: F) -> UpdatePeers<U>
    where
        F: Fn(T) -> U,
    {
        let Self { sender, receiver, dir } = self;

        let sender = f(sender);
        let receiver = f(receiver);
        UpdatePeers { sender, receiver, dir }
    }
}

/// State for [`State::SynSent`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct SynSent {
    /// The ISS (initial send sequence number).
    iss: SeqNum,

    /// The logical length of the segment.
    logical_len: u32,

    /// The window scale (if set in the initial SYN).
    advertised_window_scale: Option<WindowScale>,

    /// The advertised window size. Converted into a sequence number once we
    /// know what the other stack's ISN is.
    ///
    /// RFC 1323 2.2:
    ///   The Window field in a SYN (i.e., a <SYN> or <SYN,ACK>) segment itself
    ///   is never scaled.
    window_size: WindowSize,

    /// The stack the above state comes from.
    dir: ConnectionDirection,
}

/// State transitions for in-range segments by direction:
/// - Original
///   - SYN: Update data, stay in SynSent
///   - SYN/ACK: Invalid
///   - RST: Invalid
///   - FIN: Invalid
///   - ACK: Invalid
/// - Reply
///   - SYN(/ACK): WaitingOnOpeningAcks
///   - RST: Delete connection
///   - FIN: Invalid
///   - ACK: Invalid
fn update_for_syn_sent(
    state: SynSent,
    segment: &SegmentHeader,
    payload_len: usize,
    dir: ConnectionDirection,
) -> (State, bool) {
    let SynSent { iss, logical_len, advertised_window_scale, window_size, dir: state_dir } = state;

    // This is another packet in the same direction as the first one.  Update
    // existing parameters for initial SYN, but only for packets that look to be
    // valid retransmits of the initial SYN. This behavior is copied over from
    // gVisor.
    if dir == state_dir {
        if let Some(_) = segment.ack {
            return (State::SynSent(state), false);
        }

        match segment.control {
            None | Some(Control::FIN) | Some(Control::RST) => {
                return (State::SynSent(state), false);
            }
            Some(Control::SYN) => {}
        };

        if segment.seq != state.iss || segment.options.window_scale() != advertised_window_scale {
            return (State::SynSent(state), false);
        }

        // If it's a valid retransmit of the original SYN, update
        // any state that changed and let it through.
        let seg_window_size = WindowSize::from_u32(u16::from(segment.wnd).into()).unwrap();

        (
            State::SynSent(SynSent {
                iss,
                logical_len: u32::max(segment.len(payload_len), logical_len),
                advertised_window_scale: advertised_window_scale,
                window_size: core::cmp::max(seg_window_size, window_size),
                dir: state_dir,
            }),
            true,
        )
    } else {
        // RFC 9293 3.10.7.3:
        //   If SND.UNA < SEG.ACK =< SND.NXT, then the ACK is
        //   acceptable.
        match segment.ack {
            None => {}
            Some(ack) => {
                if !(ack.after(iss) && ack.before(iss + logical_len + 1)) {
                    return (State::SynSent(state), false);
                }
            }
        };

        match segment.control {
            None | Some(Control::FIN) => (State::SynSent(state), false),
            // RFC 9293 3.10.7.3:
            //   If the RST bit is set,
            //   If the ACK was acceptable, then signal to the user
            //   "error: connection reset", drop the segment, enter
            //   CLOSED state, delete TCB, and return. Otherwise (no
            //   ACK), drop the segment and return.
            //
            // For our purposes, we delete the connection because we
            // know the receiver will tear down the connection.
            Some(Control::RST) => match segment.ack {
                None => (State::SynSent(state), false),
                // We previously validated the ACK, so this RST must be valid.
                Some(_) => (State::Closed, true),
            },

            Some(Control::SYN) => {
                let sender_advertised_window_scale = segment.options.window_scale();
                let sender_window_size =
                    WindowSize::from_u32(u16::from(segment.wnd).into()).unwrap();
                let receiver_max_next_seq = iss + logical_len;

                let (receiver_unacked_data, receiver_saw_ack, sender_max_wnd_seq) =
                    match segment.ack {
                        Some(ack) => {
                            (ack.before(receiver_max_next_seq), true, ack + sender_window_size)
                        }
                        // This is a simultaneous open.
                        //
                        // See the comment on receiver.max_wnd_seq below for an
                        // explanation of this calculation.
                        None => (true, false, iss + sender_window_size),
                    };

                // RFC 1323 2.2:
                //   This option is an offer, not a promise; both sides
                //   must send Window Scale options in their SYN
                //   segments to enable window scaling in either
                //   direction.
                let (receiver_window_scale, sender_window_scale) =
                    match (advertised_window_scale, sender_advertised_window_scale) {
                        (Some(receiver), Some(sender)) => (receiver, sender),
                        _ => (WindowScale::ZERO, WindowScale::ZERO),
                    };

                let new_peers = UpdatePeers {
                    sender: WaitingOnOpeningAcksPeer {
                        peer: Peer {
                            window_scale: sender_window_scale,
                            max_wnd: sender_window_size,
                            max_wnd_seq: sender_max_wnd_seq,
                            max_next_seq: segment.seq + segment.len(payload_len),
                            // The sender always has unacked data here
                            // because the SYN we just saw sent needs to
                            // be acked.
                            unacked_data: true,
                            fin_state: FinState::NotSent,
                            unreplied_rst: false,
                        },
                        iss: segment.seq,
                        saw_ack: false,
                        advertised_window_scale: sender_advertised_window_scale,
                    },
                    receiver: WaitingOnOpeningAcksPeer {
                        peer: Peer {
                            window_scale: receiver_window_scale,
                            max_wnd: window_size,
                            // We're still waiting on an ACK from the receiver
                            // stack, so this is slightly different from the normal
                            // calculation. It's still valid because we can assume
                            // that the implicit ACK number is the sender ISS (no
                            // data ACKed).
                            max_wnd_seq: segment.seq + window_size,
                            max_next_seq: receiver_max_next_seq,
                            unacked_data: receiver_unacked_data,
                            fin_state: FinState::NotSent,
                            unreplied_rst: false,
                        },
                        iss,
                        saw_ack: receiver_saw_ack,
                        advertised_window_scale,
                    },
                    dir,
                };

                (State::WaitingOnOpeningAcks(new_peers.into_peer_pair()), true)
            }
        }
    }
}

fn update_for_waiting_on_opening_acks(
    mut peers: UpdatePeers<WaitingOnOpeningAcksPeer>,
    segment: &SegmentHeader,
    payload_len: usize,
) -> (State, bool) {
    let logical_len = segment.len(payload_len);
    let &SegmentHeader { seq, ack, wnd, control, options: _, push: _ } = segment;

    // If this is a SYN, it can only be valid if it has the same parameters as
    // the one that was originally sent. This is the same check as in `SynSent`,
    // but if we're here in `WaitingOnOpeningAcks` then both peers have sent
    // SYNs.
    if control == Some(Control::SYN)
        && (segment.seq != peers.sender.iss
            || segment.options.window_scale() != peers.sender.advertised_window_scale)
    {
        return (State::WaitingOnOpeningAcks(peers.into_peer_pair()), false);
    }

    let seq_valid = WaitingOnOpeningAcksPeer::seq_valid(peers.as_ref(), seq, logical_len);

    // From RFC 9293 section 3.5.3:
    //   In all states except SYN-SENT, all reset (RST) segments are validated
    //   by checking their SEQ fields. A reset is valid if its sequence number
    //   is in the window. In the SYN-SENT state (a RST received in response to
    //   an initial SYN), the RST is acceptable if the ACK field acknowledges
    //   the SYN.
    //
    // And for SYN-SENT, from RFC 9293 section 3.10.7.3:
    //   If the RST bit is set, if the ACK was acceptable, then signal to the
    //   user "error: connection reset", drop the segment, enter CLOSED state,
    //   delete TCB, and return. Otherwise (no ACK), drop the segment and
    //   return.
    if control == Some(Control::RST) {
        // This is not backwards: the sender having received an ACK means that
        // the receiver must have previously sent a SYN/ACK, and so can't be in
        // SYN-SENT.
        let receiver_maybe_in_syn_sent = !peers.sender.saw_ack;
        let ack_valid_for_syn_sent_rst = ack
            .map(|ack| {
                ack.after(peers.receiver.iss) && ack.before(peers.receiver.peer.max_next_seq + 1)
            })
            .unwrap_or(false);

        // If we don't know whether the receiver is in SYN-SENT, we have to be
        // lenient and count the segment as valid if it works for either
        // SYN-SENT or the synchronized case.
        let rst_valid = seq_valid || (receiver_maybe_in_syn_sent && ack_valid_for_syn_sent_rst);

        if rst_valid {
            peers.sender.peer.unreplied_rst = true;
        }
        return (State::WaitingOnOpeningAcks(peers.into_peer_pair()), rst_valid);
    }

    if !seq_valid {
        return (State::WaitingOnOpeningAcks(peers.into_peer_pair()), false);
    }

    let ack = match ack {
        Some(ack) => ack,
        None => {
            // A SYN could be invalid, but we have to be a little lenient
            // because we don't know what state the receiver is in. We'll catch
            // up quickly once the dust settles (RST or a regular/challenge
            // (SYN/)ACK).
            //
            // Anything that's not a SYN is invalid, since they need an ACK.
            return (
                State::WaitingOnOpeningAcks(peers.into_peer_pair()),
                control == Some(Control::SYN),
            );
        }
    };

    if !WaitingOnOpeningAcksPeer::ack_valid(peers.as_ref(), ack) {
        return (State::WaitingOnOpeningAcks(peers.into_peer_pair()), false);
    }

    peers.sender = peers.sender.update_sender(seq, logical_len, ack, wnd, control);
    peers.receiver = peers.receiver.update_receiver(ack);

    let peers = peers.into_peer_pair();
    // For a valid FIN, we move to Established because that's the state that
    // handles the closing handshake. It's possible the receiver will consider
    // this FIN invalid and send an RST, but no matter what the connection isn't
    // long for this world.
    if control == Some(Control::FIN) || (peers.original.saw_ack && peers.reply.saw_ack) {
        (
            State::Established(PeerPair { original: peers.original.peer, reply: peers.reply.peer }),
            true,
        )
    } else {
        (State::WaitingOnOpeningAcks(peers), true)
    }
}

/// State transitions for in-range segments regardless of direction:
/// - SYN: Invalid
/// - RST: Established (Unreplied RST)
/// - FIN: Established (Closing)
/// - ACK: Established
///
/// This state deletes the connection once FINs from both peers have been ACKed.
fn update_for_established(
    mut peers: UpdatePeers<Peer>,
    segment: &SegmentHeader,
    payload_len: usize,
) -> (State, bool) {
    // NOTE: Segments after a FIN are somewhat invalid, but we do not
    // attempt to handle them specially. Per RFC 9293 3.10.7.4:
    //
    //   Seventh, process the segment text. [After FIN,] this should not
    //   occur since a FIN has been received from the remote side. Ignore
    //   the segment text.
    //
    // Because these segments aren't completely invalid, handling them
    // properly (and consistently with the endpoints) is difficult. It is
    // not needed for correctness, since the connection will be torn down as
    // soon as there's an ACK for both FINs anyway. This extra invalid data
    // does not change that.
    //
    // Neither Linux nor gVisor do anything special for these segments.

    let logical_len = segment.len(payload_len);
    let &SegmentHeader { seq, ack, wnd, control, options: _, push: _ } = segment;

    if control == Some(Control::SYN) || !Peer::seq_valid(peers.as_ref(), seq, logical_len) {
        return (State::Established(peers.into_peer_pair()), false);
    }

    // RST segments are valid if the sequence number is valid.
    if control == Some(Control::RST) {
        peers.sender.unreplied_rst = true;
        return (State::Established(peers.into_peer_pair()), true);
    }

    // From RFC 9293:
    //   If the ACK control bit is set, this field contains the value of the
    //   next sequence number the sender of the segment is expecting to receive.
    //   Once a connection is established, this is always sent.
    let ack = match ack {
        Some(ack) => ack,
        None => {
            return (State::Established(peers.into_peer_pair()), false);
        }
    };

    if !Peer::ack_valid(peers.as_ref(), ack) {
        return (State::Established(peers.into_peer_pair()), false);
    }

    peers.sender = peers.sender.update_sender(seq, logical_len, ack, wnd, control);
    peers.receiver = peers.receiver.update_receiver(ack);

    if peers.sender.fin_state.acked() && peers.receiver.fin_state.acked() {
        // Removing the entry immediately is not expected to break any
        // use-cases. The endpoints are ultimately responsible for
        // respecting the TIME_WAIT state.
        //
        // The NAT entry will be removed as a consequence, but this is only
        // a problem if a server wanted to reopen the connection with the
        // client (but only during TIME_WAIT).
        //
        // TODO(https://fxbug.dev/355200767): Add TimeWait and reopening
        // connections once simultaneous open is supported.
        (State::Closed, true)
    } else {
        (State::Established(peers.into_peer_pair()), true)
    }
}

#[cfg(test)]
mod tests {
    use super::{FinState, Peer, PeerPair, State, SynSent, UpdatePeers, WaitingOnOpeningAcksPeer};

    use assert_matches::assert_matches;
    use netstack3_base::{
        Control, HandshakeOptions, Segment, SegmentHeader, SegmentOptions, SeqNum,
        UnscaledWindowSize, WindowScale, WindowSize,
    };
    use test_case::test_case;

    use crate::conntrack::ConnectionDirection;

    const ORIGINAL_ISS: SeqNum = SeqNum::new(0);
    const ORIGINAL_WND: UnscaledWindowSize = UnscaledWindowSize::from_u16(16);
    const ORIGINAL_WS: WindowScale = WindowScale::new(3).unwrap();
    const ORIGINAL_PAYLOAD_LEN: usize = 12;

    const REPLY_ISS: SeqNum = SeqNum::new(8192);
    const REPLY_WND: UnscaledWindowSize = UnscaledWindowSize::from_u16(17);
    const REPLY_WS: WindowScale = WindowScale::new(4).unwrap();
    const REPLY_PAYLOAD_LEN: usize = 13;

    // The below constants represent the segments and states along the happy
    // path of the standard TCP connection handshake (i.e. SYN, SYN/ACK,
    // ACK). This is meant to make sure all of the tests are using consistent
    // information and to make it obvious what each test case is doing
    // differently from the others.

    fn default_syn_sent_state() -> SynSent {
        SynSent {
            iss: ORIGINAL_ISS,
            logical_len: 1,
            advertised_window_scale: Some(ORIGINAL_WS),
            window_size: ORIGINAL_WND << WindowScale::ZERO,
            dir: ConnectionDirection::Original,
        }
    }

    fn default_original_waiting_on_opening_acks_inner_peer() -> Peer {
        Peer {
            window_scale: ORIGINAL_WS,
            max_wnd: ORIGINAL_WND << WindowScale::ZERO,
            max_wnd_seq: REPLY_ISS + (ORIGINAL_WND << WindowScale::ZERO),
            max_next_seq: ORIGINAL_ISS + 1,
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }
    }

    fn default_original_waiting_on_opening_acks_peer() -> WaitingOnOpeningAcksPeer {
        WaitingOnOpeningAcksPeer {
            peer: default_original_waiting_on_opening_acks_inner_peer(),
            iss: ORIGINAL_ISS,
            saw_ack: true,
            advertised_window_scale: Some(ORIGINAL_WS),
        }
    }

    fn default_reply_waiting_on_opening_acks_inner_peer() -> Peer {
        Peer {
            window_scale: REPLY_WS,
            max_wnd: REPLY_WND << WindowScale::ZERO,
            max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << WindowScale::ZERO),
            max_next_seq: REPLY_ISS + 1,
            unacked_data: true,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }
    }

    fn default_reply_waiting_on_opening_acks_peer() -> WaitingOnOpeningAcksPeer {
        WaitingOnOpeningAcksPeer {
            peer: default_reply_waiting_on_opening_acks_inner_peer(),
            iss: REPLY_ISS,
            saw_ack: false,
            advertised_window_scale: Some(REPLY_WS),
        }
    }

    fn default_original_established_peer() -> Peer {
        Peer {
            window_scale: ORIGINAL_WS,
            max_wnd: ORIGINAL_WND << ORIGINAL_WS,
            max_wnd_seq: REPLY_ISS + 1 + (ORIGINAL_WND << ORIGINAL_WS),
            max_next_seq: ORIGINAL_ISS + 1,
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }
    }

    fn default_reply_established_peer() -> Peer {
        Peer {
            window_scale: REPLY_WS,
            // In the non-simultaneous-open case, the "reply" peer hasn't sent a
            // plain ACK, and window scale is never applied in a packet with the SYN
            // flag set.
            max_wnd: REPLY_WND << WindowScale::ZERO,
            max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << WindowScale::ZERO),
            max_next_seq: REPLY_ISS + 1,
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }
    }

    fn valid_original_syn_segment() -> SegmentHeader {
        let (header, _) = Segment::<()>::syn(
            ORIGINAL_ISS,
            ORIGINAL_WND,
            HandshakeOptions { window_scale: Some(ORIGINAL_WS), ..Default::default() },
        )
        .into_parts();
        header
    }

    fn valid_reply_syn_ack_segment() -> SegmentHeader {
        let (header, _) = Segment::<()>::syn_ack(
            REPLY_ISS,
            ORIGINAL_ISS + 1,
            REPLY_WND,
            HandshakeOptions { window_scale: Some(REPLY_WS), ..Default::default() },
        )
        .into_parts();
        header
    }

    fn valid_original_established_segment() -> SegmentHeader {
        let (header, _) = Segment::<()>::ack(
            ORIGINAL_ISS + 1,
            REPLY_ISS + 1,
            ORIGINAL_WND,
            SegmentOptions::default(),
        )
        .into_parts();
        header
    }

    fn valid_reply_established_segment() -> SegmentHeader {
        let (header, _) = Segment::<()>::ack(
            REPLY_ISS + 1,
            ORIGINAL_ISS + 1,
            REPLY_WND,
            SegmentOptions::default(),
        )
        .into_parts();
        header
    }

    impl Peer {
        pub fn arbitrary() -> Peer {
            Peer {
                max_next_seq: SeqNum::new(0),
                window_scale: WindowScale::new(0).unwrap(),
                max_wnd_seq: SeqNum::new(0),
                unacked_data: false,
                max_wnd: WindowSize::new(0).unwrap(),
                fin_state: FinState::NotSent,
                unreplied_rst: false,
            }
        }
    }

    #[test]
    fn new_state_valid() {
        assert_eq!(
            State::new(&valid_original_syn_segment(), 0),
            Some(State::SynSent(default_syn_sent_state()))
        );

        assert_eq!(
            State::new(&valid_original_syn_segment(), 10),
            Some(State::SynSent(SynSent {
                // 10 plus the SYN.
                logical_len: 11,
                ..default_syn_sent_state()
            }))
        );
    }

    #[test]
    fn new_state_invalid() {
        assert_eq!(
            State::new(
                &SegmentHeader {
                    // We don't allow picking up connections already in progress.
                    ack: Some(SeqNum::new(0)),
                    ..valid_original_syn_segment()
                },
                ORIGINAL_PAYLOAD_LEN
            ),
            None
        );
    }

    #[test_case(None)]
    #[test_case(Some(Control::FIN))]
    #[test_case(Some(Control::RST))]
    fn syn_sent_original_non_syn_segment(control: Option<Control>) {
        let state = State::SynSent(default_syn_sent_state());
        let segment = SegmentHeader { control, ..valid_original_syn_segment() };

        let expected_state = state.clone();
        assert_eq!(
            state.update(&segment, /* payload_len */ 0, ConnectionDirection::Original),
            (expected_state, false)
        );
    }

    #[test_case(SegmentHeader {
        seq: ORIGINAL_ISS + 1,
        ..valid_original_syn_segment()
    }; "different ISS")]
    #[test_case(SegmentHeader {
        options: HandshakeOptions {
            window_scale: Some(WindowScale::new(ORIGINAL_WS.get() + 1).unwrap()),
            ..Default::default()
        }.into(),
        ..valid_original_syn_segment()
    }; "different window scale")]
    #[test_case(SegmentHeader {
        ack: Some(SeqNum::new(10)),
        ..valid_original_syn_segment()
    }; "ack not allowed")]
    fn syn_sent_original_syn_not_retransmit(segment: SegmentHeader) {
        let state = State::SynSent(default_syn_sent_state());

        let expected_state = state.clone();
        assert_eq!(
            state.update(&segment, /* payload_len */ 0, ConnectionDirection::Original),
            (expected_state, false)
        );
    }

    #[test]
    fn syn_sent_original_syn_retransmit() {
        let state = State::SynSent(default_syn_sent_state());
        let segment = SegmentHeader {
            wnd: (u16::from(ORIGINAL_WND) + 10).into(),
            ..valid_original_syn_segment()
        };

        let result = assert_matches!(
            state.update(
                &segment,
                ORIGINAL_PAYLOAD_LEN,
                ConnectionDirection::Original
            ),
            (State::SynSent(s), true) => s
        );

        assert_eq!(
            result,
            SynSent {
                logical_len: ORIGINAL_PAYLOAD_LEN as u32 + 1,
                window_size: UnscaledWindowSize::from(u16::from(ORIGINAL_WND) + 10)
                    << WindowScale::ZERO,
                ..default_syn_sent_state()
            }
        )
    }

    #[test_case(None)]
    #[test_case(Some(Control::FIN))]
    fn syn_sent_reply_non_syn_segment(control: Option<Control>) {
        let state = State::SynSent(default_syn_sent_state());
        let segment = SegmentHeader { control, ..valid_reply_syn_ack_segment() };

        let expected_state = state.clone();
        assert_eq!(
            state.update(&segment, /* payload_len */ 0, ConnectionDirection::Reply),
            (expected_state, false)
        );
    }

    #[test_case(ORIGINAL_ISS, None; "small invalid")]
    #[test_case(
        ORIGINAL_ISS + 1, Some(State::Closed);
        "smallest valid"
    )]
    #[test_case(
        ORIGINAL_ISS + 2,
        None;
        "large invalid"
    )]
    fn syn_sent_reply_rst_segment(ack: SeqNum, new_state: Option<State>) {
        let state = State::SynSent(default_syn_sent_state());
        let segment = SegmentHeader {
            ack: Some(ack),
            control: Some(Control::RST),
            ..valid_reply_syn_ack_segment()
        };

        let (expected_state, valid) = match new_state {
            Some(state) => (state, true),
            None => (state.clone(), false),
        };

        assert_eq!(
            state.update(&segment, /*payload_len*/ 0, ConnectionDirection::Reply),
            (expected_state, valid)
        );
    }

    #[test_case(None)]
    #[test_case(Some(REPLY_WS))]
    fn syn_sent_reply_simultaneous_open(reply_advertised_window_scale: Option<WindowScale>) {
        let state = State::SynSent(default_syn_sent_state());
        let segment = SegmentHeader {
            ack: None,
            options: HandshakeOptions {
                window_scale: reply_advertised_window_scale,
                ..Default::default()
            }
            .into(),
            ..valid_reply_syn_ack_segment()
        };

        let new_state = assert_matches!(
            state.update(
                &segment,
                REPLY_PAYLOAD_LEN,
                ConnectionDirection::Reply
            ),
            (State::WaitingOnOpeningAcks(s), true) => s
        );

        let (original_window_scale, reply_window_scale) = match reply_advertised_window_scale {
            Some(s) => (ORIGINAL_WS, s),
            None => (WindowScale::ZERO, WindowScale::ZERO),
        };

        assert_eq!(
            new_state,
            PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        window_scale: original_window_scale,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        window_scale: reply_window_scale,
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    advertised_window_scale: reply_advertised_window_scale,
                    ..default_reply_waiting_on_opening_acks_peer()
                }
            }
        );
    }

    #[test_case(ORIGINAL_ISS; "too low")]
    #[test_case(ORIGINAL_ISS + 2; "too high")]
    fn syn_sent_reply_syn_ack_not_in_range(ack: SeqNum) {
        let state = State::SynSent(default_syn_sent_state());
        let segment = SegmentHeader { ack: Some(ack), ..valid_reply_syn_ack_segment() };

        let expected_state = state.clone();
        assert_eq!(
            state.update(&segment, /* payload_len */ 0, ConnectionDirection::Reply),
            (expected_state, false)
        );
    }

    #[test_case(None, 0, false)]
    #[test_case(Some(REPLY_WS), 0, false)]
    #[test_case(None, ORIGINAL_PAYLOAD_LEN, true)]
    #[test_case(Some(REPLY_WS), ORIGINAL_PAYLOAD_LEN, true)]
    fn syn_sent_reply_syn_ack(
        reply_advertised_window_scale: Option<WindowScale>,
        syn_payload_len: usize,
        expected_unacked_data: bool,
    ) {
        let state = State::SynSent(SynSent {
            logical_len: syn_payload_len as u32 + 1,
            ..default_syn_sent_state()
        });
        let segment = SegmentHeader {
            options: HandshakeOptions {
                window_scale: reply_advertised_window_scale,
                ..Default::default()
            }
            .into(),
            ..valid_reply_syn_ack_segment()
        };

        let new_state = assert_matches!(
            state.update(
                &segment,
                /* payload_len */ 0,
                ConnectionDirection::Reply
            ),
            (State::WaitingOnOpeningAcks(s), true) => s
        );

        let (original_window_scale, reply_window_scale) = match reply_advertised_window_scale {
            Some(s) => (ORIGINAL_WS, s),
            None => (WindowScale::ZERO, WindowScale::ZERO),
        };

        assert_eq!(
            new_state,
            PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        window_scale: original_window_scale,
                        max_next_seq: ORIGINAL_ISS + syn_payload_len + 1,
                        unacked_data: expected_unacked_data,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        window_scale: reply_window_scale,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    advertised_window_scale: reply_advertised_window_scale,
                    ..default_reply_waiting_on_opening_acks_peer()
                }
            }
        );
    }

    struct StateUpdateTestArgs {
        segment: SegmentHeader,
        payload_len: usize,
        dir: ConnectionDirection,
        expected: Option<State>,
    }

    #[test_case(
        StateUpdateTestArgs {
            segment: valid_original_established_segment(),
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: default_original_established_peer(),
                reply: default_reply_established_peer(),
            })),
        }; "original ack establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: valid_original_established_segment(),
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    max_next_seq: ORIGINAL_ISS + 1 + ORIGINAL_PAYLOAD_LEN,
                    unacked_data: true,
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "original ack with payload establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(REPLY_ISS),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd: ORIGINAL_WND << ORIGINAL_WS,
                        max_wnd_seq: REPLY_ISS + (ORIGINAL_WND << ORIGINAL_WS),
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "original ack unacked syn"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(REPLY_ISS + 1),
                ..valid_original_syn_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    // Because of the SYN flag, the window is unscaled.
                    max_wnd: ORIGINAL_WND << WindowScale::ZERO,
                    max_wnd_seq: REPLY_ISS + 1 + (ORIGINAL_WND << WindowScale::ZERO),
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "syn ack establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::FIN),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    fin_state: FinState::Sent(ORIGINAL_ISS + 1),
                    max_next_seq: ORIGINAL_ISS + 2,
                    unacked_data: true,
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "fin ack establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: valid_reply_syn_ack_segment(),
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: default_original_waiting_on_opening_acks_peer(),
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "reply syn ack retransmission"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: valid_original_syn_segment(),
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: default_original_waiting_on_opening_acks_peer(),
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "original syn retransmission without ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: None,
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: default_original_waiting_on_opening_acks_peer(),
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "reply syn retransmission without ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: None,
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: default_original_waiting_on_opening_acks_peer(),
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        // Window size scaled (17 << 4).
                        max_wnd: WindowSize::from_u32(272).unwrap(),
                        // ack (1) + max_wnd (272).
                        max_wnd_seq: SeqNum::new(273),
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply plain ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: REPLY_ISS + 1,
                control: Some(Control::FIN),
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    // We never saw a plain ACK from this peer, so we never saw a
                    // scaled window.
                    max_wnd: ORIGINAL_WND << WindowScale::ZERO,
                    max_wnd_seq: REPLY_ISS + (ORIGINAL_WND << WindowScale::ZERO),
                    ..default_original_established_peer()
                },
                reply: Peer {
                    // These are actually scaled, since we saw a FIN/ACK (only
                    // unscaled with SYN).
                    max_wnd: REPLY_WND << REPLY_WS,
                    max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << REPLY_WS),
                    fin_state: FinState::Sent(REPLY_ISS + 1),
                    max_next_seq: REPLY_ISS + 2,
                    unacked_data: true,
                    ..default_reply_established_peer()
                },
            })),
        }; "reply fin ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: ORIGINAL_ISS + 1,
                ..valid_original_syn_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "original syn with different iss"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                options: HandshakeOptions {
                    window_scale: Some(WindowScale::new(ORIGINAL_WS.get() + 1).unwrap()),
                    ..Default::default()
                }.into(),
                ..valid_original_syn_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "original syn with different window scale"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: REPLY_ISS + 1,
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "reply syn ack with different iss"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                options: HandshakeOptions {
                    window_scale: Some(WindowScale::new(REPLY_WS.get() + 1).unwrap()),
                    ..Default::default()
                }.into(),
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "reply syn ack with different window scale"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: None,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "plain segment missing ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: None,
                control: Some(Control::FIN),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "fin missing ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: valid_original_established_segment().seq + 100,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "ack seq too high (eq I)"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: valid_original_established_segment().seq - 100,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "ack seq and len too low (eq II)"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(valid_original_established_segment().ack.unwrap() + 10_000),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "ack too high (eq III)"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                // MAXACKWINDOW in this case is 65535*2^4 (4 is the receive window scale).
                ack: Some(valid_original_established_segment().ack.unwrap() - 1_100_000),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "ack too low (eq IV)"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "original rst with ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ack: None,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "original rst without ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ack: Some(valid_original_established_segment().ack.unwrap() + 10_000),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: default_reply_waiting_on_opening_acks_peer(),
            })),
        }; "original rst with invalid ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_original_established_segment().seq + 100,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "original rst with invalid seq"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: default_original_waiting_on_opening_acks_peer(),
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply rst with ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ack: None,
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: default_original_waiting_on_opening_acks_peer(),
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply rst without ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_reply_established_segment().seq + 100,
                ack: None,
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "reply rst with invalid seq"
    )]
    fn waiting_on_opening_acks_test(args: StateUpdateTestArgs) {
        let state = State::WaitingOnOpeningAcks(PeerPair {
            original: default_original_waiting_on_opening_acks_peer(),
            reply: default_reply_waiting_on_opening_acks_peer(),
        });

        let (new_state, valid) = match args.expected {
            Some(new_state) => (new_state, true),
            None => (state.clone(), false),
        };

        assert_eq!(state.update(&args.segment, args.payload_len, args.dir), (new_state, valid));
    }

    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                ack: Some(REPLY_ISS + REPLY_PAYLOAD_LEN + 1),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    max_wnd_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1 + (ORIGINAL_WND << ORIGINAL_WS),
                    max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_original_established_peer()
                },
                reply: Peer {
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                    ..default_reply_established_peer()
                },
            })),
        }; "original ack acknowledging all data establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                ack: Some(REPLY_ISS + 1),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_original_established_peer()
                },
                reply: Peer {
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_reply_established_peer()
                },
            })),
        }; "original ack acknowledging syn bit only establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                ack: Some(REPLY_ISS + REPLY_PAYLOAD_LEN + 1),
                ..valid_original_established_segment()
            },
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    max_wnd_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1 + (ORIGINAL_WND << ORIGINAL_WS),
                    max_next_seq: ORIGINAL_ISS + 2 * ORIGINAL_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_original_established_peer()
                },
                reply: Peer {
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                    ..default_reply_established_peer()
                },
            })),
        }; "original ack with payload acknowledging all data establishes connection"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                control: None,
                ack: Some(ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1),
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        // Window size scaled (17 << 4).
                        max_wnd: WindowSize::from_u32(272).unwrap(),
                        // ack (13) + max_wnd (272).
                        max_wnd_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1 + 272,
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply plain ack acknowledging all original data"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                control: Some(Control::FIN),
                ack: Some(ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1),
                ..valid_reply_syn_ack_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    max_wnd: ORIGINAL_WND << WindowScale::ZERO,
                    max_wnd_seq: REPLY_ISS + (ORIGINAL_WND << WindowScale::ZERO),
                    max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                    ..default_original_established_peer()
                },
                reply: Peer {
                    max_wnd: REPLY_WND << REPLY_WS,
                    max_wnd_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1 + (REPLY_WND << REPLY_WS),
                    fin_state: FinState::Sent(REPLY_ISS + REPLY_PAYLOAD_LEN + 1),
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 2,
                    unacked_data: true,
                    ..default_reply_established_peer()
                },
            })),
        }; "reply fin ack acknowledging all original data"
    )]
    fn waiting_on_opening_acks_with_data_test(args: StateUpdateTestArgs) {
        let state = State::WaitingOnOpeningAcks(PeerPair {
            original: WaitingOnOpeningAcksPeer {
                peer: Peer {
                    max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_original_waiting_on_opening_acks_inner_peer()
                },
                ..default_original_waiting_on_opening_acks_peer()
            },
            reply: WaitingOnOpeningAcksPeer {
                peer: Peer {
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                    ..default_reply_waiting_on_opening_acks_inner_peer()
                },
                ..default_reply_waiting_on_opening_acks_peer()
            },
        });

        let (new_state, valid) = match args.expected {
            Some(new_state) => (new_state, true),
            None => (state.clone(), false),
        };

        assert_eq!(state.update(&args.segment, args.payload_len, args.dir), (new_state, valid));
    }

    #[test_case(
        StateUpdateTestArgs {
            segment: valid_original_established_segment(),
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_established_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: true,
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "original plain ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(REPLY_ISS + REPLY_PAYLOAD_LEN + 1),
                ..valid_original_established_segment()
            },
            payload_len: 20,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1 + (ORIGINAL_WND << ORIGINAL_WS),
                        max_next_seq: ORIGINAL_ISS + 1 + 20,
                        unacked_data: true,
                        ..default_original_established_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        unacked_data: false,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: true,
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "original ack with payload acknowledging all data"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(REPLY_ISS + 1),
                ..valid_original_syn_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: REPLY_ISS + 1 + (ORIGINAL_WND << WindowScale::ZERO),
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: true,
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "original syn ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(REPLY_ISS),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: REPLY_ISS + (ORIGINAL_WND << ORIGINAL_WS),
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_established_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "original ack with unacked syn"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::FIN),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    fin_state: FinState::Sent(valid_original_established_segment().seq),
                    ..default_original_established_peer()
                },
                reply: Peer {
                    max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_reply_established_peer()
                },
            })),
        }; "original fin ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                seq: REPLY_ISS,
                control: Some(Control::SYN),
                options: HandshakeOptions {
                    window_scale: Some(REPLY_WS),
                    ..Default::default()
                }
                .into(),
                ..valid_reply_established_segment()
            },
            payload_len: REPLY_PAYLOAD_LEN,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply syn ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: Some(ORIGINAL_ISS),
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd: REPLY_WND << REPLY_WS,
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << REPLY_WS),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply ack with unacked syn"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: valid_reply_established_segment(),
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd: REPLY_WND << REPLY_WS,
                        max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << REPLY_WS),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply plain ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_original_established_segment().seq + 100,
                ack: Some(REPLY_ISS + 1),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "original rst with invalid seq and valid ack for syn sent"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_original_established_segment().seq + 100,
                ack: Some(REPLY_ISS),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "original rst with invalid seq and invalid ack for syn sent"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_original_established_segment().seq + 100,
                ack: None,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "original rst with invalid seq and no ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_reply_established_segment().seq + 100,
                ack: Some(ORIGINAL_ISS + 1),
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::WaitingOnOpeningAcks(PeerPair {
                original: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                        unacked_data: true,
                        ..default_original_waiting_on_opening_acks_inner_peer()
                    },
                    saw_ack: false,
                    ..default_original_waiting_on_opening_acks_peer()
                },
                reply: WaitingOnOpeningAcksPeer {
                    peer: Peer {
                        unreplied_rst: true,
                        max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                        max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                        ..default_reply_waiting_on_opening_acks_inner_peer()
                    },
                    ..default_reply_waiting_on_opening_acks_peer()
                },
            })),
        }; "reply rst with invalid seq and valid ack for syn sent"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_reply_established_segment().seq + 100,
                ack: Some(ORIGINAL_ISS),
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "reply rst with invalid seq and invalid ack for syn sent"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_reply_established_segment().seq + 100,
                ack: None,
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "reply rst with invalid seq and no ack"
    )]
    fn waiting_on_opening_acks_simultaneous_open(args: StateUpdateTestArgs) {
        let state = State::WaitingOnOpeningAcks(PeerPair {
            original: WaitingOnOpeningAcksPeer {
                peer: Peer {
                    max_next_seq: ORIGINAL_ISS + ORIGINAL_PAYLOAD_LEN + 1,
                    unacked_data: true,
                    ..default_original_waiting_on_opening_acks_inner_peer()
                },
                saw_ack: false,
                ..default_original_waiting_on_opening_acks_peer()
            },
            reply: WaitingOnOpeningAcksPeer {
                peer: Peer {
                    max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                    max_next_seq: REPLY_ISS + REPLY_PAYLOAD_LEN + 1,
                    ..default_reply_waiting_on_opening_acks_inner_peer()
                },
                ..default_reply_waiting_on_opening_acks_peer()
            },
        });

        let (new_state, valid) = match args.expected {
            Some(new_state) => (new_state, true),
            None => (state.clone(), false),
        };

        assert_eq!(state.update(&args.segment, args.payload_len, args.dir), (new_state, valid));
    }

    #[test_case(ConnectionDirection::Original; "original sends first ack")]
    #[test_case(ConnectionDirection::Reply; "reply sends first ack")]
    fn waiting_on_opening_acks_simultaneous_open_establishes_connection(
        first_ack_dir: ConnectionDirection,
    ) {
        let state = State::WaitingOnOpeningAcks(PeerPair {
            original: WaitingOnOpeningAcksPeer {
                peer: Peer {
                    max_next_seq: ORIGINAL_ISS + 1,
                    unacked_data: true,
                    ..default_original_waiting_on_opening_acks_inner_peer()
                },
                saw_ack: false,
                ..default_original_waiting_on_opening_acks_peer()
            },
            reply: WaitingOnOpeningAcksPeer {
                peer: Peer {
                    max_wnd_seq: ORIGINAL_ISS + (REPLY_WND << WindowScale::ZERO),
                    max_next_seq: REPLY_ISS + 1,
                    unacked_data: true,
                    ..default_reply_waiting_on_opening_acks_inner_peer()
                },
                ..default_reply_waiting_on_opening_acks_peer()
            },
        });

        let (first_segment, second_segment, second_ack_dir) = match first_ack_dir {
            ConnectionDirection::Original => (
                valid_original_established_segment(),
                valid_reply_established_segment(),
                ConnectionDirection::Reply,
            ),
            ConnectionDirection::Reply => (
                valid_reply_established_segment(),
                valid_original_established_segment(),
                ConnectionDirection::Original,
            ),
        };

        let (state, valid) = state.update(&first_segment, /* payload_len */ 0, first_ack_dir);
        assert!(valid);
        assert_matches!(state, State::WaitingOnOpeningAcks(_));

        let (state, valid) =
            state.update(&second_segment, /* payload_len */ 0, second_ack_dir);
        assert!(valid);
        assert_eq!(
            state,
            State::Established(PeerPair {
                original: default_original_established_peer(),
                reply: Peer {
                    max_wnd: REPLY_WND << REPLY_WS,
                    max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << REPLY_WS),
                    ..default_reply_established_peer()
                },
            })
        );
    }

    #[test_case(FinState::NotSent, SeqNum::new(9) => FinState::Sent(SeqNum::new(9)))]
    #[test_case(FinState::Sent(SeqNum::new(9)), SeqNum::new(8) => FinState::Sent(SeqNum::new(8)))]
    #[test_case(FinState::Sent(SeqNum::new(9)), SeqNum::new(9) => FinState::Sent(SeqNum::new(9)))]
    #[test_case(FinState::Sent(SeqNum::new(9)), SeqNum::new(10) => FinState::Sent(SeqNum::new(9)))]
    #[test_case(FinState::Acked, SeqNum::new(9) => FinState::Acked)]
    fn fin_state_update_fin_sent(fin_state: FinState, seq: SeqNum) -> FinState {
        fin_state.update_fin_sent(seq)
    }

    #[test_case(FinState::NotSent, SeqNum::new(10) => FinState::NotSent)]
    #[test_case(FinState::Sent(SeqNum::new(9)), SeqNum::new(9) => FinState::Sent(SeqNum::new(9)))]
    #[test_case(FinState::Sent(SeqNum::new(9)), SeqNum::new(10) => FinState::Acked)]
    #[test_case(FinState::Acked, SeqNum::new(10) => FinState::Acked)]
    fn fin_state_update_ack_received(fin_state: FinState, ack: SeqNum) -> FinState {
        fin_state.update_ack_received(ack)
    }

    const RECV_MAX_NEXT_SEQ: SeqNum = SeqNum::new(66_001);
    const RECV_MAX_WND_SEQ: SeqNum = SeqNum::new(1424);

    #[test_case(SeqNum::new(1) => true; "success low")]
    #[test_case(RECV_MAX_NEXT_SEQ => true; "success high")]
    #[test_case(RECV_MAX_NEXT_SEQ + 1 => false; "bad equation III")]
    #[test_case(SeqNum::new(0) => false; "bad equation IV")]
    fn ack_valid_test(ack: SeqNum) -> bool {
        let peers = UpdatePeers {
            sender: Peer { max_next_seq: SeqNum::new(1024), ..Peer::arbitrary() },

            // MAXACKWINDOW is going to be 66000 due to window shift of 0.
            receiver: Peer {
                window_scale: WindowScale::new(0).unwrap(),
                max_wnd: WindowSize::new(400).unwrap(),
                max_next_seq: RECV_MAX_NEXT_SEQ,
                max_wnd_seq: RECV_MAX_WND_SEQ,
                ..Peer::arbitrary()
            },
            // The direction doesn't matter.
            dir: ConnectionDirection::Original,
        };

        Peer::ack_valid(peers.as_ref(), ack)
    }

    #[test_case(SeqNum::new(424), 200 => true; "success low")]
    #[test_case(RECV_MAX_WND_SEQ, 0 => true; "success high")]
    #[test_case(RECV_MAX_WND_SEQ + 1, 0 => false; "bad equation I")]
    #[test_case(SeqNum::new(424), 199 => false; "bad equation II")]
    fn seq_valid_test(seq: SeqNum, len: u32) -> bool {
        let peers = UpdatePeers {
            sender: Peer { max_next_seq: SeqNum::new(1024), ..Peer::arbitrary() },

            receiver: Peer {
                window_scale: WindowScale::new(0).unwrap(),
                max_wnd: WindowSize::new(400).unwrap(),
                max_next_seq: RECV_MAX_NEXT_SEQ,
                max_wnd_seq: RECV_MAX_WND_SEQ,
                ..Peer::arbitrary()
            },
            // The direction doesn't matter.
            dir: ConnectionDirection::Original,
        };

        Peer::seq_valid(peers.as_ref(), seq, len)
    }

    struct PeerUpdateSenderArgs {
        seq: SeqNum,
        len: u32,
        ack: SeqNum,
        wnd: UnscaledWindowSize,
        control: Option<Control>,
    }

    #[test_case(
        Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        },
        PeerUpdateSenderArgs {
            seq: SeqNum::new(1025),
            len: 10,
            ack: SeqNum::new(100),
            wnd: UnscaledWindowSize::from_u32(4),
            control: None,
        } => Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(32).unwrap(),
            max_wnd_seq: SeqNum::new(132),
            max_next_seq: SeqNum::new(1035),
            unacked_data: true,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }; "packet larger"
    )]
    #[test_case(
        Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        },
        PeerUpdateSenderArgs {
            seq: SeqNum::new(1000),
            len: 10,
            ack: SeqNum::new(0),
            wnd: UnscaledWindowSize::from_u32(0),
            control: None,
        } => Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }; "packet smaller"
    )]
    #[test_case(
        Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        },
        PeerUpdateSenderArgs {
            seq: SeqNum::new(1000),
            len: 10,
            ack: SeqNum::new(0),
            wnd: UnscaledWindowSize::from_u32(0),
            control: Some(Control::FIN),
        } => Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::Sent(SeqNum::new(1000 + 9)),
            unreplied_rst: false,
        }; "fin sent"
    )]
    #[test_case(
        Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        },
        PeerUpdateSenderArgs {
            seq: SeqNum::new(1000),
            len: 10,
            ack: SeqNum::new(0),
            wnd: UnscaledWindowSize::from_u32(4),
            control: Some(Control::SYN),
        } => Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: false,
        }; "syn ack ignores window_scale"
    )]
    #[test_case(
        Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: true,
        },
        PeerUpdateSenderArgs {
            seq: SeqNum::new(1000),
            len: 10,
            ack: SeqNum::new(0),
            wnd: UnscaledWindowSize::from_u32(0),
            control: None,
        } => Peer {
            window_scale: WindowScale::new(3).unwrap(),
            max_wnd: WindowSize::new(16).unwrap(),
            max_wnd_seq: SeqNum::new(127),
            max_next_seq: SeqNum::new(1024),
            unacked_data: false,
            fin_state: FinState::NotSent,
            unreplied_rst: true,
        }; "preserves unreplied rst"
    )]
    fn peer_update_sender_test(peer: Peer, args: PeerUpdateSenderArgs) -> Peer {
        peer.update_sender(args.seq, args.len, args.ack, args.wnd, args.control)
    }

    #[test_case(
        Peer { max_next_seq: SeqNum::new(1024), ..Peer::arbitrary() },
        SeqNum::new(1024) => Peer { max_next_seq: SeqNum::new(1024), ..Peer::arbitrary() };
        "unset unacked data"
    )]
    #[test_case(
        Peer { max_next_seq: SeqNum::new(1024), ..Peer::arbitrary() },
        SeqNum::new(1023) => Peer { max_next_seq: SeqNum::new(1024), ..Peer::arbitrary() };
        "don't unset unacked data"
    )]
    #[test_case(
        Peer { fin_state: FinState::Sent(SeqNum::new(9)), ..Peer::arbitrary() },
        SeqNum::new(10) => Peer { fin_state: FinState::Acked, ..Peer::arbitrary() };
        "update fin state"
    )]
    #[test_case(
        Peer { unreplied_rst: true, ..Peer::arbitrary() },
        SeqNum::new(0) => Peer { unreplied_rst: false, ..Peer::arbitrary() };
        "reset unreplied rst"
    )]
    fn peer_update_receiver_test(peer: Peer, ack: SeqNum) -> Peer {
        peer.update_receiver(ack)
    }

    // This is mostly to ensure that we don't accidentally overflow a u32, since
    // it's such a basic calculation.
    #[test]
    fn peer_max_ack_window() {
        let max_peer = Peer { window_scale: WindowScale::MAX, ..Peer::arbitrary() };
        let min_peer = Peer { window_scale: WindowScale::new(0).unwrap(), ..Peer::arbitrary() };

        assert_eq!(max_peer.max_ack_window(), 1_081_344_000u32);
        assert_eq!(min_peer.max_ack_window(), 66_000u32);
    }

    #[test_case(
        StateUpdateTestArgs {
            segment: valid_original_established_segment(),
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    // This is becoming true because segment.seq + payload_len >
                    // original.max_next_seq.
                    unacked_data: true,
                    max_next_seq:
                        default_original_established_peer().max_next_seq + ORIGINAL_PAYLOAD_LEN,
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "update original"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: valid_reply_established_segment(),
            payload_len: REPLY_PAYLOAD_LEN,
            dir: ConnectionDirection::Reply,
            expected: Some(State::Established(PeerPair {
                original: default_original_established_peer(),
                reply: Peer {
                    // These are scaled since we saw a plain ACK (only unscaled with SYN).
                    max_wnd: REPLY_WND << REPLY_WS,
                    max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << REPLY_WS),
                    // This peer just sent new data.
                    unacked_data: true,
                    max_next_seq: default_reply_established_peer().max_next_seq + REPLY_PAYLOAD_LEN,
                    ..default_reply_established_peer()
                },
            })),
        }; "update reply"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::FIN),
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::Established(PeerPair {
                original: default_original_established_peer(),
                reply: Peer {
                    // These are scaled since we saw a FIN/ACK (only unscaled with SYN).
                    max_wnd: REPLY_WND << REPLY_WS,
                    max_wnd_seq: ORIGINAL_ISS + 1 + (REPLY_WND << REPLY_WS),
                    max_next_seq: default_reply_established_peer().max_next_seq + 1,
                    unacked_data: true,
                    fin_state: FinState::Sent(valid_reply_established_segment().seq),
                    ..default_reply_established_peer()
                },
            })),
        }; "closing"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                ack: None,
                ..valid_original_established_segment()
            },
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "missing ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                // Too low. Doesn't meet equation II.
                seq: valid_original_established_segment().seq - 100,
                ..valid_original_established_segment()
            },
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "invalid equation bounds"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::SYN),
                ..valid_original_established_segment()
            },
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "SYN not allowed"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                // Fails equation III.
                ack: Some(valid_original_established_segment().ack.unwrap() + 10_000),
                ..valid_original_established_segment()
            },
            payload_len: ORIGINAL_PAYLOAD_LEN,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "invalid ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    unreplied_rst: true,
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "original rst with ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ack: None,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    unreplied_rst: true,
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "original rst without ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ack: Some(valid_original_established_segment().ack.unwrap() + 10_000),
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: Some(State::Established(PeerPair {
                original: Peer {
                    unreplied_rst: true,
                    ..default_original_established_peer()
                },
                reply: default_reply_established_peer(),
            })),
        }; "original rst with invalid ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_original_established_segment().seq - 100,
                ..valid_original_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Original,
            expected: None,
        }; "original rst with invalid seq"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::Established(PeerPair {
                original: default_original_established_peer(),
                reply: Peer {
                    unreplied_rst: true,
                    ..default_reply_established_peer()
                },
            })),
        }; "reply rst with ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                ack: None,
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: Some(State::Established(PeerPair {
                original: default_original_established_peer(),
                reply: Peer {
                    unreplied_rst: true,
                    ..default_reply_established_peer()
                },
            })),
        }; "reply rst without ack"
    )]
    #[test_case(
        StateUpdateTestArgs {
            segment: SegmentHeader {
                control: Some(Control::RST),
                seq: valid_reply_established_segment().seq - 200,
                ..valid_reply_established_segment()
            },
            payload_len: 0,
            dir: ConnectionDirection::Reply,
            expected: None,
        }; "reply rst with invalid seq"
    )]
    fn established_test(args: StateUpdateTestArgs) {
        let state = State::Established(PeerPair {
            original: default_original_established_peer(),
            reply: default_reply_established_peer(),
        });

        let (new_state, valid) = match args.expected {
            Some(new_state) => (new_state, true),
            None => (state.clone(), false),
        };

        assert_eq!(state.update(&args.segment, args.payload_len, args.dir), (new_state, valid));
    }

    #[test]
    fn closing_complete_test() {
        let state = State::Established(PeerPair {
            original: Peer {
                window_scale: WindowScale::new(2).unwrap(),
                max_wnd: WindowSize::new(0).unwrap(),
                max_wnd_seq: SeqNum::new(70_000),
                max_next_seq: SeqNum::new(1024),
                unacked_data: true,
                fin_state: FinState::Sent(SeqNum::new(1023)),
                unreplied_rst: false,
            },
            reply: Peer {
                window_scale: WindowScale::new(0).unwrap(),
                max_wnd: WindowSize::new(400).unwrap(),
                max_wnd_seq: SeqNum::new(1424),
                max_next_seq: SeqNum::new(66_001),
                unacked_data: false,
                fin_state: FinState::Acked,
                unreplied_rst: false,
            },
        });

        let segment = SegmentHeader {
            seq: SeqNum::new(66_100),
            ack: Some(SeqNum::new(1024)),
            wnd: UnscaledWindowSize::from(10),
            ..Default::default()
        };

        assert_matches!(
            state.update(&segment, /* payload_len */ 0, ConnectionDirection::Reply),
            (State::Closed, true)
        );
    }
}
