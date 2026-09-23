// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_METADATA_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_METADATA_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/dimensions.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/ImageMetadata`].
//
// Instances are guaranteed to represent images whose dimensions are supported
// by the display stack. See `Dimensions` for details on validity guarantees.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class ImageMetadata {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // True iff `image_metadata` is convertible to a valid ImageMetadata.
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_display_types::wire::ImageMetadata& fidl_image_metadata);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr ImageMetadata(const ImageMetadata::ConstructorArgs& args);

  // `fidl_image_metadata` must be convertible to a valid ImageMetadata.
  explicit constexpr ImageMetadata(
      const fuchsia_hardware_display_types::wire::ImageMetadata& fidl_image_metadata);

  constexpr ImageMetadata(const ImageMetadata&) noexcept = default;
  constexpr ImageMetadata(ImageMetadata&&) noexcept = default;
  constexpr ImageMetadata& operator=(const ImageMetadata&) noexcept = default;
  constexpr ImageMetadata& operator=(ImageMetadata&&) noexcept = default;
  ~ImageMetadata() = default;

  friend constexpr bool operator==(const ImageMetadata& lhs, const ImageMetadata& rhs);
  friend constexpr bool operator!=(const ImageMetadata& lhs, const ImageMetadata& rhs);

  constexpr fuchsia_hardware_display_types::wire::ImageMetadata ToFidl() const;

  constexpr const Dimensions& dimensions() const { return dimensions_; }
  constexpr ImageTilingType tiling_type() const { return tiling_type_; }

  constexpr int32_t width() const { return dimensions_.width(); }
  constexpr int32_t height() const { return dimensions_.height(); }

 private:
  struct ConstructorArgs {
    int32_t width;
    int32_t height;
    ImageTilingType tiling_type;
  };

  Dimensions dimensions_;
  ImageTilingType tiling_type_;
};

// static
constexpr bool ImageMetadata::IsValid(
    const fuchsia_hardware_display_types::wire::ImageMetadata& fidl_image_metadata) {
  return Dimensions::IsValid(fidl_image_metadata.dimensions);
}

constexpr ImageMetadata::ImageMetadata(const ImageMetadata::ConstructorArgs& args)
    : dimensions_({.width = args.width, .height = args.height}), tiling_type_(args.tiling_type) {}

constexpr ImageMetadata::ImageMetadata(
    const fuchsia_hardware_display_types::wire::ImageMetadata& fidl_image_metadata)
    : dimensions_(Dimensions::From(fidl_image_metadata.dimensions)),
      tiling_type_(fidl_image_metadata.tiling_type) {}

constexpr bool operator==(const ImageMetadata& lhs, const ImageMetadata& rhs) {
  return lhs.dimensions_ == rhs.dimensions_ && lhs.tiling_type_ == rhs.tiling_type_;
}

constexpr bool operator!=(const ImageMetadata& lhs, const ImageMetadata& rhs) {
  return !(lhs == rhs);
}

constexpr fuchsia_hardware_display_types::wire::ImageMetadata ImageMetadata::ToFidl() const {
  return fuchsia_hardware_display_types::wire::ImageMetadata{
      // The casts are guaranteed not to overflow (causing UB) because of the
      // allowed ranges on image widths and heights.
      .dimensions = dimensions_.ToFidl(),
      .tiling_type = tiling_type_.ToFidl(),
  };
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_IMAGE_METADATA_H_
