// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_

#include <lib/arch/riscv64/feature.h>
#include <lib/arch/riscv64/paging-traits.h>

#include <phys/main.h>

#include "arch-phys-info.h"

using ArchLowerPagingTraits = arch::RiscvSv39PagingTraits;
using ArchUpperPagingTraits = ArchLowerPagingTraits;

inline constexpr auto kArchNormalMemoryType = arch::RiscvMemoryType::kPma;

inline arch::RiscvMemoryType ArchMmioMemoryType() {
  return gArchPhysInfo->cpu_features[arch::RiscvFeature::kSvpbmt] ? arch::RiscvMemoryType::kIo
                                                                  : arch::RiscvMemoryType::kPma;
}

inline arch::RiscvPagingTraitsBase::SystemState ArchCreatePagingState() { return {}; }

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_
