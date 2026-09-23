// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/cobalt/bin/system-metrics/system_metrics_daemon.h"

#include <fuchsia/metrics/cpp/fidl.h>
#include <fuchsia/metrics/cpp/fidl_test_base.h>
#include <lib/async/cpp/executor.h>
#include <lib/fidl/cpp/binding_set.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/sys/cpp/testing/component_context_provider.h>
#include <lib/zx/time.h>

#include <fstream>
#include <future>
#include <utility>

#include <gtest/gtest.h>

#include "src/cobalt/bin/system-metrics/metrics_registry.cb.h"
#include "src/cobalt/bin/system-metrics/testing/fake_cpu_stats_fetcher.h"
#include "src/cobalt/bin/testing/log_metric_method.h"
#include "src/cobalt/bin/testing/stub_metric_event_logger.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/lib/timekeeper/test_clock.h"

using cobalt::FakeCpuStatsFetcher;
using cobalt::LogMetricMethod;
using cobalt::StubMetricEventLogger_Sync;
using fuchsia_system_metrics::FuchsiaLifetimeEventsMigratedMetricDimensionEvents;
using DeviceState = fuchsia_system_metrics::CpuPercentageMigratedMetricDimensionDeviceState;
using fuchsia_system_metrics::FuchsiaUpPingMigratedMetricDimensionUptime;
using fuchsia_system_metrics::FuchsiaUptimeMigratedMetricDimensionUptimeRange;

namespace {
typedef FuchsiaUptimeMigratedMetricDimensionUptimeRange UptimeRange;
constexpr int kHour = 3600;
constexpr int kDay = 24 * kHour;
constexpr int kWeek = 7 * kDay;
}  // namespace

class SystemMetricsDaemonTest : public gtest::TestLoopFixture {
 public:
  // Note that we first save an unprotected pointer in fake_clock_ and then
  // give ownership of the pointer to daemon_.
  SystemMetricsDaemonTest()
      : executor_(dispatcher()),
        fake_clock_(new timekeeper::TestClock()),
        daemon_(new SystemMetricsDaemon(
            dispatcher(), context_provider_.context(), &stub_logger_,
            std::unique_ptr<timekeeper::TestClock>(fake_clock_),
            std::unique_ptr<cobalt::CpuStatsFetcher>(new FakeCpuStatsFetcher()), nullptr, "tmp/")) {
    daemon_->cpu_bucket_config_ = SystemMetricsDaemon::InitializeLinearBucketConfig(
        fuchsia_system_metrics::kCpuPercentageMigratedIntBucketsFloor,
        fuchsia_system_metrics::kCpuPercentageMigratedIntBucketsNumBuckets,
        fuchsia_system_metrics::kCpuPercentageMigratedIntBucketsStepSize);
  }

  inspect::Inspector Inspector() { return daemon_->inspector_.inspector(); }

  // Run a promise to completion on the default async executor.
  void RunPromiseToCompletion(fpromise::promise<> promise) {
    bool done = false;
    executor_.schedule_task(std::move(promise).and_then([&]() { done = true; }));
    RunLoopUntilIdle();
    ASSERT_TRUE(done);
  }

  fpromise::result<inspect::Hierarchy> GetHierachyFromInspect() {
    fpromise::result<inspect::Hierarchy> hierarchy;
    RunPromiseToCompletion(inspect::ReadFromInspector(Inspector())
                               .then([&](fpromise::result<inspect::Hierarchy>& result) {
                                 hierarchy = std::move(result);
                               }));
    return hierarchy;
  }

  void TearDown() {
    std::ifstream file("tmp/activation");
    if (file) {
      EXPECT_EQ(0, std::remove("tmp/activation"));
    }
  }

  void UpdateState(fuchsia::ui::activity::State state) { daemon_->UpdateState(state); }

  zx::duration LogFuchsiaUpPing(zx::duration uptime) { return daemon_->LogFuchsiaUpPing(uptime); }

  bool LogFuchsiaLifetimeEventBoot() { return daemon_->LogFuchsiaLifetimeEventBoot(); }

  zx::duration LogActiveTime() { return daemon_->LogActiveTime(); }

  bool LogFuchsiaLifetimeEventActivation() { return daemon_->LogFuchsiaLifetimeEventActivation(); }

  zx::duration LogFuchsiaUptime() { return daemon_->LogFuchsiaUptime(); }

  void RepeatedlyLogUpPing() { daemon_->RepeatedlyLogUpPing(); }

  void LogLifetimeEvents() { daemon_->LogLifetimeEvents(); }

  void LogLifetimeEventBoot() { daemon_->LogLifetimeEventBoot(); }

  void LogLifetimeEventActivation() { daemon_->LogLifetimeEventActivation(); }

  void RepeatedlyLogUptime() { daemon_->RepeatedlyLogUptime(); }

  void RepeatedlyLogActiveTime() { daemon_->RepeatedlyLogActiveTime(); }

  zx::duration LogCpuUsage() { return daemon_->LogCpuUsage(); }

  void PrepareForLogCpuUsage() {
    daemon_->cpu_data_stored_ = 599;
    daemon_->activity_state_to_cpu_map_.clear();
    daemon_->activity_state_to_cpu_map_[fuchsia::ui::activity::State::ACTIVE][345u] = 599u;
  }

