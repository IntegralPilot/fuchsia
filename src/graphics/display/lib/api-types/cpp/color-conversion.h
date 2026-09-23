// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COLOR_CONVERSION_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COLOR_CONVERSION_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <zircon/assert.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <span>

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.engine/ColorConversion`].
//
// Instances are guaranteed to represent valid color conversion configurations.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
//
// Out-of-tree drivers must not use this class, because it will be reworked.
class ColorConversion {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // True iff `fidl_config` is convertible to a valid ColorConversion.
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_display_engine::wire::ColorConversion& fidl_config);

  static const ColorConversion kIdentity;

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr ColorConversion(const ColorConversion::ConstructorArgs& args);

  // `fidl_config` must be convertible to a valid ColorConversion.
  explicit constexpr ColorConversion(
      const fuchsia_hardware_display_engine::wire::ColorConversion& fidl_config);

  constexpr ColorConversion(const ColorConversion&) noexcept = default;
  constexpr ColorConversion(ColorConversion&&) noexcept = default;
  constexpr ColorConversion& operator=(const ColorConversion&) noexcept = default;
  constexpr ColorConversion& operator=(ColorConversion&&) noexcept = default;
  ~ColorConversion() = default;

  friend constexpr bool operator==(const ColorConversion& lhs, const ColorConversion& rhs);
  friend constexpr bool operator!=(const ColorConversion& lhs, const ColorConversion& rhs);

  constexpr fuchsia_hardware_display_engine::wire::ColorConversion ToFidl() const;

  std::array<float, 3> preoffsets() const { return preoffsets_; }
  std::array<std::array<float, 3>, 3> coefficients() const { return coefficients_; }
  std::array<float, 3> postoffsets() const { return postoffsets_; }

 private:
  struct ConstructorArgs {
    std::array<float, 3> preoffsets;
    std::array<std::array<float, 3>, 3> coefficients;
    std::array<float, 3> postoffsets;
  };

  // True iff all values from the given `values` are finite (not NaN or
  // infinity).
  //
  // TODO(https://fxbug.dev/42085024): Make it constexpr when C++23 is
  // supported. C++23 makes `std::isfinite()` a constexpr function.
  static bool AreAllFinite(std::span<const float> values);

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const ColorConversion::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(
      const fuchsia_hardware_display_engine::wire::ColorConversion& fidl_config);

  std::array<float, 3> preoffsets_ = {};
  std::array<std::array<float, 3>, 3> coefficients_ = {};
  std::array<float, 3> postoffsets_ = {};
};

// static
inline bool ColorConversion::AreAllFinite(std::span<const float> values) {
  return std::all_of(values.begin(), values.end(), [](float f) { return std::isfinite(f); });
}

// static
constexpr bool ColorConversion::IsValid(
    const fuchsia_hardware_display_engine::wire::ColorConversion& fidl_config) {
  if (!AreAllFinite(fidl_config.preoffsets)) {
    return false;
  }
  if (!AreAllFinite(fidl_config.coefficients[0])) {
    return false;
  }
  if (!AreAllFinite(fidl_config.coefficients[1])) {
    return false;
  }
  if (!AreAllFinite(fidl_config.coefficients[2])) {
    return false;
  }
  if (!AreAllFinite(fidl_config.postoffsets)) {
    return false;
  }
  return true;
}

constexpr ColorConversion::ColorConversion(const ColorConversion::ConstructorArgs& args)
    : preoffsets_(args.preoffsets),
      coefficients_(args.coefficients),
      postoffsets_(args.postoffsets) {
  DebugAssertIsValid(args);
}

constexpr ColorConversion::ColorConversion(
    const fuchsia_hardware_display_engine::wire::ColorConversion& fidl_config) {
  DebugAssertIsValid(fidl_config);
  std::copy(fidl_config.preoffsets.begin(), fidl_config.preoffsets.end(), preoffsets_.begin());
  std::copy(fidl_config.coefficients[0].begin(), fidl_config.coefficients[0].end(),
            coefficients_[0].begin());
  std::copy(fidl_config.coefficients[1].begin(), fidl_config.coefficients[1].end(),
            coefficients_[1].begin());
  std::copy(fidl_config.coefficients[2].begin(), fidl_config.coefficients[2].end(),
            coefficients_[2].begin());
  std::copy(fidl_config.postoffsets.begin(), fidl_config.postoffsets.end(), postoffsets_.begin());
}

constexpr bool operator==(const ColorConversion& lhs, const ColorConversion& rhs) {
  if (!std::equal(lhs.preoffsets_.begin(), lhs.preoffsets_.end(), rhs.preoffsets_.begin())) {
    return false;
  }
  if (!std::equal(lhs.coefficients_.begin(), lhs.coefficients_.end(), rhs.coefficients_.begin())) {
    return false;
  }
  if (!std::equal(lhs.postoffsets_.begin(), lhs.postoffsets_.end(), rhs.postoffsets_.begin())) {
    return false;
  }
  return true;
}

constexpr bool operator!=(const ColorConversion& lhs, const ColorConversion& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_display_engine::wire::ColorConversion ColorConversion::ToFidl() const {
  fuchsia_hardware_display_engine::wire::ColorConversion fidl_config;
  std::copy(preoffsets_.begin(), preoffsets_.end(), fidl_config.preoffsets.begin());
  std::copy(coefficients_[0].begin(), coefficients_[0].end(), fidl_config.coefficients[0].begin());
  std::copy(coefficients_[1].begin(), coefficients_[1].end(), fidl_config.coefficients[1].begin());
  std::copy(coefficients_[2].begin(), coefficients_[2].end(), fidl_config.coefficients[2].begin());
  std::copy(postoffsets_.begin(), postoffsets_.end(), fidl_config.postoffsets.begin());
  return fidl_config;
}

// static
constexpr void ColorConversion::DebugAssertIsValid(const ColorConversion::ConstructorArgs& args) {
  ZX_DEBUG_ASSERT(AreAllFinite(args.preoffsets));
  ZX_DEBUG_ASSERT(AreAllFinite(args.coefficients[0]));
  ZX_DEBUG_ASSERT(AreAllFinite(args.coefficients[1]));
  ZX_DEBUG_ASSERT(AreAllFinite(args.coefficients[2]));
  ZX_DEBUG_ASSERT(AreAllFinite(args.postoffsets));
}

// static
constexpr void ColorConversion::DebugAssertIsValid(
    const fuchsia_hardware_display_engine::wire::ColorConversion& fidl_config) {
  ZX_DEBUG_ASSERT(AreAllFinite(fidl_config.preoffsets));
  ZX_DEBUG_ASSERT(AreAllFinite(fidl_config.coefficients[0]));
  ZX_DEBUG_ASSERT(AreAllFinite(fidl_config.coefficients[1]));
  ZX_DEBUG_ASSERT(AreAllFinite(fidl_config.coefficients[2]));
  ZX_DEBUG_ASSERT(AreAllFinite(fidl_config.postoffsets));
}

// static
inline const ColorConversion ColorConversion::kIdentity({
    .preoffsets = {0.0f, 0.0f, 0.0f},
    .coefficients =
        {
            std::to_array({1.0f, 0.0f, 0.0f}),
            std::to_array({0.0f, 1.0f, 0.0f}),
            std::to_array({0.0f, 0.0f, 1.0f}),
        },
    .postoffsets = {0.0f, 0.0f, 0.0f},
});

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COLOR_CONVERSION_H_
