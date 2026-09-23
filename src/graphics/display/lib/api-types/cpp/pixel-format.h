// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_PIXEL_FORMAT_H_
#define SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_PIXEL_FORMAT_H_

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstdint>
#include <string_view>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

// Equivalent to the FIDL type [`fuchsia.images2/PixelFormat`].
//
// See `::fuchsia_images2::wire::PixelFormat` for references.
//
// Instances are guaranteed to represent values supported by the display stack.
//
// This is a value type. Instances can be stored in containers. Copying, moving
// and destruction are trivial.
class PixelFormat {
 public:
  // True iff `fidl_pixel_format` is convertible to a valid PixelFormat.
  [[nodiscard]] static constexpr bool IsSupported(
      fuchsia_images2::wire::PixelFormat fidl_pixel_format);

  /// `fidl_pixel_format` must be a format supported by the display stack.
  explicit constexpr PixelFormat(fuchsia_images2::wire::PixelFormat fidl_pixel_format);

  constexpr PixelFormat(const PixelFormat&) noexcept = default;
  constexpr PixelFormat(PixelFormat&&) noexcept = default;
  constexpr PixelFormat& operator=(const PixelFormat&) noexcept = default;
  constexpr PixelFormat& operator=(PixelFormat&&) noexcept = default;
  ~PixelFormat() = default;

  constexpr fuchsia_images2::wire::PixelFormat ToFidl() const;

  // Raw numerical value of the equivalent FIDL value.
  //
  // This is intended to be used for developer-facing output, such as logging
  // and Inspect. The values have the same stability guarantees as the
  // equivalent FIDL type.
  constexpr uint32_t ValueForLogging() const;

  // Returns a developer-facing string representation.
  std::string_view ToString() const;

  // The number of planes used by this format.
  constexpr int PlaneCount() const;

  // The number of bytes it takes to encode one pixel using this format.
  //
  // At the moment, the return value is not defined for pixel formats that have
  // multiple planes.
  constexpr int EncodingSize() const;

  static const PixelFormat kR8G8B8A8;
  static const PixelFormat kB8G8R8A8;
  static const PixelFormat kI420;
  static const PixelFormat kNv12;
  static const PixelFormat kYuy2;
  static const PixelFormat kYv12;
  static const PixelFormat kB8G8R8;
  static const PixelFormat kR5G6B5;
  static const PixelFormat kR3G3B2;
  static const PixelFormat kL8;
  static const PixelFormat kR8;
  static const PixelFormat kA2R10G10B10;
  static const PixelFormat kA2B10G10R10;
  static const PixelFormat kP010;
  static const PixelFormat kR8G8B8;

 private:
  friend constexpr bool operator==(const PixelFormat& lhs, const PixelFormat& rhs);
  friend constexpr bool operator!=(const PixelFormat& lhs, const PixelFormat& rhs);

  fuchsia_images2::wire::PixelFormat format_;
};

// static
constexpr bool PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat fidl_pixel_format) {
  switch (fidl_pixel_format) {
    case fuchsia_images2::wire::PixelFormat::kR8G8B8A8:
    case fuchsia_images2::wire::PixelFormat::kB8G8R8A8:
    case fuchsia_images2::wire::PixelFormat::kI420:
    case fuchsia_images2::wire::PixelFormat::kNv12:
    case fuchsia_images2::wire::PixelFormat::kYuy2:
    case fuchsia_images2::wire::PixelFormat::kYv12:
    case fuchsia_images2::wire::PixelFormat::kB8G8R8:
    case fuchsia_images2::wire::PixelFormat::kR5G6B5:
    case fuchsia_images2::wire::PixelFormat::kR3G3B2:
    case fuchsia_images2::wire::PixelFormat::kL8:
    case fuchsia_images2::wire::PixelFormat::kR8:
    case fuchsia_images2::wire::PixelFormat::kA2R10G10B10:
    case fuchsia_images2::wire::PixelFormat::kA2B10G10R10:
    case fuchsia_images2::wire::PixelFormat::kP010:
    case fuchsia_images2::wire::PixelFormat::kR8G8B8:
      return true;

    default:
      return false;
  }
  return false;
}

constexpr PixelFormat::PixelFormat(fuchsia_images2::wire::PixelFormat fidl_pixel_format)
    : format_(fidl_pixel_format) {
  ZX_DEBUG_ASSERT(IsSupported(fidl_pixel_format));
}

constexpr bool operator==(const PixelFormat& lhs, const PixelFormat& rhs) {
  return lhs.format_ == rhs.format_;
}

constexpr bool operator!=(const PixelFormat& lhs, const PixelFormat& rhs) { return !(lhs == rhs); }

constexpr fuchsia_images2::wire::PixelFormat PixelFormat::ToFidl() const { return format_; }