  void CheckValues(LogMetricMethod expected_log_method_invoked, size_t expected_call_count,
                   uint32_t expected_metric_id, std::vector<uint32_t> expected_last_event_codes,
                   size_t expected_event_count = 0) {
    EXPECT_EQ(expected_log_method_invoked, stub_logger_.last_log_method_invoked());
    EXPECT_EQ(expected_call_count, stub_logger_.call_count());
    EXPECT_EQ(expected_metric_id, stub_logger_.last_metric_id());
    EXPECT_THAT(stub_logger_.last_event_codes(),
                testing::ElementsAreArray(expected_last_event_codes));
    EXPECT_EQ(expected_event_count, stub_logger_.event_count());
  }

  void CheckUptimeValues(size_t expected_call_count,
                         std::vector<uint32_t> expected_last_event_codes,
                         int64_t expected_last_up_hours) {
    EXPECT_EQ(expected_call_count, stub_logger_.call_count());
    EXPECT_EQ(fuchsia_system_metrics::kFuchsiaUptimeMigratedMetricId,
              stub_logger_.last_metric_id());
    EXPECT_THAT(stub_logger_.last_event_codes(),
                testing::ElementsAreArray(expected_last_event_codes));
    EXPECT_EQ(expected_last_up_hours, stub_logger_.last_integer());
  }

  void CheckActiveTimeValues(size_t expected_call_count,
                             zx::duration expected_last_active_seconds) {
    EXPECT_EQ(cobalt::LogMetricMethod::kLogInteger, stub_logger_.last_log_method_invoked());
    EXPECT_EQ(expected_call_count, stub_logger_.call_count());
    EXPECT_EQ(fuchsia_system_metrics::kActiveTimeMetricId, stub_logger_.last_metric_id());
    EXPECT_EQ(expected_last_active_seconds.to_secs(), stub_logger_.last_integer());
  }

  void DoFuchsiaUpPingTest(zx::duration now_seconds, zx::duration expected_sleep_seconds,
                           size_t expected_call_count, uint32_t expected_last_event_code) {
    stub_logger_.reset();
    EXPECT_EQ(expected_sleep_seconds, LogFuchsiaUpPing(now_seconds));
    CheckValues(cobalt::LogMetricMethod::kLogOccurrence, expected_call_count,
                fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId, {expected_last_event_code});
  }

  void DoFuchsiaUptimeTest(zx::duration now_seconds, zx::duration expected_sleep_seconds,
                           uint32_t expected_event_code, int64_t expected_up_hours) {
    stub_logger_.reset();
    SetBootClockToDaemonStartTime();

    // Do not increment the monotonic clock. Uptime should be logged in terms of the boot clock.
    IncrementBootTime(now_seconds);
    EXPECT_EQ(expected_sleep_seconds, LogFuchsiaUptime());
    CheckUptimeValues(1u, {expected_event_code}, expected_up_hours);
  }

  // Advances our two fake clocks (one used by the SystemMetricDaemon, one used by the MessageLoop)
  // by the specified amount, and then checks to make sure that the expected metric was logged.
  void AdvanceTimeAndCheck(
      zx::duration advance_time_seconds, size_t expected_call_count, uint32_t expected_metric_id,
      std::vector<uint32_t> expected_last_event_codes,
      LogMetricMethod expected_log_method_invoked = cobalt::LogMetricMethod::kDefault) {
    bool expected_activity = (expected_call_count != 0);
    IncrementMonotonicAndBootTime(advance_time_seconds);
    EXPECT_EQ(expected_activity, RunLoopFor(advance_time_seconds));
    expected_log_method_invoked = (expected_call_count == 0 ? cobalt::LogMetricMethod::kDefault
                                                            : expected_log_method_invoked);
    CheckValues(expected_log_method_invoked, expected_call_count, expected_metric_id,
                std::move(expected_last_event_codes));
    stub_logger_.reset();
  }

  // This method is used by the test of the method RepeatedlyLogUptime(). It
  // advances our two fake clocks by the specified amount, and then checks to
  // make sure that RepeatedlyLogUptime() made the expected logging calls in the
  // meantime.
  void AdvanceAndCheckUptime(zx::duration advance_time_seconds, size_t expected_call_count,
                             std::vector<uint32_t> expected_last_event_codes,
                             int64_t expected_last_up_hours) {
    bool expected_activity = (expected_call_count != 0);
    IncrementMonotonicAndBootTime(advance_time_seconds);
    EXPECT_EQ(expected_activity, RunLoopFor(advance_time_seconds));
    if (expected_activity) {
      CheckUptimeValues(expected_call_count, std::move(expected_last_event_codes),
                        expected_last_up_hours);
    }
    stub_logger_.reset();
  }

  void AdvanceAndCheckActiveTime(zx::duration advance_time_seconds, size_t expected_call_count,
                                 zx::duration expected_active_time_seconds) {
    bool expected_activity = (expected_call_count != 0);
    IncrementMonotonicAndBootTime(advance_time_seconds);
    EXPECT_EQ(expected_activity, RunLoopFor(advance_time_seconds));
    if (expected_activity) {
      CheckActiveTimeValues(expected_call_count, expected_active_time_seconds);
    }
    stub_logger_.reset();
  }

