// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_
#define ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_

#include <lib/arch/arm64/memory.h>
#include <lib/arch/arm64/paging-traits.h>
#include <lib/arch/arm64/system.h>

inline constexpr arch::ArmMairNormalAttribute kArchNormalMemoryType = {
    .inner = arch::ArmCacheabilityAttribute::kWriteBackReadWriteAllocate,
    .outer = arch::ArmCacheabilityAttribute::kWriteBackReadWriteAllocate,
};

inline constexpr arch::ArmMairAttribute ArchMmioMemoryType() {
  return arch::ArmDeviceMemory::kNonGatheringNonReorderingEarlyAck;
}

inline arch::ArmSystemPagingState ArchCreatePagingState(
    const arch::ArmMemoryAttrIndirectionRegister& mair) {
  return arch::ArmSystemPagingState::Create<arch::ArmCurrentEl>(
      mair, arch::ArmShareabilityAttribute::kInner);
}

#endif  // ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_
