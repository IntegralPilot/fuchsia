// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/display-info.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/device/audio.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>

#include <algorithm>
#include <cinttypes>
#include <cstddef>
#include <cstring>
#include <optional>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/string_printf.h>

#include "src/graphics/display/drivers/coordinator/added-display-info.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"

namespace display_coordinator {

DisplayInfo::DisplayInfo(display::DisplayId display_id,
                         fbl::Vector<display::PixelFormat> pixel_formats,
                         fbl::Vector<display::ModeAndId> preferred_modes)
    : IdMappable(display_id),
      preferred_modes(std::move(preferred_modes)),
      pixel_formats(std::move(pixel_formats)) {
  ZX_DEBUG_ASSERT(display_id != display::kInvalidDisplayId);
}

DisplayInfo::~DisplayInfo() = default;

void DisplayInfo::InitializeInspect(inspect::Node* parent_node) {
  node = parent_node->CreateChild(fbl::StringPrintf("display-%" PRIu64, id().value()).c_str());

  for (const display::ModeAndId& mode_and_id : preferred_modes) {
    auto child = node.CreateChild(
        fbl::StringPrintf("preferred-mode-%" PRIu16, mode_and_id.id().value()).c_str());
    child.CreateDouble("vsync-hz", mode_and_id.mode().refresh_rate_millihertz() / 1000.0,
                       &properties);
    child.CreateInt("width-pixels", mode_and_id.mode().active_area().width(), &properties);
    child.CreateInt("height-pixels", mode_and_id.mode().active_area().height(), &properties);
    properties.emplace(std::move(child));
  }
}

// static
zx::result<std::unique_ptr<DisplayInfo>> DisplayInfo::Create(AddedDisplayInfo added_display_info) {
  ZX_DEBUG_ASSERT(added_display_info.display_id != display::kInvalidDisplayId);
  display::DisplayId display_id = added_display_info.display_id;

  fbl::Vector<display::Mode> preferred_modes = std::move(added_display_info.preferred_modes);
  if (preferred_modes.is_empty()) {
    fdf::error("Failed to create DisplayInfo: The display doesn't provide valid preferred modes.");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AllocChecker alloc_checker;
  fbl::Vector<display::ModeAndId> preferred_modes_and_ids;
  preferred_modes_and_ids.reserve(preferred_modes.size(), &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate preferred modes and ids");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // TODO(https://fxbug.dev/316631158): This assumes preferred modes have
  // `ModeId`s from 1 to `preferred_modes.size()`. The mapping between `ModeId`
  // and `Mode` should be made available to the display engine driver.
  for (uint16_t i = 0; i < preferred_modes.size(); ++i) {
    ZX_DEBUG_ASSERT_MSG(preferred_modes_and_ids.size() < preferred_modes.size(),
                        "The push_back() below was not supposed to allocate memory, but it might");
    preferred_modes_and_ids.push_back({{
                                          .id = display::ModeId(i + 1),
                                          .mode = preferred_modes[i],
                                      }},
                                      &alloc_checker);
    ZX_DEBUG_ASSERT_MSG(alloc_checker.check(),
                        "The push_back() above failed to allocate memory; "
                        "it was not supposed to allocate at all");
  }

  auto display_info = fbl::make_unique_checked<DisplayInfo>(
      &alloc_checker, display_id, std::move(added_display_info.pixel_formats),
      std::move(preferred_modes_and_ids));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate DisplayInfo for display ID: {}", display_id.value());
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(display_info));
}

// TODO(https://fxbug.dev/439019150): Provide display panel metadata when
// it's available.

int DisplayInfo::GetHorizontalSizeMm() const { return 0; }

int DisplayInfo::GetVerticalSizeMm() const { return 0; }

std::string_view DisplayInfo::GetManufacturerName() const { return {}; }

std::string DisplayInfo::GetMonitorName() const { return {}; }

std::string DisplayInfo::GetMonitorSerial() const { return {}; }

std::optional<zx::duration> DisplayInfo::GetCommittedModeVsyncInterval() const {
  if (committed_mode_id == display::kInvalidModeId) {
    return std::nullopt;
  }

  const auto mode_it = std::find_if(
      preferred_modes.begin(), preferred_modes.end(),
      [&](const display::ModeAndId& mode_and_id) { return mode_and_id.id() == committed_mode_id; });
  if (mode_it == preferred_modes.end()) {
    return std::nullopt;
  }

  const int32_t refresh_rate_millihertz = mode_it->mode().refresh_rate_millihertz();
  if (refresh_rate_millihertz <= 0) {
    return std::nullopt;
  }

  // 1 millihertz is 1e-3 Hz, so the period in nanoseconds is 1e12 divided by
  // the refresh rate in millihertz.
  static constexpr int64_t kNanosecondsPerMillihertzPeriod = 1'000'000'000'000;
  return zx::nsec(kNanosecondsPerMillihertzPeriod / refresh_rate_millihertz);
}

}  // namespace display_coordinator
