// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_RECTANGLE_H_
#define SRC_UI_SCENIC_LIB_TYPES_RECTANGLE_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>

#include "src/ui/scenic/lib/types/extent2.h"
#include "src/ui/scenic/lib/types/point2.h"
#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

// Safer, more convenient alternative to `fuchsia_math::Rect`.
class Rectangle {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Returns true iff the args can be used to construct a valid Rectangle.
  // If `should_assert` is true, invalid args will trigger a FX_DCHECK.
  //
  // Validity constraints:
  // - signed/unsigned conversion MUST NOT result in numeric overflow
  // - both the origin and opposite corner MUST be representable without numeric overflow
  [[nodiscard]] static constexpr bool IsValid(const Rectangle::ConstructorArgs& args,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::Rect& rect,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::Rect& rect,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::RectU& rect,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::RectU& rect,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const Point2& origin, const Extent2& extent,
                                              bool should_assert = false);

  // Constructors.  All arguments must be valid; use `IsValid()` to validate if you're not sure.
  [[nodiscard]] static constexpr Rectangle From(const fuchsia_math::Rect& fidl_rectangle);
  [[nodiscard]] static constexpr Rectangle From(const fuchsia_math::wire::Rect& fidl_rectangle);
  [[nodiscard]] static constexpr Rectangle From(const fuchsia_math::RectU& fidl_rectangle);
  [[nodiscard]] static constexpr Rectangle From(const fuchsia_math::wire::RectU& fidl_rectangle);
  [[nodiscard]] static constexpr Rectangle From(const Point2& origin, const Extent2& extent);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Rectangle(const Rectangle::ConstructorArgs& args);

  // Empty rectangle.  Allows usage as key in std C++ containers.
  constexpr Rectangle();

  constexpr Rectangle(const Rectangle&) noexcept = default;
  constexpr Rectangle(Rectangle&&) noexcept = default;
  constexpr Rectangle& operator=(const Rectangle&) noexcept = default;
  constexpr Rectangle& operator=(Rectangle&&) noexcept = default;
  ~Rectangle() = default;

  friend constexpr bool operator==(const Rectangle& lhs, const Rectangle& rhs);
  friend constexpr bool operator!=(const Rectangle& lhs, const Rectangle& rhs);

  // Conversion to FIDL types.
  fuchsia_math::RectU ToFidlRectU() const;
  constexpr fuchsia_math::wire::RectU ToWireRectU() const;
  fuchsia_math::RectF ToFidlRectF() const;

  constexpr const Point2& origin() const { return origin_; }
  constexpr const Extent2& extent() const { return extent_; }
  // The corner diagonally opposite from the origin.
  constexpr Point2 opposite() const { return Point2({.x = x() + width(), .y = y() + height()}); }

  constexpr int32_t x() const { return origin_.x(); }
  constexpr int32_t y() const { return origin_.y(); }
  constexpr int32_t width() const { return extent_.width(); }
  constexpr int32_t height() const { return extent_.height(); }

  constexpr bool Contains(const Point2& point) const;

  // TODO(https://fxbug.dev/426058270): the concept of "rectangle emptiness" needs to be specified
  // in order to support other utility functions, like:
  //   - `bool Contains(Point)`
  //   - `bool Intersects(Rectangle)`
  // bool IsEmpty() const;

 private:
  struct ConstructorArgs {
    int32_t x;
    int32_t y;
    int32_t width;
    int32_t height;
  };

  Point2 origin_;
  Extent2 extent_;
};

