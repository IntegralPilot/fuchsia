<!-- Copyright 2026 The Fuchsia Authors. All rights reserved.
     Use of this source code is governed by a BSD-style license that can be
     found in the LICENSE file. -->

# UART Framing Protocol Library (`uart_fpl`) API Reference

The `uart_fpl` crate (`src/developer/lib/uart_fpl`) provides binary packet framing, streaming parsing, dynamic protocol negotiation, and sliding-window flow control primitives for transporting multiplexed packet streams over raw serial (UART) links.

This document serves as the high-level API reference for developers integrating with or maintaining `uart_fpl`. It focuses on library types, state machines, method contracts, lifecycle management, and architectural justifications (such as end-to-end backpressure), deliberately omitting low-level wire formats and bit-level representations.

---

## 1. Architectural Overview & Component Topology

`uart_fpl` is structured into five modular layers:

```
+-------------------------------------------------------------------------+
|                              Client Layer                               |
|        (Host CLI/Daemon: ffx-uart-driver, Target Runner: fdomain)       |
+------------------------------------+------------------------------------+
                                     |
               +---------------------+--------------------+
               |                                          |
               v                                          v
+-----------------------------+            +-----------------------------+
|    Dynamic Negotiation      |            |    Reliable Flow Control    |
|   (handshake::HostHandshake,|            |   (resend_sp::ResendSender, |
|    handshake::TargetHandshake)          |    resend_sp::ResendReceiver)|
+--------------+--------------+            +--------------+--------------+
               |                                          |
               +---------------------+--------------------+
                                     |
                                     v
               +------------------------------------------+
               |        Framing & Checksum Engine         |
               |      (frame::Frame, encode_frame,        |
               |       parser::FrameParser)               |
               +---------------------+--------------------+
                                     |
                                     v
+-------------------------------------------------------------------------+
|                          Physical / Virtual                             |
|                           UART Byte Stream                              |
+-------------------------------------------------------------------------+
```

### Module Breakdown

| Module | Primary Public Types | Responsibility |
| :--- | :--- | :--- |
| `frame` | `Frame`, `FrameType`, `ProtocolId`, `encode_frame` | In-memory frame representations, type enums, and zero-allocation frame serialization. |
| `parser` | `FrameParser` | Streaming parser with sync preamble recovery, two-tier header verification, and bounded memory buffers. |
| `handshake` | `HostHandshake`, `TargetHandshake`, `HandshakeResponse`, `HandshakeStatus` | Channel 0 dynamic protocol negotiation state machines and idempotent retransmission handling. |
| `resend_sp` | `ResendSender`, `ResendReceiver`, `FrameStatus`, `AckOutcome` | Sliding-window Go-Back-N transmission, sequence wrapping math, Karn's RTT tracking, and cumulative ACK validation. |
| `window` | `AckTracker` | Thread-safe, lock-free cumulative ACK coalescing and async task notification. |
| `error` | `FrameError`, `HandshakeError`, `RetransmissionLimitExceeded`, `UnexpectedSeqError` | Strongly typed errors across framing, negotiation, and transport. |

---

## 2. Framing & Serialization API (`frame.rs`)

### Constants

- **`CONTROL_CHANNEL_ID: u16 = 0`**: Reserved logical channel identifier used exclusively for link negotiation, session management, and link-level control signaling.
- **`MAX_PAYLOAD_SIZE: usize = 1024`**: Maximum permissible payload size in bytes accepted by encoders and parsers for a single frame.
- **`MAX_FRAME_SIZE: usize = 1041`**: Total maximum wire length of an encoded frame (13-byte header + 1024-byte payload + 4-byte CRC-32).
- **`DEFAULT_WINDOW_SIZE: u8 = 64`**: Default Go-Back-N sliding-window size for `ResendSender`.
- **`DEFAULT_MAX_RETRANSMISSION_ATTEMPTS: usize = 60`**: Default consecutive timeout retransmission limit before declaring a link failure.
- **`DEFAULT_RETRANSMISSION_TIMEOUT: Duration = Duration::from_millis(1000)`**: Default base timeout interval for retransmissions.

### Types & Enums