  // Rewinds the SystemMetricsDaemon's boot clock back to the daemon's startup time.
  void SetBootClockToDaemonStartTime() { fake_clock_->SetBoot(daemon_->start_time_); }

  // The monotonic clock will never tick without the boot clock also ticking.
  void IncrementMonotonicAndBootTime(const zx::duration duration) {
    fake_clock_->SetMonotonic(fake_clock_->MonotonicNow() + duration);
    IncrementBootTime(duration);
  }

  void IncrementBootTime(const zx::duration duration) {
    fake_clock_->SetBoot(fake_clock_->BootNow() + duration);
  }

 protected:
  async::Executor executor_;
  sys::testing::ComponentContextProvider context_provider_;
  timekeeper::TestClock* fake_clock_;
  StubMetricEventLogger_Sync stub_logger_;
  std::unique_ptr<SystemMetricsDaemon> daemon_;
};

// Tests the method LogCpuUsage() and read from inspect
TEST_F(SystemMetricsDaemonTest, InspectCpuUsage) {
  stub_logger_.reset();
  PrepareForLogCpuUsage();
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  EXPECT_EQ(zx::sec(1), LogCpuUsage());
  // Call count is 1. Just one call to LogCobaltEvents, with 60 events.
  CheckValues(cobalt::LogMetricMethod::kLogMetricEvents, 1,
              fuchsia_system_metrics::kCpuPercentageMigratedMetricId, {DeviceState::Active}, 1);

  // Get hierarchy, node, and readings
  fpromise::result<inspect::Hierarchy> hierarchy = GetHierachyFromInspect();
  ASSERT_TRUE(hierarchy.is_ok());

  auto* metric_node = hierarchy.value().GetByPath({SystemMetricsDaemon::kInspecPlatformtNodeName});
  ASSERT_TRUE(metric_node);
  auto* cpu_node = metric_node->GetByPath({SystemMetricsDaemon::kCPUNodeName});
  ASSERT_TRUE(cpu_node);
  auto* cpu_max =
      cpu_node->node().get_property<inspect::DoubleArrayValue>(SystemMetricsDaemon::kReadingCPUMax);
  ASSERT_TRUE(cpu_max);

  // Expect 6 readings in the array
  EXPECT_EQ(SystemMetricsDaemon::kCPUArraySize, cpu_max->value().size());
  EXPECT_EQ(12.34, cpu_max->value()[0]);
}

// Tests the method LogFuchsiaUptime(). Uses a local FakeLogger_Sync and
// does not use FIDL. Does not use the message loop.
TEST_F(SystemMetricsDaemonTest, LogFuchsiaUptime) {
  DoFuchsiaUptimeTest(zx::sec(0), zx::sec(kHour), UptimeRange::LessThanTwoWeeks, 0);
  DoFuchsiaUptimeTest(zx::sec(kHour - 1), zx::sec(1), UptimeRange::LessThanTwoWeeks, 0);
  DoFuchsiaUptimeTest(zx::sec(5), zx::sec(kHour - 5), UptimeRange::LessThanTwoWeeks, 0);
  DoFuchsiaUptimeTest(zx::sec(kDay), zx::sec(kHour), UptimeRange::LessThanTwoWeeks, 24);
  DoFuchsiaUptimeTest(zx::sec(kDay + (6 * kHour) + 10), zx::sec(kHour - 10),
                      UptimeRange::LessThanTwoWeeks, 30);
  DoFuchsiaUptimeTest(zx::sec(kWeek), zx::sec(kHour), UptimeRange::LessThanTwoWeeks, 168);
  DoFuchsiaUptimeTest(zx::sec(kWeek), zx::sec(kHour), UptimeRange::LessThanTwoWeeks, 168);
  DoFuchsiaUptimeTest(zx::sec(2 * kWeek), zx::sec(kHour), UptimeRange::TwoWeeksOrMore, 336);
  DoFuchsiaUptimeTest(zx::sec((2 * kWeek) + (6 * kDay) + 10), zx::sec(kHour - 10),
                      UptimeRange::TwoWeeksOrMore, 480);
}