// static
constexpr bool Rectangle::IsValid(const Rectangle::ConstructorArgs& args, bool should_assert) {
  if (args.width < 0) {
    FX_DCHECK(!should_assert) << "width must be non-negative: " << args.width;
    return false;
  }
  if (args.height < 0) {
    FX_DCHECK(!should_assert) << "height must be non-negative: " << args.height;
    return false;
  }

  const int32_t& x = args.x;
  const int32_t& y = args.y;
  const int32_t& width = args.width;
  const int32_t& height = args.height;

  // The following overflow checks are based on those described here:
  //    https://wiki.sei.cmu.edu/confluence/display/c/INT32-C.
  //    +Ensure+that+operations+on+signed+integers+do+not+result+in+overflow
  if (x > 0 && width > (INT_MAX - x)) {
    FX_DCHECK(!should_assert) << "Overflow when adding x and width.  x=" << x
                              << ", width=" << width;
    return false;
  }
  if (y > 0 && height > (INT_MAX - y)) {
    FX_DCHECK(!should_assert) << "Overflow when adding y and height.  y=" << y
                              << ", height=" << height;
    return false;
  }
  return true;
}

// static
constexpr bool Rectangle::IsValid(const fuchsia_math::Rect& rect, bool should_assert) {
  const ConstructorArgs args{
      .x = rect.x(), .y = rect.y(), .width = rect.width(), .height = rect.height()};
  return IsValid(args, should_assert);
}

// static
constexpr bool Rectangle::IsValid(const fuchsia_math::wire::Rect& rect, bool should_assert) {
  const ConstructorArgs args{.x = rect.x, .y = rect.y, .width = rect.width, .height = rect.height};
  return IsValid(args, should_assert);
}

// static
constexpr bool Rectangle::IsValid(const fuchsia_math::RectU& rect, bool should_assert) {
  if (rect.x() > INT32_MAX || rect.y() > INT32_MAX || rect.width() > INT32_MAX ||
      rect.height() > INT32_MAX) {
    FX_DCHECK(!should_assert) << "Overflow when casting from unsigned to signed.  x=" << rect.x()
                              << ", y=" << rect.y() << ", width=" << rect.width()
                              << ", height=" << rect.height();
    return false;
  }

  const ConstructorArgs args{.x = static_cast<int32_t>(rect.x()),
                             .y = static_cast<int32_t>(rect.y()),
                             .width = static_cast<int32_t>(rect.width()),
                             .height = static_cast<int32_t>(rect.height())};
  return IsValid(args, should_assert);
}

// static
constexpr bool Rectangle::IsValid(const fuchsia_math::wire::RectU& rect, bool should_assert) {
  if (rect.x > INT32_MAX || rect.y > INT32_MAX || rect.width > INT32_MAX ||
      rect.height > INT32_MAX) {
    FX_DCHECK(!should_assert) << "Overflow when casting from unsigned to signed.  x=" << rect.x
                              << ", y=" << rect.y << ", width=" << rect.width
                              << ", height=" << rect.height;
    return false;
  }

  const ConstructorArgs args{.x = static_cast<int32_t>(rect.x),
                             .y = static_cast<int32_t>(rect.y),
                             .width = static_cast<int32_t>(rect.width),
                             .height = static_cast<int32_t>(rect.height)};
  return IsValid(args, should_assert);
}

// static
constexpr bool Rectangle::IsValid(const Point2& origin, const Extent2& extent, bool should_assert) {
  const ConstructorArgs args{
      .x = origin.x(), .y = origin.y(), .width = extent.width(), .height = extent.height()};
  return IsValid(args, should_assert);
}

constexpr Rectangle::Rectangle(const Rectangle::ConstructorArgs& args)
    : origin_({.x = args.x, .y = args.y}), extent_({.width = args.width, .height = args.height}) {
  auto _ = IsValid(args, true);
}

constexpr Rectangle::Rectangle() : Rectangle({.x = 0, .y = 0, .width = 0, .height = 0}) {}

// static
constexpr Rectangle Rectangle::From(const fuchsia_math::Rect& fidl_rectangle) {
  return Rectangle({.x = fidl_rectangle.x(),
                    .y = fidl_rectangle.y(),
                    .width = fidl_rectangle.width(),
                    .height = fidl_rectangle.height()});
}

// static
constexpr Rectangle Rectangle::From(const fuchsia_math::wire::Rect& fidl_rectangle) {
  return Rectangle({.x = fidl_rectangle.x,
                    .y = fidl_rectangle.y,
                    .width = fidl_rectangle.width,
                    .height = fidl_rectangle.height});
}

