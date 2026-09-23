// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_POINT2_F_H_
#define SRC_UI_SCENIC_LIB_TYPES_POINT2_F_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

#include <cmath>

#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class Point2F {
 public:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs {
    float x;
    float y;
  };

  // Returns true iff the args can be used to construct a valid Point2F.
  // If `should_assert` is true, invalid args will trigger a FX_DCHECK.
  //
  // Validity constraints:
  // - x and y MUST be finite.
  [[nodiscard]] static constexpr bool IsValid(float x, float y, bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::VecF& fidl_vec,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::VecF& fidl_vec,
                                              bool should_assert = false);

  // Constructors.  All arguments must be valid; use `IsValid()` to validate if you're not sure.
  [[nodiscard]] static constexpr Point2F From(const fuchsia_math::VecF& fidl_vec);
  [[nodiscard]] static constexpr Point2F From(const fuchsia_math::wire::VecF& fidl_vec);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Point2F(const ConstructorArgs& args);

  constexpr Point2F(const Point2F&) noexcept = default;
  constexpr Point2F(Point2F&&) noexcept = default;
  constexpr Point2F& operator=(const Point2F&) noexcept = default;
  constexpr Point2F& operator=(Point2F&&) noexcept = default;
  ~Point2F() = default;

  friend constexpr bool operator==(const Point2F& lhs, const Point2F& rhs);
  friend constexpr bool operator!=(const Point2F& lhs, const Point2F& rhs);

  fuchsia_math::VecF ToFidl() const;
  constexpr fuchsia_math::wire::VecF ToWire() const;

  constexpr float x() const { return x_; }
  constexpr float y() const { return y_; }
  constexpr bool IsOrigin() const { return x_ == 0.f && y_ == 0.f; }

 private:
  float x_;
  float y_;
};

// static
constexpr bool Point2F::IsValid(float x, float y, bool should_assert) {
  if (!std::isfinite(x)) {
    FX_DCHECK(!should_assert) << "x must be finite: " << x;
    return false;
  }
  if (!std::isfinite(y)) {
    FX_DCHECK(!should_assert) << "y must be finite: " << y;
    return false;
  }
  return true;
}

// static
constexpr bool Point2F::IsValid(const fuchsia_math::VecF& fidl_vec, bool should_assert) {
  return IsValid(fidl_vec.x(), fidl_vec.y(), should_assert);
}

// static
constexpr bool Point2F::IsValid(const fuchsia_math::wire::VecF& fidl_vec, bool should_assert) {
  return IsValid(fidl_vec.x, fidl_vec.y, should_assert);
}

constexpr Point2F::Point2F(const ConstructorArgs& args) : x_(args.x), y_(args.y) {
  auto _ = IsValid(x_, y_, true);
}

// static
constexpr Point2F Point2F::From(const fuchsia_math::VecF& fidl_vec) {
  auto _ = IsValid(fidl_vec, true);
  return Point2F({.x = fidl_vec.x(), .y = fidl_vec.y()});
}

// static
constexpr Point2F Point2F::From(const fuchsia_math::wire::VecF& fidl_vec) {
  auto _ = IsValid(fidl_vec, true);
  return Point2F({.x = fidl_vec.x, .y = fidl_vec.y});
}

constexpr bool operator==(const Point2F& lhs, const Point2F& rhs) {
  return lhs.x_ == rhs.x_ && lhs.y_ == rhs.y_;
}

constexpr bool operator!=(const Point2F& lhs, const Point2F& rhs) { return !(lhs == rhs); }

inline fuchsia_math::VecF Point2F::ToFidl() const { return fuchsia_math::VecF(x_, y_); }

constexpr fuchsia_math::wire::VecF Point2F::ToWire() const {
  return fuchsia_math::wire::VecF{.x = x_, .y = y_};
}

inline std::ostream& operator<<(std::ostream& str, const Point2F& p) {
  str << "(x:" << p.x() << ", y:" << p.y() << ")";
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::Point2F> {
  std::size_t operator()(const types::Point2F& point) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x99840f87d7192cbc;
    types::hash_combine(seed, point.x());
    types::hash_combine(seed, point.y());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_POINT2_F_H_
