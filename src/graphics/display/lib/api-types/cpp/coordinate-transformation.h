// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COORDINATE_TRANSFORMATION_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COORDINATE_TRANSFORMATION_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>
#include <string_view>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/CoordinateTransformation`].
//
// See `::fuchsia_hardware_display_types::wire::CoordinateTransformation` for references.
//
// Instances are guaranteed to represent valid enum members.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class CoordinateTransformation {
 public:
  // True iff `fidl_transformation` is convertible to a valid CoordinateTransformation.
  [[nodiscard]] static constexpr bool IsValid(
      fuchsia_hardware_display_types::wire::CoordinateTransformation fidl_transformation);

  explicit constexpr CoordinateTransformation(
      fuchsia_hardware_display_types::wire::CoordinateTransformation fidl_transformation);

  constexpr CoordinateTransformation(const CoordinateTransformation&) noexcept = default;
  constexpr CoordinateTransformation(CoordinateTransformation&&) noexcept = default;
  constexpr CoordinateTransformation& operator=(const CoordinateTransformation&) noexcept = default;
  constexpr CoordinateTransformation& operator=(CoordinateTransformation&&) noexcept = default;
  ~CoordinateTransformation() = default;

  constexpr fuchsia_hardware_display_types::wire::CoordinateTransformation ToFidl() const;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const;

  // Returns a developer-facing string representation.
  std::string_view ToString() const;

  static const CoordinateTransformation kIdentity;
  static const CoordinateTransformation kReflectX;
  static const CoordinateTransformation kReflectY;
  static const CoordinateTransformation kRotateCcw90;
  static const CoordinateTransformation kRotateCcw180;
  static const CoordinateTransformation kRotateCcw270;
  static const CoordinateTransformation kRotateCcw90ReflectX;
  static const CoordinateTransformation kRotateCcw90ReflectY;

 private:
  friend constexpr bool operator==(const CoordinateTransformation& lhs,
                                   const CoordinateTransformation& rhs);
  friend constexpr bool operator!=(const CoordinateTransformation& lhs,
                                   const CoordinateTransformation& rhs);

  fuchsia_hardware_display_types::wire::CoordinateTransformation transformation_;
};

// static
constexpr bool CoordinateTransformation::IsValid(
    fuchsia_hardware_display_types::wire::CoordinateTransformation fidl_transformation) {
  switch (fidl_transformation) {
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kIdentity:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectX:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw180:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90ReflectX:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90ReflectY:
    case fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw270:
      return true;
  }
  return false;
}

constexpr CoordinateTransformation::CoordinateTransformation(
    fuchsia_hardware_display_types::wire::CoordinateTransformation fidl_transformation)
    : transformation_(fidl_transformation) {
  ZX_DEBUG_ASSERT(IsValid(fidl_transformation));
}

constexpr bool operator==(const CoordinateTransformation& lhs,
                          const CoordinateTransformation& rhs) {
  return lhs.transformation_ == rhs.transformation_;
}

constexpr bool operator!=(const CoordinateTransformation& lhs,
                          const CoordinateTransformation& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_display_types::wire::CoordinateTransformation
CoordinateTransformation::ToFidl() const {
  return transformation_;
}

constexpr uint32_t CoordinateTransformation::ValueForLogging() const {
  return static_cast<uint32_t>(transformation_);
}

inline constexpr const CoordinateTransformation CoordinateTransformation::kIdentity(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kIdentity);
inline constexpr const CoordinateTransformation CoordinateTransformation::kReflectX(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectX);
inline constexpr const CoordinateTransformation CoordinateTransformation::kReflectY(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY);
inline constexpr const CoordinateTransformation CoordinateTransformation::kRotateCcw90(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90);
inline constexpr const CoordinateTransformation CoordinateTransformation::kRotateCcw180(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw180);
inline constexpr const CoordinateTransformation CoordinateTransformation::kRotateCcw270(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw270);
inline constexpr const CoordinateTransformation CoordinateTransformation::kRotateCcw90ReflectX(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90ReflectX);
inline constexpr const CoordinateTransformation CoordinateTransformation::kRotateCcw90ReflectY(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kRotateCcw90ReflectY);

}  // namespace display

#if __cplusplus >= 202002L
template <>
struct std::formatter<display::CoordinateTransformation> : std::formatter<std::string_view> {
  auto format(const display::CoordinateTransformation& transformation,
              std::format_context& ctx) const {
    return std::formatter<std::string_view>::format(transformation.ToString(), ctx);
  }
};
#endif

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COORDINATE_TRANSFORMATION_H_
