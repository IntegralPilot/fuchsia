// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_APPLE_AIC_H_
#define ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_APPLE_AIC_H_

#include <lib/arch/sysreg.h>
#include <stdint.h>

#include <array>
#include <optional>

namespace arch {

struct AppleIpiStatus : SysRegBase<AppleIpiStatus, uint64_t> {
  DEF_BIT(0, pending);
};
ARCH_ARM64_SYSREG(AppleIpiStatus, "S3_5_c15_c1_1");

struct AppleIpiLocalRequest : SysRegBase<AppleIpiLocalRequest, uint64_t> {
  DEF_FIELD(7, 0, target);
};
ARCH_ARM64_SYSREG(AppleIpiLocalRequest, "S3_5_c15_c0_0");

namespace apple {

inline constexpr uint32_t kAicPhysTimer = 1;
inline constexpr uint32_t kAicVirtTimer = 2;
inline constexpr uint32_t kAicExternalBase = 3;
inline constexpr uint32_t kAicMaxExternalIrqs = 1600;

struct Aic3Layout {
  static constexpr uint32_t kInfo = 0x4;
  static constexpr uint32_t kCapacity = 0xc;
  static constexpr uint32_t kConfig = 0x14;
  static constexpr uint32_t kEnable = 1;
  static constexpr uint32_t kIrqConfig = 0x10000;

  uint32_t num_irqs;
  uint32_t mask_set;
  uint32_t mask_clear;

  static constexpr bool ValidMmio(uint64_t base, uint32_t size, uint32_t event) {
    return base % 4096 == 0 && size >= kConfig + 4 && size % 4096 == 0 && base + size >= base &&
           event % 4 == 0 && event <= size - 4;
  }

  static constexpr std::optional<Aic3Layout> Create(uint32_t info, uint32_t capacity,
                                                    uint32_t size) {
    const uint32_t count = info & 0xffff;
    const uint32_t maximum = capacity & 0xffff;
    const uint32_t last_die = (info >> 24) & 0xf;
    if (last_die != 0 || count == 0 || count > kAicMaxExternalIrqs || count > maximum ||
        maximum > 4096 || maximum % 32 != 0) {
      return std::nullopt;
    }
    const uint32_t mask_set = kIrqConfig + 4 * maximum + 2 * (maximum / 8);
    const uint32_t mask_clear = mask_set + maximum / 8;
    if (mask_clear + maximum / 8 > size) {
      return std::nullopt;
    }
    return Aic3Layout{count, mask_set, mask_clear};
  }

  constexpr std::optional<uint32_t> DecodeEvent(uint32_t event) const {
    // Hardware events have type 1 and die 0 in the upper halfword.
    const uint32_t irq = event & 0xffff;
    if ((event >> 16) != 1 || irq >= num_irqs) {
      return std::nullopt;
    }
    return irq;
  }
};

// The caller serializes mask changes and completion, releasing its lock while
// invoking handlers so a handler can leave its source masked.
class Aic3State {
 public:
  explicit constexpr Aic3State(Aic3Layout layout) : layout_(layout) {}

  template <class Io>
  void SetMask(Io& io, uint32_t irq, bool masked) {
    enabled_[irq] = !masked;
    io.Write(masked ? layout_.mask_set + 4 * (irq / 32) : layout_.mask_clear + 4 * (irq / 32),
             uint32_t{1} << (irq % 32));
  }

  template <class Io>
  void Complete(Io& io, uint32_t irq, bool handled) {
    enabled_[irq] = enabled_[irq] && handled;
    if (enabled_[irq]) {
      io.Write(layout_.mask_clear + 4 * (irq / 32), uint32_t{1} << (irq % 32));
    }
  }

 private:
  Aic3Layout layout_;
  std::array<bool, kAicMaxExternalIrqs> enabled_{};
};

}  // namespace apple
}  // namespace arch

#endif  // ZIRCON_KERNEL_LIB_ARCH_INCLUDE_LIB_ARCH_ARM64_APPLE_AIC_H_
