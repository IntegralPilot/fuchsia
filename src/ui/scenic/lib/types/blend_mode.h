// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_BLEND_MODE_H_
#define SRC_UI_SCENIC_LIB_TYPES_BLEND_MODE_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.ui.composition/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>

#include <cstdint>

#include "src/ui/scenic/lib/types/util/hash_combine.h"

namespace types {

class BlendMode {
 public:
  enum class Enum : uint8_t {
    // Equivalent to:
    // - fuchsia.ui.composition.BlendMode::SRC
    // - fuchsia.ui.composition.BlendMode2::REPLACE
    // - fuchsia.hardware.display.types.AlphaMode::DISABLE
    // - android.hardware.graphics.common.BlendMode::NONE
    //
    // Note: this is named `kReplace` not e.g. `kOpaque` because the source alpha replaces the
    // destination alpha.  Consider a fully transparent source pixel (alpha == 0); in this mode the
    // destination alpha becomes zero.
    kReplace,

    // Equivalent to:
    // - fuchsia.ui.composition.BlendMode::SRC_OVER
    // - fuchsia.ui.composition.BlendMode2::PREMULTIPLIED_ALPHA
    // - fuchsia.hardware.display.types.AlphaMode::PREMULTIPLIED
    // - android.hardware.graphics.common.BlendMode::PREMULTIPLIED
    kPremultipliedAlpha,

    // Equivalent to:
    // - NO EQUIVALENT in fuchsia.ui.composition.BlendMode
    // - fuchsia.ui.composition.BlendMode2::STRAIGHT_ALPHA
    // - fuchsia.hardware.display.types.AlphaMode::HW_MULTIPLY
    // - android.hardware.graphics.common.BlendMode::COVERAGE
    kStraightAlpha,
  };

  // `fidl_mode` must be convertible to a valid BlendMode instance.
  //
  // This is not a constructor, to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.math/SizeU` has the same field names as
  // our supported designated initializer syntax.
  [[nodiscard]] static constexpr BlendMode From(const fuchsia_ui_composition::BlendMode& fidl_mode);
  [[nodiscard]] static constexpr BlendMode From(
      const fuchsia_ui_composition::BlendMode2& fidl_mode);
  [[nodiscard]] static constexpr BlendMode From(
      const fuchsia_hardware_display_types::wire::AlphaMode& fidl_mode);

  // Static "constructors".
  [[nodiscard]] static constexpr BlendMode kReplace();
  [[nodiscard]] static constexpr BlendMode kPremultipliedAlpha();
  [[nodiscard]] static constexpr BlendMode kStraightAlpha();

  BlendMode() = delete;
  explicit constexpr BlendMode(BlendMode::Enum val);
  constexpr BlendMode(const BlendMode&) noexcept = default;
  constexpr BlendMode(BlendMode&&) noexcept = default;
  constexpr BlendMode& operator=(const BlendMode&) noexcept = default;
  constexpr BlendMode& operator=(BlendMode&&) noexcept = default;
  ~BlendMode() = default;

  friend constexpr bool operator==(const BlendMode& lhs, const BlendMode& rhs);
  friend constexpr bool operator!=(const BlendMode& lhs, const BlendMode& rhs);

  constexpr fuchsia_ui_composition::BlendMode ToFlatlandBlendMode() const;
  constexpr fuchsia_ui_composition::BlendMode2 ToFlatlandBlendMode2() const;
  constexpr fuchsia_hardware_display_types::wire::AlphaMode ToDisplayAlphaMode() const;

  // Used for hashing/printing/etc; not useful for general users.
  constexpr Enum enum_value() const { return val_; }