// Tests the method LogFuchsiaUpPing(). Uses a local FakeLogger_Sync and
// does not use FIDL. Does not use the message loop.
TEST_F(SystemMetricsDaemonTest, LogFuchsiaUpPing) {
  // If we were just booted, expect 1 log event of type "Up" and a return
  // value of 60 seconds.
  DoFuchsiaUpPingTest(zx::sec(0), zx::sec(60), 1, FuchsiaUpPingMigratedMetricDimensionUptime::Up);

  // If we've been up for 10 seconds, expect 1 log event of type "Up" and a
  // return value of 50 seconds.
  DoFuchsiaUpPingTest(zx::sec(10), zx::sec(50), 1, FuchsiaUpPingMigratedMetricDimensionUptime::Up);

  // If we've been up for 59 seconds, expect 1 log event of type "Up" and a
  // return value of 1 second.
  DoFuchsiaUpPingTest(zx::sec(59), zx::sec(1), 1, FuchsiaUpPingMigratedMetricDimensionUptime::Up);

  // If we've been up for 60 seconds, expect 2 log events, the second one
  // being of type UpOneMinute, and a return value of 9 minutes.
  DoFuchsiaUpPingTest(zx::sec(60), zx::min(9), 2,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneMinute);

  // If we've been up for 61 seconds, expect 2 log events, the second one
  // being of type UpOneMinute, and a return value of 9 minutes minus 1
  // second.
  DoFuchsiaUpPingTest(zx::sec(61), zx::min(9) - zx::sec(1), 2,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneMinute);

  // If we've been up for 10 minutes minus 1 second, expect 2 log events, the
  // second one being of type UpOneMinute, and a return value of 1 second.
  DoFuchsiaUpPingTest(zx::min(10) - zx::sec(1), zx::sec(1), 2,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneMinute);

  // If we've been up for 10 minutes, expect 3 log events, the
  // last one being of type UpTenMinutes, and a return value of 50 minutes.
  DoFuchsiaUpPingTest(zx::min(10), zx::min(50), 3,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpTenMinutes);

  // If we've been up for 10 minutes plus 1 second, expect 3 log events, the
  // last one being of type UpTenMinutes, and a return value of 50 minutes
  // minus one second.
  DoFuchsiaUpPingTest(zx::min(10) + zx::sec(1), zx::min(50) - zx::sec(1), 3,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpTenMinutes);

  // If we've been up for 59 minutes, expect 3 log events, the last one being
  // of type UpTenMinutes, and a return value of 1 minute
  DoFuchsiaUpPingTest(zx::min(59), zx::min(1), 3,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpTenMinutes);

  // If we've been up for 60 minutes, expect 4 log events, the last one being
  // of type UpOneHour, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::min(60), zx::hour(1), 4,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneHour);

  // If we've been up for 61 minutes, expect 4 log events, the last one being
  // of type UpOneHour, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::min(61), zx::hour(1), 4,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneHour);

  // If we've been up for 11 hours, expect 4 log events, the last one being
  // of type UpOneHour, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(11), zx::hour(1), 4,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneHour);

  // If we've been up for 12 hours, expect 5 log events, the last one being
  // of type UpTwelveHours, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(12), zx::hour(1), 5,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpTwelveHours);

  // If we've been up for 13 hours, expect 5 log events, the last one being
  // of type UpTwelveHours, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(13), zx::hour(1), 5,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpTwelveHours);

  // If we've been up for 23 hours, expect 5 log events, the last one being
  // of type UpTwelveHours, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(23), zx::hour(1), 5,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpTwelveHours);

  // If we've been up for 24 hours, expect 6 log events, the last one being
  // of type UpOneDay, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(24), zx::hour(1), 6,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneDay);

  // If we've been up for 25 hours, expect 6 log events, the last one being
  // of type UpOneDay, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(25), zx::hour(1), 6,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpOneDay);

  // If we've been up for 73 hours, expect 7 log events, the last one being
  // of type UpOneDay, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(73), zx::hour(1), 7,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpThreeDays);

  // If we've been up for 250 hours, expect 8 log events, the last one being
  // of type UpSixDays, and a return value of 1 hour
  DoFuchsiaUpPingTest(zx::hour(250), zx::hour(1), 8,
                      FuchsiaUpPingMigratedMetricDimensionUptime::UpSixDays);
}

// Tests the method LogFuchsiaLifetimeEventBoot(). Uses a local FakeLogger_Sync
// and does not use FIDL. Does not use the message loop.
TEST_F(SystemMetricsDaemonTest, LogFuchsiaLifetimeEventBoot) {
  stub_logger_.reset();

  // The first time LogFuchsiaLifetimeEventBoot() is invoked it should log 1
  // event of type "Boot" and return true indicating a successful status.
  EXPECT_EQ(true, LogFuchsiaLifetimeEventBoot());
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, 1,
              fuchsia_system_metrics::kFuchsiaLifetimeEventsMigratedMetricId,
              {FuchsiaLifetimeEventsMigratedMetricDimensionEvents::Boot});
}

// Tests the method LogActiveTime(). Uses a local FakeLogger_Sync
// and does not use FIDL. Does not use the message loop.
TEST_F(SystemMetricsDaemonTest, LogActiveTime) {
  const zx::duration one_minute(zx::min(1));
  stub_logger_.reset();

  // Initially 0
  EXPECT_EQ(zx::min(15), LogActiveTime());
  CheckActiveTimeValues(1, zx::sec(0));

  // Set ACTIVE for 1 minute.
  stub_logger_.reset();
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  IncrementBootTime(one_minute);
  LogActiveTime();
  CheckActiveTimeValues(1, one_minute);

  // Continue Active for 1 minute.
  stub_logger_.reset();
  IncrementBootTime(one_minute);
  LogActiveTime();
  CheckActiveTimeValues(1, one_minute);

  // Continue for 1 minute, then set to IDLE and continue for another minute
  stub_logger_.reset();
  IncrementBootTime(one_minute);
  UpdateState(fuchsia::ui::activity::State::IDLE);
  IncrementBootTime(one_minute);
  LogActiveTime();
  CheckActiveTimeValues(1, one_minute);

  // Set ACTIVE and IDLE alternatively and confirm that 2 minutes active time acrues.
  stub_logger_.reset();
  IncrementBootTime(one_minute);
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  IncrementBootTime(one_minute);
  UpdateState(fuchsia::ui::activity::State::IDLE);
  IncrementBootTime(one_minute);
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  IncrementBootTime(one_minute);
  UpdateState(fuchsia::ui::activity::State::IDLE);
  IncrementBootTime(one_minute);
  LogActiveTime();
  CheckActiveTimeValues(1, one_minute * 2);
}

