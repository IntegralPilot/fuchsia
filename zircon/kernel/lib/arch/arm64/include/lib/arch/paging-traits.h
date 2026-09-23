// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_ARM64_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_
#define ZIRCON_KERNEL_LIB_ARCH_ARM64_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_

#include <lib/arch/arm64/paging-traits.h>

#include <string_view>

//
// This header gives a uniform arch-agnostic spelling to the definitions in
// <lib/arch/arm64/paging-traits.h>.
//

namespace arch {

using PagingConfiguration = ArmPagingConfiguration;

constexpr PagingConfiguration PagingConfigurationFromString(std::string_view name) {
  return ArmPagingConfigurationFromString(name);
}

template <PagingConfiguration Config>
using LowerPagingTraits = ArmLowerPagingTraits<Config>;

template <PagingConfiguration Config>
using UpperPagingTraits = ArmUpperPagingTraits<Config>;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_ARM64_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_
