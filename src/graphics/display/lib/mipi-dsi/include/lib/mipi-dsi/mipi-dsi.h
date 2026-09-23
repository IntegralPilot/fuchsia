// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_MIPI_DSI_INCLUDE_LIB_MIPI_DSI_MIPI_DSI_H_
#define SRC_GRAPHICS_DISPLAY_LIB_MIPI_DSI_INCLUDE_LIB_MIPI_DSI_MIPI_DSI_H_

#include <cstddef>
#include <cstdint>
#include <span>

// Some of the constants here are from the MIPI Alliance Specification for
// Display Serial Interface (DSI), which can be obtained from
// https://www.mipi.org/specifications/dsi-2
//
// mipi_dsi2 is Version 2.1, adopted on 24 May 2023.
//
// The DSI standard references some concepts from the MIPI Alliance Standard for
// Display Pixel Interface (DPI), which can be obtained from
// https://www.mipi.org/current-specifications
//
// mipi_dpi2 is Version 2.00, approved on 23 January 2006.

// TODO(https://fxbug.dev/328078798): Move all the constants below to the
// `mipi_dsi` namespace.

// Virtual Channel Identifier specified by the MIPI DSI Peripheral.
//
// All the MIPI DSI display panels specified in `lib/device-protocol/display-panel.h`
// are programmed to use `kMipiDsiVirtualChanId` as their Virtual Channel
// Identifier.
constexpr uint8_t kMipiDsiVirtualChanId = 0;

// Data types for DSI Processor-to-Peripheral packets.
//
// mipi_dsi2 8.7.1 "Processor-sourced Data Type Summary" Table 16 "Data Types
// for Processor-Sourced Packets"

constexpr uint8_t kMipiDsiDtVsyncStart = 0x01;
constexpr uint8_t kMipiDsiDtVsyncEnd = 0x11;
constexpr uint8_t kMipiDsiDtHsyncStart = 0x21;
constexpr uint8_t kMipiDsiDtHsyncEnd = 0x31;
constexpr uint8_t kMipiDsiDtEotp = 0x08;
constexpr uint8_t kMipiDsiDtColorModeOff = 0x02;
constexpr uint8_t kMipiDsiDtColorModeOn = 0x12;
constexpr uint8_t kMipiDsiDtPeriCmdOff = 0x22;
constexpr uint8_t kMipiDsiDtPeriCmdOn = 0x32;
constexpr uint8_t kMipiDsiDtGenShortWrite0 = 0x03;
constexpr uint8_t kMipiDsiDtGenShortWrite1 = 0x13;
constexpr uint8_t kMipiDsiDtGenShortWrite2 = 0x23;
constexpr uint8_t kMipiDsiDtGenShortRead0 = 0x04;
constexpr uint8_t kMipiDsiDtGenShortRead1 = 0x14;
constexpr uint8_t kMipiDsiDtGenShortRead2 = 0x24;
constexpr uint8_t kMipiDsiDtDcsShortWrite0 = 0x05;
constexpr uint8_t kMipiDsiDtDcsShortWrite1 = 0x15;
constexpr uint8_t kMipiDsiDtDcsRead0 = 0x06;
constexpr uint8_t kMipiDsiDtSetMaxRetPkt = 0x37;
constexpr uint8_t kMipiDsiDtNullPkt = 0x09;
constexpr uint8_t kMipiDsiDtBlakingPkt = 0x19;
constexpr uint8_t kMipiDsiDtGenLongWrite = 0x29;
constexpr uint8_t kMipiDsiDtDcsLongWrite = 0x39;
constexpr uint8_t kMipiDsiDtYcbcr42220bit = 0x0C;
constexpr uint8_t kMipiDsiDtYcbcr42224bit = 0x1C;
constexpr uint8_t kMipiDsiDtYcbcr42216bit = 0x2C;
constexpr uint8_t kMipiDsiDtRgb101010 = 0x0D;
constexpr uint8_t kMipiDsiDtRgb121212 = 0x1D;
constexpr uint8_t kMipiDsiDtYcbcr42012bit = 0x3D;
constexpr uint8_t kMipiDsiDtRgb565 = 0x0E;
constexpr uint8_t kMipiDsiDtRgb666 = 0x1E;
constexpr uint8_t kMipiDsiDtRgb666L = 0x2E;
constexpr uint8_t kMipiDsiDtRgb888 = 0x3E;
constexpr uint8_t kMipiDsiDtUnknown = 0xFF;

