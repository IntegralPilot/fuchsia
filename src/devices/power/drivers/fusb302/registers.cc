// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/registers.h"

#include <cstdint>

namespace fusb302 {

const char* SwitchBlockConfigToString(SwitchBlockConfig config) {
  switch (config) {
    case SwitchBlockConfig::kOpen:
      return "Open";
    case SwitchBlockConfig::kPullUp:
      return "Pull-up (Pu current source)";
    case SwitchBlockConfig::kPullDown:
      return "Pull-down (Pd resistor)";
    case SwitchBlockConfig::kConnectorVoltage:
      return "VCONN";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid SwitchBlockConfig: %" PRId8, static_cast<int8_t>(config));
  return "(invalid)";
}

const char* Fusb302RoleDetectionModeToString(Fusb302RoleDetectionMode mode) {
  switch (mode) {
    case Fusb302RoleDetectionMode::kReserved:
      return "(reserved)";
    case Fusb302RoleDetectionMode::kDualPowerRole:
      return "DPR (Dual Power Role)";
    case Fusb302RoleDetectionMode::kSinkOnly:
      return "only Sink";
    case Fusb302RoleDetectionMode::kSourceOnly:
      return "only Source";
  }
  ZX_DEBUG_ASSERT_MSG(false, "Invalid Fusb302RoleDetectionMode: %" PRId8,
                      static_cast<int8_t>(mode));
  return "(invalid)";
}

const char* PowerRoleDetectionStateToString(PowerRoleDetectionState state) {
  switch (state) {
    case PowerRoleDetectionState::kDetecting:
      return "Running";
    case PowerRoleDetectionState::kSourceOnCC1:
      return "Source, CC wire on CC1 pin";
    case PowerRoleDetectionState::kSourceOnCC2:
      return "Source, CC wire on CC2 pin";
    case PowerRoleDetectionState::kSinkOnCC1:
      return "Sink, CC wire on CC1 pin";
    case PowerRoleDetectionState::kSinkOnCC2:
      return "Sink, CC wire on CC2 pin";
    case PowerRoleDetectionState::kAudioAccessory:
      return "Audio Accessory connected";
  }
  return "(undocumented)";
}

const char* ReceiveTokenTypeToString(ReceiveTokenType receive_token_type) {
  switch (receive_token_type) {
    case ReceiveTokenType::kSop:
      return "SOP (PD Source <-> PD Sink)";
    case ReceiveTokenType::kSopPrime:
      return "SOP' (VCONN source <-> Cable Plug)";
    case ReceiveTokenType::kSopDoublePrime:
      return "SOP\" (VCONN source <-> Cable Plug)";
    case ReceiveTokenType::kSopPrimeDebug:
      return "SOP' Debug (reserved)";
    case ReceiveTokenType::kSopDoublePrimeDebug:
      return "SOP\" Debug (reserved)";
    case ReceiveTokenType::kUndocumented:
      return "(undocumented token)";
  }

  ZX_DEBUG_ASSERT_MSG(false, "Invalid ReceiveTokenType: %" PRId8,
                      static_cast<int8_t>(receive_token_type));
  return "(invalid token)";
}

}  // namespace fusb302
