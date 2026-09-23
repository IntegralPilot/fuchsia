// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_POWER_MODE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_POWER_MODE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>
#include <string_view>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/PowerMode`].
//
// See `::fuchsia_hardware_display_types::wire::PowerMode` for references.
//
// Instances are guaranteed to represent valid enum members.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class PowerMode {
 public:
  // True iff `fidl_power_mode` is convertible to a valid PowerMode.
  [[nodiscard]] static constexpr bool IsValid(
      fuchsia_hardware_display_types::wire::PowerMode fidl_power_mode);

  explicit constexpr PowerMode(fuchsia_hardware_display_types::wire::PowerMode fidl_power_mode);

  constexpr PowerMode(const PowerMode&) noexcept = default;
  constexpr PowerMode(PowerMode&&) noexcept = default;
  constexpr PowerMode& operator=(const PowerMode&) noexcept = default;
  constexpr PowerMode& operator=(PowerMode&&) noexcept = default;
  ~PowerMode() = default;

  constexpr fuchsia_hardware_display_types::wire::PowerMode ToFidl() const;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const;

  // Returns a developer-facing string representation.
  std::string_view ToString() const;

  static const PowerMode kOff;
  static const PowerMode kOn;
  static const PowerMode kDoze;
  static const PowerMode kDozeSuspend;

 private:
  friend constexpr bool operator==(const PowerMode& lhs, const PowerMode& rhs);
  friend constexpr bool operator!=(const PowerMode& lhs, const PowerMode& rhs);

  fuchsia_hardware_display_types::wire::PowerMode power_mode_;
};

// static
constexpr bool PowerMode::IsValid(fuchsia_hardware_display_types::wire::PowerMode fidl_power_mode) {
  switch (fidl_power_mode) {
    case fuchsia_hardware_display_types::wire::PowerMode::kOff:
    case fuchsia_hardware_display_types::wire::PowerMode::kOn:
    case fuchsia_hardware_display_types::wire::PowerMode::kDoze:
    case fuchsia_hardware_display_types::wire::PowerMode::kDozeSuspend:
      return true;
    default:
      return false;
  }
  return false;
}

constexpr PowerMode::PowerMode(fuchsia_hardware_display_types::wire::PowerMode fidl_power_mode)
    : power_mode_(fidl_power_mode) {
  ZX_DEBUG_ASSERT(IsValid(fidl_power_mode));
}

constexpr bool operator==(const PowerMode& lhs, const PowerMode& rhs) {
  return lhs.power_mode_ == rhs.power_mode_;
}

constexpr bool operator!=(const PowerMode& lhs, const PowerMode& rhs) { return !(lhs == rhs); }

constexpr fuchsia_hardware_display_types::wire::PowerMode PowerMode::ToFidl() const {
  return power_mode_;
}

constexpr uint32_t PowerMode::ValueForLogging() const { return static_cast<uint32_t>(power_mode_); }

inline constexpr const PowerMode PowerMode::kOff(
    fuchsia_hardware_display_types::wire::PowerMode::kOff);
inline constexpr const PowerMode PowerMode::kOn(
    fuchsia_hardware_display_types::wire::PowerMode::kOn);
inline constexpr const PowerMode PowerMode::kDoze(
    fuchsia_hardware_display_types::wire::PowerMode::kDoze);
inline constexpr const PowerMode PowerMode::kDozeSuspend(
    fuchsia_hardware_display_types::wire::PowerMode::kDozeSuspend);

}  // namespace display

#if __cplusplus >= 202002L
template <>
struct std::formatter<display::PowerMode> : std::formatter<std::string_view> {
  auto format(const display::PowerMode& mode, std::format_context& ctx) const {
    return std::formatter<std::string_view>::format(mode.ToString(), ctx);
  }
};
#endif

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_POWER_MODE_H_
