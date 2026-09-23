// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_BOOT_OPTIONS_INCLUDE_LIB_BOOT_OPTIONS_ARM64_H_
#define ZIRCON_KERNEL_LIB_BOOT_OPTIONS_INCLUDE_LIB_BOOT_OPTIONS_ARM64_H_

#include <stdint.h>

// This file provides declarations for special types used by arm64.inc options.
// See kernel/lib/boot-options/README.md for full details and constraints.
// It should avoid other kernel header dependencies.

enum class Arm64PhysPsciReset : uint8_t {
  kDisabled,
  kShutdown,
  kReboot,
  kRebootBootloader,
  kRebootRecovery,
};

enum class Arm64AlternateVbar : uint8_t {
  kNone,
  kAuto,
  kArchWorkaround3,
  kArchWorkaround1,
  kPsciVersion,
  kSmccc10,
};

#define ARM64_OPTION_TYPES(OPTION_TYPE) \
  OPTION_TYPE(Arm64PhysPsciReset);      \
  OPTION_TYPE(Arm64AlternateVbar);

#endif  // ZIRCON_KERNEL_LIB_BOOT_OPTIONS_INCLUDE_LIB_BOOT_OPTIONS_ARM64_H_
