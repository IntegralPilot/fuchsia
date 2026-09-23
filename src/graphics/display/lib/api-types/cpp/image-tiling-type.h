// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_TILING_TYPE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_TILING_TYPE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>
#include <string_view>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/ImageTilingTypeIdValue`].
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class ImageTilingType {
 public:
  explicit constexpr ImageTilingType(
      fuchsia_hardware_display_types::wire::ImageTilingTypeIdValue fidl_tiling_type_id_value)
      : tiling_type_id_(fidl_tiling_type_id_value) {}

  constexpr ImageTilingType(const ImageTilingType&) noexcept = default;
  constexpr ImageTilingType(ImageTilingType&&) noexcept = default;
  constexpr ImageTilingType& operator=(const ImageTilingType&) = default;
  constexpr ImageTilingType& operator=(ImageTilingType&&) noexcept = default;
  ~ImageTilingType() = default;

  constexpr fuchsia_hardware_display_types::wire::ImageTilingTypeIdValue ToFidl() const;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const;

  // Returns a developer-facing string representation.
  std::string_view ToString() const;

  // See [`fuchsia.hardware.display.types/IMAGE_TILING_TYPE_LINEAR`].
  static const ImageTilingType kLinear;

  // See [`fuchsia.hardware.display.types/IMAGE_TILING_TYPE_CAPTURE`].
  static const ImageTilingType kCapture;

 private:
  friend constexpr bool operator==(const ImageTilingType& lhs, const ImageTilingType& rhs);
  friend constexpr bool operator!=(const ImageTilingType& lhs, const ImageTilingType& rhs);

  fuchsia_hardware_display_types::wire::ImageTilingTypeIdValue tiling_type_id_;
};

constexpr inline ImageTilingType ImageTilingType::kLinear(
    fuchsia_hardware_display_types::wire::kImageTilingTypeLinear);
constexpr inline ImageTilingType ImageTilingType::kCapture{
    fuchsia_hardware_display_types::wire::kImageTilingTypeCapture};

constexpr bool operator==(const ImageTilingType& lhs, const ImageTilingType& rhs) {
  return lhs.tiling_type_id_ == rhs.tiling_type_id_;
}

constexpr bool operator!=(const ImageTilingType& lhs, const ImageTilingType& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_display_types::wire::ImageTilingTypeIdValue ImageTilingType::ToFidl()
    const {
  return tiling_type_id_;
}

constexpr uint32_t ImageTilingType::ValueForLogging() const {
  return static_cast<uint32_t>(tiling_type_id_);
}

}  // namespace display

#if __cplusplus >= 202002L
template <>
struct std::formatter<display::ImageTilingType> : std::formatter<std::string_view> {
  auto format(const display::ImageTilingType& tiling_type, std::format_context& ctx) const {
    return std::formatter<std::string_view>::format(tiling_type.ToString(), ctx);
  }
};
#endif

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_TILING_TYPE_H_
