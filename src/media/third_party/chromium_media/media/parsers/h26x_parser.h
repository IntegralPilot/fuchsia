// Copyright 2024 The Fuchsia Authors / Chromium Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_PARSERS_H26X_PARSER_H_
#define SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_PARSERS_H26X_PARSER_H_

#include <array>
#include <cstdint>
// Fuchsia change: Remove libraries in favor of "chromium_utils.h"
#include "chromium_utils.h"
#include "geometry.h"

namespace media {

struct MEDIA_EXPORT H26xSEIMasteringDisplayInfo {
  enum {
    kNumDisplayPrimaries = 3,
    kDisplayPrimaryComponents = 2,
  };

  std::array<std::array<uint16_t, kDisplayPrimaryComponents>,
             kNumDisplayPrimaries>
      display_primaries = {};
  std::array<uint16_t, 2> white_points = {};
  uint32_t max_luminance = 0;
  uint32_t min_luminance = 0;

  gfx::HdrMetadataSmpteSt2086 ToGfx() const {
    return gfx::HdrMetadataSmpteSt2086{.display_primaries = display_primaries,
                                       .white_point = white_points,
                                       .max_luminance = max_luminance,
                                       .min_luminance = min_luminance};
  }
};

struct MEDIA_EXPORT H26xSEIContentLightLevelInfo {
  uint16_t max_content_light_level = 0;
  uint16_t max_picture_average_light_level = 0;

  gfx::HdrMetadataCta861_3 ToGfx() const {
    return gfx::HdrMetadataCta861_3{
        .max_content_light_level = max_content_light_level,
        .max_frame_average_light_level = max_picture_average_light_level};
  }
};

struct MEDIA_EXPORT H26xSEIUserDataRegisteredT35 {
  H26xSEIUserDataRegisteredT35() = default;
  ~H26xSEIUserDataRegisteredT35() = default;
  H26xSEIUserDataRegisteredT35(H26xSEIUserDataRegisteredT35&&) = default;
  H26xSEIUserDataRegisteredT35& operator=(H26xSEIUserDataRegisteredT35&&) =
      default;

  uint8_t country_code = 0;
  uint8_t country_code_extension_byte = 0;
  base::HeapArray<uint8_t> payload;
};

}  // namespace media

#endif  // SRC_MEDIA_THIRD_PARTY_CHROMIUM_MEDIA_MEDIA_PARSERS_H26X_PARSER_H_
