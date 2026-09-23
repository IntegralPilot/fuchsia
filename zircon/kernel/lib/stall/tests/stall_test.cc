// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/stall.h>
#include <lib/unittest/unittest.h>

#include <ktl/enforce.h>

class StallAccumulatorTests {
 public:
  // Verifies that non-stalling threads only generate `active` time while running.
  static bool NonStallingTimeTest() {
    BEGIN_TEST;

    StallAccumulator accumulator;

    // Progressing non-stalling time should make only the `active` timer grow.
    accumulator.Update(+1, 0);
    spin(1'000);
    accumulator.Update(-1, 0);
    StallAccumulator::Stats stats1 = accumulator.Flush();
    EXPECT_LE(1'000'000, stats1.total_time_active);
    EXPECT_EQ(0, stats1.total_time_stall_some);
    EXPECT_EQ(0, stats1.total_time_stall_full);

    // Non-progressing non-stalling time should not make any timer grow.
    spin(1'000);
    StallAccumulator::Stats stats2 = accumulator.Flush();
    EXPECT_EQ(0, stats2.total_time_active);
    EXPECT_EQ(0, stats2.total_time_stall_some);
    EXPECT_EQ(0, stats2.total_time_stall_full);

    END_TEST;
  }

  // Verifies that the `some` and `full` stall timers grow with stalling threads.
  static bool StallingTimeTest() {
    BEGIN_TEST;

    StallAccumulator accumulator;

    // Verifies that a single stalling thread makes all the timers grow.
    // Checks: active == some == full >= 1 ms.
    accumulator.Update(0, +1);
    spin(1'000);
    StallAccumulator::Stats stats1 = accumulator.Flush();
    EXPECT_EQ(stats1.total_time_active, stats1.total_time_stall_some);
    EXPECT_EQ(stats1.total_time_stall_some, stats1.total_time_stall_full);
    EXPECT_GE(stats1.total_time_stall_full, 1'000'000);

    // Verifies that adding a second non-stalling thread makes `full` stop growing, but not the
    // others. Checks: active == some >= 1 ms && (some - 1 ms) >= full
    accumulator.Update(+1, 0);
    spin(1'000);
    StallAccumulator::Stats stats2 = accumulator.Flush();
    EXPECT_EQ(stats2.total_time_active, stats2.total_time_stall_some);
    ASSERT_GE(stats2.total_time_stall_some, 1'000'000);
    EXPECT_GE(stats2.total_time_stall_some - 1'000'000, stats2.total_time_stall_full);

    // Cleanup.
    accumulator.Update(-1, -1);

    END_TEST;
  }
};

class StallAggregatorTests {
 public:
  // Verifies that the aggregated stats are the average of per-CPU stats, weighted by `active` time.
  static bool MultiCpuMergeTest() {
    BEGIN_TEST;

    // Simulate a two-CPU system in which the second CPU has 3 times the `active` time of the first
    // one.
    StallAggregator aggregator;
    aggregator.SampleOnce([](StallAggregator::PerCpuStatsCallback callback) {
      callback(StallAccumulator::Stats{
          .total_time_stall_some = 3000,
          .total_time_stall_full = 300,
          .total_time_active = 10000,
      });
      callback(StallAccumulator::Stats{
          .total_time_stall_some = 4000,
          .total_time_stall_full = 400,
          .total_time_active = 30000,
      });
    });
    StallAggregator::Stats stats = aggregator.ReadStats();
    EXPECT_EQ((3000 * 1 + 4000 * 3) / (1 + 3), stats.stalled_time_some);
    EXPECT_EQ((300 * 1 + 400 * 3) / (1 + 3), stats.stalled_time_full);

    END_TEST;
  }

  // Verifies that global stats keep increasing by simulating a system with one CPU, and verifying
  // that the global stats correspond to the running total of values it reports.
  static bool TimersDoNotRestartTest() {
    BEGIN_TEST;

    StallAggregator aggregator;

    aggregator.SampleOnce([](StallAggregator::PerCpuStatsCallback callback) {
      callback(StallAccumulator::Stats{
          .total_time_stall_some = 200,
          .total_time_stall_full = 150,
          .total_time_active = 1000,
      });
    });
    StallAggregator::Stats stats1 = aggregator.ReadStats();
    EXPECT_EQ(200, stats1.stalled_time_some);
    EXPECT_EQ(150, stats1.stalled_time_full);

    aggregator.SampleOnce([](StallAggregator::PerCpuStatsCallback callback) {
      callback(StallAccumulator::Stats{
          .total_time_stall_some = 500,
          .total_time_stall_full = 10,
          .total_time_active = 4000,
      });
    });
    StallAggregator::Stats stats2 = aggregator.ReadStats();
    EXPECT_EQ(200 + 500, stats2.stalled_time_some);
    EXPECT_EQ(150 + 10, stats2.stalled_time_full);

    END_TEST;
  }

  // Verifies that, if the overall `active` time of all CPUs is zero (i.e. if the denominator of the
  // weighted average is zero), the resulting stall timers are simply left unchanged.
  static bool IdleSystemTest() {
    BEGIN_TEST;

    StallAggregator aggregator;

    // Simulate two CPUs, both totally idle.
    aggregator.SampleOnce([](StallAggregator::PerCpuStatsCallback callback) {
      callback(StallAccumulator::Stats{
          .total_time_stall_some = 0,
          .total_time_stall_full = 0,
          .total_time_active = 0,
      });
      callback(StallAccumulator::Stats{
          .total_time_stall_some = 0,
          .total_time_stall_full = 0,
          .total_time_active = 0,
      });
    });
    StallAggregator::Stats stats = aggregator.ReadStats();
    EXPECT_EQ(0, stats.stalled_time_some);
    EXPECT_EQ(0, stats.stalled_time_full);

    END_TEST;
  }

  // An implementation of the StallObserver::EventReceiver interface that simply remembers the
  // last state it received.
  class StallObserverTestEventReceiver : public StallObserver::EventReceiver {
   public:
    bool is_above_threshold = false;

   private:
    void OnAboveThreshold() override { is_above_threshold = true; }
    void OnBelowThreshold() override { is_above_threshold = false; }
  };

  enum class ObserverThresholdTestSelector { Some, Full };

  // Verifies that the StallObserver receives data from the StallAggregator and that it applies the
  // threshold correctly. Depending on the template argument, this test will exercise either the
  // "some" or "full" counter.
  template <ObserverThresholdTestSelector test_variant>
  static bool ObserverThresholdTest() {
    BEGIN_TEST;

    StallAggregator aggregator;

    // Create an observer with:
    // - threshold = 50% of the duration of one sample
    // - window = 4 samples
    StallObserverTestEventReceiver receiver;
    ktl::unique_ptr<StallObserver> observer =
        CreateStallObserverForTest(kStallSampleInterval / 2, kStallSampleInterval * 4, &receiver);

    // Register it either as observing "some" or "full", depending on the `test_variant` we are
    // executing.
    fit::deferred_action<fit::closure> cleanup;
    switch (test_variant) {
      case ObserverThresholdTestSelector::Some:
        aggregator.AddObserverSome(observer.get());
        cleanup = fit::defer<fit::closure>([&] { aggregator.RemoveObserverSome(observer.get()); });
        break;
      case ObserverThresholdTestSelector::Full:
        aggregator.AddObserverFull(observer.get());
        cleanup = fit::defer<fit::closure>([&] { aggregator.RemoveObserverFull(observer.get()); });
        break;
    }

    // Prepare two kinds of samples: one reporting no stalls and another one reporting a 25% stall.
    static const StallAccumulator::Stats kSampleStallNone{
        .total_time_stall_some = 0,
        .total_time_stall_full = 0,
        .total_time_active = kStallSampleInterval,
    };
    static const StallAccumulator::Stats kSampleStall25Percent{
        .total_time_stall_some = kStallSampleInterval / 4,
        .total_time_stall_full =
            test_variant == ObserverThresholdTestSelector::Full ? (kStallSampleInterval / 4) : 0,
        .total_time_active = kStallSampleInterval,
    };

    // Verify that the observer is initially not signaled.
    EXPECT_FALSE(receiver.is_above_threshold);

    // Report a sample with a 25% stall time. It should stay unsignaled because we are still below
    // the threshold.
    aggregator.SampleOnce(
        [](StallAggregator::PerCpuStatsCallback callback) { callback(kSampleStall25Percent); });
    EXPECT_FALSE(receiver.is_above_threshold);

    // Report a second sample with 25% stall time. This time, the cumulated stall time crosses the
    // threshold. Verify that it became signaled.
    aggregator.SampleOnce(
        [](StallAggregator::PerCpuStatsCallback callback) { callback(kSampleStall25Percent); });
    EXPECT_TRUE(receiver.is_above_threshold);

    // Now report two more samples without any stall time. It should stay signalled, because we are
    // still above threshold within the observation window (4 samples).
    aggregator.SampleOnce(
        [](StallAggregator::PerCpuStatsCallback callback) { callback(kSampleStallNone); });
    EXPECT_TRUE(receiver.is_above_threshold);
    aggregator.SampleOnce(
        [](StallAggregator::PerCpuStatsCallback callback) { callback(kSampleStallNone); });
    EXPECT_TRUE(receiver.is_above_threshold);

    // Report one final sample without stall. This time, the cumulated stall time will go back to
    // 25% over the last 4 samples, and the observer will become unsignaled.
    aggregator.SampleOnce(
        [](StallAggregator::PerCpuStatsCallback callback) { callback(kSampleStallNone); });
    EXPECT_FALSE(receiver.is_above_threshold);

    END_TEST;
  }

  // Verifies that all registered observers receive the updates.
  static bool ObserversManyTest() {
    BEGIN_TEST;

    StallAggregator aggregator;

    // Create 4 observers with the smallest possible threshold.
    StallObserverTestEventReceiver receiver1, receiver2, receiver3, receiver4;
    ktl::unique_ptr<StallObserver> observer1 =
        CreateStallObserverForTest(1, kStallSampleInterval, &receiver1);
    ktl::unique_ptr<StallObserver> observer2 =
        CreateStallObserverForTest(1, kStallSampleInterval, &receiver2);
    ktl::unique_ptr<StallObserver> observer3 =
        CreateStallObserverForTest(1, kStallSampleInterval, &receiver3);
    ktl::unique_ptr<StallObserver> observer4 =
        CreateStallObserverForTest(1, kStallSampleInterval, &receiver4);

    // Register them.
    aggregator.AddObserverSome(observer1.get());
    aggregator.AddObserverSome(observer2.get());
    aggregator.AddObserverFull(observer3.get());
    aggregator.AddObserverFull(observer4.get());
    auto cleanup = fit::defer([&] {
      aggregator.RemoveObserverSome(observer1.get());
      aggregator.RemoveObserverSome(observer2.get());
      aggregator.RemoveObserverFull(observer3.get());
      aggregator.RemoveObserverFull(observer4.get());
    });

    // Verify that the observers are initially not signaled.
    EXPECT_FALSE(receiver1.is_above_threshold);
    EXPECT_FALSE(receiver2.is_above_threshold);
    EXPECT_FALSE(receiver3.is_above_threshold);
    EXPECT_FALSE(receiver4.is_above_threshold);

    // Verify that all of them become triggered when a stall occurs.
    aggregator.SampleOnce([](StallAggregator::PerCpuStatsCallback callback) {
      callback({
          .total_time_stall_some = kStallSampleInterval,
          .total_time_stall_full = kStallSampleInterval,
          .total_time_active = kStallSampleInterval,
      });
    });
    EXPECT_TRUE(receiver1.is_above_threshold);
    EXPECT_TRUE(receiver2.is_above_threshold);
    EXPECT_TRUE(receiver3.is_above_threshold);
    EXPECT_TRUE(receiver4.is_above_threshold);

    END_TEST;
  }

 private:
  static ktl::unique_ptr<StallObserver> CreateStallObserverForTest(
      zx_duration_mono_t threshold, zx_duration_mono_t window,
      StallObserver::EventReceiver *event_receiver) {
    zx::result<ktl::unique_ptr<StallObserver>> result =
        StallObserver::Create(threshold, window, event_receiver);
    ZX_ASSERT(result.is_ok());
    return ktl::move(*result);
  }
};

UNITTEST_START_TESTCASE(stall_tests)
UNITTEST("StallAccumulator - non-stalling time", StallAccumulatorTests::NonStallingTimeTest)
UNITTEST("StallAccumulator - stalling time", StallAccumulatorTests::StallingTimeTest)
UNITTEST("StallAggregator - timers do not restart", StallAggregatorTests::TimersDoNotRestartTest)
UNITTEST("StallAggregator - merge data from multiple CPUs", StallAggregatorTests::MultiCpuMergeTest)
UNITTEST("StallAggregator - idle system", StallAggregatorTests::IdleSystemTest)
UNITTEST("StallAggregator - some observer threshold",
         StallAggregatorTests::ObserverThresholdTest<
             StallAggregatorTests::ObserverThresholdTestSelector::Some>)
UNITTEST("StallAggregator - full observer threshold",
         StallAggregatorTests::ObserverThresholdTest<
             StallAggregatorTests::ObserverThresholdTestSelector::Full>)
UNITTEST("StallAggregator - multiple observers", StallAggregatorTests::ObserversManyTest)
UNITTEST_END_TESTCASE(stall_tests, "stall", "pressure stall tests")
