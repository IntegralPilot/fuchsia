// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_

#include <lib/arch/riscv64/paging-traits.h>

#include <string_view>

//
// This header gives a uniform arch-agnostic spelling to the definitions in
// <lib/arch/riscv64/paging-traits.h>.
//

namespace arch {

using PagingConfiguration = RiscvSatp::Mode;

constexpr PagingConfiguration PagingConfigurationFromString(std::string_view name) {
  return RiscvSatpModeFromString(name);
}

template <PagingConfiguration Config>
using LowerPagingTraits = RiscvPagingTraits<Config>;

template <PagingConfiguration Config>
using UpperPagingTraits = LowerPagingTraits<Config>;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_
