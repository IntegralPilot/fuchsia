// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/zx/event.h>
#include <zircon/types.h>

#include <cstdint>
#include <unordered_map>
#include <vector>

#include "src/lib/fxl/macros.h"
#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/scheduling/vsync_timing.h"

#include <glm/glm.hpp>

namespace display {

// Display is a placeholder that provides make-believe values for screen
// resolution, vsync interval, last vsync time, etc.
class Display {
 public:
  Display(WireDisplayId id, const WireDisplayMode& mode, uint32_t width_in_mm,
          uint32_t height_in_mm, uint32_t max_layer_count,
          std::vector<fuchsia_images2::PixelFormat> pixel_formats);
  Display(WireDisplayId id, uint32_t width_in_px, uint32_t height_in_px, uint32_t max_layer_count);
  virtual ~Display() = default;

  using VsyncCallbackId = int;
  using VsyncCallback =
      fit::function<void(zx::time_monotonic timestamp, WireConfigStamp displayed_config_stamp)>;
  VsyncCallbackId AddVsyncCallback(VsyncCallback callback);
  void RemoveVsyncCallback(VsyncCallbackId id);

  using DPRCallback = fit::function<void(const glm::vec2& dpr)>;
  void SetDPRCallback(DPRCallback callback) { dpr_callback_ = std::move(callback); }

  std::shared_ptr<const scheduling::VsyncTiming> vsync_timing() { return vsync_timing_; }

  // Claiming a display means that no other display renderer can use it.
  bool is_claimed() const { return claimed_; }
  void Claim();
  void Unclaim();

  // Sets the device_pixel ratio that should be used for this specific Display.
  void set_device_pixel_ratio(const glm::vec2& device_pixel_ratio) {
    device_pixel_ratio_.store(device_pixel_ratio);
    if (dpr_callback_) {
      dpr_callback_(device_pixel_ratio);
    }
  }

  const WireDisplayMode& Mode() const { return mode_; }

  // The display's ID in the context of the DisplayManager's DisplayController.
  display::DisplayId display_id() const { return display_id_; }
  const WireDisplayMode& mode() const { return mode_; }
  uint32_t width_in_px() const { return mode_.active_area.width; }
  uint32_t height_in_px() const { return mode_.active_area.height; }
  uint32_t width_in_mm() const { return width_in_mm_; }
  uint32_t height_in_mm() const { return height_in_mm_; }

  glm::vec2 device_pixel_ratio() const { return device_pixel_ratio_.load(); }

  const std::vector<fuchsia_images2::PixelFormat>& pixel_formats() const { return pixel_formats_; }

  uint32_t maximum_refresh_rate_in_millihertz() const { return mode_.refresh_rate_millihertz; }

  uint32_t max_layer_count() const { return max_layer_count_; }

  // Event signaled by DisplayManager when ownership of the display
  // changes. This event backs Scenic's GetDisplayOwnershipEvent API.
  const zx::event& ownership_event() const { return ownership_event_; }

  // Called by DisplayManager, other users of Display should probably not call this.  Except tests.
  void OnVsync(zx::time_monotonic timestamp, WireConfigStamp displayed_config_stamp);

 protected:
  std::shared_ptr<scheduling::VsyncTiming> vsync_timing_;

 private:
  VsyncCallbackId next_vsync_callback_id_ = 0;
  std::unordered_map<VsyncCallbackId, VsyncCallback> vsync_callbacks_;
  DPRCallback dpr_callback_;

  // The maximum vsync interval we would ever expect.
  static constexpr zx::duration kMaximumVsyncInterval = zx::msec(100);
  // 240Hz should be fast enough for anybody.
  static constexpr zx::duration kMinimumVsyncInterval = zx::usec(/*1000000/240=*/4167);

  const display::DisplayId display_id_;
  const WireDisplayMode mode_;
  const uint32_t width_in_mm_;
  const uint32_t height_in_mm_;
  const uint32_t max_layer_count_;
  // |device_pixel_ratio_| may be written from FlatlandDisplay thread and read by SingletonDisplay
  // service running on the main thread.
  std::atomic<glm::vec2> device_pixel_ratio_;
  zx::event ownership_event_;
  std::vector<fuchsia_images2::PixelFormat> pixel_formats_;

  bool claimed_ = false;

  FXL_DISALLOW_COPY_AND_ASSIGN(Display);
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_H_
