// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_EXTENT2_H_
#define SRC_UI_SCENIC_LIB_TYPES_EXTENT2_H_

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class Extent2 {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // Returns true iff the args can be used to construct a valid Extent2.
  // If `should_assert` is true, invalid args will trigger a FX_DCHECK.
  //
  // Validity constraints:
  // - width and height MUST be non-negative
  [[nodiscard]] static constexpr bool IsValid(int32_t width, int32_t height,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::SizeU& fidl_size,
                                              bool should_assert = false);
  [[nodiscard]] static constexpr bool IsValid(const fuchsia_math::wire::SizeU& fidl_size,
                                              bool should_assert = false);

  // `fidl_size` must be convertible to a valid Extent2 instance.
  //
  // This is not a constructor, to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.math/SizeU` has the same field names as
  // our supported designated initializer syntax.
  [[nodiscard]] static constexpr Extent2 From(const fuchsia_math::SizeU& fidl_size);
  [[nodiscard]] static constexpr Extent2 From(const fuchsia_math::wire::SizeU& fidl_size);

  // Constructor that enables the designated initializer syntax.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Extent2(const Extent2::ConstructorArgs& args);

  constexpr Extent2(const Extent2&) noexcept = default;
  constexpr Extent2(Extent2&&) noexcept = default;
  constexpr Extent2& operator=(const Extent2&) noexcept = default;
  constexpr Extent2& operator=(Extent2&&) noexcept = default;
  ~Extent2() = default;

  friend constexpr bool operator==(const Extent2& lhs, const Extent2& rhs);
  friend constexpr bool operator!=(const Extent2& lhs, const Extent2& rhs);

  fuchsia_math::SizeU ToFidl() const;
  constexpr fuchsia_math::wire::SizeU ToWire() const;

  // Guaranteed to be non-negative.
  constexpr int32_t width() const { return width_; }

  // Guaranteed to be non-negative.
  constexpr int32_t height() const { return height_; }

  // True iff the extent represent an empty region.
  constexpr bool IsEmpty() const { return width_ == 0 || height_ == 0; }

 private:
  struct ConstructorArgs {
    int32_t width;
    int32_t height;
  };

  int32_t width_;
  int32_t height_;
};

// static
constexpr bool Extent2::IsValid(int32_t width, int32_t height, bool should_assert) {
  if (width < 0) {
    FX_DCHECK(!should_assert) << "width must be non-negative: " << width;
    return false;
  }
  if (height < 0) {
    FX_DCHECK(!should_assert) << "height must be non-negative: " << height;
    return false;
  }
  return true;
}

// static
constexpr bool Extent2::IsValid(const fuchsia_math::SizeU& fidl_size, bool should_assert) {
  if (fidl_size.width() > INT32_MAX || fidl_size.height() > INT32_MAX) {
    FX_DCHECK(!should_assert) << "Overflow when casting from unsigned to signed. width="
                              << fidl_size.width() << ", height=" << fidl_size.height();
    return false;
  }
  return IsValid(static_cast<int32_t>(fidl_size.width()), static_cast<int32_t>(fidl_size.height()),
                 should_assert);
}

// static
constexpr bool Extent2::IsValid(const fuchsia_math::wire::SizeU& fidl_size, bool should_assert) {
  if (fidl_size.width > INT32_MAX || fidl_size.height > INT32_MAX) {
    FX_DCHECK(!should_assert) << "Overflow when casting from unsigned to signed. width="
                              << fidl_size.width << ", height=" << fidl_size.height;
    return false;
  }
  return IsValid(static_cast<int32_t>(fidl_size.width), static_cast<int32_t>(fidl_size.height),
                 should_assert);
}

constexpr Extent2::Extent2(const ConstructorArgs& args) : width_(args.width), height_(args.height) {
  auto _ = IsValid(width_, height_, true);
}

// static
constexpr Extent2 Extent2::From(const fuchsia_math::SizeU& fidl_size) {
  auto _ = IsValid(fidl_size, true);
  return Extent2({
      .width = static_cast<int32_t>(fidl_size.width()),
      .height = static_cast<int32_t>(fidl_size.height()),
  });
}

// static
constexpr Extent2 Extent2::From(const fuchsia_math::wire::SizeU& fidl_size) {
  auto _ = IsValid(fidl_size, true);
  return Extent2({
      .width = static_cast<int32_t>(fidl_size.width),
      .height = static_cast<int32_t>(fidl_size.height),
  });
}

constexpr bool operator==(const Extent2& lhs, const Extent2& rhs) {
  return lhs.width_ == rhs.width_ && lhs.height_ == rhs.height_;
}

constexpr bool operator!=(const Extent2& lhs, const Extent2& rhs) { return !(lhs == rhs); }

inline fuchsia_math::SizeU Extent2::ToFidl() const {
  // The casts are guaranteed not to overflow (causing UB) because of the
  // allowed ranges on image widths and heights.
  return fuchsia_math::SizeU(static_cast<uint32_t>(width_), static_cast<uint32_t>(height_));
}

constexpr fuchsia_math::wire::SizeU Extent2::ToWire() const {
  return fuchsia_math::wire::SizeU{
      // The casts are guaranteed not to overflow (causing UB) because of the
      // allowed ranges on image widths and heights.
      .width = static_cast<uint32_t>(width_),
      .height = static_cast<uint32_t>(height_),
  };
}

inline std::ostream& operator<<(std::ostream& str, const Extent2& e) {
  str << "(w:" << e.width() << ", h:" << e.height() << ")";
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::Extent2> {
  std::size_t operator()(const types::Extent2& extent) const {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0x3c1c997b4fd67cd2;
    types::hash_combine(seed, extent.width());
    types::hash_combine(seed, extent.height());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_EXTENT2_H_
