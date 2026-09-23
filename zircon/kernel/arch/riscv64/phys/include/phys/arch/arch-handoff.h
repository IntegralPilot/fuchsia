// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_

// Note: we refrain from using the ktl namespace as <phys/handoff.h> is
// expected to be compiled in the userboot toolchain.

#include <lib/arch/riscv64/feature.h>
#include <lib/stdbind/optional.h>
#include <lib/zbi-format/driver-config.h>
#include <zircon/tls.h>

#include <cstdint>
#include <optional>

// The minimal memory region needed to encapsulate the C++ compiler thread ABI.
struct ArchTempThreadAbi {
  constexpr const void* tp() const { return static_cast<const void*>(this + 1); }

  uint64_t stack_guard = 0;
  uint64_t unsafe_stack_pointer = 0;
};

static_assert(sizeof(ArchTempThreadAbi) + ZX_TLS_STACK_GUARD_OFFSET ==
              offsetof(ArchTempThreadAbi, stack_guard));

static_assert(sizeof(ArchTempThreadAbi) + ZX_TLS_UNSAFE_SP_OFFSET ==
              offsetof(ArchTempThreadAbi, unsafe_stack_pointer));

struct ArchPatchInfo {};

// This holds (or points to) all riscv64-specific data that is handed off from
// physboot to the kernel proper at boot time.
struct ArchPhysHandoff {
  uint64_t boot_hart_id;

  // The lowest common denominator of all supported features/extensions across
  // all harts.
  arch::RiscvFeatures cpu_features;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_RISCV_PLIC) payload.
  stdbind::optional<zbi_dcfg_riscv_plic_driver_t> plic_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER) payload.
  stdbind::optional<zbi_dcfg_riscv_generic_timer_driver_t> generic_timer_driver;
};

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_
