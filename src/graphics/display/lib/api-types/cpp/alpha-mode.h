// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ALPHA_MODE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ALPHA_MODE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>
#include <string_view>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/AlphaMode`].
//
// See `::fuchsia_hardware_display_types::wire::AlphaMode` for references.
//
// Instances are guaranteed to represent valid enum members.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
//
// Out-of-tree drivers must not use this interface, because it will be reworked.
class AlphaMode {
 public:
  // True iff `fidl_alpha_mode` is convertible to a valid AlphaMode.
  [[nodiscard]] static constexpr bool IsValid(
      fuchsia_hardware_display_types::wire::AlphaMode fidl_alpha_mode);

  explicit constexpr AlphaMode(fuchsia_hardware_display_types::wire::AlphaMode fidl_alpha_mode);

  constexpr AlphaMode(const AlphaMode&) noexcept = default;
  constexpr AlphaMode(AlphaMode&&) noexcept = default;
  constexpr AlphaMode& operator=(const AlphaMode&) noexcept = default;
  constexpr AlphaMode& operator=(AlphaMode&&) noexcept = default;
  ~AlphaMode() = default;

  constexpr fuchsia_hardware_display_types::wire::AlphaMode ToFidl() const;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const;

  // Returns a developer-facing string representation.
  std::string_view ToString() const;

  static const AlphaMode kDisable;
  static const AlphaMode kHwMultiply;
  static const AlphaMode kPremultiplied;

 private:
  friend constexpr bool operator==(const AlphaMode& lhs, const AlphaMode& rhs);
  friend constexpr bool operator!=(const AlphaMode& lhs, const AlphaMode& rhs);

  fuchsia_hardware_display_types::wire::AlphaMode alpha_mode_;
};

// static
constexpr bool AlphaMode::IsValid(fuchsia_hardware_display_types::wire::AlphaMode fidl_alpha_mode) {
  switch (fidl_alpha_mode) {
    case fuchsia_hardware_display_types::wire::AlphaMode::kDisable:
    case fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied:
    case fuchsia_hardware_display_types::wire::AlphaMode::kHwMultiply:
      return true;
  }
  return false;
}

constexpr AlphaMode::AlphaMode(fuchsia_hardware_display_types::wire::AlphaMode fidl_alpha_mode)
    : alpha_mode_(fidl_alpha_mode) {
  ZX_DEBUG_ASSERT(IsValid(fidl_alpha_mode));
}

constexpr bool operator==(const AlphaMode& lhs, const AlphaMode& rhs) {
  return lhs.alpha_mode_ == rhs.alpha_mode_;
}

constexpr bool operator!=(const AlphaMode& lhs, const AlphaMode& rhs) { return !(lhs == rhs); }

constexpr fuchsia_hardware_display_types::wire::AlphaMode AlphaMode::ToFidl() const {
  return alpha_mode_;
}

constexpr uint32_t AlphaMode::ValueForLogging() const { return static_cast<uint32_t>(alpha_mode_); }

inline constexpr const AlphaMode AlphaMode::kDisable(
    fuchsia_hardware_display_types::wire::AlphaMode::kDisable);
inline constexpr const AlphaMode AlphaMode::kPremultiplied(
    fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied);
inline constexpr const AlphaMode AlphaMode::kHwMultiply(
    fuchsia_hardware_display_types::wire::AlphaMode::kHwMultiply);

}  // namespace display

#if __cplusplus >= 202002L
template <>
struct std::formatter<display::AlphaMode> : std::formatter<std::string_view> {
  auto format(const display::AlphaMode& mode, std::format_context& ctx) const {
    return std::formatter<std::string_view>::format(mode.ToString(), ctx);
  }
};
#endif

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_ALPHA_MODE_H_