// Tests the method RepeatedlyLogActiveTime(). This test uses the message loop to
// schedule future runs of work. Uses a local FakeLogger_Sync and does not use
// FIDL.
TEST_F(SystemMetricsDaemonTest, RepeatedlyLogActiveTime) {
  // Make sure the loop has no initial pending work.
  const zx::duration zero_seconds(0);
  RunLoopUntilIdle();
  auto seconds_to_wait = LogActiveTime();
  RepeatedlyLogActiveTime();

  // IDLE so no active time.
  stub_logger_.reset();
  AdvanceAndCheckActiveTime(seconds_to_wait, 1, zero_seconds);

  // Active for whole time.
  stub_logger_.reset();
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  AdvanceAndCheckActiveTime(seconds_to_wait, 1, seconds_to_wait);

  // Still active so whole time.
  stub_logger_.reset();
  AdvanceAndCheckActiveTime(seconds_to_wait, 1, seconds_to_wait);

  // IDLE again, so no active time.
  stub_logger_.reset();
  UpdateState(fuchsia::ui::activity::State::IDLE);
  AdvanceAndCheckActiveTime(seconds_to_wait, 1, zero_seconds);

  // IDLE, ACTIVE, IDLE, ACTIVE, IDLE. Active for 10 seconds.
  const zx::duration five_seconds(zx::sec(5));
  ASSERT_LT(4 * five_seconds, seconds_to_wait);

  zx::duration run_time = zx::sec(0);
  stub_logger_.reset();
  UpdateState(fuchsia::ui::activity::State::IDLE);
  AdvanceAndCheckActiveTime(five_seconds, 0, zero_seconds);
  run_time += five_seconds;

  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  AdvanceAndCheckActiveTime(five_seconds, 0, zero_seconds);
  run_time += five_seconds;

  UpdateState(fuchsia::ui::activity::State::IDLE);
  AdvanceAndCheckActiveTime(five_seconds, 0, zero_seconds);
  run_time += five_seconds;

  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  AdvanceAndCheckActiveTime(five_seconds, 0, zero_seconds);
  run_time += five_seconds;

  UpdateState(fuchsia::ui::activity::State::IDLE);
  AdvanceAndCheckActiveTime(seconds_to_wait - run_time, 1, 2 * five_seconds);
}

// Tests that RepeatedlyLogActiveTime() computes durations using the boot clock but sleeps based on
// the monotonic clock.
TEST_F(SystemMetricsDaemonTest, RepeatedlyLogActiveTimeLogsBootTime) {
  const zx::duration seconds_to_wait = LogActiveTime();
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  RepeatedlyLogActiveTime();
  RunLoopUntilIdle();

  stub_logger_.reset();
  IncrementBootTime(seconds_to_wait);
  EXPECT_FALSE(RunLoopUntilIdle());

  AdvanceAndCheckActiveTime(/*advance_time_seconds=*/seconds_to_wait, /*expected_call_count=*/1,
                            /*expected_active_time_seconds=*/2 * seconds_to_wait);
}

// Tests the method LogFuchsiaLifetimeEventActivation(). Uses a local FakeLogger_Sync
// and does not use FIDL. Does not use the message loop.
TEST_F(SystemMetricsDaemonTest, LogFuchsiaLifetimeEventActivation) {
  stub_logger_.reset();
  // The first time LogFuchsiaLifetimeEventActivation() is invoked it should log 1
  // event of type "Activation" and return true indicating a successful status.
  EXPECT_EQ(true, LogFuchsiaLifetimeEventActivation());
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, 1,
              fuchsia_system_metrics::kFuchsiaLifetimeEventsMigratedMetricId,
              {FuchsiaLifetimeEventsMigratedMetricDimensionEvents::Activation});
  stub_logger_.reset();

  // The second time LogFuchsiaLifetimeEventActivation() it should log zero events
  // and return true to indicate successful completion.
  EXPECT_EQ(true, LogFuchsiaLifetimeEventActivation());
  CheckValues(cobalt::LogMetricMethod::kDefault, 0, -1, {});
}