#### `FrameType`
Identifies the semantics of a frame:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FrameType {
    Data,          // Unreliable or sequenced payload data
    Ack,           // Cumulative sequence acknowledgment
    Close,         // Logical channel teardown
    Reset,         // Transport link reset
    NegotiateReq,  // Protocol negotiation request (Channel 0 only)
    NegotiateResp, // Protocol negotiation response (Channel 0 only)
    Unknown(u8),   // Forward-compatible unmapped frame type
}
```
Conversions: Implements `From<u8>`, `Into<u8>`, and `Display`.

#### `ProtocolId`
Identifies the framing and transport protocol negotiated across Channel 0:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolId {
    ResendSP,      // Sliding-window Go-Back-N with CRC-32 (wire ID 1)
    Unknown(u32),  // Forward-compatible representation for future protocol proposals
}
```
Methods:
- `wire_id(self) -> u32`: Returns the 32-bit wire integer representation.

#### `Frame`
In-memory representation of a validated, parsed protocol frame:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub session_id: u32,
    pub channel_id: u16,
    pub seq: u8,
    pub frame_type: FrameType,
    pub payload: Vec<u8>,
}
```
Constructors & Methods:
- `Frame::new(session_id: u32, channel_id: u16, seq: u8, frame_type: FrameType, payload: Vec<u8>) -> Result<Self, FrameError>`: Constructs a frame, returning `FrameError::PayloadTooLarge` if `payload.len() > MAX_PAYLOAD_SIZE`.
- `encode(&self) -> Result<Vec<u8>, FrameError>`: Serializes the frame into wire bytes.
- `wire_len(&self) -> usize`: Returns the total wire length in bytes (header + payload + checksums).

### Serialization Function

#### `encode_frame`
```rust
pub fn encode_frame(
    session_id: u32,
    channel_id: u16,
    seq: u8,
    frame_type: FrameType,
    payload: &[u8],
) -> Result<Vec<u8>, FrameError>
```
Zero-allocation wire serialization helper for transmitting byte slices directly without allocating an intermediate `Frame` struct. Encodes the binary header, header checksum (`HdrChk`), and trailing CRC-32. Returns `FrameError::PayloadTooLarge` if payload exceeds 1,024 bytes.

---

## 3. Streaming Frame Parser API (`parser.rs`)

### `FrameParser`
A streaming push-parser that accepts arbitrary byte slices from a serial or socket reader and emits validated frames.

```rust
pub struct FrameParser { /* private fields */ }
```

#### Lifecycle & Operations

- **`FrameParser::new() -> Self`**: Creates a new parser with an empty buffer and zeroed metrics. (Also implements `Default`).
- **`feed(&mut self, data: &[u8])`**: Ingests an incoming slice of bytes into the internal streaming buffer.
  - *Memory Guarantee*: Memory consumption is strictly bounded to 64 KiB (`MAX_BUFFER_CAPACITY`). If continuous incoming noise or garbage causes unconsumed bytes to exceed 64 KiB, the oldest unparsed bytes are drained automatically to prevent memory exhaustion.
  - *Compaction*: Consumed bytes are automatically reclaimed when the internal cursor passes a compaction threshold (4 KiB or 50% buffer capacity).
- **`next_frame(&mut self) -> Option<Frame>`**: Extracts the next complete, validated frame.
  - Returns `None` if more bytes are needed to complete a frame.
  - Automatically handles preamble resynchronization: slides past framing noise to align on valid sync preambles.
  - Enforces two-tier validation: verifies the header checksum before evaluating payload length, preventing corrupted lengths from causing buffer stalls.
- **`take_unconsumed(&mut self) -> Vec<u8>`**: Drains and returns all unparsed bytes currently residing in the buffer, resetting parser cursor to zero.
  - *Crucial Transition Contract*: When transitioning an active link between phases (e.g. from the Channel 0 handshake phase to the continuous data bridging loop), callers **must** call `take_unconsumed()` to extract any ingress data bytes received in the same read buffer as the handshake response, feeding them into the subsequent reader.
- **`checksum_errors(&self) -> u64`**: Returns the cumulative count of corrupted frames (header checksum or CRC-32 mismatches) encountered, useful for link-quality telemetry (`ffx uart status`).
- **`reset(&mut self)`**: Clears internal buffers, cursors, and error statistics.
- **`is_empty(&self) -> bool`**, **`unconsumed(&self) -> &[u8]`**, **`unconsumed_len(&self) -> usize`**: Inspection methods for buffer status and unparsed byte slices.

---

## 4. Channel 0 Dynamic Protocol Negotiation API (`handshake.rs`)

`uart_fpl` coordinates transport negotiation over Channel 0 before general data streams begin.

### `HostHandshake`
Coordinates protocol negotiation from the host side:

```rust
pub struct HostHandshake { /* private fields */ }
```

- **`new(proposed: Vec<ProtocolId>) -> Self`**: Initializes the host handshake with an ordered preference list of protocols (e.g., `vec![ProtocolId::ResendSP]`).
- **`start(&self) -> Result<(FrameType, Vec<u8>), HandshakeError>`**: Generates the initial negotiation request: `(FrameType::NegotiateReq, wire_payload)`. The caller encodes this payload into a frame on `CONTROL_CHANNEL_ID` and transmits it.
- **`handle_response(&self, payload: &[u8]) -> Result<ProtocolId, HandshakeError>`**: Parses and validates the target's `NegotiateResp` payload. Confirms the target selected a mutually agreeable protocol from the host's proposed list and returns the agreed `ProtocolId`.

### `TargetHandshake`
Coordinates protocol negotiation from the target side:

```rust
pub struct TargetHandshake { /* private fields */ }
```

- **`new(supported: Vec<ProtocolId>) -> Self`**: Initializes the target handler with the set of protocols supported by the target.
- **`handle_request(&self, payload: &[u8]) -> Result<(Option<ProtocolId>, Vec<u8>), HandshakeError>`**: Parses an incoming `NegotiateReq` payload, finds the highest-preference protocol supported by both sides, and serializes the `NegotiateResp` payload.
  - Returns `(Some(selected_protocol), response_payload)` on success.
  - Returns `(None, response_payload)` with `HandshakeStatus::NoCommonProtocol` if no common protocol exists.

#### Idempotent Handshake Retransmission Contract
Over noisy or lossy serial lines, the target's `NegotiateResp` may be dropped in transit. When this happens, the host's retry loop will retransmit the original `NegotiateReq` with the identical `session_id`.

**Contract**: The target implementation (e.g., in `fdomain-uart-driver`) **must** treat repeated `NegotiateReq` frames containing the active `session_id` as idempotent retransmissions: it must re-emit the cached `NegotiateResp` frame rather than rejecting the packet as out-of-order or triggering a transport reset. If a `NegotiateReq` arrives with a *new* session ID, it indicates a host restart, requiring a clean state reset.

---

## 5. Sliding-Window Flow Control API (`resend_sp.rs`)

The `resend_sp` module provides the Go-Back-N sliding-window state machines for reliable, sequenced packet transport.

### `ResendSender`
Maintains the transmission sliding window, sequence allocation, in-flight buffering, Karn's RTT measurement, and timeout retransmissions.

```rust
pub struct ResendSender { /* private fields */ }
```

- **`new(window_size: u8, max_retransmission_attempts: usize) -> Self`**: Creates a sender. Panics if `window_size == 0 || window_size > 128` or `max_retransmission_attempts == 0`. Default is window size 64, 60 maximum attempts.
- **`can_send(&self) -> bool`**: Returns `true` if `in_flight() < window_size`.
- **`in_flight(&self) -> u8`**: Returns the number of frames currently outstanding without acknowledgment (`next_seq - base`).
- **`is_idle(&self) -> bool`**: Returns `true` when all transmitted frames have been acknowledged (`base == next_seq`).
- **`enqueue_frame(&mut self, session_id: u32, channel_id: u16, frame_type: FrameType, payload: &[u8]) -> Result<(u8, Vec<u8>), FrameError>`**:
  - If `can_send()` is `false`, returns `Err(FrameError::WindowFull { .. })`.
  - Otherwise, assigns `next_seq`, serializes the frame, buffers it in an internal slot with an `Instant::now()` timestamp, increments `next_seq` (wrapping modulo 256), and returns `Ok((assigned_seq, wire_bytes))`.
- **`handle_ack(&mut self, ack_seq: u8) -> AckOutcome`**:
  - Evaluates an incoming cumulative ACK against the active window `[base, next_seq)`.
  - If valid: frees all buffer slots from `base` through `ack_seq`, advances `base = ack_seq + 1`, resets consecutive retransmission attempts to 0, and returns `AckOutcome::Advanced`.
  - If out-of-window (e.g., stale or duplicate ACK): returns `AckOutcome::OutOfWindow`.
- **`handle_timeout(&mut self) -> Result<Vec<Vec<u8>>, RetransmissionLimitExceeded>`**:
  - Invoked when the retransmission timer fires (default 1,000 ms).
  - If idle, returns an empty vector.
  - Increments consecutive retransmission attempts. If attempts reach `max_retransmission_attempts`, returns `Err(RetransmissionLimitExceeded)`.
  - Otherwise, returns clones of all unacknowledged frames `[base, next_seq)` to be retransmitted over the wire, updates their timestamps, and marks them as retransmitted.
- **`reset(&mut self)`**: Resets `base`, `next_seq`, and clears all buffered slots.

#### `AckOutcome`
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AckOutcome {
    Advanced {
        new_base: u8,
        rtt: Option<Duration>, // None if acknowledged frame was retransmitted (Karn's algorithm)
        all_acked: bool,
    },
    OutOfWindow {
        base: u8,
        next_seq: u8,
    },
}
```

