// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_EARLY_BOOT_H_
#define ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_EARLY_BOOT_H_

#include <cstddef>
#include <cstdint>

// These are placeholder on other architectures for preparing access to
// address ranges in early boot while the data cache is off. But x86 kernels
// are booted with the data cache left on so this is a no-op.
inline void ArchEarlyBootSyncData(uintptr_t addr, size_t size) {}
inline void ArchEarlyBootSyncAllocation(uintptr_t addr, size_t size) {}

// Whether any early boot data access synchronization scheme is necessary,
// equivalent to the above routines not being a no-op.
constexpr bool kArchEarlyBootDataSynchronization = false;

#endif  // ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_EARLY_BOOT_H_
