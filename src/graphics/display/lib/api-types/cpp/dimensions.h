// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DIMENSIONS_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DIMENSIONS_H_

#include <fidl/fuchsia.math/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>

namespace display {

// FIDL type [`fuchsia.math/SizeU`] representation useful for the display stack.
//
// Similar to the VkExtent2D concept in the Vulkan API.
//
// See `::fuchsia_math::wire::SizeU` for references.
//
// Instances represent the dimensions (sizes) of rectangular axis-aligned
// regions inside raster images. Instances are guaranteed to represent
// dimensions supported by the display stack.
//
// The canonical representation of empty region dimensions is 0x0 (width and
// height are both zero). Instances are guaranteed to only use the canonical
// representation for empty region dimensions.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class Dimensions {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // The maximum image width supported by the display stack.
  static constexpr int kMaxWidth = 65535;

  // The maximum image height supported by the display stack.
  static constexpr int kMaxHeight = 65535;

  // True iff `fidl_size` is convertible to a valid Dimensions instance.
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::SizeU& fidl_size);

  // `fidl_size` must be convertible to a valid Dimensions instance.
  //
  // This is not a constructor to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.math/SizeU` has the same field names as
  // our supported designated initializer syntax.
  [[nodiscard]] static constexpr Dimensions From(const fuchsia_math::wire::SizeU& fidl_size);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Dimensions(const Dimensions::ConstructorArgs& args);

  constexpr Dimensions(const Dimensions&) noexcept = default;
  constexpr Dimensions(Dimensions&&) noexcept = default;
  constexpr Dimensions& operator=(const Dimensions&) noexcept = default;
  constexpr Dimensions& operator=(Dimensions&&) noexcept = default;
  ~Dimensions() = default;

  friend constexpr bool operator==(const Dimensions& lhs, const Dimensions& rhs);
  friend constexpr bool operator!=(const Dimensions& lhs, const Dimensions& rhs);

  constexpr fuchsia_math::wire::SizeU ToFidl() const;

  // Guaranteed to be in [0, `kMaxWidth`].
  constexpr int32_t width() const { return width_; }

  // Guaranteed to be in [0, `kMaxHeight`].
  constexpr int32_t height() const { return height_; }

  // True iff the dimensions represent an empty region.
  constexpr bool IsEmpty() const { return width_ == 0; }

 private:
  struct ConstructorArgs {
    int32_t width;
    int32_t height;
  };

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const Dimensions::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(const fuchsia_math::wire::SizeU& fidl_size);

  int32_t width_;
  int32_t height_;
};

// static
constexpr bool Dimensions::IsValid(const fuchsia_math::wire::SizeU& fidl_size) {
  if (fidl_size.width < 0) {
    return false;
  }
  if (fidl_size.width > kMaxWidth) {
    return false;
  }
  if (fidl_size.height < 0) {
    return false;
  }
  if (fidl_size.height > kMaxHeight) {
    return false;
  }

  const bool width_is_zero = (fidl_size.width == 0);
  const bool height_is_zero = (fidl_size.height == 0);
  return width_is_zero == height_is_zero;
}

constexpr Dimensions::Dimensions(const ConstructorArgs& args)
    : width_(args.width), height_(args.height) {
  DebugAssertIsValid(args);
}

// static
constexpr Dimensions Dimensions::From(const fuchsia_math::wire::SizeU& fidl_size) {
  DebugAssertIsValid(fidl_size);
  return Dimensions({
      .width = static_cast<int32_t>(fidl_size.width),
      .height = static_cast<int32_t>(fidl_size.height),
  });
}

constexpr bool operator==(const Dimensions& lhs, const Dimensions& rhs) {
  return lhs.width_ == rhs.width_ && lhs.height_ == rhs.height_;
}

constexpr bool operator!=(const Dimensions& lhs, const Dimensions& rhs) { return !(lhs == rhs); }

constexpr fuchsia_math::wire::SizeU Dimensions::ToFidl() const {
  return fuchsia_math::wire::SizeU{
      // The casts are guaranteed not to overflow (causing UB) because of the
      // allowed ranges on image widths and heights.
      .width = static_cast<uint32_t>(width_),
      .height = static_cast<uint32_t>(height_),
  };
}

// static
constexpr void Dimensions::DebugAssertIsValid(const Dimensions::ConstructorArgs& args) {
  ZX_DEBUG_ASSERT(args.width >= 0);
  ZX_DEBUG_ASSERT(args.width <= kMaxWidth);
  ZX_DEBUG_ASSERT(args.height >= 0);
  ZX_DEBUG_ASSERT(args.height <= kMaxHeight);
  ZX_DEBUG_ASSERT((args.width == 0) == (args.height == 0));
}

// static
constexpr void Dimensions::DebugAssertIsValid(const fuchsia_math::wire::SizeU& fidl_size) {
  ZX_DEBUG_ASSERT(fidl_size.width <= kMaxWidth);
  ZX_DEBUG_ASSERT(fidl_size.height <= kMaxHeight);
  ZX_DEBUG_ASSERT((fidl_size.width == 0) == (fidl_size.height == 0));
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_DIMENSIONS_H_
