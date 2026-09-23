// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_VSYNC_MONITOR_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_VSYNC_MONITOR_H_

#include <lib/async/cpp/task.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>

#include <atomic>
#include <optional>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

// Maintains statistics about Vsync timing, refresh frequency, jitter, and stalls.
//
// Frequency and jitter statistics are computed from the time interval between
// consecutive Vsync events originating from the same display. If no display mode
// has been committed to the display engine yet, only the Vsync frequency is
// recorded in the histogram and jitter is omitted.
class VsyncMonitor {
 public:
  // `dispatcher` must be non-null and must outlive the `VsyncMonitor`
  // instance.
  explicit VsyncMonitor(inspect::Node inspect_root, async_dispatcher_t* dispatcher);

  VsyncMonitor(const VsyncMonitor&) = delete;
  VsyncMonitor(VsyncMonitor&&) = delete;
  VsyncMonitor& operator=(const VsyncMonitor&) = delete;
  VsyncMonitor& operator=(VsyncMonitor&&) = delete;

  ~VsyncMonitor();

  // Initialization code not suitable for the constructor.
  zx::result<> Initialize();

  void Deinitialize();

  // Called when a display engine driver sends a Vsync event.
  //
  // `expected_vsync_interval` is nullopt if no display mode has been committed
  // to the engine yet (for example, when the display was initialized by the
  // bootloader before the display coordinator started).
  void OnVsync(display::DisplayId display_id, zx::time_monotonic vsync_timestamp_mono,
               zx::time_boot vsync_timestamp_boot_aproximate,
               display::DriverConfigStamp vsync_config_stamp,
               std::optional<zx::duration> expected_vsync_interval);

 private:
  // The time elapsed since the last observed Vsync event.
  //
  // Returns a zero duration if no Vsync event was observed yet.
  //
  // Safe to call from any thread.
  zx::duration TimeSinceLastVsync() const;

  // Periodically reads `last_vsync_timestamp_` and increments
  // `vsync_stalls_detected_` if no vsync has been observed in a given time
  // period.
  void UpdateStatistics();

  std::atomic<zx::time_monotonic> last_vsync_timestamp_mono_;
  std::atomic<zx::time_boot> last_vsync_timestamp_boot_;

  // The display that generated the most recent Vsync event.
  //
  // Used to avoid measuring an interval between Vsync events that come from
  // two different displays.
  display::DisplayId last_vsync_display_id_ = display::kInvalidDisplayId;

  inspect::Node inspect_root_;
  // TODO(b/475953032): Remove once it is no longer used.
  inspect::UintProperty last_vsync_ns_property_;
  inspect::UintProperty last_vsync_timestamp_mono_ns_property_;
  // TODO(b/475953032): The exact boot timestamp is not plumbed in yet.
  inspect::UintProperty last_vsync_timestamp_approximate_boot_ns_property_;
  // TODO(b/475953032): Remove once it is no longer used.
  inspect::UintProperty last_vsync_interval_ns_property_;
  inspect::UintProperty last_vsync_interval_mono_ns_property_;
  inspect::UintProperty last_vsync_interval_boot_ns_property_;
  inspect::UintProperty last_vsync_config_stamp_property_;

  // The number of Vsync events observed. Denominator for the histograms below.
  inspect::UintProperty vsync_count_property_;

  // The Vsync interval implied by the committed display mode. Zero if unknown.
  inspect::UintProperty expected_vsync_interval_ns_property_;

  // Negative iff the last Vsync event arrived earlier than the interval implied
  // by the committed display mode.
  inspect::IntProperty last_vsync_jitter_ns_property_;

  // Distribution of instantaneous Vsync refresh frequencies in Hz, rounded to
  // the nearest integer Hz.
  inspect::LinearUintHistogram vsync_frequency_hz_histogram_;

  // Distribution of the deltas between the observed Vsync intervals and the
  // interval implied by the committed display mode.
  inspect::LinearIntHistogram vsync_jitter_us_histogram_;

  // A snapshot of the time elapsed since the last Vsync event, refreshed
  // whenever Vsync delivery is checked for stalls.
  //
  // `time_since_last_vsync_ns`, which is computed when the inspect tree is
  // read, is more accurate. This property is a fallback for readers that do not
  // resolve lazy values.
  inspect::UintProperty time_since_last_vsync_sampled_ns_property_;

  // Publishes the values that must be computed when the inspect tree is read.
  inspect::LazyNode live_values_;

  // Fields that track how often vsync was detected to have been stalled.
  std::atomic_bool vsync_stalled_ = false;
  inspect::UintProperty vsync_stalls_detected_;

  async_dispatcher_t& dispatcher_;
  async::TaskClosureMethod<VsyncMonitor, &VsyncMonitor::UpdateStatistics> updater_{this};
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_VSYNC_MONITOR_H_
