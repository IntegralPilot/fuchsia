// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/vsync-monitor.h"

#include <lib/async-testing/test_loop.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fpromise/single_threaded_executor.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <cstdint>
#include <optional>
#include <utility>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"

namespace display_coordinator {

namespace {

constexpr display::DisplayId kDisplayId(1);
constexpr display::DisplayId kOtherDisplayId(2);

// The Vsync interval of a 1 Hz LTPO display mode.
constexpr zx::duration kVsyncInterval1Hz = zx::sec(1);

// The Vsync interval of a 60 Hz display mode.
constexpr zx::duration kVsyncInterval60Hz = zx::nsec(16'666'666);

// The Vsync interval of a 120 Hz display mode.
constexpr zx::duration kVsyncInterval120Hz = zx::nsec(8'333'333);

// The count in the histogram bucket that `value` falls into.
//
// Returns zero if `histogram` has no bucket covering `value`.
template <typename ArrayValue>
typename ArrayValue::value_type BucketCount(const ArrayValue& histogram,
                                            typename ArrayValue::value_type value) {
  for (const auto& bucket : histogram.GetBuckets()) {
    if (value >= bucket.floor && value < bucket.upper_limit) {
      return bucket.count;
    }
  }
  return 0;
}

// The sum of the counts in all the buckets of `histogram`.
template <typename ArrayValue>
typename ArrayValue::value_type TotalCount(const ArrayValue& histogram) {
  typename ArrayValue::value_type total = 0;
  for (const auto& bucket : histogram.GetBuckets()) {
    total += bucket.count;
  }
  return total;
}

class VsyncMonitorTest : public ::testing::Test {
 public:
  void SetUp() override {
    vsync_monitor_.emplace(inspector_.GetRoot().CreateChild("vsync_monitor"),
                           test_loop_.dispatcher());
  }

 protected:
  // Reports a Vsync event on `kDisplayId` with an expected interval of 60 Hz.
  void ReportVsync(zx::time_monotonic timestamp_mono) {
    ReportVsync(kDisplayId, timestamp_mono, kVsyncInterval60Hz);
  }

  void ReportVsync(display::DisplayId display_id, zx::time_monotonic timestamp_mono,
                   std::optional<zx::duration> expected_vsync_interval) {
    vsync_monitor_->OnVsync(display_id, timestamp_mono, zx::time_boot(timestamp_mono.get()),
                            display::DriverConfigStamp(1), expected_vsync_interval);
  }

  // The `vsync_monitor` node, snapshotted from the inspect tree.
  //
  // The returned reference is invalidated by the next call to this method.
  const inspect::NodeValue& ReadVsyncMonitorNode() {
    fpromise::result<inspect::Hierarchy> hierarchy_result =
        fpromise::run_single_threaded(inspect::ReadFromInspector(inspector_));
    ZX_ASSERT(hierarchy_result.is_ok());
    hierarchy_ = hierarchy_result.take_value();

    const inspect::Hierarchy* vsync_monitor = hierarchy_.GetByPath({"vsync_monitor"});
    ZX_ASSERT(vsync_monitor != nullptr);
    return vsync_monitor->node();
  }

  template <typename PropertyValue>
  const PropertyValue& ReadProperty(const char* name) {
    const PropertyValue* property = ReadVsyncMonitorNode().get_property<PropertyValue>(name);
    ZX_ASSERT_MSG(property != nullptr, "No inspect property named \"%s\"", name);
    return *property;
  }

  const inspect::UintArrayValue& ReadVsyncFrequencyHistogram() {
    return ReadProperty<inspect::UintArrayValue>("vsync_frequency_hz");
  }

  const inspect::IntArrayValue& ReadVsyncJitterHistogram() {
    return ReadProperty<inspect::IntArrayValue>("vsync_jitter_us");
  }

  uint64_t ReadUintProperty(const char* name) {
    return ReadProperty<inspect::UintPropertyValue>(name).value();
  }

  int64_t ReadIntProperty(const char* name) {
    return ReadProperty<inspect::IntPropertyValue>(name).value();
  }

  fdf_testing::ScopedGlobalLogger logger_;
  async::TestLoop test_loop_;
  inspect::Inspector inspector_;
  std::optional<VsyncMonitor> vsync_monitor_;

 private:
  // Backs the references returned by `ReadVsyncMonitorNode()`.
  inspect::Hierarchy hierarchy_;
};

TEST_F(VsyncMonitorTest, HistogramsAreExposed) {
  EXPECT_EQ(inspect::ArrayDisplayFormat::kLinearHistogram,
            ReadVsyncFrequencyHistogram().GetDisplayFormat());
  EXPECT_EQ(inspect::ArrayDisplayFormat::kLinearHistogram,
            ReadVsyncJitterHistogram().GetDisplayFormat());
}

TEST_F(VsyncMonitorTest, FirstVsyncIsNotRecordedInHistograms) {
  ReportVsync(zx::time_monotonic(zx::sec(1).get()));

  EXPECT_EQ(1u, ReadUintProperty("vsync_count"));
  EXPECT_EQ(0u, TotalCount(ReadVsyncFrequencyHistogram()));
  EXPECT_EQ(0, TotalCount(ReadVsyncJitterHistogram()));
}

TEST_F(VsyncMonitorTest, VsyncFrequenciesAreRecorded) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  for (int i = 0; i < 5; ++i) {
    ReportVsync(timestamp_mono);
    timestamp_mono += kVsyncInterval60Hz;
  }

  EXPECT_EQ(5u, ReadUintProperty("vsync_count"));

