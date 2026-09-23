// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_NOP_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_NOP_H_

#include <array>
#include <cstdint>
#include <span>

namespace arch {

// Encodes information on the riscv64 `nop` instruction.
// See //zircon/kernel/lib/arch/include/lib/arch/nop.h for expectations for
// the static members of this struct.
struct RiscvNopTraits {
  static constexpr uint16_t kCnop[] = {0x01};
  static constexpr uint16_t kNop[] = {0x13, 0};

  static constexpr std::array<std::span<const uint16_t>, 2> kNopPatterns = {
      std::span{kNop},
      std::span{kCnop},
  };
};

}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_RISCV64_NOP_H_
