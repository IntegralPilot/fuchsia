<!-- Copyright 2026 The Fuchsia Authors. All rights reserved.
     Use of this source code is governed by a BSD-style license that can be
     found in the LICENSE file. -->

# UART Framing Protocol Library (`uart_fpl`)

`uart_fpl` is a lightweight, transport-independent binary framing, multiplexing, and protocol negotiation library for transmitting structured packet streams over raw, byte-oriented serial (UART) links.

It provides shared, platform-independent primitives used by host-side background daemons (`ffx-uart-driver`) and target-side drivers (`fdomain-uart-driver`) to multiplex multiple concurrent logical communication channels over a single physical or virtual serial interface.

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

---

## 2. Logical Channel Multiplexing

A raw serial interface provides only a single flat byte stream. Without packet-level multiplexing, access to the serial link is an exclusive, single-client bottleneck:
* If a developer runs a continuous log monitor (`ffx log`) or serves package files over UART, the serial link is completely monopolized.
* Concurrent control queries (`ffx target echo`, `ffx component list`) or interactive diagnostics are blocked until the streaming command terminates.

`uart_fpl` solves this by embedding a 16-bit `Channel ID` into every frame:

```
Physical / Virtual UART Link
 ├── Channel 0: Control & Protocol Negotiation (Handshake)
 ├── Channel 1: Primary Data Channel (e.g. Diagnostic / Control RPC)
 └── Channel 2+: Auxiliary streaming & diagnostic channels
```

* **Host Multiplexing**: The host-side background daemon (`ffx-uart-driver`) listens on a local UNIX domain socket. Each incoming client connection is assigned a distinct `channel_id` (channels 1 through 65535).
* **Target Demultiplexing**: The target driver (`fdomain-uart-driver`) demultiplexes incoming frames by `channel_id`, creating an independent FIDL stream socket for each active channel.
* **Granular Teardown**: When a client terminates, a `FrameType::Close` frame tears down that specific channel on the target without disrupting concurrent streams on other channels.
* **Dedicated Control Plane**: `Channel 0` is permanently reserved for transport signaling, dynamic handshake negotiation, and in-band reboot detection.

---

## 3. Documentation Index

The documentation for `uart_fpl` is divided into two focused specifications:

* **[Wire Protocol Specification (`PROTOCOL.md`)](./PROTOCOL.md)**:
  * Complete 13-byte binary frame layout diagram and exact byte offsets.
  * Field definitions and wire representations.
  * 1-byte CRC-8 header checksum (`HdrChk`) and IEEE 802.3 CRC-32 checksum formulas.
  * Dynamic handshake negotiation wire payloads (`NEGOTIATE_REQ`, `NEGOTIATE_RESP`).
  * Protocol-level design decisions (session IDs, corrupted length protection, Go-Back-N wire mechanics).

* **[Rust API Reference & Integration Guide (`API.md`)](./API.md)**:
  * Comprehensive public Rust API reference (`Frame`, `FrameParser`, `HostHandshake`, `TargetHandshake`, `ResendSender`, `ResendReceiver`, `AckTracker`).
  * Method signatures, contracts, and error handling (`UnexpectedSeqError`).
  * High-level architectural justifications (such as end-to-end backpressure without local receiver queueing).
  * Runnable Rust code examples for handshake negotiation, sender loop, and receiver check-and-commit workflows.

---

## 4. Running Tests

Unit tests are defined in `BUILD.gn`:

```bash
# Run host unit tests
fx test //src/developer/lib/uart_fpl:uart_fpl_test --host
```
