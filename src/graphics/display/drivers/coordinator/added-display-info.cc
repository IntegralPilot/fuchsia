// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/added-display-info.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>

#include <algorithm>
#include <cstdint>

#include <fbl/alloc_checker.h>
#include <fbl/vector.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display_coordinator {

// static
zx::result<std::unique_ptr<AddedDisplayInfo>> AddedDisplayInfo::Create(
    const fuchsia_hardware_display_engine::wire::RawDisplayInfo& fidl_display_info) {
  const display::DisplayId display_id(fidl_display_info.display_id);
  if (display_id == display::kInvalidDisplayId) {
    fdf::error("AddedDisplayInfo creation failed: invalid display ID");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (fidl_display_info.pixel_formats.size() == 0) {
    fdf::error("AddedDisplayInfo creation failed: empty pixel formats list");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (fidl_display_info.preferred_modes.empty()) {
    fdf::error("AddedDisplayInfo creation failed: empty preferred modes list");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::Vector<display::PixelFormat> pixel_formats;
  fbl::AllocChecker alloc_checker;
  pixel_formats.reserve(fidl_display_info.pixel_formats.size(), &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("AddedDisplayInfo creation failed: out of memory allocating pixel formats");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  for (const fuchsia_images2::wire::PixelFormat& fidl_pixel_format :
       fidl_display_info.pixel_formats) {
    if (!display::PixelFormat::IsSupported(fidl_pixel_format)) {
      fdf::error("AddedDisplayInfo creation failed: unsupported pixel format {}",
                 static_cast<uint32_t>(fidl_pixel_format));
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    display::PixelFormat pixel_format(fidl_pixel_format);

    ZX_DEBUG_ASSERT_MSG(pixel_formats.size() < fidl_display_info.pixel_formats.size(),
                        "The push_back() below was not supposed to allocate memory, but it might");
    pixel_formats.push_back(pixel_format, &alloc_checker);
    ZX_DEBUG_ASSERT_MSG(alloc_checker.check(),
                        "The push_back() above failed to allocate memory; "
                        "it was not supposed to allocate at all");
  }

  fbl::Vector<display::Mode> preferred_modes;
  preferred_modes.reserve(fidl_display_info.preferred_modes.size(), &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("AddedDisplayInfo creation failed: out of memory allocating display modes");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  for (const fuchsia_hardware_display_types::wire::Mode& fidl_preferred_mode :
       fidl_display_info.preferred_modes) {
    ZX_DEBUG_ASSERT_MSG(preferred_modes.size() < fidl_display_info.preferred_modes.size(),
                        "The push_back() below was not supposed to allocate memory, but it might");
    if (!display::Mode::IsValid(fidl_preferred_mode)) {
      fdf::error("AddedDisplayInfo creation failed: invalid preferred mode for display ID {}",
                 display_id.value());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    preferred_modes.push_back(display::Mode::From(fidl_preferred_mode), &alloc_checker);
    ZX_DEBUG_ASSERT_MSG(alloc_checker.check(),
                        "The push_back() above failed to allocate memory; "
                        "it was not supposed to allocate at all");
  }

  auto display_info = fbl::make_unique_checked<AddedDisplayInfo>(&alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("AddedDisplayInfo creation failed: out of memory allocating instance");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  *display_info = {
      .display_id = display_id,
      .pixel_formats = std::move(pixel_formats),
      .preferred_modes = std::move(preferred_modes),
  };
  return zx::ok(std::move(display_info));
}

}  // namespace display_coordinator
