// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_EXTENT2_F_H_
#define SRC_UI_SCENIC_LIB_TYPES_EXTENT2_F_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

#include <cmath>

#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class Extent2F {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Returns true iff the args can be used to construct a valid Extent2F.
  // If `should_assert` is true, invalid args will trigger a FX_DCHECK.
  //
  // Validity constraints:
  // - width and height MUST be finite and non-negative.
  [[nodiscard]] static constexpr bool IsValid(float width, float height,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::SizeF& fidl_size,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::SizeF& fidl_size,
                                              bool should_assert = false);

  // `fidl_size` must be convertible to a valid Extent2F instance.
  [[nodiscard]] static constexpr Extent2F From(const fuchsia_math::SizeF& fidl_size);
  [[nodiscard]] static constexpr Extent2F From(const fuchsia_math::wire::SizeF& fidl_size);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Extent2F(const ConstructorArgs& args);

  constexpr Extent2F(const Extent2F&) noexcept = default;
  constexpr Extent2F(Extent2F&&) noexcept = default;
  constexpr Extent2F& operator=(const Extent2F&) noexcept = default;
  constexpr Extent2F& operator=(Extent2F&&) noexcept = default;
  ~Extent2F() = default;

  friend constexpr bool operator==(const Extent2F& lhs, const Extent2F& rhs);
  friend constexpr bool operator!=(const Extent2F& lhs, const Extent2F& rhs);

  fuchsia_math::SizeF ToFidl() const;
  constexpr fuchsia_math::wire::SizeF ToWire() const;

  // Guaranteed to be finite and non-negative.
  constexpr float width() const { return width_; }

  // Guaranteed to be finite and non-negative.
  constexpr float height() const { return height_; }

  // True iff the extent represent an empty region.
  constexpr bool IsEmpty() const { return width_ == 0.f || height_ == 0.f; }

 private:
  struct ConstructorArgs {
    float width;
    float height;
  };

  float width_;
  float height_;
};

// static
constexpr bool Extent2F::IsValid(float width, float height, bool should_assert) {
  if (!std::isfinite(width) || width < 0.f) {
    FX_DCHECK(!should_assert) << "width must be finite and non-negative, not: " << width;
    return false;
  }
  if (!std::isfinite(height) || height < 0.f) {
    FX_DCHECK(!should_assert) << "height must be finite and non-negative, not: " << height;
    return false;
  }
  return true;
}

// static
constexpr bool Extent2F::IsValid(const fuchsia_math::SizeF& fidl_size, bool should_assert) {
  return IsValid(fidl_size.width(), fidl_size.height(), should_assert);
}

// static
constexpr bool Extent2F::IsValid(const fuchsia_math::wire::SizeF& fidl_size, bool should_assert) {
  return IsValid(fidl_size.width, fidl_size.height, should_assert);
}

constexpr Extent2F::Extent2F(const ConstructorArgs& args)
    : width_(args.width), height_(args.height) {
  auto _ = IsValid(width_, height_, true);
}

// static
constexpr Extent2F Extent2F::From(const fuchsia_math::SizeF& fidl_size) {
  auto _ = IsValid(fidl_size, true);
  return Extent2F({.width = fidl_size.width(), .height = fidl_size.height()});
}

// static
constexpr Extent2F Extent2F::From(const fuchsia_math::wire::SizeF& fidl_size) {
  auto _ = IsValid(fidl_size, true);
  return Extent2F({.width = fidl_size.width, .height = fidl_size.height});
}

constexpr bool operator==(const Extent2F& lhs, const Extent2F& rhs) {
  return lhs.width_ == rhs.width_ && lhs.height_ == rhs.height_;
}

constexpr bool operator!=(const Extent2F& lhs, const Extent2F& rhs) { return !(lhs == rhs); }

inline fuchsia_math::SizeF Extent2F::ToFidl() const { return fuchsia_math::SizeF(width_, height_); }

constexpr fuchsia_math::wire::SizeF Extent2F::ToWire() const {
  return fuchsia_math::wire::SizeF{.width = width_, .height = height_};
}

inline std::ostream& operator<<(std::ostream& str, const Extent2F& e) {
  str << "(w:" << e.width() << ", h:" << e.height() << ")";
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::Extent2F> {
  std::size_t operator()(const types::Extent2F& extent) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x31ac2ab07f39b07b;
    types::hash_combine(seed, extent.width());
    types::hash_combine(seed, extent.height());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_EXTENT2_F_H_
