// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_
#define ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_

#include <lib/arch/x86/boot-cpuid.h>
#include <lib/arch/x86/paging-traits.h>

#include <hwreg/x86msr.h>

constexpr arch::X86PagingTraitsBase::MemoryType kArchNormalMemoryType = {};
constexpr arch::X86PagingTraitsBase::MemoryType ArchMmioMemoryType() { return {}; }

inline auto ArchCreatePagingState() {
  auto state =
      arch::X86PagingTraitsBase::SystemState::Create(hwreg::X86MsrIo{}, arch::BootCpuidIo{});
  // TODO(https://fxbug.dev/382573743): The kernel proper's paging code does
  // not yet support 1GiB pages.
  state.page1gb = false;
  return state;
}

#endif  // ZIRCON_KERNEL_ARCH_X86_PHYS_INCLUDE_PHYS_ARCH_ADDRESS_SPACE_H_
