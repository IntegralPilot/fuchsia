<!-- Copyright 2026 The Fuchsia Authors. All rights reserved.
     Use of this source code is governed by a BSD-style license that can be
     found in the LICENSE file. -->

# UART Framing Protocol Library: Wire Protocol Specification

This document specifies the binary wire format, byte offsets, checksum algorithms, and transport state machines for the Fuchsia UART Framing Protocol Library (`uart_fpl`).

For the Rust crate software API reference, method contracts, and integration examples, see [`API.md`](./API.md).
For a high-level overview of the library and channel multiplexing topology, see [`README.md`](./README.md).

---

## 1. Frame Layout & Wire Format

Every packet transmitted over the serial link follows a structured binary layout with sync words, session tracking, logical channel multiplexing, sequence tracking, length-prefixed payload, an 8-bit header checksum, and a 32-bit CRC checksum:

```
+-----------+------------+------------+-----+------------+-------------+---------+----------------+---------------+
| Sync Word | Session ID | Channel ID | Seq | Frame Type | Payload Len | Hdr Chk | Payload (data) | CRC32 Checksum|
|   (2B)    |  (4B BE)   |  (2B BE)   | (1B)|    (1B)    |   (2B BE)   |  (1B)   |   (N Bytes)    |    (4B BE)    |
+-----------+------------+------------+-----+------------+-------------+---------+----------------+---------------+
|  Offset 0 |  Offset 2  |  Offset 6  |Off 8|  Offset 9  |  Offset 10  |  Off 12 |   Offset 13    |  Offset 13+N  |
+-----------+------------+------------+-----+------------+-------------+---------+----------------+---------------+
```

### Field Definitions

| Field | Offset | Size | Type | Description |
| :--- | :--- | :--- | :--- | :--- |
| **Sync Word** | 0 | 2 bytes | `[u8; 2]` | Canonical preamble (`0xAA 0x55`) used for frame boundary delineation and continuous stream resynchronization. |
| **Session ID** | 2 | 4 bytes | `u32` (BE) | Unique random identifier generated per connection session. Prevents sequence collisions from stale in-flight FIFO buffers across host or target restarts. |
| **Channel ID** | 6 | 2 bytes | `u16` (BE) | Logical channel identifier for multiplexing multiple independent streams over a single serial link (Channel 0 = Control/Handshake, Channels 1..=65535 = Data). |
| **Sequence Number** | 8 | 1 byte | `u8` | Packet sequence number (0 to 255) for Go-Back-N sliding-window flow control in `ResendSP`. Wraps modulo 256. |
| **Frame Type** | 9 | 1 byte | `u8` | Semantic frame indicator (`0x01` = `DATA`, `0x02` = `ACK`, `0x03` = `CLOSE`, `0x04` = `RESET`, `0x10` = `NEGOTIATE_REQ`, `0x11` = `NEGOTIATE_RESP`). |
| **Payload Length** | 10 | 2 bytes | `u16` (BE) | Length $N$ of the subsequent payload in bytes (Big-Endian, maximum 1024 bytes). |
| **Header Checksum** | 12 | 1 byte | `u8` | 8-bit CRC checksum (`HdrChk`, polynomial `0x07`) calculated over the 10 header bytes (offsets 2 through 11). |
| **Payload** | 13 | $N$ bytes | `[u8]` | Raw frame payload data ($0 \le N \le 1024$). |
| **CRC32 Checksum** | 13+$N$ | 4 bytes | `u32` (BE) | IEEE 802.3 CRC-32 checksum (Big-Endian) calculated over header fields, header checksum, and payload (offsets 2 through 12+$N$). |

---

## 2. Checksum Algorithms

### 2.1 1-Byte Header Checksum (`HdrChk`)
Calculated over offsets 2 through 11 (Session ID, Channel ID, Seq, Frame Type, Payload Len) using polynomial `0x07` (`x^8 + x^2 + x + 1`), matching Pigweed `pw_ulink`:

```rust
fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0x00;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if (crc & 0x80) != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}
```

* **Purpose**: Guarantees that corrupted `Payload Length` headers are rejected immediately before the parser attempts to allocate memory or buffer trailing payload bytes.

### 2.2 32-Bit Frame Checksum (IEEE 802.3 CRC-32)
Calculated over all bytes from offset 2 up to the end of the payload (offsets 2 through 12+$N$):

$$\text{CRC-32} = \text{CRC32}(\text{Header}[2..12] \parallel \text{HdrChk} \parallel \text{Payload})$$

* **Polynomial**: Standard IEEE 802.3 polynomial (`0xEDB88320` reversed / `0x04C11DB7` normal).
* **Wire Representation**: 4 bytes, Big-Endian.

---

## 3. Dynamic Protocol Negotiation (Channel 0)

Before client data exchange begins, the host and target negotiate the framing protocol across `Channel ID = 0`.

### 3.1 Negotiation Request (`NEGOTIATE_REQ = 0x10`)
The host transmits a prioritized list of proposed protocol IDs:

```
+------------+------------------+------------------+-----+
| Count (1B) | Protocol ID 0    | Protocol ID 1    | ... |
|            | (4B BE)          | (4B BE)          |     |
+------------+------------------+------------------+-----+
```

