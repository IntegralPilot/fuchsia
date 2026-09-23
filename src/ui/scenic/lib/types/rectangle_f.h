// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_RECTANGLE_F_H_
#define SRC_UI_SCENIC_LIB_TYPES_RECTANGLE_F_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/types/extent2_f.h"
#include "src/ui/scenic/lib/types/point2_f.h"
#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class Rectangle;

class RectangleF {
 public:
  // Enables creating instances using the designated initializer syntax.
  using ConstructorArgs = fuchsia_math::wire::RectF;

  // Returns true iff the args can be used to construct a valid RectangleF.
  // If `should_assert` is true, invalid args will trigger a FX_DCHECK.
  //
  // Validity constraints:
  // - All components MUST be finite.
  // - Extent components MUST be non-negative.
  // - The opposite corner MUST be representable without non-finite values.
  [[nodiscard]] static constexpr bool IsValid(const ConstructorArgs& args,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::RectF& rect,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const Point2F& origin, const Extent2F& extent,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const Rectangle& rectangle,
                                              bool should_assert = false);

  // Constructors.  All arguments must be valid; use `IsValid()` to validate if you're not sure.
  [[nodiscard]] static constexpr RectangleF From(const fuchsia_math::RectF& fidl_rectangle);
  [[nodiscard]] static constexpr RectangleF From(const fuchsia_math::wire::RectF& fidl_rectangle);
  [[nodiscard]] static constexpr RectangleF From(const Point2F& origin, const Extent2F& extent);
  // May lose precision.
  [[nodiscard]] static RectangleF From(const Rectangle& rectangle);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr RectangleF(const ConstructorArgs& args);

  // Empty rectangle.  Allows usage as key in std C++ containers.
  constexpr RectangleF();

  constexpr RectangleF(const RectangleF&) noexcept = default;
  constexpr RectangleF(RectangleF&&) noexcept = default;
  constexpr RectangleF& operator=(const RectangleF&) noexcept = default;
  constexpr RectangleF& operator=(RectangleF&&) noexcept = default;
  ~RectangleF() = default;

  friend constexpr bool operator==(const RectangleF& lhs, const RectangleF& rhs);
  friend constexpr bool operator!=(const RectangleF& lhs, const RectangleF& rhs);

  fuchsia_math::RectF ToFidl() const;
  constexpr fuchsia_math::wire::RectF ToWire() const;

  constexpr const Point2F& origin() const { return origin_; }
  constexpr const Extent2F& extent() const { return extent_; }
  // The corner diagonally opposite from the origin.
  constexpr Point2F opposite() const { return Point2F({.x = x() + width(), .y = y() + height()}); }

  constexpr float x() const { return origin_.x(); }
  constexpr float y() const { return origin_.y(); }
  constexpr float width() const { return extent_.width(); }
  constexpr float height() const { return extent_.height(); }

  constexpr bool Contains(const Point2F& point, float epsilon = 1e-3f) const;
  constexpr bool Contains(const Point2F::ConstructorArgs& point, float epsilon = 1e-3f) const;

  constexpr RectangleF ScaledBy(float scale) const;

 private:
  Point2F origin_;
  Extent2F extent_;
};

// static
constexpr bool RectangleF::IsValid(const ConstructorArgs& args, bool should_assert) {
  if (!Point2F::IsValid(args.x, args.y, should_assert)) {
    return false;
  }
  if (!Extent2F::IsValid(args.width, args.height, should_assert)) {
    return false;
  }

  const auto opposite_x = args.x + args.width;
  const auto opposite_y = args.y + args.height;

  return Point2F::IsValid(opposite_x, opposite_y, should_assert);
}

// static
constexpr bool RectangleF::IsValid(const fuchsia_math::RectF& rect, bool should_assert) {
  const ConstructorArgs args{
      .x = rect.x(), .y = rect.y(), .width = rect.width(), .height = rect.height()};
  return IsValid(args, should_assert);
}

// static
constexpr bool RectangleF::IsValid(const Point2F& origin, const Extent2F& extent,
                                   bool should_assert) {
  const ConstructorArgs args{
      .x = origin.x(), .y = origin.y(), .width = extent.width(), .height = extent.height()};
  return IsValid(args, should_assert);
}

constexpr RectangleF::RectangleF(const ConstructorArgs& args)
    : origin_({.x = args.x, .y = args.y}), extent_({.width = args.width, .height = args.height}) {
  auto _ = IsValid(args, true);
}

constexpr RectangleF::RectangleF()
    : RectangleF({.x = 0.f, .y = 0.f, .width = 0.f, .height = 0.f}) {}

// static
constexpr RectangleF RectangleF::From(const fuchsia_math::RectF& fidl_rectangle) {
  return RectangleF({.x = fidl_rectangle.x(),
                     .y = fidl_rectangle.y(),
                     .width = fidl_rectangle.width(),
                     .height = fidl_rectangle.height()});
}

// static
constexpr RectangleF RectangleF::From(const fuchsia_math::wire::RectF& fidl_rectangle) {
  return RectangleF({.x = fidl_rectangle.x,
                     .y = fidl_rectangle.y,
                     .width = fidl_rectangle.width,
                     .height = fidl_rectangle.height});
}

// static
constexpr RectangleF RectangleF::From(const Point2F& origin, const Extent2F& extent) {
  return RectangleF(
      {.x = origin.x(), .y = origin.y(), .width = extent.width(), .height = extent.height()});
}

constexpr bool operator==(const RectangleF& lhs, const RectangleF& rhs) {
  return lhs.origin_ == rhs.origin_ && lhs.extent_ == rhs.extent_;
}

constexpr bool operator!=(const RectangleF& lhs, const RectangleF& rhs) { return !(lhs == rhs); }

inline fuchsia_math::RectF RectangleF::ToFidl() const {
  return fuchsia_math::RectF(x(), y(), width(), height());
}

constexpr fuchsia_math::wire::RectF RectangleF::ToWire() const {
  return {.x = x(), .y = y(), .width = width(), .height = height()};
}

constexpr bool RectangleF::Contains(const Point2F& point, float epsilon) const {
  return point.x() >= x() - epsilon && point.x() <= x() + width() + epsilon &&
         point.y() >= y() - epsilon && point.y() <= y() + height() + epsilon;
}

constexpr bool RectangleF::Contains(const Point2F::ConstructorArgs& point, float epsilon) const {
  return Contains(Point2F(point), epsilon);
}

constexpr RectangleF RectangleF::ScaledBy(float scale) const {
  return RectangleF(
      {.x = x() * scale, .y = y() * scale, .width = width() * scale, .height = height() * scale});
}

inline std::ostream& operator<<(std::ostream& str, const RectangleF& r) {
  str << "{x=" << r.x() << ", y=" << r.y() << ", width=" << r.width() << ", height=" << r.height()
      << "}";
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::RectangleF> {
  std::size_t operator()(const types::RectangleF& rectangle) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0xd7990113c7e1c2ca;
    types::hash_combine(seed, rectangle.x());
    types::hash_combine(seed, rectangle.y());
    types::hash_combine(seed, rectangle.width());
    types::hash_combine(seed, rectangle.height());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_RECTANGLE_F_H_
