// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/display.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <zircon/syscalls.h>

#include "src/ui/scenic/lib/utils/logging.h"

namespace display {

Display::Display(WireDisplayId id, const WireDisplayMode& mode, uint32_t width_in_mm,
                 uint32_t height_in_mm, uint32_t max_layer_count,
                 std::vector<fuchsia_images2::PixelFormat> pixel_formats)
    : vsync_timing_(std::make_shared<scheduling::VsyncTiming>()),
      display_id_(id),
      mode_(mode),
      width_in_mm_(width_in_mm),
      height_in_mm_(height_in_mm),
      max_layer_count_(max_layer_count),
      pixel_formats_(std::move(pixel_formats)) {
  zx::event::create(0, &ownership_event_);
  device_pixel_ratio_.store({1.f, 1.f});

  // Most displays will have a longer interval.  If so, `OnVsync()` will adjust.
  vsync_timing_->AddVsyncInterval(kMinimumVsyncInterval);
}

Display::Display(WireDisplayId id, uint32_t width_in_px, uint32_t height_in_px,
                 uint32_t max_layer_count)
    : Display(id,
              WireDisplayMode{.active_area = {.width = width_in_px, .height = height_in_px},
                              .refresh_rate_millihertz = 60'000},
              0, 0, max_layer_count, {fuchsia_images2::PixelFormat::kB8G8R8A8}) {}

void Display::Claim() {
  FX_DCHECK(!claimed_);
  claimed_ = true;
}

void Display::Unclaim() {
  FX_DCHECK(claimed_);
  claimed_ = false;
}

Display::VsyncCallbackId Display::AddVsyncCallback(VsyncCallback callback) {
  const VsyncCallbackId id = ++next_vsync_callback_id_;
  FX_DCHECK(!vsync_callbacks_.contains(id));
  vsync_callbacks_[id] = std::move(callback);
  return id;
}

void Display::RemoveVsyncCallback(VsyncCallbackId id) {
  if (!vsync_callbacks_.erase(id)) {
    FX_LOGS(ERROR) << "Removing an unregistered vsync callback.";
  }
}

void Display::OnVsync(zx::time_monotonic timestamp, WireConfigStamp displayed_config_stamp) {
  // Estimate current vsync interval. Need to include a maximum to mitigate any
  // potential issues during long breaks.
  const zx::duration time_since_last_vsync = timestamp - vsync_timing_->last_vsync_time();
  if (time_since_last_vsync < kMaximumVsyncInterval) {
    vsync_timing_->AddVsyncInterval(std::max(kMinimumVsyncInterval, time_since_last_vsync));
  }

  vsync_timing_->set_last_vsync_time(timestamp);

  TRACE_INSTANT("gfx", "Display::OnVsync", TRACE_SCOPE_PROCESS, "Timestamp", timestamp.get(),
                "Vsync interval", vsync_timing_->vsync_interval().get());

  for (const auto& [id, callback] : vsync_callbacks_) {
    FLATLAND_VERBOSE_LOG << "Display::OnVsync(): display_id=" << display_id_.value()
                         << "  callback_id=" << id << "  timestamp=" << timestamp.get()
                         << "  displayed_config_stamp=" << displayed_config_stamp.value
                         << "  ... invoking vsync callback";
    callback(timestamp, displayed_config_stamp);
  }
}

}  // namespace display