 private:
  Enum val_;
};

constexpr BlendMode::BlendMode(BlendMode::Enum val) : val_(val) {}

// static
constexpr BlendMode BlendMode::From(const fuchsia_ui_composition::BlendMode& fidl_mode) {
  switch (fidl_mode) {
    case fuchsia_ui_composition::BlendMode::kSrc:
      return BlendMode::kReplace();
    case fuchsia_ui_composition::BlendMode::kSrcOver:
      return BlendMode::kPremultipliedAlpha();
  }
}

// static
constexpr BlendMode BlendMode::From(const fuchsia_ui_composition::BlendMode2& fidl_mode) {
  switch (fidl_mode) {
    case fuchsia_ui_composition::BlendMode2::kReplace:
      return BlendMode::kReplace();
    case fuchsia_ui_composition::BlendMode2::kPremultipliedAlpha:
      return BlendMode::kPremultipliedAlpha();
    case fuchsia_ui_composition::BlendMode2::kStraightAlpha:
      return BlendMode::kStraightAlpha();
    default:
      FX_CHECK(false) << "Unsupported BlendMode: " << static_cast<uint32_t>(fidl_mode);
      return BlendMode::kReplace();
  }
}

// static
constexpr BlendMode BlendMode::From(
    const fuchsia_hardware_display_types::wire::AlphaMode& fidl_mode) {
  switch (fidl_mode) {
    case fuchsia_hardware_display_types::wire::AlphaMode::kDisable:
      return BlendMode::kReplace();
    case fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied:
      return BlendMode::kPremultipliedAlpha();
    case fuchsia_hardware_display_types::wire::AlphaMode::kHwMultiply:
      return BlendMode::kStraightAlpha();
  }
}

// static
constexpr BlendMode BlendMode::kReplace() { return BlendMode(Enum::kReplace); }

// static
constexpr BlendMode BlendMode::kPremultipliedAlpha() {
  return BlendMode(Enum::kPremultipliedAlpha);
}

// static
constexpr BlendMode BlendMode::kStraightAlpha() { return BlendMode(Enum::kStraightAlpha); }

constexpr bool operator==(const BlendMode& lhs, const BlendMode& rhs) {
  return lhs.val_ == rhs.val_;
}

constexpr bool operator!=(const BlendMode& lhs, const BlendMode& rhs) { return !(lhs == rhs); }

constexpr fuchsia_ui_composition::BlendMode BlendMode::ToFlatlandBlendMode() const {
  switch (val_) {
    case BlendMode::Enum::kReplace:
      return fuchsia_ui_composition::BlendMode::kSrc;
    case BlendMode::Enum::kPremultipliedAlpha:
      return fuchsia_ui_composition::BlendMode::kSrcOver;
    case BlendMode::Enum::kStraightAlpha:
      ZX_ASSERT_MSG(false,
                    "No conversion from kStraightAlpha to fuchsia_ui_composition::BlendMode");
      return fuchsia_ui_composition::BlendMode::kSrcOver;
  }
}

constexpr fuchsia_ui_composition::BlendMode2 BlendMode::ToFlatlandBlendMode2() const {
  switch (val_) {
    case BlendMode::Enum::kReplace:
      return fuchsia_ui_composition::BlendMode2::kReplace;
    case BlendMode::Enum::kPremultipliedAlpha:
      return fuchsia_ui_composition::BlendMode2::kPremultipliedAlpha;
    case BlendMode::Enum::kStraightAlpha:
      return fuchsia_ui_composition::BlendMode2::kStraightAlpha;
  }
}

constexpr fuchsia_hardware_display_types::wire::AlphaMode BlendMode::ToDisplayAlphaMode() const {
  switch (val_) {
    case BlendMode::Enum::kReplace:
      return fuchsia_hardware_display_types::wire::AlphaMode::kDisable;
    case BlendMode::Enum::kPremultipliedAlpha:
      return fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied;
    case BlendMode::Enum::kStraightAlpha:
      return fuchsia_hardware_display_types::wire::AlphaMode::kHwMultiply;
  }
}

inline std::ostream& operator<<(std::ostream& str, const BlendMode& bm) {
  switch (bm.enum_value()) {
    case BlendMode::Enum::kReplace:
      str << "OPAQUE";
      break;
    case BlendMode::Enum::kPremultipliedAlpha:
      str << "PREMULT_ALPHA";
      break;
    case BlendMode::Enum::kStraightAlpha:
      str << "STRAIGHT_ALPHA";
      break;
  }
  return str;
}

}  // namespace types

namespace std {

template <>
struct hash<types::BlendMode> {
  std::size_t operator()(const types::BlendMode& bm) {
    // Random seed (`openssl rand -hex 8`) avoids collisions with types with the same memory layout.
    std::size_t seed = 0xd19e282a837f0e07;
    types::hash_combine(seed, bm.enum_value());
    return seed;
  }
};

}  // namespace std

#endif  // SRC_UI_SCENIC_LIB_TYPES_BLEND_MODE_H_
