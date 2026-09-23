// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/vsync-monitor.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fpromise/promise.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <optional>
#include <utility>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

namespace {

// vsync delivery is considered to be stalled if at least this amount of time
// has elapsed since vsync was last observed.
constexpr zx::duration kVsyncStallThreshold = zx::sec(10);
constexpr zx::duration kVsyncMonitorInterval = kVsyncStallThreshold / 2;

// Scale for the `vsync_frequency_hz` histogram: 1 Hz buckets covering
// [0 Hz, 140 Hz).
//
// A 1 Hz linear bucket size accurately resolves LTPO low-power refresh rates
// (1 Hz, 10 Hz, 24 Hz, 30 Hz) as well as standard and high refresh rates
// (60 Hz, 90 Hz, 120 Hz).
constexpr uint64_t kVsyncFrequencyHistogramFloorHz = 0;
constexpr uint64_t kVsyncFrequencyHistogramStepHz = 1;
constexpr size_t kVsyncFrequencyHistogramBucketCount = 140;

// Scale for the `vsync_jitter_us` histogram: 250 us buckets covering
// [-10 ms, +10 ms).
//
// The range brackets the two interesting failure modes - a display refreshing
// at twice the committed rate (-8.33 ms at 60 Hz) and a display refreshing at
// half the committed rate (+8.33 ms at 120 Hz). The resolution keeps a healthy
// display within a couple of buckets around zero.
constexpr int64_t kVsyncJitterHistogramFloorUs = -10'000;
constexpr int64_t kVsyncJitterHistogramStepUs = 250;
constexpr size_t kVsyncJitterHistogramBucketCount = 80;

}  // namespace

VsyncMonitor::VsyncMonitor(inspect::Node inspect_root, async_dispatcher_t* dispatcher)
    : inspect_root_(std::move(inspect_root)),
      last_vsync_ns_property_(inspect_root_.CreateUint("last_vsync_timestamp_ns", 0)),
      last_vsync_timestamp_mono_ns_property_(
          inspect_root_.CreateUint("last_vsync_timestamp_mono_ns", 0)),
      last_vsync_timestamp_approximate_boot_ns_property_(
          inspect_root_.CreateUint("last_vsync_timestamp_approximate_boot_ns", 0)),
      last_vsync_interval_ns_property_(inspect_root_.CreateUint("last_vsync_interval_ns", 0)),
      last_vsync_interval_mono_ns_property_(
          inspect_root_.CreateUint("last_vsync_interval_mono_ns", 0)),
      last_vsync_interval_boot_ns_property_(
          inspect_root_.CreateUint("last_vsync_interval_boot_ns", 0)),
      last_vsync_config_stamp_property_(inspect_root_.CreateUint(
          "last_vsync_config_stamp", display::kInvalidDriverConfigStamp.value())),
      vsync_count_property_(inspect_root_.CreateUint("vsync_count", 0)),
      expected_vsync_interval_ns_property_(
          inspect_root_.CreateUint("expected_vsync_interval_ns", 0)),
      last_vsync_jitter_ns_property_(inspect_root_.CreateInt("last_vsync_jitter_ns", 0)),
      vsync_frequency_hz_histogram_(inspect_root_.CreateLinearUintHistogram(
          "vsync_frequency_hz", kVsyncFrequencyHistogramFloorHz, kVsyncFrequencyHistogramStepHz,
          kVsyncFrequencyHistogramBucketCount)),
      vsync_jitter_us_histogram_(inspect_root_.CreateLinearIntHistogram(
          "vsync_jitter_us", kVsyncJitterHistogramFloorUs, kVsyncJitterHistogramStepUs,
          kVsyncJitterHistogramBucketCount)),
      time_since_last_vsync_sampled_ns_property_(
          inspect_root_.CreateUint("time_since_last_vsync_sampled_ns", 0)),
      vsync_stalls_detected_(inspect_root_.CreateUint("vsync_stalls", 0)),
      dispatcher_(*dispatcher) {
  ZX_DEBUG_ASSERT(dispatcher != nullptr);

  // `time_since_last_vsync_ns` is only meaningful when it is computed at the
  // moment the inspect tree is read. The `UpdateStatistics()` snapshot of the
  // same value is kept as a fallback for readers that do not resolve lazy
  // values.
  live_values_ = inspect_root_.CreateLazyValues("live", [this] {
    inspect::Inspector inspector;
    inspector.GetRoot().CreateUint("time_since_last_vsync_ns", TimeSinceLastVsync().to_nsecs(),
                                   &inspector);
    return fpromise::make_ok_promise(std::move(inspector));
  });
}

VsyncMonitor::~VsyncMonitor() { Deinitialize(); }

