// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COLOR_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COLOR_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <zircon/assert.h>

#include <algorithm>
#include <array>
#include <cinttypes>
#include <cstdint>
#include <span>

#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display {

// Equivalent to the FIDL type [`fuchsia.hardware.display.types/Color`].
//
// Instances are guaranteed to represent color constants whose pixel formats are
// supported by the display stack.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class Color {
 private:
  // Enables creating instances using the designated initializer syntax.
  struct ConstructorArgs;

 public:
  // True iff `fidl_color` is convertible to a valid Color.
  [[nodiscard]] static constexpr bool IsValid(
      const fuchsia_hardware_display_types::wire::Color& fidl_color);

  // `fidl_color` must be convertible to a valid Color.
  //
  // This is not a constructor to allow designated initializer syntax. Making
  // this a constructor would introduce ambiguity when designated initializer
  // syntax is used, because `fuchsia.hardware.display.types/Color` has the same
  // field names as our supported designated initializer syntax.
  [[nodiscard]] static constexpr Color From(
      const fuchsia_hardware_display_types::wire::Color& fidl_color);

  // Constructor that enables the designated initializer syntax with containers.
  //
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr Color(const Color::ConstructorArgs& args);

  constexpr Color(const Color&) noexcept = default;
  constexpr Color(Color&&) noexcept = default;
  constexpr Color& operator=(const Color&) noexcept = default;
  constexpr Color& operator=(Color&&) noexcept = default;
  ~Color() = default;

  constexpr bool operator==(const Color& rhs) const = default;

  constexpr fuchsia_hardware_display_types::wire::Color ToFidl() const;

  // Guaranteed to meet the requirements in the FIDL documentation.
  constexpr PixelFormat format() const { return format_; }

  // Guaranteed to meet the requirements in the FIDL documentation.
  constexpr std::span<const uint8_t> bytes() const { return bytes_; }

  // True iff `format` meets the requirements in the FIDL documentation.
  static constexpr bool SupportsFormat(PixelFormat format);

 private:
  struct ConstructorArgs {
    PixelFormat format;
    std::span<const uint8_t> bytes;
  };

  // In debug mode, asserts that IsValid() would return true.
  //
  // IsValid() variant with developer-friendly debug assertions.
  static constexpr void DebugAssertIsValid(const Color::ConstructorArgs& args);
  static constexpr void DebugAssertIsValid(
      const fuchsia_hardware_display_types::wire::Color& fidl_color);

  // Container for static_asserts on private data members.
  static void StaticAsserts();

  PixelFormat format_;
  std::array<uint8_t, 8> bytes_;

  static constexpr int kBytesElements =
      static_cast<int>(std::tuple_size_v<decltype(Color::bytes_)>);
};

// static
constexpr bool Color::SupportsFormat(PixelFormat format) {
  return format.PlaneCount() == 1 && format.EncodingSize() <= kBytesElements;
}

// static
constexpr bool Color::IsValid(const fuchsia_hardware_display_types::wire::Color& fidl_color) {
  if (!PixelFormat::IsSupported(fidl_color.format)) {
    return false;
  }
  const PixelFormat pixel_format(fidl_color.format);

  if (!Color::SupportsFormat(pixel_format)) {
    return false;
  }

  for (int i = pixel_format.EncodingSize(); i < kBytesElements; ++i) {
    if (fidl_color.bytes[i] != 0) {
      return false;
    }
  }
  return true;
}

constexpr Color::Color(const Color::ConstructorArgs& args) : format_(args.format), bytes_({}) {
  DebugAssertIsValid(args);
  std::ranges::copy(args.bytes, bytes_.begin());
}

// static
constexpr Color Color::From(const fuchsia_hardware_display_types::wire::Color& fidl_color) {
  DebugAssertIsValid(fidl_color);
  return Color({
      .format = PixelFormat(fidl_color.format),
      .bytes = fidl_color.bytes,
  });
}

constexpr fuchsia_hardware_display_types::wire::Color Color::ToFidl() const {
  fuchsia_hardware_display_types::wire::Color fidl_color{.format = format_.ToFidl()};
  std::ranges::copy(bytes_, fidl_color.bytes.begin());
  return fidl_color;
}

// static
constexpr void Color::DebugAssertIsValid(const Color::ConstructorArgs& args) {
  ZX_DEBUG_ASSERT_MSG(Color::SupportsFormat(args.format), "Unsupported color format %u" PRIu32,
                      args.format.ValueForLogging());

  for (int i = args.format.EncodingSize(); i < kBytesElements; ++i) {
    ZX_DEBUG_ASSERT_MSG(args.bytes[i] == 0, "Padding byte %d set to %d", i, int{args.bytes[i]});
  }
}

// static
constexpr void Color::DebugAssertIsValid(
    const fuchsia_hardware_display_types::wire::Color& fidl_color) {
  ZX_DEBUG_ASSERT(PixelFormat::IsSupported(fidl_color.format));
  const PixelFormat pixel_format(fidl_color.format);

  ZX_DEBUG_ASSERT_MSG(Color::SupportsFormat(pixel_format), "Unsupported color format %" PRIu32,
                      pixel_format.ValueForLogging());

  for (int i = pixel_format.EncodingSize(); i < kBytesElements; ++i) {
    ZX_DEBUG_ASSERT_MSG(fidl_color.bytes[i] == 0, "Padding byte %d set to %d", i,
                        int{fidl_color.bytes[i]});
  }
}

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_COLOR_H_