// Tests the method RepeatedlyLogUptime(). This test uses the message loop to
// schedule future runs of work. Uses a local FakeLogger_Sync and does not use
// FIDL.
TEST_F(SystemMetricsDaemonTest, RepeatedlyLogUptime) {
  RunLoopUntilIdle();

  // Invoke the method under test. This should cause the uptime to be logged
  // once, and schedules the next run for approximately 1 hour in the future.
  // (More precisely, the next run should occur in 1 hour minus the amount of
  // time after the daemon's start time which this method is invoked.)
  RepeatedlyLogUptime();

  // The first event should have been logged, with an uptime of 0 hours.
  CheckUptimeValues(1u, {UptimeRange::LessThanTwoWeeks}, 0);
  stub_logger_.reset();

  // Advance the clock by 30 seconds. Nothing should have happened.
  AdvanceAndCheckUptime(zx::sec(30), 0, {}, -1);

  // Advance the clock to the next hour. The system metrics daemon has been up
  // for 1 hour by now, so the second event should have been logged.
  AdvanceAndCheckUptime(zx::sec(kHour - 30), 1, {UptimeRange::LessThanTwoWeeks}, 1);

  // Advance the clock by 1 day. At this point, the daemon has been up for 25
  // hours. Since the last time we checked |stub_logger_|, the daemon should
  // have logged the uptime 24 times, with the most recent value equal to 25.
  AdvanceAndCheckUptime(zx::sec(kDay), 24, {UptimeRange::LessThanTwoWeeks}, 25);

  // Advance the clock by 1 week. At this point, the daemon has been up for 8
  // days + 1 hour. Since the last time we checked |stub_logger_|, the daemon
  // should have logged the uptime 168 times, with the most recent value equal
  // to 193.
  AdvanceAndCheckUptime(zx::sec(kWeek), 168, {UptimeRange::LessThanTwoWeeks}, 193);

  // Advance the clock 1 more week. At this point, the daemon has been up for
  // 15 days + 1 hour. Since the last time we checked |stub_logger_|, the daemon
  // should have logged the uptime 168 times, with the most recent value equal
  // to 361.
  AdvanceAndCheckUptime(zx::sec(kWeek), 168, {UptimeRange::TwoWeeksOrMore}, 361);
}

// Tests that RepeatedlyLogUptime() logs values from the boot clock but sleeps based on the
// monotonic clock.
TEST_F(SystemMetricsDaemonTest, RepeatedlyLogUptimeLogsBootTime) {
  RunLoopUntilIdle();

  // Invoke the method under test. This should cause the uptime to be logged
  // once, and schedules the next run for approximately 1 hour in the future.
  // (More precisely, the next run should occur in 1 hour minus the amount of
  // time after the daemon's start time which this method is invoked.)
  RepeatedlyLogUptime();

  // The first event should have been logged, with an uptime of 0 hours.
  CheckUptimeValues(/*expected_call_count=*/1u, {UptimeRange::LessThanTwoWeeks},
                    /*expected_last_up_hours=*/0);
  stub_logger_.reset();

  // Advance the boot clock by 1 hour. Nothing should have happened.
  IncrementBootTime(zx::hour(1));
  EXPECT_FALSE(RunLoopUntilIdle());

  // Advance the monotonic and boot clocks by 1 hour. The system metrics daemon has been up for 2
  // hours, so 1 event with an uptime of 2 hours should have been logged.
  IncrementMonotonicAndBootTime(zx::hour(1));
  EXPECT_TRUE(RunLoopFor(zx::hour(1)));
  CheckUptimeValues(/*expected_call_count=*/1u, {UptimeRange::LessThanTwoWeeks},
                    /*expected_last_up_hours=*/2);
}

// Tests the method RepeatedlyLogUpPing(). This test differs
// from the previous ones because it makes use of the message loop in order to
// schedule future runs of work. Uses a local FakeLogger_Sync and does not use
// FIDL.
TEST_F(SystemMetricsDaemonTest, RepeatedlyLogUpPing) {
  // Make sure the loop has no initial pending work.
  RunLoopUntilIdle();

  // Invoke the method under test. This kicks of the first run and schedules
  // the second run for 1 minute plus 5 seconds in the future.
  RepeatedlyLogUpPing();

  // The initial event should have been logged.
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, 1,
              fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId,
              {FuchsiaUpPingMigratedMetricDimensionUptime::Up});
  stub_logger_.reset();

  // Advance the clock by 30 seconds. Nothing should have happened.
  AdvanceTimeAndCheck(zx::sec(30), 0, -1, {}, cobalt::LogMetricMethod::kLogOccurrence);
  // Advance the clock by 30 seconds again. Nothing should have happened
  // because the first run of RepeatedlyLogUpPing() added a 5
  // second buffer to the next scheduled run time.
  AdvanceTimeAndCheck(zx::sec(30), 0, -1, {}, cobalt::LogMetricMethod::kLogOccurrence);

  // Advance the clock by 5 seconds to t=65s. Now expect the second batch
  // of work to occur. This consists of two events the second of which is
  // |UpOneMinute|. The third batch of work should be schedule for
  // t = 10m + 5s.
  AdvanceTimeAndCheck(zx::sec(5), 2, fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId,
                      {FuchsiaUpPingMigratedMetricDimensionUptime::UpOneMinute},
                      cobalt::LogMetricMethod::kLogOccurrence);

  // Advance the clock to t=10m. Nothing should have happened because the
  // previous round added a 5s buffer.
  AdvanceTimeAndCheck(zx::min(10) - zx::sec(65), 0, -1, {},
                      cobalt::LogMetricMethod::kLogOccurrence);

  // Advance the clock 5 s to t=10m + 5s. Now expect the third batch of
  // work to occur. This consists of three events the second of which is
  // |UpTenMinutes|. The fourth batch of work should be scheduled for
  // t = 1 hour + 5s.
  AdvanceTimeAndCheck(zx::sec(5), 3, fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId,
                      {FuchsiaUpPingMigratedMetricDimensionUptime::UpTenMinutes},
                      cobalt::LogMetricMethod::kLogOccurrence);

  // Advance the clock to t=1h. Nothing should have happened because the
  // previous round added a 5s buffer.
  AdvanceTimeAndCheck(zx::min(60) - (zx::min(10) + zx::sec(5)), 0, -1, {},
                      cobalt::LogMetricMethod::kLogOccurrence);

  // Advance the clock 5 s to t=1h + 5s. Now expect the fourth batch of
  // work to occur. This consists of 4 events the last of which is
  // |UpOneHour|.
  AdvanceTimeAndCheck(zx::sec(5), 4, fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId,
                      {FuchsiaUpPingMigratedMetricDimensionUptime::UpOneHour},
                      cobalt::LogMetricMethod::kLogOccurrence);
}

