// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ARCH_PHYS_INFO_H_
#define ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ARCH_PHYS_INFO_H_

#include <lib/arch/arm64/smccc.h>

// Note this uses std instead of ktl because it has to be used by the
// generator program.
#include <array>

// This is data that physboot et al might need from phys early start-up.
// It's initialized in physload and then referred to by reference elsewhere.
struct ArchPhysInfo {
  std::array<uint64_t, arch::kArmPsciRegisters> psci_reset_registers = {};
  bool smccc_use_hvc = false;
  bool smccc_disabled = true;
  bool have_psci_features = false;
};

#endif  // ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ARCH_PHYS_INFO_H_
