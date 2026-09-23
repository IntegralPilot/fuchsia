// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-fidl.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/driver/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/time.h>

#include <array>
#include <mutex>
#include <span>

#include <sdk/lib/driver/logging/cpp/logger.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display {

DisplayEngineEventsFidl::DisplayEngineEventsFidl() = default;
DisplayEngineEventsFidl::~DisplayEngineEventsFidl() = default;

void DisplayEngineEventsFidl::SetListener(
    fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener> client_end) {
  // TODO(fxbug.dev/473698115): The current FIDL API contract allows a potential
  // deadlock here. The Coordinator is allowed to call
  // [`fuchsia.hardware.display.engine/Engine.UnsetListener()`] in an event
  // handler, such as [`fuchsia.hardware.display.engine/EngineListener.
  // OnDisplayRemoved()`].
  std::lock_guard lock_guard(fidl_client_mutex_);
  if (client_end.is_valid()) {
    fidl_client_ =
        fdf::WireSyncClient<fuchsia_hardware_display_engine::EngineListener>(std::move(client_end));
  } else {
    fidl_client_ = fdf::WireSyncClient<fuchsia_hardware_display_engine::EngineListener>();
  }
}

namespace {

// Used for all FIDL calls coming from this client.
constexpr fdf_arena_tag_t kArenaTag = 'DISP';

}  // namespace

void DisplayEngineEventsFidl::OnDisplayAdded(display::DisplayId display_id,
                                             std::span<const display::ModeAndId> preferred_modes,
                                             std::span<const display::PixelFormat> pixel_formats) {
  std::lock_guard lock_guard(fidl_client_mutex_);

  ZX_DEBUG_ASSERT(preferred_modes.size() <= kMaxPreferredModes);
  ZX_DEBUG_ASSERT(pixel_formats.size() <= kMaxPixelFormats);

  if (!fidl_client_.is_valid()) {
    fdf::warn("OnDisplayAdded() emitted with invalid event listener; event dropped");
    return;
  }

  std::array<fuchsia_hardware_display_types::wire::Mode, kMaxPreferredModes>
      fidl_preferred_modes_buffer;
  for (size_t i = 0; i < preferred_modes.size(); ++i) {
    fidl_preferred_modes_buffer[i] = preferred_modes[i].mode().ToFidl();
  }

  std::array<fuchsia_images2::wire::PixelFormat, kMaxPixelFormats> fidl_pixel_formats_buffer;
  for (size_t i = 0; i < pixel_formats.size(); ++i) {
    fidl_pixel_formats_buffer[i] = pixel_formats[i].ToFidl();
  }

  fuchsia_hardware_display_engine::wire::RawDisplayInfo fidl_display_info = {
      .display_id = display_id.ToFidl(),
      .preferred_modes = fidl::VectorView<fuchsia_hardware_display_types::wire::Mode>::FromExternal(
          fidl_preferred_modes_buffer.data(), preferred_modes.size()),
      .edid_bytes = {},
      .pixel_formats = fidl::VectorView<fuchsia_images2::wire::PixelFormat>::FromExternal(
          fidl_pixel_formats_buffer.data(), pixel_formats.size()),
  };

  // TODO(https://fxbug.dev/430058446): Eliminate the per-call dynamic memory
  // allocation caused by fdf::Arena creation.
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_transport_status =
      fidl_client_.buffer(arena)->OnDisplayAdded(std::move(fidl_display_info));
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnDisplayAdded: {}",
               fidl_transport_status.error().FormatDescription());
  }
}

void DisplayEngineEventsFidl::OnDisplayRemoved(display::DisplayId display_id) {
  std::lock_guard lock_guard(fidl_client_mutex_);

  if (!fidl_client_.is_valid()) {
    fdf::warn("OnDisplayRemoved() emitted with invalid event listener; event dropped");
    return;
  }

  // TODO(https://fxbug.dev/430058446): Eliminate the per-call dynamic memory
  // allocation caused by fdf::Arena creation.
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_transport_status =
      fidl_client_.buffer(arena)->OnDisplayRemoved(display_id.ToFidl());
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnDisplayRemoved: {}",
               fidl_transport_status.error().FormatDescription());
  }
}

void DisplayEngineEventsFidl::OnDisplayVsync(display::DisplayId display_id,
                                             zx::time_monotonic timestamp,
                                             display::DriverConfigStamp config_stamp) {
  std::lock_guard lock_guard(fidl_client_mutex_);

  if (!fidl_client_.is_valid()) {
    fdf::warn("OnDisplayVsync() emitted with invalid event listener; event dropped");
    return;
  }

  // TODO(https://fxbug.dev/430058446): Eliminate the per-call dynamic memory
  // allocation caused by fdf::Arena creation.
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_transport_status = fidl_client_.buffer(arena)->OnDisplayVsync(
      display_id.ToFidl(), timestamp, config_stamp.ToFidl());
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnDisplayVsync: {}",
               fidl_transport_status.error().FormatDescription());
  }
}

void DisplayEngineEventsFidl::OnCaptureComplete() {
  std::lock_guard lock_guard(fidl_client_mutex_);

  if (!fidl_client_.is_valid()) {
    fdf::warn("OnCaptureComplete() emitted with invalid event listener; event dropped");
    return;
  }

  // TODO(https://fxbug.dev/430058446): Eliminate the per-call dynamic memory
  // allocation caused by fdf::Arena creation.
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_transport_status = fidl_client_.buffer(arena)->OnCaptureComplete();
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnCaptureComplete: {}",
               fidl_transport_status.error().FormatDescription());
  }
}

}  // namespace display