// static
constexpr Rectangle Rectangle::From(const fuchsia_math::RectU& fidl_rectangle) {
  auto _ = IsValid(fidl_rectangle, true);
  return Rectangle({.x = static_cast<int32_t>(fidl_rectangle.x()),
                    .y = static_cast<int32_t>(fidl_rectangle.y()),
                    .width = static_cast<int32_t>(fidl_rectangle.width()),
                    .height = static_cast<int32_t>(fidl_rectangle.height())});
}

// static
constexpr Rectangle Rectangle::From(const fuchsia_math::wire::RectU& fidl_rectangle) {
  auto _ = IsValid(fidl_rectangle, true);
  return Rectangle({.x = static_cast<int32_t>(fidl_rectangle.x),
                    .y = static_cast<int32_t>(fidl_rectangle.y),
                    .width = static_cast<int32_t>(fidl_rectangle.width),
                    .height = static_cast<int32_t>(fidl_rectangle.height)});
}

// static
constexpr Rectangle Rectangle::From(const Point2& origin, const Extent2& extent) {
  return Rectangle(
      {.x = origin.x(), .y = origin.y(), .width = extent.width(), .height = extent.height()});
}

constexpr bool operator==(const Rectangle& lhs, const Rectangle& rhs) {
  return lhs.origin_ == rhs.origin_ && lhs.extent_ == rhs.extent_;
}

constexpr bool operator!=(const Rectangle& lhs, const Rectangle& rhs) { return !(lhs == rhs); }

inline fuchsia_math::RectU Rectangle::ToFidlRectU() const {
  FX_CHECK(origin_.x() >= 0) << origin_.x();
  FX_CHECK(origin_.y() >= 0) << origin_.y();
  FX_CHECK(extent_.width() >= 0) << extent_.width();
  FX_CHECK(extent_.height() >= 0) << extent_.height();
  return fuchsia_math::RectU(static_cast<uint32_t>(origin_.x()), static_cast<uint32_t>(origin_.y()),
                             static_cast<uint32_t>(extent_.width()),
                             static_cast<uint32_t>(extent_.height()));
}

constexpr fuchsia_math::wire::RectU Rectangle::ToWireRectU() const {
  FX_CHECK(origin_.x() >= 0) << origin_.x();
  FX_CHECK(origin_.y() >= 0) << origin_.y();
  FX_CHECK(extent_.width() >= 0) << extent_.width();
  FX_CHECK(extent_.height() >= 0) << extent_.height();
  return fuchsia_math::wire::RectU{
      .x = static_cast<uint32_t>(origin_.x()),
      .y = static_cast<uint32_t>(origin_.y()),
      .width = static_cast<uint32_t>(extent_.width()),
      .height = static_cast<uint32_t>(extent_.height()),
  };
}

inline fuchsia_math::RectF Rectangle::ToFidlRectF() const {
  return fuchsia_math::RectF(static_cast<float>(origin_.x()), static_cast<float>(origin_.y()),
                             static_cast<float>(extent_.width()),
                             static_cast<float>(extent_.height()));
}

constexpr bool Rectangle::Contains(const Point2& point) const {
  return point.x() >= x() && point.x() < x() + width() && point.y() >= y() &&
         point.y() < y() + height();
}

inline std::ostream& operator<<(std::ostream& str, const Rectangle& r) {
  str << "{x=" << r.x() << ", y=" << r.y() << ", width=" << r.width() << ", height=" << r.height()
      << "}";
  return str;
}

}  //  namespace types

namespace std {

template <>
struct hash<types::Rectangle> {
  std::size_t operator()(const types::Rectangle& rectangle) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0xad8e0502e32834fe;
    types::hash_combine(seed, rectangle.x());
    types::hash_combine(seed, rectangle.y());
    types::hash_combine(seed, rectangle.width());
    types::hash_combine(seed, rectangle.height());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_RECTANGLE_H_
