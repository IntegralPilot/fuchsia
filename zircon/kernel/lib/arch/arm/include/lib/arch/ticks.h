// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_ARCH_ARM_INCLUDE_LIB_ARCH_TICKS_H_
#define ZIRCON_KERNEL_LIB_ARCH_ARM_INCLUDE_LIB_ARCH_TICKS_H_

#ifndef __ASSEMBLER__
#include <lib/arch/intrin.h>

#include <cstdint>

namespace arch {

// This is the C++ type that the assembly macro `sample_ticks` delivers.
// Higher-level kernel code knows how to translate this into the Zircon
// monotonic clock's zx_instant_mono_ticks_t.
struct EarlyTicks {
  uint64_t cntpct, cntvct;

  [[gnu::always_inline]] static EarlyTicks Get() {
    return {
        __arm_rsr64("cp15:0:c14"),
        __arm_rsr64("cp15:1:c14"),
    };
  }

  [[gnu::always_inline]] static EarlyTicks Zero() { return {0, 0}; }
};

}  // namespace arch

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_KERNEL_LIB_ARCH_ARM_INCLUDE_LIB_ARCH_TICKS_H_
