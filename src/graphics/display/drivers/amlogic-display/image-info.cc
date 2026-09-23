// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/image-info.h"

#include <fidl/fuchsia.hardware.amlogiccanvas/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>

namespace amlogic_display {

ImageInfo::~ImageInfo() {
  fdf::info("Destroying image on canvas {}", canvas_idx);
  if (canvas.has_value()) {
    fidl::WireResult result = fidl::WireCall(canvas.value())->Free(canvas_idx);
    if (!result.ok()) {
      fdf::warn("Failed to call Canvas Free: {}", result.error().FormatDescription());
    } else if (result->is_error()) {
      fdf::warn("Canvas Free failed: {}", zx::make_result(result->error_value()));
    }
  }
  if (pmt) {
    pmt.unpin();
  }
}

}  // namespace amlogic_display
