// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_POINT2_H_
#define SRC_UI_SCENIC_LIB_TYPES_POINT2_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/wire.h>

#include <ostream>

#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class Point2 {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Constructors.
  [[nodiscard]] static constexpr Point2 From(const fuchsia_math::Vec& fidl_vec);
  [[nodiscard]] static constexpr Point2 From(const fuchsia_math::wire::Vec& fidl_vec);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Point2(const Point2::ConstructorArgs& args);

  constexpr Point2(const Point2&) noexcept = default;
  constexpr Point2(Point2&&) noexcept = default;
  constexpr Point2& operator=(const Point2&) noexcept = default;
  constexpr Point2& operator=(Point2&&) noexcept = default;
  ~Point2() = default;

  friend constexpr bool operator==(const Point2& lhs, const Point2& rhs);
  friend constexpr bool operator!=(const Point2& lhs, const Point2& rhs);

  fuchsia_math::Vec ToFidl() const;
  constexpr fuchsia_math::wire::Vec ToWire() const;

  constexpr int32_t x() const { return x_; }
  constexpr int32_t y() const { return y_; }
  constexpr bool IsOrigin() const { return x_ == 0 && y_ == 0; }

 private:
  struct ConstructorArgs {
    int32_t x;
    int32_t y;
  };

  int32_t x_;
  int32_t y_;
};

constexpr Point2::Point2(const ConstructorArgs& args) : x_(args.x), y_(args.y) {}

// static
constexpr Point2 Point2::From(const fuchsia_math::Vec& fidl_vec) {
  return Point2({.x = fidl_vec.x(), .y = fidl_vec.y()});
}

// static
constexpr Point2 Point2::From(const fuchsia_math::wire::Vec& fidl_vec) {
  return Point2({.x = fidl_vec.x, .y = fidl_vec.y});
}

constexpr bool operator==(const Point2& lhs, const Point2& rhs) {
  return lhs.x_ == rhs.x_ && lhs.y_ == rhs.y_;
}

constexpr bool operator!=(const Point2& lhs, const Point2& rhs) { return !(lhs == rhs); }

inline fuchsia_math::Vec Point2::ToFidl() const { return fuchsia_math::Vec(x_, y_); }

constexpr fuchsia_math::wire::Vec Point2::ToWire() const {
  return fuchsia_math::wire::Vec{
      .x = x_,
      .y = y_,
  };
}

inline std::ostream& operator<<(std::ostream& str, const Point2& p) {
  str << "(x:" << p.x() << ", y:" << p.y() << ")";
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::Point2> {
  std::size_t operator()(const types::Point2& point) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x0a47627f3f222d81;
    types::hash_combine(seed, point.x());
    types::hash_combine(seed, point.y());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_POINT2_H_
