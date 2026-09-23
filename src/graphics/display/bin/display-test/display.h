// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_DISPLAY_H_
#define SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_DISPLAY_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>

#include <cmath>

#include <fbl/string.h>
#include <fbl/vector.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"

namespace display_test {

struct ColorCorrectionArgs {
  ::fidl::Array<float, 3> preoffsets = {nanf("pre"), 0.0, 0.0};
  ::fidl::Array<float, 3> postoffsets = {nanf("post"), 0.0, 0.0};
  ::fidl::Array<float, 9> coeff = {1, 0, 0, 0, 1, 0, 0, 0, 1};
};

class Display {
 public:
  explicit Display(const fuchsia_hardware_display::wire::Info& info);

  void Init(const fidl::WireSyncClient<fuchsia_hardware_display::Coordinator>& dc,
            ColorCorrectionArgs color_correction_args = ColorCorrectionArgs());

  fuchsia_images2::wire::PixelFormat format() const { return pixel_formats_[format_idx_]; }
  fuchsia_hardware_display_types::wire::Mode mode() const { return modes_[mode_idx_]; }
  display::DisplayId id() const { return id_; }

  bool set_format_idx(uint32_t idx) {
    format_idx_ = idx;
    return format_idx_ < pixel_formats_.size();
  }

  bool set_mode_idx(uint32_t idx) {
    mode_idx_ = idx;
    return mode_idx_ < modes_.size();
  }

  void set_grayscale(bool grayscale) { apply_color_correction_ = grayscale_ = grayscale; }
  void apply_color_correction(bool apply) { apply_color_correction_ = apply; }

  void Dump();

 private:
  uint32_t format_idx_ = 0;
  uint32_t mode_idx_ = 0;
  bool apply_color_correction_ = false;
  bool grayscale_ = false;

  display::DisplayId id_;
  std::vector<fuchsia_images2::wire::PixelFormat> pixel_formats_;
  std::vector<fuchsia_hardware_display_types::wire::Mode> modes_;

  std::string manufacturer_name_;
  std::string monitor_name_;
  std::string monitor_serial_;

  // Display physical dimension in millimeters
  uint32_t horizontal_size_mm_;
  uint32_t vertical_size_mm_;
  // flag used to indicate whether the values are actual values or fallback
  bool using_fallback_sizes_;

  uint32_t max_layer_count_;
};

}  // namespace display_test

#endif  // SRC_GRAPHICS_DISPLAY_BIN_DISPLAY_TEST_DISPLAY_H_