  // The first Vsync event does not produce an interval.
  const inspect::UintArrayValue& vsync_frequency_histogram = ReadVsyncFrequencyHistogram();
  EXPECT_EQ(4u, TotalCount(vsync_frequency_histogram));
  EXPECT_EQ(4u, BucketCount(vsync_frequency_histogram, uint64_t{60}));
}

TEST_F(VsyncMonitorTest, Ltpo1HzFrequencyIsRecorded) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  for (int i = 0; i < 4; ++i) {
    ReportVsync(kDisplayId, timestamp_mono, kVsyncInterval1Hz);
    timestamp_mono += kVsyncInterval1Hz;
  }

  const inspect::UintArrayValue& vsync_frequency_histogram = ReadVsyncFrequencyHistogram();
  EXPECT_EQ(3u, TotalCount(vsync_frequency_histogram));
  EXPECT_EQ(3u, BucketCount(vsync_frequency_histogram, uint64_t{1}));
}

TEST_F(VsyncMonitorTest, JitterIsZeroAtTheCommittedRefreshRate) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  for (int i = 0; i < 5; ++i) {
    ReportVsync(timestamp_mono);
    timestamp_mono += kVsyncInterval60Hz;
  }

  {
    const inspect::IntArrayValue& vsync_jitter_histogram = ReadVsyncJitterHistogram();
    EXPECT_EQ(4, TotalCount(vsync_jitter_histogram));
    EXPECT_EQ(4, BucketCount(vsync_jitter_histogram, int64_t{0}));
  }

  EXPECT_EQ(static_cast<uint64_t>(kVsyncInterval60Hz.to_nsecs()),
            ReadUintProperty("expected_vsync_interval_ns"));
  EXPECT_EQ(0, ReadIntProperty("last_vsync_jitter_ns"));
}

// Regression coverage for b/562145214, where a display committed to a 60 Hz
// mode refreshed at 120 Hz.
TEST_F(VsyncMonitorTest, JitterIsNegativeWhenRefreshingFasterThanCommittedMode) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  for (int i = 0; i < 5; ++i) {
    ReportVsync(kDisplayId, timestamp_mono, kVsyncInterval60Hz);
    timestamp_mono += kVsyncInterval120Hz;
  }

  {
    const inspect::IntArrayValue& vsync_jitter_histogram = ReadVsyncJitterHistogram();
    EXPECT_EQ(4, TotalCount(vsync_jitter_histogram));
    EXPECT_EQ(4, BucketCount(vsync_jitter_histogram, int64_t{-8'333}));
  }

  // The Vsync frequency is that of a 120 Hz display, while the expected
  // interval remains the one of the committed 60 Hz mode.
  EXPECT_EQ(4u, BucketCount(ReadVsyncFrequencyHistogram(), uint64_t{120}));
  EXPECT_EQ(static_cast<uint64_t>(kVsyncInterval60Hz.to_nsecs()),
            ReadUintProperty("expected_vsync_interval_ns"));
  EXPECT_LT(ReadIntProperty("last_vsync_jitter_ns"), 0);
}

TEST_F(VsyncMonitorTest, JitterIsNotRecordedWhenTheCommittedModeIsUnknown) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  for (int i = 0; i < 5; ++i) {
    ReportVsync(kDisplayId, timestamp_mono, std::nullopt);
    timestamp_mono += kVsyncInterval60Hz;
  }

  EXPECT_EQ(4u, TotalCount(ReadVsyncFrequencyHistogram()));
  EXPECT_EQ(0, TotalCount(ReadVsyncJitterHistogram()));
  EXPECT_EQ(0u, ReadUintProperty("expected_vsync_interval_ns"));
}

TEST_F(VsyncMonitorTest, IntervalsBetweenDifferentDisplaysAreNotRecorded) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  for (int i = 0; i < 5; ++i) {
    ReportVsync(i % 2 == 0 ? kDisplayId : kOtherDisplayId, timestamp_mono, kVsyncInterval60Hz);
    timestamp_mono += kVsyncInterval60Hz;
  }

  EXPECT_EQ(5u, ReadUintProperty("vsync_count"));
  EXPECT_EQ(0u, TotalCount(ReadVsyncFrequencyHistogram()));
  EXPECT_EQ(0, TotalCount(ReadVsyncJitterHistogram()));
}

TEST_F(VsyncMonitorTest, StalledVsyncDeliveryIsRecorded) {
  zx::time_monotonic timestamp_mono(zx::sec(1).get());
  ReportVsync(timestamp_mono);
  timestamp_mono += zx::sec(20);
  ReportVsync(timestamp_mono);

  // A 20-second stall has a rounded frequency of 0 Hz and lands in bucket 0.
  const inspect::UintArrayValue& vsync_frequency_histogram = ReadVsyncFrequencyHistogram();
  EXPECT_EQ(1u, TotalCount(vsync_frequency_histogram));
  EXPECT_EQ(1u, BucketCount(vsync_frequency_histogram, uint64_t{0}));
}

TEST_F(VsyncMonitorTest, TimeSinceLastVsyncIsComputedWhenInspectIsRead) {
  // No Vsync event was observed yet.
  EXPECT_EQ(0u, ReadUintProperty("time_since_last_vsync_ns"));

  ReportVsync(zx::clock::get_monotonic());
  const uint64_t time_since_last_vsync_ns = ReadUintProperty("time_since_last_vsync_ns");

  // The value is measured when the inspect tree is read, so it is positive but
  // much smaller than the Vsync stall threshold.
  EXPECT_GT(time_since_last_vsync_ns, 0u);
  EXPECT_LT(time_since_last_vsync_ns, static_cast<uint64_t>(zx::sec(10).to_nsecs()));
}

}  // namespace

}  // namespace display_coordinator