// Data types for DSI Peripheral-to-Processor (response) packets.
//
// mipi_dsi2 8.10 "Peripheral-to-Processor Transactions – Detailed Format
// Description" Table 23 "Data Types for Peripheral-Sourced Packets"

constexpr uint8_t kMipiDsiRspGenShort1 = 0x11;
constexpr uint8_t kMipiDsiRspGenShort2 = 0x12;
constexpr uint8_t kMipiDsiRspGenLong = 0x1A;
constexpr uint8_t kMipiDsiRspDcsLong = 0x1C;
constexpr uint8_t kMipiDsiRspDcsShort1 = 0x21;
constexpr uint8_t kMipiDsiRspDcsShort2 = 0x22;

namespace mipi_dsi {

// mipi_dsi2 4.2 "Command And Video Modes", pages 14-15.
enum class DsiOperationMode : uint8_t {
  kVideo = 0,
  kCommand = 1,
};

// The sequence of packets used to convey pixel and timing data in Video Mode.
//
// The MIPI DSI standard defines multiple methods for transmitting video data
// (pixel data and synchronization pulses). The methods differ in complexity
// required in the receiver and performance characteristics, such as DSI link
// power consumption.
//
// The DSI standard calls these methods "packet sequences", because they differ
// in the packet data types that are used.
//
// mipi_dsi2 8.11.1 "Transmission Packet Sequences", pages 120-121
enum class DsiVideoModePacketSequencing : uint8_t {
  // Packets explicitly mark the start and end of each synchronization pulse.
  //
  // Synchronization pulses (Horizontal and Vertical) are marked with both Start
  // and End packets. This minimizes the complexity of recovering video timing
  // in the receiver, at the expense of sending more data across the DSI link.
  //
  // Each line of pixel data is transmitted at the equivalent DPI transmission
  // rate, possibly by splitting the line into multiple pixel stream packets and
  // null packets.
  //
  // mipi_dsi2 8.11.2 "Non-Burst Mode with Sync Pulses", page 122
  kSyncPulsesNoBurst = 0,

  // Only the start of each synchronization pulse is marked by a packet.
  //
  // Synchronization pulses (Horizontal and Vertical) are conveyed by Start
  // packets. This reduces DSI link usage, at the expense of having the receiver
  // be responsible for inferring the end of synchronization pulses.
  //
  // Each line of pixel data is transmitted at the equivalent DPI transmission
  // rate, possibly by splitting the line into multiple pixel stream packets and
  // null packets.
  //
  // mipi_dsi2 8.11.3 "Non-Burst Mode with Sync Events", page 123
  kSyncEventsNoBurst = 1,

  // Each line of pixel data is transmitted as quickly as possible.
  //
  // Synchronization pulses (Horizontal and Vertical) are conveyed by Start
  // packets. This reduces DSI link usage, at the expense of having the receiver
  // be responsible for inferring the end of synchronization pulses.
  //
  // Each line of pixel data is transmitted in a single packet, saturating the
  // DSI link bandwidth. The DSI link may be driven to a LP (Low Power) mode
  // between the end of the pixel stream packet and the next synchronization
  // packet. This reduces the power usage of the DSI link, but places increased
  // demands on receiver logic such as line buffers.
  //
  // mipi_dsi2 8.11.4 "Burst Mode", page 124
  kBurst = 2,
};

// Describes the pixel data format and layout in a DSI pixel stream packet.
//
// MIPI DSI specifies pixel stream packets as a subset of the Processor-Sourced
// (Processor-to-Peripheral Direction) packets. Each pixel stream packet format
// has its own Packet Data Type value (bits 0-5 in the Data Identifier). So, the
// Data Type values for pixel stream packets are a subset of the values defined
// in mipi_dsi2 8.7.1 "Processor-sourced Data Type Summary".
//
// Each enum member's value is the Packet Data Type value. This makes it
// convenient to use the enum members with display engine hardware that relies
// on the DSI pixel stream packet Data Type values.
//
// Most layouts described by the DSI standard are "packed", meaning that every
// bit of the packet is meaningful. By contrast, "loosely packed" formats leave
// some bits unused, which enables some optimizations in the receiver's decoding
// logic. This enum's naming scheme only calls out loosely packed formats.
enum class DsiPixelStreamPacketFormat : uint8_t {
  // mipi_dsi2 8.8.14 "Loosely Packed Pixel Stream, 20-bit YCbCr 4:2:2 Format,
  // Data Type = 00 1100 (0x0C)", page 93
  k20BitYcbcr422LooselyPacked = 0x0c,

