// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_TICKS_H_
#define ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_TICKS_H_

// The assembler provides the `rdtime` mnemonic, so no macro is needed here.

#ifndef __ASSEMBLER__

#include <cstdint>

namespace arch {

// This is the C++ type that the `rdtime` instruction delivers.
// Higher-level kernel code knows how to translate this into the Zircon
// monotonic clock's zx_instant_mono_ticks_t.
struct EarlyTicks {
  uint64_t time;

  [[gnu::always_inline]] static EarlyTicks Get() {
    EarlyTicks sample;
    __asm__ volatile("rdtime %0" : "=r"(sample.time));
    return sample;
  }

  static constexpr EarlyTicks Zero() { return {0}; }
};

}  // namespace arch

#endif  // __ASSEMBLER__

#endif  // ZIRCON_KERNEL_LIB_ARCH_RISCV64_INCLUDE_LIB_ARCH_TICKS_H_