### `ResendReceiver`
Maintains the receiving sequence state machine and cumulative ACK progression.

```rust
pub struct ResendReceiver { /* private fields */ }
```

- **`new() -> Self`**: Creates a receiver expecting sequence 0.
- **`expected_seq(&self) -> u8`**: Returns the sequence number currently expected.
- **`inspect(&self, seq: u8) -> FrameStatus`**:
  - Evaluates an incoming frame sequence *without mutating receiver state*.
  - Returns `FrameStatus::InOrder { seq }` if `seq == expected_seq`.
  - Returns `FrameStatus::OutOfOrder { seq, expected_seq, ack_seq }` if out-of-order or duplicate.
- **`advance_in_order(&mut self, seq: u8) -> Result<u8, UnexpectedSeqError>`**:
  - Verifies that `seq == expected_seq`. If mismatched, returns `Err(UnexpectedSeqError)`.
  - Advances `expected_seq` by 1 (modulo 256), records `seq` as the latest acknowledged frame, and returns `Ok(seq)` (the cumulative ACK sequence to be transmitted back to the sender).
- **`accept_seq(&mut self, seq: u8) -> FrameStatus`**:
  - Convenience single-step handler: inspects `seq`, and if in-order, automatically advances sequence progression via `advance_in_order(seq)` and returns `FrameStatus::InOrder`. If out-of-order, returns `FrameStatus::OutOfOrder` without mutating state.
  - Callers implementing two-phase flow control (e.g. attempting non-blocking writes to downstream sinks before committing sequence advancement) should use `inspect()` followed by `advance_in_order()` instead.