constexpr uint32_t PixelFormat::ValueForLogging() const { return static_cast<uint32_t>(format_); }

constexpr int PixelFormat::PlaneCount() const {
  switch (format_) {
    case fuchsia_images2::wire::PixelFormat::kR8G8B8A8:
    case fuchsia_images2::wire::PixelFormat::kB8G8R8A8:
      return 1;
    case fuchsia_images2::wire::PixelFormat::kI420:
      return 3;
    case fuchsia_images2::wire::PixelFormat::kNv12:
    case fuchsia_images2::wire::PixelFormat::kYuy2:
      return 2;
    case fuchsia_images2::wire::PixelFormat::kYv12:
      return 3;
    case fuchsia_images2::wire::PixelFormat::kB8G8R8:
    case fuchsia_images2::wire::PixelFormat::kR5G6B5:
    case fuchsia_images2::wire::PixelFormat::kR3G3B2:
    case fuchsia_images2::wire::PixelFormat::kL8:
    case fuchsia_images2::wire::PixelFormat::kR8:
    case fuchsia_images2::wire::PixelFormat::kA2R10G10B10:
    case fuchsia_images2::wire::PixelFormat::kA2B10G10R10:
      return 1;
    case fuchsia_images2::wire::PixelFormat::kP010:
      return 2;
    case fuchsia_images2::wire::PixelFormat::kR8G8B8:
      return 1;

    default:
      ZX_DEBUG_ASSERT_MSG(false, "Unsupported pixel format: %" PRIu32, ValueForLogging());
      return 0;
  }
}

constexpr int PixelFormat::EncodingSize() const {
  switch (format_) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      return 4;

    case fuchsia_images2::PixelFormat::kB8G8R8:
      return 3;

    case fuchsia_images2::PixelFormat::kR5G6B5:
      return 2;

    case fuchsia_images2::PixelFormat::kR3G3B2:
    case fuchsia_images2::PixelFormat::kL8:
    case fuchsia_images2::PixelFormat::kR8:
      return 1;

    case fuchsia_images2::PixelFormat::kA2R10G10B10:
    case fuchsia_images2::PixelFormat::kA2B10G10R10:
      return 4;

    case fuchsia_images2::PixelFormat::kR8G8B8:
      return 3;

    default:
      ZX_DEBUG_ASSERT_MSG(false, "Unsupported pixel format: %" PRIu32, ValueForLogging());
      return 0;
  }
}

inline constexpr const PixelFormat PixelFormat::kR8G8B8A8(
    fuchsia_images2::wire::PixelFormat::kR8G8B8A8);
inline constexpr const PixelFormat PixelFormat::kB8G8R8A8(
    fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
inline constexpr const PixelFormat PixelFormat::kI420(fuchsia_images2::wire::PixelFormat::kI420);
inline constexpr const PixelFormat PixelFormat::kNv12(fuchsia_images2::wire::PixelFormat::kNv12);
inline constexpr const PixelFormat PixelFormat::kYuy2(fuchsia_images2::wire::PixelFormat::kYuy2);
inline constexpr const PixelFormat PixelFormat::kYv12(fuchsia_images2::wire::PixelFormat::kYv12);
inline constexpr const PixelFormat PixelFormat::kB8G8R8(
    fuchsia_images2::wire::PixelFormat::kB8G8R8);
inline constexpr const PixelFormat PixelFormat::kR5G6B5(
    fuchsia_images2::wire::PixelFormat::kR5G6B5);
inline constexpr const PixelFormat PixelFormat::kR3G3B2(
    fuchsia_images2::wire::PixelFormat::kR3G3B2);
inline constexpr const PixelFormat PixelFormat::kL8(fuchsia_images2::wire::PixelFormat::kL8);
inline constexpr const PixelFormat PixelFormat::kR8(fuchsia_images2::wire::PixelFormat::kR8);
inline constexpr const PixelFormat PixelFormat::kA2R10G10B10(
    fuchsia_images2::wire::PixelFormat::kA2R10G10B10);
inline constexpr const PixelFormat PixelFormat::kA2B10G10R10(
    fuchsia_images2::wire::PixelFormat::kA2B10G10R10);
inline constexpr const PixelFormat PixelFormat::kP010(fuchsia_images2::wire::PixelFormat::kP010);
inline constexpr const PixelFormat PixelFormat::kR8G8B8(
    fuchsia_images2::wire::PixelFormat::kR8G8B8);

}  // namespace display

#if __cplusplus >= 202002L
template <>
struct std::formatter<display::PixelFormat> : std::formatter<std::string_view> {
  auto format(const display::PixelFormat& format, std::format_context& ctx) const {
    return std::formatter<std::string_view>::format(format.ToString(), ctx);
  }
};
#endif

#endif  // SRC_GRAPHICS_DISPLAY_LIB_API_TYPES_CPP_PIXEL_FORMAT_H_