// Tests that RepeatedlyLogUpPing() logs values from the boot clock but sleeps based on the
// monotonic clock.
TEST_F(SystemMetricsDaemonTest, RepeatedlyLogUpPingLogsBootTime) {
  // Invoke the method under test. This kicks of the first run and schedules
  // the second run for 1 minute plus 5 seconds in the future.
  RepeatedlyLogUpPing();

  // The initial event should have been logged.
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, /*expected_call_count=*/1,
              fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId,
              {FuchsiaUpPingMigratedMetricDimensionUptime::Up});
  stub_logger_.reset();

  // Advance the monotonic clock by 65 seconds, but the boot clock by an extra 10 minutes. Expect
  // the second batch of work to occur. This consists of three events the last of which is
  // |UpTenMinutes|.
  IncrementBootTime(zx::min(10));
  AdvanceTimeAndCheck(zx::sec(65), /*expected_call_count=*/3,
                      fuchsia_system_metrics::kFuchsiaUpPingMigratedMetricId,
                      {FuchsiaUpPingMigratedMetricDimensionUptime::UpTenMinutes},
                      cobalt::LogMetricMethod::kLogOccurrence);
}

// Tests the method LogLifetimeEvents(). This test differs
// from the previous ones because it makes use of the message loop in order to
// schedule future runs of work. Uses a local FakeLogger_Sync and does not use
// FIDL.
TEST_F(SystemMetricsDaemonTest, LogLifetimeEvents) {
  // Make sure the loop has no initial pending work.
  RunLoopUntilIdle();

  // Invoke the method under test. This kicks of the first run and schedules the
  // second run.
  LogLifetimeEvents();

  // Two initial events should be logged, one for Activation and one for Boot.
  // Activation is the last event logged.
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, 2,
              fuchsia_system_metrics::kFuchsiaLifetimeEventsMigratedMetricId,
              {FuchsiaLifetimeEventsMigratedMetricDimensionEvents::Activation});
}

// Tests the method LogLifetimeEventActivation(). This test differs
// from the previous ones because it makes use of the message loop in order to
// schedule future runs of work. Uses a local FakeLogger_Sync and does not use
// FIDL.
TEST_F(SystemMetricsDaemonTest, LogLifetimeEventActivation) {
  // Make sure the loop has no initial pending work.
  RunLoopUntilIdle();

  // Invoke the method under test. This kicks of the first run and schedules the
  // second run.
  LogLifetimeEventActivation();

  // The initial event should have been logged.
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, 1,
              fuchsia_system_metrics::kFuchsiaLifetimeEventsMigratedMetricId,
              {FuchsiaLifetimeEventsMigratedMetricDimensionEvents::Activation});
  stub_logger_.reset();

  // Advance the clock by 2 hours. Nothing should have happened.
  AdvanceTimeAndCheck(zx::hour(2), 0, -1, {}, cobalt::LogMetricMethod::kLogOccurrence);
}

// Tests the method LogLifetimeEventBoot(). This test differs
// from the previous ones because it makes use of the message loop in order to
// schedule future runs of work. Uses a local FakeLogger_Sync and does not use
// FIDL.
TEST_F(SystemMetricsDaemonTest, LogLifetimeEventBoot) {
  // Make sure the loop has no initial pending work.
  RunLoopUntilIdle();

  // Invoke the method under test. This kicks of the first run and schedules the
  // second run.
  LogLifetimeEventBoot();

  // The initial event should have been logged.
  CheckValues(cobalt::LogMetricMethod::kLogOccurrence, 1,
              fuchsia_system_metrics::kFuchsiaLifetimeEventsMigratedMetricId,
              {FuchsiaLifetimeEventsMigratedMetricDimensionEvents::Boot});
  stub_logger_.reset();

  // Advance the clock by 2 hours. Nothing should have happened.
  AdvanceTimeAndCheck(zx::hour(2), 0, -1, {}, cobalt::LogMetricMethod::kLogOccurrence);
}

// Tests the method LogCpuUsage(). Uses a local FakeLogger_Sync and
// does not use FIDL. Does not use the message loop.
TEST_F(SystemMetricsDaemonTest, LogCpuUsage) {
  stub_logger_.reset();
  PrepareForLogCpuUsage();
  UpdateState(fuchsia::ui::activity::State::ACTIVE);
  EXPECT_EQ(zx::sec(1), LogCpuUsage());
  // Call count is 1. Just one call to LogCobaltEvents, with 60 events.
  CheckValues(cobalt::LogMetricMethod::kLogMetricEvents, 1,
              fuchsia_system_metrics::kCpuPercentageMigratedMetricId, {DeviceState::Active}, 1);
}

