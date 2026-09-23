// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_RECTANGLE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_RECTANGLE_H_

#include <fidl/fuchsia.math/cpp/wire.h>
#include <zircon/assert.h>

#include <cstdint>

#include "src/graphics/display/lib/api-types/cpp/dimensions.h"

namespace display {

// FIDL type [`fuchsia.math/RectU`] representation useful for the display stack.
//
// Also similar to the VkRect2D in the Vulkan API.
//
// See `::fuchsia_math::wire::RectU` for references.
//
// Instances represent rectangular axis-aligned regions inside raster images.
// The display stack uses the Vulkan coordinate space. The origin is at the
// image's top-left corner. The X axis points to the right, and the Y axis
// points downwards.
//
// Instances are guaranteed to represent regions of images whose dimensions are
// supported by the display stack. See `Dimensions` for details on validity
// guarantees.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class Rectangle {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // True iff `fidl_rectangle` is convertible to a valid Rectangle.
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::RectU& fidl_rectangle);

  // `fidl_rectangle` must be convertible to a valid Rectangle.
  //
  // This is not a constructor to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.math/RectU` has the same field names as
  // our supported designated initializer syntax.
  [[nodiscard]] static constexpr Rectangle From(const fuchsia_math::wire::RectU& fidl_rectangle);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Rectangle(const Rectangle::ConstructorArgs& args);

  constexpr Rectangle(const Rectangle&) noexcept = default;
  constexpr Rectangle(Rectangle&&) noexcept = default;
  constexpr Rectangle& operator=(const Rectangle&) noexcept = default;
  constexpr Rectangle& operator=(Rectangle&&) noexcept = default;
  ~Rectangle() = default;

  friend constexpr bool operator==(const Rectangle& lhs, const Rectangle& rhs);
  friend constexpr bool operator!=(const Rectangle& lhs, const Rectangle& rhs);

  constexpr fuchsia_math::wire::RectU ToFidl() const;

  // Guaranteed to be in [0, `Dimensions::kMaxWidth`].
  constexpr int32_t x() const { return x_; }

  // Guaranteed to be in [0, `Dimensions::kMaxHeight`].
  constexpr int32_t y() const { return y_; }

  constexpr const Dimensions& dimensions() const { return dimensions_; }

  constexpr int32_t width() const { return dimensions_.width(); }
  constexpr int32_t height() const { return dimensions_.height(); }

 private:
  struct ConstructorArgs {
    int32_t x;
    int32_t y;
    int32_t width;
    int32_t height;
  };

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const Rectangle::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(const fuchsia_math::wire::RectU& fidl_rectangle);

  int32_t x_;
  int32_t y_;
  Dimensions dimensions_;
};

// static
constexpr bool Rectangle::IsValid(const fuchsia_math::wire::RectU& fidl_rectangle) {
  if (fidl_rectangle.x < 0) {
    return false;
  }
  if (fidl_rectangle.x > Dimensions::kMaxWidth) {
    return false;
  }
  if (fidl_rectangle.y < 0) {
    return false;
  }
  if (fidl_rectangle.y > Dimensions::kMaxHeight) {
    return false;
  }
  if (fidl_rectangle.width < 0) {
    return false;
  }
  if (fidl_rectangle.width > Dimensions::kMaxWidth - fidl_rectangle.x) {
    return false;
  }
  if (fidl_rectangle.height < 0) {
    return false;
  }
  if (fidl_rectangle.height > Dimensions::kMaxHeight - fidl_rectangle.y) {
    return false;
  }

  return true;
}

constexpr Rectangle::Rectangle(const Rectangle::ConstructorArgs& args)
    : x_(args.x), y_(args.y), dimensions_({.width = args.width, .height = args.height}) {
  DebugAssertIsValid(args);
}

// static
constexpr Rectangle Rectangle::From(const fuchsia_math::wire::RectU& fidl_rectangle) {
  DebugAssertIsValid(fidl_rectangle);
  return Rectangle({
      .x = static_cast<int32_t>(fidl_rectangle.x),
      .y = static_cast<int32_t>(fidl_rectangle.y),
      .width = static_cast<int32_t>(fidl_rectangle.width),
      .height = static_cast<int32_t>(fidl_rectangle.height),
  });
}

constexpr bool operator==(const Rectangle& lhs, const Rectangle& rhs) {
  return lhs.x_ == rhs.x_ && lhs.y_ == rhs.y_ && lhs.dimensions_ == rhs.dimensions_;
}

constexpr bool operator!=(const Rectangle& lhs, const Rectangle& rhs) { return !(lhs == rhs); }

constexpr fuchsia_math::wire::RectU Rectangle::ToFidl() const {
  return fuchsia_math::wire::RectU{
      // The casts are guaranteed not to overflow (causing UB) because of the
      // allowed ranges on image widths and heights.
      .x = static_cast<uint32_t>(x_),
      .y = static_cast<uint32_t>(y_),
      .width = static_cast<uint32_t>(dimensions_.width()),
      .height = static_cast<uint32_t>(dimensions_.height()),
  };
}

// static
constexpr void Rectangle::DebugAssertIsValid(const Rectangle::ConstructorArgs& args) {
  ZX_DEBUG_ASSERT(args.x >= 0);
  ZX_DEBUG_ASSERT(args.x <= Dimensions::kMaxWidth);
  ZX_DEBUG_ASSERT(args.y >= 0);
  ZX_DEBUG_ASSERT(args.y <= Dimensions::kMaxHeight);
  ZX_DEBUG_ASSERT(args.width >= 0);
  ZX_DEBUG_ASSERT(args.width <= Dimensions::kMaxWidth - args.x);
  ZX_DEBUG_ASSERT(args.height >= 0);
  ZX_DEBUG_ASSERT(args.height <= Dimensions::kMaxHeight - args.y);
  ZX_DEBUG_ASSERT((args.width == 0) == (args.height == 0));
}

// static
constexpr void Rectangle::DebugAssertIsValid(const fuchsia_math::wire::RectU& fidl_rectangle) {
  ZX_DEBUG_ASSERT(fidl_rectangle.x >= 0);
  ZX_DEBUG_ASSERT(fidl_rectangle.x <= Dimensions::kMaxWidth);
  ZX_DEBUG_ASSERT(fidl_rectangle.y >= 0);
  ZX_DEBUG_ASSERT(fidl_rectangle.y <= Dimensions::kMaxHeight);
  ZX_DEBUG_ASSERT(fidl_rectangle.width >= 0);
  ZX_DEBUG_ASSERT(fidl_rectangle.width <= Dimensions::kMaxWidth - fidl_rectangle.x);
  ZX_DEBUG_ASSERT(fidl_rectangle.height >= 0);
  ZX_DEBUG_ASSERT(fidl_rectangle.height <= Dimensions::kMaxHeight - fidl_rectangle.y);
  ZX_DEBUG_ASSERT((fidl_rectangle.width == 0) == (fidl_rectangle.height == 0));
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_RECTANGLE_H_