  // mipi_dsi2 8.8.15 "Packed Pixel Stream, 24-bit YCbCr 4:2:2 Format, Data
  // Type = 01 1100 (0x1C)", page 94
  k24BitYcbcr422 = 0x1c,

  // mipi_dsi2 8.8.16 "Packed Pixel Stream, 16-bit YCbCr 4:2:2 Format, Data
  // Type = 10 1100 (0x2C)", page 95
  k16BitYcbcr422 = 0x2c,

  // The MIPI DSI 1 standard does not include the 20-bit YCbCr 4:2:2 format.
  //
  // mipi_dsi2 8.8.17 "Packed Pixel Stream, 20-bit YCbCr 4:2:2 Format, Data
  // Type = 11 1100 (0x3C)", page 96
  k20BitYcbcr422 = 0x3c,

  // mipi_dsi2 8.8.18 "Packed Pixel Stream, 30-bit Format, Long Packet, Data
  // Type = 00 1101 (0x0D)", page 97
  k30BitR10G10B10 = 0x0d,

  // mipi_dsi2 8.8.19 "Packed Pixel Stream, 36-bit Format, Long Packet, Data
  // Type = 01 1101 (0x1D)", page 98
  k36BitR12G12B12 = 0x1d,

  // mipi_dsi2 8.8.20 "Packed Pixel Stream, 12-bit YCbCr 4:2:0 Format, Data
  // Type = 11 1101 (0x3D)", page 99
  k12BitYcbcr420 = 0x3d,

  // mipi_dsi2 8.8.21 "Packed Pixel Stream, 16-bit Format, Long Packet, Data
  // Type 00 1110 (0x0E)", page 100
  k16BitR5G6B5 = 0x0e,

  // mipi_dsi2 8.8.22 "Packed Pixel Stream, 18-bit Format, Long Packet, Data
  // Type = 01 1110 (0x1E)", page 101
  k18BitR6G6B6 = 0x1e,

  // mipi_dsi2 8.8.23 "Pixel Stream, 18-bit Format in Three Bytes, Long Packet,
  // Data Type = 10 1110 (0x2E)", page 102
  k18BitR6G6B6LooselyPacked = 0x2e,

  // mipi_dsi2 8.8.24 "Packed Pixel Stream, 24-bit Format, Long Packet, Data
  // Type = 11 1110 (0x3E)", page 103
  k24BitR8G8B8 = 0x3e,

  // Compressed pixel data without any particular format designation.
  //
  // The compression format can be negotiated using Compression Mode Command
  // packets and/or Picture Parameter Set packets.
  //
  // mipi_dsi1 8.8.25 "Compressed Pixel Stream, Long Packet, Data Type =
  // 00 1011 (0x0B)", pages 104-105
  kCompressed = 0x0b,
};

// Describes a DSI Host(Processor)-to-Peripheral command and the Peripheral's
// response.
//
// The Processor sends an outgoing packet composed of `virtual_channel_id`,
// `data_type` and `payload`.
//
// `response` may point to a buffer to receive the payload bytes sent by the
// Peripheral (before the Processor finishes reading from the Peripheral), or
// a buffer that has the response payloads (after the reading finishes).
struct DsiCommandAndResponse {
  // Specifies the Virtual Channel the packet is transmitted on.
  // Also known as "virtual channel identifier" in MIPI-DSI specs.
  //
  // Must be >= 0 and <= 3.
  // Must match the hardware configuration of the Peripheral.
  uint8_t virtual_channel_id;

  // Specifies the type (short or long) and the format of the outgoing packet.
  //
  // Must be a valid Processor-sourced Data Type specified in mipi_dsi1
  // Section 8.7 "Processor to Peripheral Direction (Processor-Sourced) Packet
  // Data Types".
  uint8_t data_type;

  // The payload (application data) bytes in the Processor-to-Peripheral packet.
  //
  // Depending on the Processor-to-Peripheral `data_type`, `payload` may be
  // empty.
  //
  // The size of `payload` must match the packet format specified in
  // mipi_dsi1 Section 8.8 "Processor-to-Peripheral Transactions – Detailed
  // Format Description".
  std::span<const uint8_t> payload;

  // The buffer for the payload (application data) bytes in the
  // Peripheral-to-Processor packet.
  //
  // Empty if the Processor expects to receive no payload bytes from the
  // Peripheral.
  //
  // Otherwise, the Processor expects to receive exactly `response.size()`
  // bytes of payload in the response sent by the Peripheral.
  std::span<uint8_t> response_payload;
};

}  // namespace mipi_dsi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_MIPI_DSI_INCLUDE_LIB_MIPI_DSI_MIPI_DSI_H_
