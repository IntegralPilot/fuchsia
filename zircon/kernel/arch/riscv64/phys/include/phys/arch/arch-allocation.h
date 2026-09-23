// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ARCH_ALLOCATION_H_
#define ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ARCH_ALLOCATION_H_

#include <stdint.h>

#include <ktl/optional.h>

constexpr ktl::optional<uint64_t> kArchAllocationMinAddr = ktl::nullopt;
constexpr ktl::optional<uint64_t> kArchAllocationMaxAddr = ktl::nullopt;

#endif  // ZIRCON_KERNEL_ARCH_RISCV64_PHYS_INCLUDE_PHYS_ARCH_ARCH_ALLOCATION_H_
