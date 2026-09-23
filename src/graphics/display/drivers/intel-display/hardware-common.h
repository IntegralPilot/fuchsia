// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_HARDWARE_COMMON_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_HARDWARE_COMMON_H_

#include <array>
#include <format>
#include <span>

namespace registers {

enum class Platform {
  kSkylake,
  kKabyLake,
  kTigerLake,
  kTestDevice,
};

}  // namespace registers

namespace intel_display {

enum DdiId {
  DDI_A = 0,
  DDI_B,
  DDI_C,
  DDI_D,
  DDI_E,

  DDI_TC_1 = DDI_D,
  DDI_TC_2,
  DDI_TC_3,
  DDI_TC_4,
  DDI_TC_5,
  DDI_TC_6,
};

namespace internal {

constexpr std::array kDdisKabyLake = {
    DDI_A, DDI_B, DDI_C, DDI_D, DDI_E,
};

constexpr std::array kDdisTigerLake = {
    DDI_A, DDI_B, DDI_C, DDI_TC_1, DDI_TC_2, DDI_TC_3, DDI_TC_4, DDI_TC_5, DDI_TC_6,
};

}  // namespace internal

template <registers::Platform P>
constexpr std::span<const DdiId> DdiIds() {
  switch (P) {
    case registers::Platform::kKabyLake:
    case registers::Platform::kSkylake:
    case registers::Platform::kTestDevice:
      return internal::kDdisKabyLake;
    case registers::Platform::kTigerLake:
      return internal::kDdisTigerLake;
  }
}

// TODO(https://fxbug.dev/42060657): Support Transcoder D on Tiger Lake.
enum TranscoderId {
  TRANSCODER_A = 0,
  TRANSCODER_B,
  TRANSCODER_C,
  TRANSCODER_EDP,
};

namespace internal {

constexpr std::array kTranscodersKabyLake = {
    TRANSCODER_A,
    TRANSCODER_B,
    TRANSCODER_C,
    TRANSCODER_EDP,
};

constexpr std::array kTranscodersTigerLake = {
    TRANSCODER_A,
    TRANSCODER_B,
    TRANSCODER_C,
};

}  // namespace internal

template <registers::Platform P>
constexpr std::span<const TranscoderId> TranscoderIds() {
  switch (P) {
    case registers::Platform::kKabyLake:
    case registers::Platform::kSkylake:
    case registers::Platform::kTestDevice:
      return internal::kTranscodersKabyLake;
    case registers::Platform::kTigerLake:
      return internal::kTranscodersTigerLake;
  }
}

// TODO(https://fxbug.dev/42060657): Support Pipe D on Tiger Lake.
enum PipeId {
  PIPE_A = 0,
  PIPE_B,
  PIPE_C,
  PIPE_INVALID,
};

namespace internal {

constexpr std::array kPipeIdsKabyLake = {
    PIPE_A,
    PIPE_B,
    PIPE_C,
};

constexpr std::array kPipeIdsTigerLake = {
    PIPE_A,
    PIPE_B,
    PIPE_C,
};

}  // namespace internal

template <registers::Platform P>
constexpr std::span<const PipeId> PipeIds() {
  switch (P) {
    case registers::Platform::kKabyLake:
    case registers::Platform::kSkylake:
    case registers::Platform::kTestDevice:
      return internal::kPipeIdsKabyLake;
    case registers::Platform::kTigerLake:
      return internal::kPipeIdsTigerLake;
  }
}

enum PllId {
  DPLL_INVALID = -1,
  DPLL_0 = 0,
  DPLL_1,
  DPLL_2,
  DPLL_3,

  DPLL_TC_1,
  DPLL_TC_2,
  DPLL_TC_3,
  DPLL_TC_4,
  DPLL_TC_5,
  DPLL_TC_6,
};

namespace internal {

constexpr std::array kPllIdsKabyLake = {
    DPLL_0,
    DPLL_1,
    DPLL_2,
    DPLL_3,
};

// TODO(https://fxbug.dev/42061706): Add support for DPLL4.
constexpr std::array kPllIdsTigerLake = {
    DPLL_0, DPLL_1, DPLL_2, DPLL_TC_1, DPLL_TC_2, DPLL_TC_3, DPLL_TC_4, DPLL_TC_5, DPLL_TC_6,
};

}  // namespace internal

template <registers::Platform P>
constexpr std::span<const PllId> PllIds() {
  switch (P) {
    case registers::Platform::kSkylake:
    case registers::Platform::kKabyLake:
    case registers::Platform::kTestDevice:
      return internal::kPllIdsKabyLake;
    case registers::Platform::kTigerLake:
      return internal::kPllIdsTigerLake;
  }
}

// An upper bound on the number of displays that could be connected
// simultaneously. The bound holds across all display engine versions
// supported by this driver.
//
// Not all display engines can support this exact number of displays.
constexpr int kMaximumConnectedDisplayCount = 4;

}  // namespace intel_display

template <>
struct std::formatter<intel_display::DdiId> {
  constexpr auto parse(auto& ctx) { return ctx.begin(); }
  auto format(intel_display::DdiId ddi_id, std::format_context& ctx) const {
    return std::format_to(ctx.out(), "{}", static_cast<int>(ddi_id));
  }
};

template <>
struct std::formatter<intel_display::TranscoderId> {
  constexpr auto parse(auto& ctx) { return ctx.begin(); }
  auto format(intel_display::TranscoderId transcoder_id, std::format_context& ctx) const {
    return std::format_to(ctx.out(), "{}", transcoder_id);
  }
};

template <>
struct std::formatter<intel_display::PipeId> {
  constexpr auto parse(auto& ctx) { return ctx.begin(); }
  auto format(intel_display::PipeId pipe_id, std::format_context& ctx) const {
    return std::format_to(ctx.out(), "{}", static_cast<int>(pipe_id));
  }
};

template <>
struct std::formatter<intel_display::PllId> {
  constexpr auto parse(auto& ctx) { return ctx.begin(); }
  auto format(intel_display::PllId pll_id, std::format_context& ctx) const {
    return std::format_to(ctx.out(), "{}", static_cast<int>(pll_id));
  }
};

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_HARDWARE_COMMON_H_
