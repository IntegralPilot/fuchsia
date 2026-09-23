// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/pixel.h"

#include <lib/syslog/cpp/macros.h>

#include <cstdint>

namespace utils {

namespace {
// List of supported pixel formats
const std::vector<fuchsia_images2::PixelFormat> kSupportedPixelFormats = {
    fuchsia_images2::PixelFormat::kB8G8R8A8, fuchsia_images2::PixelFormat::kR8G8B8A8,
    fuchsia_images2::PixelFormat::kR5G6B5};
}  // namespace

uint8_t LinearToSrgb(const float val) {
  // Function to convert from linear RGB to sRGB.
  // (https://en.wikipedia.org/wiki/SRGB#From_CIE_XYZ_to_sRGB)
  if (0.f <= val && val <= 0.0031308f) {
    return static_cast<uint8_t>(roundf((val * 12.92f) * 255U));
  }
  return static_cast<uint8_t>(roundf(((powf(val, 1.0f / 2.4f) * 1.055f) - 0.055f) * 255U));
}

Pixel Pixel::FromUnormBgra(float blue, float green, float red, float alpha) {
  return Pixel{LinearToSrgb(blue), LinearToSrgb(green), LinearToSrgb(red),
               static_cast<uint8_t>(roundf(alpha * 255U))};
}

Pixel Pixel::FromVmo(const uint8_t* vmo_host, uint32_t stride, uint32_t x, uint32_t y,
                     fuchsia_images2::PixelFormat type) {
  if (type == fuchsia_images2::PixelFormat::kB8G8R8A8) {
    return FromVmoBgra(vmo_host, stride, x, y);
  }
  if (type == fuchsia_images2::PixelFormat::kR5G6B5) {
    return FromVmoRgb565(vmo_host, stride, x, y);
  }
  FX_DCHECK(type == fuchsia_images2::PixelFormat::kR8G8B8A8);
  return FromVmoRgba(vmo_host, stride, x, y);
}

Pixel Pixel::FromVmoRgb565(const uint8_t* vmo_host, uint32_t stride, uint32_t x, uint32_t y) {
  uint16_t pixel;
  memcpy(&pixel, vmo_host + (y * stride + x) * sizeof(uint16_t), sizeof(uint16_t));
  uint8_t r5 = pixel >> 11;
  uint8_t g6 = (pixel >> 5) & 0x3F;
  uint8_t b5 = pixel & 0x1F;
  uint8_t r8 = static_cast<uint8_t>(r5 * 255.0f / 31 + 0.5f);
  uint8_t g8 = static_cast<uint8_t>(g6 * 255.0f / 63 + 0.5f);
  uint8_t b8 = static_cast<uint8_t>(b5 * 255.0f / 31 + 0.5f);

  return utils::Pixel(b8, g8, r8, 255);
}

Pixel Pixel::FromVmoRgba(const uint8_t* vmo_host, uint32_t stride, uint32_t x, uint32_t y) {
  uint8_t r = vmo_host[y * stride * 4 + x * 4];
  uint8_t g = vmo_host[y * stride * 4 + x * 4 + 1];
  uint8_t b = vmo_host[y * stride * 4 + x * 4 + 2];
  uint8_t a = vmo_host[y * stride * 4 + x * 4 + 3];
  return utils::Pixel(b, g, r, a);
}

Pixel Pixel::FromVmoBgra(const uint8_t* vmo_host, uint32_t stride, uint32_t x, uint32_t y) {
  uint8_t b = vmo_host[y * stride * 4 + x * 4];
  uint8_t g = vmo_host[y * stride * 4 + x * 4 + 1];
  uint8_t r = vmo_host[y * stride * 4 + x * 4 + 2];
  uint8_t a = vmo_host[y * stride * 4 + x * 4 + 3];
  return utils::Pixel(b, g, r, a);
}

std::vector<uint8_t> Pixel::ToFormat(fuchsia_images2::PixelFormat type) const {
  std::vector<uint8_t> bytes;
  ToFormat(type, bytes);
  return bytes;
}

void Pixel::ToFormat(fuchsia_images2::PixelFormat type, std::vector<uint8_t>& bytes) const {
  switch (type) {
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      ToBgra(bytes);
      break;
    case fuchsia_images2::PixelFormat::kR5G6B5:
      ToRgb565(bytes);
      break;
    default:
      FX_DCHECK(type == fuchsia_images2::PixelFormat::kR8G8B8A8);
      ToRgba(bytes);
  }
}

void Pixel::ToRgb565(std::vector<uint8_t>& bytes) const {
  uint16_t color = static_cast<uint16_t>(((red >> 3) << 11) | ((green >> 2) << 5) | (blue >> 3));
  bytes.resize(sizeof(color));

  memcpy(bytes.data(), &color, sizeof(color));
}

bool Pixel::IsFormatSupported(fuchsia_images2::PixelFormat type) {
  return std::any_of(kSupportedPixelFormats.begin(), kSupportedPixelFormats.end(),
                     [type](fuchsia_images2::PixelFormat supported) { return supported == type; });
}

std::ostream& operator<<(std::ostream& stream, const Pixel& pixel) {
  return stream << "{Pixel:"
                << " r:" << static_cast<unsigned int>(pixel.red)
                << " g:" << static_cast<unsigned int>(pixel.green)
                << " b:" << static_cast<unsigned int>(pixel.blue)
                << " a:" << static_cast<unsigned int>(pixel.alpha) << "}";
}

}  // namespace utils
