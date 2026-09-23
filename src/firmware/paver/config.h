// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_CONFIG_H_
#define SRC_FIRMWARE_PAVER_CONFIG_H_

#include <vector>

namespace paver {

enum class Arch {
  kX64,
  kArm64,
  kRiscv64,
};

// Get the architecture of the currently running platform.
inline constexpr Arch GetCurrentArch() {
#if defined(__x86_64__)
  return Arch::kX64;
#elif defined(__aarch64__)
  return Arch::kArm64;
#elif defined(__riscv)
  return Arch::kRiscv64;
#else
#error "Unknown arch"
#endif
}

struct PaverConfig {
  // The architecture of the system the paver is running on.
  Arch arch = GetCurrentArch();
  // In GptPartitioner, when matching multiple GPT-formatted devices, we pick the target GPT using
  // the existence of the Fuchsia system partition.  The system partition is detected based on
  // matching any of these labels.
  std::vector<const char*> system_partition_names = {"super", "fvm"};
  // Enables ABR wear leveling for Astro sysconfig.
  bool astro_sysconfig_abr_wear_leveling;
  // The current slot (e.g., "a", "b") for Zircon Verified Boot.
  std::string zvb_current_slot;
  // The UUID of the boot partition used by Zircon Verified Boot.
  std::string zvb_boot_partition_uuid;
  // The suffix of the current boot slot for Android-style A/B updates (e.g., "_a", "_b").
  std::string android_boot_slot_suffix;
};

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_CONFIG_H_
