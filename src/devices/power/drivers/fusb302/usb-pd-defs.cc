// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/usb-pd-defs.h"

#include <zircon/assert.h>

#include <cinttypes>
#include <cstdint>

namespace usb_pd {

const char* PowerRoleToString(PowerRole power_role) {
  switch (power_role) {
    case PowerRole::kSink:
      return "Sink";
    case PowerRole::kSource:
      return "Source";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid PowerRole: %" PRId8, static_cast<int8_t>(power_role));
  return "(invalid)";
}

const char* DataRoleToString(DataRole data_role) {
  switch (data_role) {
    case DataRole::kDownstreamFacingPort:
      return "DFP";
    case DataRole::kUpstreamFacingPort:
      return "UFP";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid DataRole: %" PRId8, static_cast<int8_t>(data_role));
  return "(invalid)";
}

const char* ConfigChannelPinIdToString(ConfigChannelPinId pin_id) {
  switch (pin_id) {
    case ConfigChannelPinId::kCc1:
      return "CC1";
    case ConfigChannelPinId::kCc2:
      return "CC2";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid ConfigChannelPinId: %" PRId8, static_cast<int8_t>(pin_id));
  return "(invalid)";
}

const char* ConfigChannelPinSwitchToString(ConfigChannelPinSwitch cc_switch) {
  switch (cc_switch) {
    case ConfigChannelPinSwitch::kNone:
      return "(not connected)";
    case ConfigChannelPinSwitch::kCc1:
      return "CC1";
    case ConfigChannelPinSwitch::kCc2:
      return "CC2";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid ConfigChannelPinSwitch: %" PRId8,
                      static_cast<int8_t>(cc_switch));
  return "(invalid)";
}

const char* ConfigChannelTerminationToString(ConfigChannelTermination termination) {
  switch (termination) {
    case ConfigChannelTermination::kUnknown:
      return "(unknown)";
    case ConfigChannelTermination::kOpen:
      return "Open";
    case ConfigChannelTermination::kRa:
      return "Ra (Accessory)";
    case ConfigChannelTermination::kRd:
      return "Rd (Sink)";
    case ConfigChannelTermination::kRpStandardUsb:
      return "Rp (Standard USB Power)";
    case ConfigChannelTermination::kRp1500mA:
      return "Rp (1.5A Type C Power / SinkTxOK)";
    case ConfigChannelTermination::kRp3000mA:
      return "Rp (3.0A Type C Power / SinkTxNG)";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid ConfigChannelTermination: %" PRId8,
                      static_cast<int8_t>(termination));
  return "(invalid)";
}

// Tests for ConfigChannelPinIdFromInverse().
static_assert(ConfigChannelPinIdFromInverse(ConfigChannelPinId::kCc1) == ConfigChannelPinId::kCc2);
static_assert(ConfigChannelPinIdFromInverse(ConfigChannelPinId::kCc2) == ConfigChannelPinId::kCc1);

// Tests for ConfigChannelPinIdFromSwitch().
static_assert(static_cast<ConfigChannelPinId>(ConfigChannelPinSwitch::kCc1) ==
              ConfigChannelPinId::kCc1);
static_assert(static_cast<ConfigChannelPinId>(ConfigChannelPinSwitch::kCc2) ==
              ConfigChannelPinId::kCc2);

}  // namespace usb_pd
