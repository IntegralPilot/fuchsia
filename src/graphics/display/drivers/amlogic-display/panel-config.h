// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PANEL_CONFIG_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PANEL_CONFIG_H_

#include <lib/device-protocol/display-panel.h>

#include <cstdint>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

// To simplify compatibility checks, DsiOpcode and PowerOpcode should match the
// AMLogic MIPI-DSI tuning guide.

// DSI packet DI (Data Identifier) values that convey special operations.
//
// The values here share a namespace with valid DSI DIs, described by Section
// 8.5.1 "Data Identifier Byte" and Section 8.7 "Processor to Peripheral
// Direction (Processor-Sourced) Packet Data Types" of the DSI spec.
enum DsiOpcode : uint8_t {
  // Drive a GPIO pin.
  //
  // <op> <size=2|3> <gpio_id=0> <value> [delay_ms]
  //
  // gpio_id 0 is connected to the RESX (LCD RESET) pin. All other IDs are
  // invalid.
  //
  // DSI packet types ending in 0b0000 are reserved, so this opcode will not
  // overlap any valid DSI packet DI (Data Identifier) value.
  kDsiOpGpio = 0xf0,

  // Attempt to read `count` values from the MIPI-DSI register at `address`.
  //
  // <op> <size=2> <address> <count=1|2|3|4>
  //
  // This opcode overlaps the DSI packet DI (Data Identifier) value for "Packed
  // Pixel Stream, 20-bit YCbCr, 4:2:2 Format" data type, Virtual Channel 3.
  //
  // TODO(https://fxbug.dev/322438328): Support reading larger-sized registers.
  kDsiOpReadReg = 0xfc,

  // Odd extended delay command to take several delays and gather them into
  // one big sleep. Behaves as an exit if byte 1 is 0xff or 0x0.
  //
  // <op> <size> <sleep_ms_1> <sleep_ms_2> ... <sleep_ms_N>
  //
  // This opcode overlaps the DSI packet DI (Data Identifier) value for "Packed
  // Pixel Stream, 12-bit YCbCr, 4:2:0 Format" data type, Virtual Channel 3.
  kDsiOpDelay = 0xfd,

  // Simple sleep for N millis, or exit if N=0xff || N=0x0.
  //
  // <op> <sleep_ms>
  //
  // DSI packet types ending in 0b1111 are reserved, so this opcode will not
  // overlap any valid DSI packet DI (Data Identifier) value.
  kDsiOpSleep = 0xff,

  // Everything else is potentially a DSI command.
};

// The DSI operations are encoded as a sequence of variable-length operations.
// The first byte in each operation is a `DsiOpcode` value, followed by the
// opcode's arguments.
using DsiOperationSequence = std::span<const uint8_t>;

enum PowerOpcode : uint8_t {
  // Drive a GPIO pin.
  kPowerOpGpio = 0,
  // Turn the device on/off.
  kPowerOpSignal = 2,
  // Wait for a GPIO input to reach a value.
  kPowerOpAwaitGpio = 4,
  kPowerOpExit = 0xff,
};

struct PowerOp {
  enum PowerOpcode op;
  uint8_t index;
  uint8_t value;
  uint8_t sleep_ms;
};

struct PanelConfig {
  // Used for logging / debugging / inspection.
  //
  // Must be non-null.
  const char* name;

  // A sequence of DSI operations used in the panel power on sequence.
  const DsiOperationSequence dsi_on;

  // A sequence of DSI operations used in the panel power off sequence.
  const DsiOperationSequence dsi_off;

  // Power operation sequence to power on the panel.
  const std::span<const PowerOp> power_on;

  // Power operation sequence to power off the panel.
  const std::span<const PowerOp> power_off;

  // The number of D-PHY data lanes used by the display's DSI connection.
  //
  // Must be >= 1 and <= 4.
  //
  // Chosen to satisfy the following constraints:
  // * display engine: maximum supported data lanes in the D-PHY transmitter
  //   PHY Protocol Interface (PPI).
  // * SoC or board: maximum supported data lanes in the D-PHY transmitter
  // * board: number of data lanes in the connection (traces and cables)
  // * DDIC: maximum supported data lanes in the D-PHY receiver
  int dphy_data_lane_count;

  // Must be non-negative.
  //
  // Chosen to meet the maximum frequency constraints for all the components
  // along the DSI connection, which include the display engine, the D-PHY
  // transmitter in the host SoC or board, the DSI connection cables and
  // traces, and the D-PHY receiver in the DDIC.
  //
  // The MIPI D-PHY clock frequency does not trivially translate into bitrates
  // for the data lanes. Use `maximum_per_data_lane_bits_per_second()` to get
  // the corresponding maximum data lane bitrate.
  int64_t maximum_dphy_clock_lane_frequency_hz;

  // Timing known to satisfy all the constraints for the panel and DDIC.
  //
  // All the timing fields must be valid.
  //
  // TODO(https://fxbug.dev/322242348): The current implementation of the
  // amlogic-display driver requires that the timing must have progressive
  // fields.
  //
  // The MIPI-DSI standard requires that the timing must have no repeated
  // pixels.
  display::DisplayTiming display_timing;

  constexpr int64_t maximum_per_data_lane_bit_per_second() const {
    // The MIPI D-PHY Clock lane uses a DDR (Double Data Rate) clock signal.
    // Thus, the per lane data rate is 2 times of the clock signal frequency.
    return maximum_dphy_clock_lane_frequency_hz * 2;
  }
};

// If the `panel_type` is supported, returns the panel configuration.
// Otherwise returns nullptr.
const PanelConfig* GetPanelConfig(display::PanelType panel_type);

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PANEL_CONFIG_H_