zx::result<> VsyncMonitor::Initialize() {
  zx_status_t post_status = updater_.PostDelayed(&dispatcher_, kVsyncMonitorInterval);
  if (post_status != ZX_OK) {
    fdf::error("Failed to schedule vsync monitor: {}", zx::make_result(post_status));
    return zx::error(post_status);
  }

  return zx::ok();
}

void VsyncMonitor::Deinitialize() { updater_.Cancel(); }

zx::duration VsyncMonitor::TimeSinceLastVsync() const {
  const zx::time_monotonic last_vsync_timestamp_mono =
      last_vsync_timestamp_mono_.load(std::memory_order_relaxed);
  if (last_vsync_timestamp_mono.get() == 0) {
    // No Vsync event was observed yet.
    return zx::duration(0);
  }
  const zx::duration time_since_last_vsync = zx::clock::get_monotonic() - last_vsync_timestamp_mono;
  return std::max(time_since_last_vsync, zx::duration(0));
}

void VsyncMonitor::UpdateStatistics() {
  time_since_last_vsync_sampled_ns_property_.Set(TimeSinceLastVsync().to_nsecs());

  if (vsync_stalled_) {
    return;
  }

  zx::time_monotonic now = zx::clock::get_monotonic();
  zx::duration since_last_vsync = now - last_vsync_timestamp_mono_.load();

  if (since_last_vsync > kVsyncStallThreshold) {
    vsync_stalled_ = true;
    vsync_stalls_detected_.Add(1);
  }

  zx_status_t status = updater_.PostDelayed(&dispatcher_, kVsyncMonitorInterval);
  if (status != ZX_OK) {
    fdf::error("Failed to schedule vsync monitor: {}", zx::make_result(status));
  }
}

void VsyncMonitor::OnVsync(display::DisplayId display_id, zx::time_monotonic vsync_timestamp_mono,
                           zx::time_boot vsync_timestamp_approximate_boot,
                           display::DriverConfigStamp vsync_config_stamp,
                           std::optional<zx::duration> expected_vsync_interval) {
  last_vsync_ns_property_.Set(vsync_timestamp_mono.get());
  last_vsync_timestamp_mono_ns_property_.Set(vsync_timestamp_mono.get());
  last_vsync_timestamp_approximate_boot_ns_property_.Set(vsync_timestamp_approximate_boot.get());

  const zx::time_monotonic previous_vsync_timestamp_mono =
      last_vsync_timestamp_mono_.load(std::memory_order_relaxed);

  zx::duration vsync_interval_duration_mono = vsync_timestamp_mono - previous_vsync_timestamp_mono;
  last_vsync_interval_ns_property_.Set(vsync_interval_duration_mono.to_nsecs());
  last_vsync_interval_mono_ns_property_.Set(vsync_interval_duration_mono.to_nsecs());
  last_vsync_config_stamp_property_.Set(vsync_config_stamp.value());

  zx::duration vsync_interval_duration_boot =
      vsync_timestamp_approximate_boot - last_vsync_timestamp_boot_.load(std::memory_order_relaxed);
  last_vsync_interval_boot_ns_property_.Set(vsync_interval_duration_boot.to_nsecs());

  vsync_count_property_.Add(1);

  // The first Vsync event has no predecessor to be measured against, and an
  // interval that spans two displays does not describe either display.
  const bool vsync_interval_is_meaningful =
      previous_vsync_timestamp_mono.get() != 0 && display_id == last_vsync_display_id_;
  const int64_t interval_ns = vsync_interval_duration_mono.to_nsecs();
  if (vsync_interval_is_meaningful && interval_ns > 0) {
    static constexpr int64_t kNanosPerSecond = 1'000'000'000;
    const uint64_t vsync_frequency_hz =
        static_cast<uint64_t>((kNanosPerSecond + (interval_ns / 2)) / interval_ns);
    vsync_frequency_hz_histogram_.Insert(vsync_frequency_hz);

    if (expected_vsync_interval.has_value()) {
      const zx::duration vsync_jitter = vsync_interval_duration_mono - *expected_vsync_interval;
      last_vsync_jitter_ns_property_.Set(vsync_jitter.to_nsecs());
      vsync_jitter_us_histogram_.Insert(vsync_jitter.to_usecs());
    }
  }

  expected_vsync_interval_ns_property_.Set(
      expected_vsync_interval.has_value()
          ? static_cast<uint64_t>(expected_vsync_interval->to_nsecs())
          : 0);
  time_since_last_vsync_sampled_ns_property_.Set(0);

  last_vsync_display_id_ = display_id;
  last_vsync_timestamp_mono_.store(vsync_timestamp_mono);
  last_vsync_timestamp_boot_.store(vsync_timestamp_approximate_boot);
  vsync_stalled_ = false;
}

}  // namespace display_coordinator