- **`current_ack_seq(&self) -> Option<u8>`**:
  - Returns the latest cumulative ACK sequence.
  - Returns `None` if sequence 0 has not yet been received. This prevents spurious emission of `ACK 255` on startup packet loss.
- **`reset(&mut self)`**: Resets `expected_seq` to 0 and clears acknowledgment history.

#### `FrameStatus`
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStatus {
    InOrder { seq: u8 },
    OutOfOrder {
        seq: u8,
        expected_seq: u8,
        ack_seq: Option<u8>,
    },
}
```

---

## 6. Asynchronous Cumulative ACK Tracking API (`window.rs`)

### `AckTracker`
`AckTracker` provides thread-safe, lock-free cumulative ACK coordination between asynchronous receiver and writer tasks.

```rust
#[derive(Debug, Clone, Default)]
pub struct AckTracker { /* private Arc-wrapped state */ }
```

In Go-Back-N, acknowledgments are cumulative: acknowledging sequence $N$ implicitly acknowledges all sequences prior to $N$. When a receiver processes multiple packets in rapid succession, queueing individual ACK frames creates reverse-path traffic storms. `AckTracker` coalesces these updates into a single atomic word.

- **`set_ack(&self, session_id: u32, seq: u8)`**: Records the latest cumulative ACK and wakes any task waiting on `wait_ack()`. Both `session_id` and `seq` are updated atomically in a single `AtomicU64` to prevent torn reads.
- **`take_ack(&self) -> Option<(u32, u8)>`**: Non-blocking extraction of the pending cumulative ACK. Clears the pending flag and returns `Some((session_id, seq))` if an ACK was pending, or `None` if already consumed.
- **`wait_ack(&self) -> (u32, u8)`**: Async future that suspends until a new cumulative ACK is recorded via `set_ack()`, returning the latest `(session_id, seq)`.
- **`register_waker(&self, cx: &mut Context<'_>)`**: Registers a task waker for manual polling contexts.

---

## 7. Error Handling API (`error.rs`)

All errors implement `std::error::Error`, `thiserror::Error`, and are `Clone`, `Copy`, `PartialEq`, `Eq`.

### `FrameError`
Errors encountered during frame construction, serialization, or window queueing:
- **`PayloadTooLarge(usize)`**: Payload length exceeds `MAX_PAYLOAD_SIZE` (1,024 bytes).
- **`WindowFull { window_size: u8, in_flight: u8 }`**: Transmission queue cannot accept another frame because the sliding window is full.

### `HandshakeError`
Errors encountered during Channel 0 protocol negotiation:
- **`MalformedPayload`**: Byte payload was truncated or contained invalid counts/fields.
- **`ProtocolNotProposed`**: Target selected a protocol that the host did not propose.
- **`NoCommonProtocol`**: Target and host do not share any supported protocol.
- **`UnsupportedStatus(u8)`**: Peer responded with an unrecognized status code.

### `RetransmissionLimitExceeded`
Fatal error returned by `ResendSender::handle_timeout` when consecutive retransmission attempts reach `max_retransmission_attempts`:
```rust
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
#[error("Too many retransmission failures, base={base}, next_seq={next_seq}, attempts={attempts}")]
pub struct RetransmissionLimitExceeded {
    pub base: u8,
    pub next_seq: u8,
    pub attempts: usize,
}
```

### `UnexpectedSeqError`
Error returned by `ResendReceiver::advance_in_order` when the sequence number being committed does not match `expected_seq`:
```rust
#[derive(Debug, Error, PartialEq, Eq, Clone, Copy)]
#[error("Unexpected sequence number: expected {expected}, received {received}")]
pub struct UnexpectedSeqError {
    pub expected: u8,
    pub received: u8,
}
```

---

## 8. High-Level Architectural Justifications

This section documents the architectural choices and design trade-offs embodied in the `uart_fpl` API.

### 8.1 End-to-End Backpressure Without Local Receiver Queueing

A primary architectural decision in `uart_fpl` is that **`ResendReceiver` does not buffer, reorder, or queue out-of-order frames**.

#### The Problem
In standard networking stacks (like TCP), receivers maintain out-of-order reassembly queues to absorb misordered packets. However, serial connections (physical UARTs or USB-to-serial bridges) operate under severe memory and CPU constraints:
1. The target runner (`fdomain-uart-driver`) runs on minimal embedded targets with tight heap limits.
2. If downstream consumers (such as FIDL channels or local sockets) become backpressured or blocked on I/O, a buffering receiver would accumulate frames in memory, risking OOM crashes.

#### The Solution: Two-Phase Check-and-Commit
`uart_fpl` enforces backpressure at the link level by decoupling sequence inspection from sequence advancement:

```
Incoming Frame (seq)
        │
        ▼
ResendReceiver::inspect(seq)
        │
   [In-Order?] ─── No ───► Drop frame, emit current cumulative ACK
        │
       Yes
        │
        ▼
Downstream Sink::try_send(payload)
        │
   [Capacity?] ─── No (Full) ──► Drop frame! DO NOT advance receiver!
        │
       Yes (Accepted)
        │
        ▼
ResendReceiver::advance_in_order(seq)
        │
        ▼
AckTracker::set_ack(seq) ──► Emit Cumulative ACK to Sender
```

1. **Step 1 (Inspect)**: The receiver checks `inspect(seq)`. If the frame is out of order, it is dropped immediately and the current cumulative ACK is returned.
2. **Step 2 (Non-Blocking Dispatch)**: If the frame is in-order, the caller does **not** advance the receiver immediately. Instead, it attempts to dispatch the payload downstream via a non-blocking operation (e.g. `mpsc::Sender::try_send` or `Sink::poll_ready`).
3. **Step 3 (Conditional Commit)**:
   - **If downstream accepts the data**: The caller commits by calling `receiver.advance_in_order(seq)` (which verifies `seq == expected_seq`, advances `expected_seq`, and updates `AckTracker`).
   - **If downstream is full / saturated**: The caller **drops the frame and does NOT call `advance_in_order(seq)`**.
4. **Natural Sender Throttling**: Because `advance_in_order()` was not called, the cumulative ACK does not advance. The remote sender's sliding window fills up until `can_send()` returns `false`, stalling further transmissions at the source.
5. **Recovery**: Once the downstream bottleneck drains, the remote sender's retransmission timer expires, re-sending the unacknowledged frames into now-available receiver buffers.

This guarantees true end-to-end backpressure without a single byte of receiver-side reassembly buffering.

---

### 8.2 Logical Channel Multiplexing (`channel_id`)

A physical serial port is inherently a single byte stream. Without packet-level channel multiplexing, access to the serial link is an exclusive, single-client bottleneck:
- If a developer runs a continuous log streaming session (`ffx log`) or serves package files over UART, the link is completely monopolized.
- Concurrent control queries (`ffx target echo`, `ffx component list`) would be blocked until the stream terminates.

By embedding a 16-bit `channel_id` into the framing API:
- Multiple concurrent host client connections are multiplexed over a single serial daemon (`ffx-uart-driver`).
- Target-side demux maps each `channel_id` to an independent FIDL channel.
- Channels are torn down cleanly with `FrameType::Close` without disrupting concurrent connections.

---

### 8.3 Dedicated Control Plane (`CONTROL_CHANNEL_ID = 0`)

`CONTROL_CHANNEL_ID: u16 = 0` is permanently reserved for transport signaling and handshake negotiation:
1. **Separation of Concerns**: User channels (`1..=65535`) handle client application data. Channel 0 manages transport lifecycle.
2. **In-Band Reboot & Session Reset**: Serial links lack out-of-band hardware disconnect events. When an endpoint reboots, it emits a `NegotiateReq` (carrying a fresh `session_id`) directly into Channel 0. The parser processes this reset cleanly without desynchronizing data channels.
3. **Queue Independence**: Control frames on Channel 0 operate outside client Go-Back-N data window queues, preventing handshakes from being blocked behind backpressured data streams.

---

### 8.4 32-Bit Session IDs

Hardware UART FIFOs, USB bridge chips (e.g. FTDI FT4232H), and kernel TTY buffers frequently retain unread bytes across host restarts or target reboots:
- In a sliding-window protocol where sequence numbers always start at 0, stale in-flight packets or delayed ACKs from a previous session could be accepted as valid packets in a new session.
- Tagging every frame with a random 32-bit `session_id` allows immediate rejection of stale frames and instantaneous detection of peer reboots.

---

### 8.5 Cumulative ACK Coalescing (`AckTracker`)

In asymmetric or half-duplex links (and even full-duplex UARTs), emitting an individual ACK packet for every incoming data packet consumes substantial reverse bandwidth:
- Under rapid bursts (e.g. 64 KB FIDL transfers split into seventy 1 KB frames), generating 70 individual ACK packets causes reverse FIFO contention.
- `AckTracker` coalesces incoming acknowledgments in memory: the writer task only emits the latest cumulative ACK sequence. If packets 0 through 15 arrive in a single burst, only a single cumulative ACK for packet 15 needs to cross the wire.

---

### 8.6 Karn's Algorithm for Accurate RTT Measurement

Estimating round-trip time (RTT) is necessary to dynamically calibrate retransmission timeouts. However, naive RTT estimation suffers from ambiguity when packets are retransmitted: when an ACK arrives for a retransmitted packet, it is impossible to determine whether the ACK corresponds to the original transmission or the retransmission.

`ResendSender` implements **Karn's Algorithm**:
- Frames that have undergone retransmission have `retransmitted = true`.
- When an ACK arrives for a retransmitted frame, `AckOutcome::Advanced` returns `rtt: None`.
- RTT samples are recorded *exclusively* for frames acknowledged on their first transmission, preventing retransmission bursts from poisoning RTT estimates and inflating timeouts.

---

## 9. Usage Examples

### 9.1 Handshake Negotiation Flow

```rust
use uart_fpl::{
    FrameParser, FrameType, HostHandshake, ProtocolId, TargetHandshake, encode_frame,
};

// 1. Host initiates negotiation proposing ResendSP
let host = HostHandshake::new(vec![ProtocolId::ResendSP]);
let (req_type, req_payload) = host.start().expect("Host handshake start");
let host_req_wire = encode_frame(
    /*session_id=*/ 0x12345678,
    /*channel_id=*/ 0,
    /*seq=*/ 0,
    req_type,
    &req_payload,
).expect("Encode negotiate req");

// 2. Target receives and evaluates request
let target = TargetHandshake::new(vec![ProtocolId::ResendSP]);
let mut target_parser = FrameParser::new();
target_parser.feed(&host_req_wire);

let parsed_req = target_parser.next_frame().expect("Target parses req");
let (selected, resp_payload) = target
    .handle_request(&parsed_req.payload)
    .expect("Target handles req");
assert_eq!(selected, Some(ProtocolId::ResendSP));

let target_resp_wire = encode_frame(
    /*session_id=*/ 0x12345678,
    /*channel_id=*/ 0,
    /*seq=*/ 0,
    FrameType::NegotiateResp,
    &resp_payload,
).expect("Encode negotiate resp");

// 3. Host completes negotiation
let mut host_parser = FrameParser::new();
host_parser.feed(&target_resp_wire);

let parsed_resp = host_parser.next_frame().expect("Host parses resp");
let negotiated_proto = host
    .handle_response(&parsed_resp.payload)
    .expect("Host handles resp");
assert_eq!(negotiated_proto, ProtocolId::ResendSP);
```

### 9.2 Reliable Sender Loop

```rust
use uart_fpl::{AckOutcome, FrameType, ResendSender};

let mut sender = ResendSender::new(/*window_size=*/ 64, /*max_attempts=*/ 60);

// Check window capacity before sending
if sender.can_send() {
    let (seq, wire_bytes) = sender
        .enqueue_frame(
            /*session_id=*/ 0x12345678,
            /*channel_id=*/ 1,
            FrameType::Data,
            b"Hello Fuchsia UART",
        )
        .expect("Enqueue frame");
    // Transmit wire_bytes over serial port...
}

// When an incoming ACK is parsed:
match sender.handle_ack(/*ack_seq=*/ 0) {
    AckOutcome::Advanced { new_base, rtt, all_acked } => {
        if let Some(measured_rtt) = rtt {
            // Update RTT estimator with measured sample
        }
        if all_acked {
            // All outstanding data has been acknowledged
        }
    }
    AckOutcome::OutOfWindow { base, next_seq } => {
        // Stale or duplicate ACK, safely ignore
    }
}
```

### 9.3 Reliable Receiver Loop with Two-Phase Backpressure

```rust
use uart_fpl::{AckTracker, FrameStatus, FrameType, ResendReceiver};

let mut receiver = ResendReceiver::new();
let ack_tracker = AckTracker::new();

// In the packet reception handler:
let frame_status = receiver.inspect(parsed_frame.seq);

match frame_status {
    FrameStatus::InOrder { seq } => {
        // Step 1: Attempt non-blocking dispatch downstream
        match downstream_channel.try_send(parsed_frame.payload) {
            Ok(()) => {
                // Step 2: Downstream accepted payload; advance receiver
                let ack_seq = receiver
                    .advance_in_order(seq)
                    .expect("In-order sequence verified by inspect");
                ack_tracker.set_ack(parsed_frame.session_id, ack_seq);
            }
            Err(_full_error) => {
                // Downstream is full! Drop packet and DO NOT advance receiver.
                // Sender will stall when window fills, naturally backpressuring.
            }
        }
    }
    FrameStatus::OutOfOrder { seq: _, expected_seq: _, ack_seq } => {
        // Duplicate or gap packet: drop payload, re-emit latest cumulative ACK
        if let Some(ack) = ack_seq {
            ack_tracker.set_ack(parsed_frame.session_id, ack);
        }
    }
}
```