class MockLogger : public ::fuchsia::metrics::testing::MetricEventLogger_TestBase {
 public:
  void LogMetricEvents(std::vector<fuchsia::metrics::MetricEvent> events,
                       LogMetricEventsCallback callback) override {
    num_calls_++;
    num_events_ += events.size();
    callback(fpromise::ok());
  }
  void LogOccurrence(uint32_t metric_id, uint64_t count, std::vector<uint32_t> event_codes,
                     LogOccurrenceCallback callback) override {
    num_calls_++;
    num_events_ += 1;
    callback(fpromise::ok());
  }
  void NotImplemented_(const std::string& name) override {
    ASSERT_TRUE(false) << name << " is not implemented";
  }
  int num_calls() const { return num_calls_; }
  int num_events() const { return num_events_; }

 private:
  int num_calls_ = 0;
  int num_events_ = 0;
};

class MockLoggerFactory : public ::fuchsia::metrics::testing::MetricEventLoggerFactory_TestBase {
 public:
  MockLogger* logger() { return logger_.get(); }
  uint32_t received_project_id() const { return received_project_id_; }

  void CreateMetricEventLogger(fuchsia::metrics::ProjectSpec project,
                               ::fidl::InterfaceRequest<fuchsia::metrics::MetricEventLogger> logger,
                               CreateMetricEventLoggerCallback callback) override {
    received_project_id_ = project.project_id();
    logger_.reset(new MockLogger());
    logger_bindings_.AddBinding(logger_.get(), std::move(logger));
    callback(fpromise::ok());
  }

  void NotImplemented_(const std::string& name) override {
    ASSERT_TRUE(false) << name << " is not implemented";
  }

 private:
  uint32_t received_project_id_;
  std::unique_ptr<MockLogger> logger_;
  fidl::BindingSet<fuchsia::metrics::MetricEventLogger> logger_bindings_;
};

class SystemMetricsDaemonInitializationTest : public gtest::TestLoopFixture {
 public:
  ~SystemMetricsDaemonInitializationTest() override = default;

  bool LogFuchsiaLifetimeEvent() {
    // The SystemMetricsDaemon will make asynchronous calls to the MockLogger*s that are also
    // running in this class/tests thread. So the call to the SystemMetricsDaemon needs to be made
    // on a different thread, such that the MockLogger*s running on the main thread can respond to
    // those calls.
    std::future<bool> result =
        std::async([this]() { return daemon_->LogFuchsiaLifetimeEventBoot(); });
    while (result.wait_for(std::chrono::milliseconds(1)) != std::future_status::ready) {
      // Run the main thread's loop, allowing the MockLogger* objects to respond to requests.
      RunLoopUntilIdle();
    }
    return result.get();
  }

 protected:
  void SetUp() override {
    // Create a MockLoggerFactory and add it to the services the fake context can provide.
    auto service_provider = context_provider_.service_directory_provider();
    logger_factory_ = new MockLoggerFactory();
    service_provider->AddService(factory_bindings_.GetHandler(logger_factory_, dispatcher()));

    // Initialize the SystemMetricsDaemon with the fake context, and other fakes.
    daemon_ = std::unique_ptr<SystemMetricsDaemon>(new SystemMetricsDaemon(
        dispatcher(), context_provider_.context(), nullptr,
        std::unique_ptr<timekeeper::TestClock>(fake_clock_),
        std::unique_ptr<cobalt::CpuStatsFetcher>(new FakeCpuStatsFetcher()), nullptr, "/tmp"));
  }

  // Note that we first save an unprotected pointer in fake_clock_ and then
  // give ownership of the pointer to daemon_.
  timekeeper::TestClock* fake_clock_ = new timekeeper::TestClock();
  std::unique_ptr<SystemMetricsDaemon> daemon_;

  MockLoggerFactory* logger_factory_;
  fidl::BindingSet<fuchsia::metrics::MetricEventLoggerFactory> factory_bindings_;
  sys::testing::ComponentContextProvider context_provider_;
};

// Tests the initialization of a new SystemMetricsDaemon's connection to the Cobalt FIDL objects.
TEST_F(SystemMetricsDaemonInitializationTest, LogSomethingAnything) {
  // Make sure the Logger has not been initialized yet.
  EXPECT_EQ(0u, logger_factory_->received_project_id());
  EXPECT_EQ(nullptr, logger_factory_->logger());

  // When LogFuchsiaLifetimeEvent() is invoked the first time, it connects to the LoggerFactory,
  // gets a logger, and returns false to indicate the logging failed and should
  // be retried.
  EXPECT_EQ(false, LogFuchsiaLifetimeEvent());

  // Make sure the Logger has now been initialized, and for the correct project, but has not yet
  // logged anything.
  EXPECT_EQ(fuchsia_system_metrics::kProjectId, logger_factory_->received_project_id());
  ASSERT_NE(nullptr, logger_factory_->logger());
  EXPECT_EQ(0, logger_factory_->logger()->num_calls());

  // Second call to LogFuchsiaLifetimeEvent() succeeds at logging the metric, and returns
  // success.
  EXPECT_EQ(true, LogFuchsiaLifetimeEvent());
  EXPECT_EQ(1, logger_factory_->logger()->num_calls());
}
