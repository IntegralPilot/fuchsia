// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_CLOCK_DRIVERS_VIM3_CLK_MESON_GATE_H_
#define SRC_DEVICES_CLOCK_DRIVERS_VIM3_CLK_MESON_GATE_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/assert.h>

#include <cstdint>
#include <optional>

namespace vim3_clock {

enum class RegisterBank { Hiu, Dos };

using meson_gate_descriptor_t = struct meson_gate_descriptor {
  const uint32_t id;
  const uint32_t offset;
  const uint32_t mask;
  const RegisterBank bank;
};

class MesonGate {
 public:
  MesonGate(uint32_t id, uint32_t offset, uint32_t mask, fdf::MmioView mmio)
      : id_(id), offset_(offset), mask_(mask), mmio_(mmio) {}

  void Enable();
  void Disable();

 private:
  void EnableHw();
  void DisableHw();

  // Number of times Enable has been called on this clock.
  // Clock is enabled iff `vote_count_` is greater than 0.
  int vote_count_ = 0;

  const uint32_t id_;
  const uint32_t offset_;
  const uint32_t mask_;
  fdf::MmioView mmio_;
};

constexpr uint32_t kG12bHhiSysCpuClkCntl1 = (0x57 << 2);
constexpr uint32_t kG12bHhiSysCpubClkCntl1 = (0x80 << 2);
constexpr uint32_t kG12bHhiSysCpubClkCntl = (0x82 << 2);
constexpr uint32_t kG12bHhiTsClkCntl = (0x64 << 2);
constexpr uint32_t kG12bHhiXtalDivnCntl = (0x2f << 2);
constexpr uint32_t kG12bDosGclkEn0 = (0x3f01 << 2);
constexpr uint32_t kG12bHhiGclkMpeg0 = (0x50 << 2);
constexpr uint32_t kG12bHhiGclkMpeg1 = (0x51 << 2);
constexpr uint32_t kG12bHhiGclkMpeg2 = (0x52 << 2);
constexpr uint32_t kHhiSysCpuClkCntl0 = (0x67 << 2);

// clang-format off
static constexpr meson_gate_descriptor_t kGateDescriptors[] = {
  // Sys CPU Clock Gates
  {.id = 0,   .offset = kG12bHhiSysCpuClkCntl1,  .mask=(1 << 24),        .bank=RegisterBank::Hiu},
  {.id = 1,   .offset = kG12bHhiSysCpuClkCntl1,  .mask=(1 << 1),         .bank=RegisterBank::Hiu},
  {.id = 2,   .offset = kG12bHhiXtalDivnCntl,    .mask=(1 << 11),        .bank=RegisterBank::Hiu},

  // Sys CPUB Clock Gates
  {.id = 3,   .offset = kG12bHhiSysCpubClkCntl1, .mask=(1 << 24),        .bank=RegisterBank::Hiu},
  {.id = 4,   .offset = kG12bHhiSysCpubClkCntl1, .mask=(1 << 1),         .bank=RegisterBank::Hiu},

  // Graphics
  {.id = 5,   .offset = kG12bDosGclkEn0,         .mask= 0x3ff,           .bank=RegisterBank::Dos},
  {.id = 6,   .offset = kG12bDosGclkEn0,         .mask=(0x7fff << 12),   .bank=RegisterBank::Dos},

  // MPeg 0 DOS
  {.id = 7,   .offset = kG12bHhiGclkMpeg0,       .mask=(1 << 1),         .bank=RegisterBank::Hiu},

  // USB Gates
  {.id = 8,   .offset = kG12bHhiGclkMpeg1,       .mask=(1 << 26),        .bank=RegisterBank::Hiu},
  {.id = 9,   .offset = kG12bHhiGclkMpeg2,       .mask=(1 << 8),         .bank=RegisterBank::Hiu},


  {.id = 10,  .offset = kG12bHhiXtalDivnCntl,    .mask=(1 << 12),        .bank=RegisterBank::Hiu},

  {.id = 11,  .offset = kG12bHhiGclkMpeg1,       .mask=(1 << 0),         .bank=RegisterBank::Hiu},
  {.id = 12,  .offset = kG12bHhiGclkMpeg0,       .mask=(1 << 26),        .bank=RegisterBank::Hiu},

  // Temp Sensors
  {.id = 13,  .offset = kG12bHhiTsClkCntl,       .mask=(1 << 8),         .bank=RegisterBank::Hiu},

};
// clang-format on

}  // namespace vim3_clock

#endif  // SRC_DEVICES_CLOCK_DRIVERS_VIM3_CLK_MESON_GATE_H_
