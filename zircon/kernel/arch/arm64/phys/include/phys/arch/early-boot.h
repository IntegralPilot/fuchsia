// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_EARLY_BOOT_H_
#define ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_EARLY_BOOT_H_

#include <lib/arch/cache.h>

#include <cstddef>
#include <cstdint>

// Synchronize a range of boot-time data between the data cache and main
// memory. Intended to be read in early boot while the data cache is off (i.e.,
// before InitMemory()), ensuring that reads are coherent with those after
// caches are re-enabled.
inline void ArchEarlyBootSyncData(uintptr_t addr, size_t size) {
  arch::CleanDataCacheRange(addr, size);
}

// Ditto for a range to be allocated. In this case, we are assuming that we
// will write to this range and we do not care what was there previously.
inline void ArchEarlyBootSyncAllocation(uintptr_t addr, size_t size) {
  arch::InvalidateDataCacheRange(addr, size);
}

// Whether any early boot data access synchronization scheme is necessary,
// equivalent to the above routines not being a no-op.
constexpr bool kArchEarlyBootDataSynchronization = true;

#endif  // ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_EARLY_BOOT_H_
