// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ADDED_DISPLAY_INFO_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ADDED_DISPLAY_INFO_H_

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/zx/result.h>

#include <memory>

#include <fbl/vector.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display_coordinator {

// Bundles engine driver information about a newly added display.
//
// This structure was designed to shuttle display information between
// dispatchers. It is not thread-safe, and it is not suitable for more
// complex processing.
struct AddedDisplayInfo {
  // Returns a valid instance.
  //
  // Fails with ZX_ERR_INVALID_ARGS if `fidl_display_info` cannot be used to
  // produce a valid instance. Fails with ZX_ERR_NO_MEMORY on OOM. All failures
  // result in logging.
  //
  // Instances are always created on the heap so they can be conveniently passed
  // between dispatchers.
  static zx::result<std::unique_ptr<AddedDisplayInfo>> Create(
      const fuchsia_hardware_display_engine::wire::RawDisplayInfo& fidl_display_info);

  // Guaranteed to be valid in valid instances.
  //
  // This constraint is particularly important, because it lets us choose to
  // store instances in a dictionary keyed by `display_id` in the future.
  display::DisplayId display_id;

  // Guaranteed to be non-empty in valid instances.
  fbl::Vector<display::PixelFormat> pixel_formats;

  // Guaranteed to be non-empty in valid instances.
  fbl::Vector<display::Mode> preferred_modes;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_ADDED_DISPLAY_INFO_H_
