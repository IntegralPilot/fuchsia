// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <dev/arm_smmu/utils.h>

namespace arm_smmu {

const char* ArmSmmuModeToString(ArmSmmuMode mode) {
  switch (mode) {
    case ArmSmmuMode::kDisabled:
      return "Disabled";
    case ArmSmmuMode::kPassthru:
      return "Passthru";
    case ArmSmmuMode::kEnforced:
      return "Enforced";
    default:
      return "Unknown";
  }
}

const char* ArmCbarTypeToString(CBAR_Type type) {
  switch (type) {
    case CBAR_Type::kS2Translation:
      return "Translate: Stage 2 Context";
    case CBAR_Type::kS1TS2Bypass:
      return "Translate: Stage 1 Context with Stage 2 Bypass";
    case CBAR_Type::kS1TS2Fault:
      return "Translate: Stage 1 Context with Stage 2 Fault";
    case CBAR_Type::kS1TS2Translate:
      return "Translate: Stage 1 Context with Stage 2 Translate";
    default:
      return "Unknown";
  }
}

const char* ArmS2crTypeToString(S2CR_Type type) {
  switch (type) {
    case S2CR_Type::kTranslation:
      return "Translation";
    case S2CR_Type::kBypass:
      return "Bypass";
    case S2CR_Type::kFault:
      return "Fault";
    case S2CR_Type::kInvalid:
      return "Invalid";
  }
  return "Unknown";
}

const char* AddrModeToString(AddrMode mode) {
  switch (mode) {
    case AddrMode::k32Bit:
      return "32Bit";
    case AddrMode::kExt32Bit:
      return "Ext32Bit";
    case AddrMode::k64Bit:
      return "64Bit";
    case AddrMode::kInvalid:
      return "Invalid";
    default:
      return "Unknown";
  }
}

const char* BtiModeToString(BtiMode mode) {
  switch (mode) {
    case BtiMode::kFault:
      return "Fault";
    case BtiMode::kBypass:
      return "Bypass";
    case BtiMode::kTranslation:
      return "Translation";
    case BtiMode::kAdopted:
      return "Adopted";
    case BtiMode::kShutdown:
      return "Shutdown";
    case BtiMode::kInvalid:
      return "Invalid";
    default:
      return "Unknown";
  }
}

}  // namespace arm_smmu
