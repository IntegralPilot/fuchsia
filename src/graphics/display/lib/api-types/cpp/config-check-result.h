// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CONFIG_CHECK_RESULT_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CONFIG_CHECK_RESULT_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>
#include <string_view>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/ConfigCheckResult`].
//
// See `::fuchsia_hardware_display_types::wire::ConfigCheckResult` for references.
//
// Instances are guaranteed to represent valid enum members.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class ConfigCheckResult {
 public:
  // True iff `fidl_result` is convertible to a valid ConfigCheckResult.
  [[nodiscard]] static constexpr bool IsValid(
      fuchsia_hardware_display_types::wire::ConfigResult fidl_result);

  explicit constexpr ConfigCheckResult(
      fuchsia_hardware_display_types::wire::ConfigResult fidl_result);

  constexpr ConfigCheckResult(const ConfigCheckResult&) noexcept = default;
  constexpr ConfigCheckResult(ConfigCheckResult&&) noexcept = default;
  constexpr ConfigCheckResult& operator=(const ConfigCheckResult&) noexcept = default;
  constexpr ConfigCheckResult& operator=(ConfigCheckResult&&) noexcept = default;
  ~ConfigCheckResult() = default;

  constexpr fuchsia_hardware_display_types::wire::ConfigResult ToFidl() const;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const;

  // Returns a developer-facing string representation.
  std::string_view ToString() const;

  static const ConfigCheckResult kOk;
  static const ConfigCheckResult kEmptyConfig;
  static const ConfigCheckResult kInvalidConfig;
  static const ConfigCheckResult kUnsupportedConfig;
  static const ConfigCheckResult kTooManyDisplays;
  static const ConfigCheckResult kUnsupportedDisplayModes;

 private:
  friend constexpr bool operator==(const ConfigCheckResult& lhs, const ConfigCheckResult& rhs);
  friend constexpr bool operator!=(const ConfigCheckResult& lhs, const ConfigCheckResult& rhs);

  fuchsia_hardware_display_types::wire::ConfigResult result_;
};

// static
constexpr bool ConfigCheckResult::IsValid(
    fuchsia_hardware_display_types::wire::ConfigResult fidl_result) {
  switch (fidl_result) {
    case fuchsia_hardware_display_types::wire::ConfigResult::kOk:
    case fuchsia_hardware_display_types::wire::ConfigResult::kEmptyConfig:
    case fuchsia_hardware_display_types::wire::ConfigResult::kInvalidConfig:
    case fuchsia_hardware_display_types::wire::ConfigResult::kUnsupportedConfig:
    case fuchsia_hardware_display_types::wire::ConfigResult::kTooManyDisplays:
    case fuchsia_hardware_display_types::wire::ConfigResult::kUnsupportedDisplayModes:
      return true;
  }
  return false;
}

constexpr ConfigCheckResult::ConfigCheckResult(
    fuchsia_hardware_display_types::wire::ConfigResult fidl_result)
    : result_(fidl_result) {
  ZX_DEBUG_ASSERT(IsValid(fidl_result));
}

constexpr bool operator==(const ConfigCheckResult& lhs, const ConfigCheckResult& rhs) {
  return lhs.result_ == rhs.result_;
}

constexpr bool operator!=(const ConfigCheckResult& lhs, const ConfigCheckResult& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_display_types::wire::ConfigResult ConfigCheckResult::ToFidl() const {
  return result_;
}

constexpr uint32_t ConfigCheckResult::ValueForLogging() const {
  return static_cast<uint32_t>(result_);
}

inline constexpr const ConfigCheckResult ConfigCheckResult::kOk(
    fuchsia_hardware_display_types::wire::ConfigResult::kOk);
inline constexpr const ConfigCheckResult ConfigCheckResult::kEmptyConfig(
    fuchsia_hardware_display_types::wire::ConfigResult::kEmptyConfig);
inline constexpr const ConfigCheckResult ConfigCheckResult::kInvalidConfig(
    fuchsia_hardware_display_types::wire::ConfigResult::kInvalidConfig);
inline constexpr const ConfigCheckResult ConfigCheckResult::kUnsupportedConfig(
    fuchsia_hardware_display_types::wire::ConfigResult::kUnsupportedConfig);
inline constexpr const ConfigCheckResult ConfigCheckResult::kTooManyDisplays(
    fuchsia_hardware_display_types::wire::ConfigResult::kTooManyDisplays);
inline constexpr const ConfigCheckResult ConfigCheckResult::kUnsupportedDisplayModes(
    fuchsia_hardware_display_types::wire::ConfigResult::kUnsupportedDisplayModes);

}  // namespace display

#if __cplusplus >= 202002L
template <>
struct std::formatter<display::ConfigCheckResult> : std::formatter<std::string_view> {
  auto format(const display::ConfigCheckResult& result, std::format_context& ctx) const {
    return std::formatter<std::string_view>::format(result.ToString(), ctx);
  }
};
#endif

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_CONFIG_CHECK_RESULT_H_
