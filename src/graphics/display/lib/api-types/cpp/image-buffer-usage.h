// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_BUFFER_USAGE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_BUFFER_USAGE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/ImageBufferUsage`].
class ImageBufferUsage {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr ImageBufferUsage(const ImageBufferUsage::ConstructorArgs& args);

  explicit constexpr ImageBufferUsage(
      fuchsia_hardware_display_types::wire::ImageBufferUsage fidl_image_buffer_usage);

  constexpr ImageBufferUsage(const ImageBufferUsage&) noexcept = default;
  constexpr ImageBufferUsage(ImageBufferUsage&&) noexcept = default;
  constexpr ImageBufferUsage& operator=(const ImageBufferUsage&) noexcept = default;
  constexpr ImageBufferUsage& operator=(ImageBufferUsage&&) noexcept = default;
  ~ImageBufferUsage() = default;

  friend constexpr bool operator==(const ImageBufferUsage& lhs, const ImageBufferUsage& rhs);
  friend constexpr bool operator!=(const ImageBufferUsage& lhs, const ImageBufferUsage& rhs);

  constexpr fuchsia_hardware_display_types::wire::ImageBufferUsage ToFidl() const;

  constexpr ImageTilingType tiling_type() const { return tiling_type_; }

 private:
  struct ConstructorArgs {
    ImageTilingType tiling_type;
  };

  ImageTilingType tiling_type_;
};

constexpr ImageBufferUsage::ImageBufferUsage(const ImageBufferUsage::ConstructorArgs& args)
    : tiling_type_(args.tiling_type) {}

constexpr ImageBufferUsage::ImageBufferUsage(
    fuchsia_hardware_display_types::wire::ImageBufferUsage fidl_image_buffer_usage)
    : tiling_type_(fidl_image_buffer_usage.tiling_type) {}

constexpr bool operator==(const ImageBufferUsage& lhs, const ImageBufferUsage& rhs) {
  return lhs.tiling_type_ == rhs.tiling_type_;
}

constexpr bool operator!=(const ImageBufferUsage& lhs, const ImageBufferUsage& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_display_types::wire::ImageBufferUsage ImageBufferUsage::ToFidl() const {
  return fuchsia_hardware_display_types::wire::ImageBufferUsage{
      .tiling_type = tiling_type_.ToFidl(),
  };
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_BUFFER_USAGE_H_
