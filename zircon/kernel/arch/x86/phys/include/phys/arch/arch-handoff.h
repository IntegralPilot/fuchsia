// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_
#define ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_

// Note: we refrain from using the ktl namespace as <phys/handoff.h> is
// expected to be compiled in the userboot toolchain.

#include <zircon/tls.h>

#include <cstdint>

struct ArchPatchInfo {};

// This holds (or points to) all x86-specific data that is handed off from
// physboot to the kernel proper at boot time.
struct ArchPhysHandoff {};

// The minimal memory region needed to encapsulate the C++ compiler thread ABI
struct ArchTempThreadAbi {
  constexpr const void* tp() const { return static_cast<const void*>(this); }

  // The x86 ABI also mandates that the first word is a pointer to itself.
  void* self = this;
  uint64_t padding = 0;
  uint64_t stack_guard = 0;
  uint64_t unsafe_stack_pointer = 0;
};

static_assert(ZX_TLS_STACK_GUARD_OFFSET == offsetof(ArchTempThreadAbi, stack_guard));

static_assert(ZX_TLS_UNSAFE_SP_OFFSET == offsetof(ArchTempThreadAbi, unsafe_stack_pointer));

#endif  // ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_
