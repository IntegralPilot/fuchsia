// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_HOST_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_
#define ZIRCON_KERNEL_LIB_ARCH_HOST_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_

#include <lib/arch/x86/paging-traits.h>

//
// This header gives a version of the <lib/arch/paging-traits.h> API intended
// only for testing.
//

namespace arch {

using PagingConfiguration = X86PagingLevelCount;

constexpr PagingConfiguration PagingConfigurationFromString(std::string_view name) {
  using namespace std::string_view_literals;

  if (name == "4level"sv) {
    return X86PagingLevelCount::k4;
  }

  ZX_PANIC("Only the x86 4-level host paging configuration is supported for host; not \"%.*s\"",
           static_cast<int>(name.size()), name.data());
}

template <PagingConfiguration Config>
  requires(Config == X86PagingLevelCount::k4)
using LowerPagingTraits = X86FourLevelPagingTraits;

template <PagingConfiguration Config>
using UpperPagingTraits = LowerPagingTraits<Config>;

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_HOST_INCLUDE_LIB_ARCH_PAGING_TRAITS_H_
