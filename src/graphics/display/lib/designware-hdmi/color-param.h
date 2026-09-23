// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_COLOR_PARAM_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_COLOR_PARAM_H_

#include <cinttypes>

namespace designware_hdmi {

// TODO(https://fxbug.dev/42085777): Revise the naming of the enum type and member names.
// Add proper documentatation.
enum class ColorFormat : uint8_t {
  kCfRgb = 0,
  kCf444 = 1,
};

// TODO(https://fxbug.dev/42085777): Revise the naming of the enum type and member names.
// Add proper documentatation.
enum class ColorDepth : uint8_t {
  kCd24B = 4,
  kCd30B = 5,
  kCd36B = 6,
  kCd48B = 7,
};

// Color configuration of the HDMI transmitter.
//
// TODO(https://fxbug.dev/42085777): Revise the naming of the enum type and member names.
// Add proper documentatation.
struct ColorParam {
  ColorFormat input_color_format;
  ColorFormat output_color_format;
  ColorDepth color_depth;
};

}  // namespace designware_hdmi

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DESIGNWARE_HDMI_COLOR_PARAM_H_