* `Count`: Number of proposed protocols ($1 \le \text{Count} \le 255$).
* `Protocol ID`: 32-bit big-endian integer. Currently defined protocols:
  * `1` = `ResendSP` (Sliding-window Go-Back-N ARQ with CRC-32).
  * Future proposals are preserved forward-compatibly as `ProtocolId::Unknown(id)`.

### 3.2 Negotiation Response (`NEGOTIATE_RESP = 0x11`)
The target evaluates the host's proposals against its supported protocols, selects the highest-priority mutual match, and responds:

```
+-------------+-----------------------+
| Status (1B) | Selected Protocol ID  |
|             | (4B BE)               |
+-------------+-----------------------+
```

* **Status Codes**:
  * `0x00` = `Success`: Mutually supported protocol agreed upon.
  * `0x01` = `NoCommonProtocol`: Target supports none of the proposed protocols.
  * `0x02` = `MalformedRequest`: Request payload was truncated or malformed.
* **Selected Protocol ID**: 32-bit big-endian integer of the agreed protocol (or `0` on failure).

### 3.3 Idempotent Handshake Retransmission Contract
Over lossy or noisy UART channels, the target's `NEGOTIATE_RESP` may be corrupted or dropped.
* The host will retransmit its `NEGOTIATE_REQ` carrying the identical `session_id`.
* **Contract**: The target **must** treat repeated `NEGOTIATE_REQ` frames bearing the active `session_id` as idempotent retransmissions. It re-emits its cached `NEGOTIATE_RESP` without tearing down active channels or resetting sequence state.
* If a `NEGOTIATE_REQ` arrives with a *new* `session_id`, it indicates a host restart, prompting a complete state and channel reset.

---

## 4. Sliding-Window Flow Control (`ResendSP`)

`ResendSP` (`ProtocolId = 1`) implements Go-Back-N sliding-window flow control:

### 4.1 Sequence Space & Window Bounds
* Sequence numbers are 8-bit unsigned integers (`0..=255`), wrapping modulo 256.
* Default window size $W = 64$ frames (maximum permissible window $W \le 128$).
* In-flight capacity invariant:
  $$\text{in\_flight} = (\text{next\_seq} - \text{base}) \pmod{256} < W$$

### 4.2 Cumulative Acknowledgments (`TYPE_ACK = 0x02`)
* ACKs are cumulative: receiving an ACK for sequence $K$ acknowledges all outstanding frames from `base` through $K$ inclusive.
* A receiver only emits ACKs for in-order sequence delivery. Out-of-order or duplicate frames prompt re-emission of the current cumulative ACK.
* ACKs are coalesced in memory via `AckTracker`: if frames 0 through 7 arrive contiguously, only a single cumulative ACK for frame 7 is transmitted across the wire.

### 4.3 Timeout & Retransmission
* When the retransmission timer fires (default 1,000 ms) and unacknowledged frames remain in flight, the sender retransmits all frames in the active window $[\text{base}, \text{next\_seq})$.
* Retransmission limit: default 60 consecutive timeout attempts before declaring transport failure.
* **Karn's Algorithm**: Retransmitted frames are flagged. ACKs matching retransmitted frames do not generate round-trip time (RTT) samples, preventing RTT estimator pollution.

---

## 5. Protocol Design Decisions & Rationale

### 5.1 Why 1-Byte Header Checksum (`HdrChk`) Prevents Corrupted Length Stalls
A trailing CRC-32 can only be verified after reading the entire declared frame length. If line noise flips bits in the `Payload Length` field (e.g. corrupting 16 bytes to 1024 bytes), a receiver without a header checksum must wait for 1024 bytes of incoming data before detecting CRC failure. This wedges the parser and drops subsequent valid frames. `HdrChk` verifies the header integrity at offset 12, allowing immediate rejection and $O(N)$ sync recovery.

### 5.2 Why 32-Bit Session IDs Prevent Stale FIFO Collisions Across Reboots
UART FIFOs, USB-to-serial adapters (e.g. FTDI FT4232H), and kernel TTY buffers frequently hold unread bytes across target reboots or host tool restarts. Because sequence numbers always begin at 0, stale in-flight frames or delayed ACKs from a prior session could be accepted as valid data in the new session. Random 32-bit session IDs ensure instant rejection of stale frames.

### 5.3 Why Channel 0 is Reserved as the Dedicated Control Plane
* **Control vs Data Separation**: User client connections are allocated dynamic channels ($1 \le \text{Channel ID} \le 65535$). Channel 0 permanently serves as the control plane.
* **In-Band Reboot Detection**: When a target reboots, it emits a `NEGOTIATE_REQ` (with a fresh session ID) directly into Channel 0 without corrupting active client framing.
* **Queue Independence**: Control signaling operates outside client Go-Back-N data window queues, preventing handshakes and resets from blocking behind saturated data streams.

### 5.4 Why Go-Back-N Was Selected Over Stop-and-Wait and Selective Repeat
* **Stop-and-Wait**: Incurs a full round-trip delay per packet. On USB-serial bridges or remote tunnels with 20-50 ms RTT, throughput is capped at 10-25 KB/s.
* **Selective Repeat**: Requires complex out-of-order reassembly queues, dynamic heap allocations, and SACK bitmaps, which risk OOM crashes on embedded targets.
* **Go-Back-N**: Fully saturates 1 MBaud serial bandwidth (~98 KB/s) with static buffers and zero receiver-side reassembly queueing.
