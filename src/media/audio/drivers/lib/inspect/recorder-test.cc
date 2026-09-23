// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/lib/inspect/recorder.h"

#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace audio {
namespace {

using ::inspect::testing::BoolIs;
using ::inspect::testing::ChildrenMatch;
using ::inspect::testing::DoubleIs;
using ::inspect::testing::IntIs;
using ::inspect::testing::NameMatches;
using ::inspect::testing::NodeMatches;
using ::inspect::testing::PropertyList;
using ::inspect::testing::UintIs;
using ::testing::AllOf;
using ::testing::DoubleEq;
using ::testing::ElementsAre;
using ::testing::IsSupersetOf;
using ::testing::UnorderedElementsAre;

class RecorderTest : public gtest::RealLoopFixture {
 protected:
  void SetUp() override {
    gtest::RealLoopFixture::SetUp();
    inspector_ =
        std::make_unique<inspect::ComponentInspector>(dispatcher(), inspect::PublishOptions{});
    recorder_ = std::make_unique<Recorder>(inspector_->root());
  }

  inspect::Hierarchy GetHierarchy() {
    fpromise::result<inspect::Hierarchy> result =
        RunPromise(inspect::ReadFromInspector(inspector_->inspector()));
    EXPECT_TRUE(result.is_ok());
    return std::move(result.value());
  }

  std::unique_ptr<Recorder>& recorder() { return recorder_; }

 private:
  std::unique_ptr<inspect::ComponentInspector> inspector_;
  std::unique_ptr<Recorder> recorder_;
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(RecorderTest, PowerTransitions) {
  zx::time called_at = zx::time(100);
  zx::time completed_at = zx::time(200);

  recorder()->RecordSocPowerUp(called_at, completed_at);
  ASSERT_THAT(
      GetHierarchy(),
      AllOf(NodeMatches(PropertyList(ElementsAre(BoolIs(std::string(kCurrentPowerState), true)))),
            ChildrenMatch(UnorderedElementsAre(
                AllOf(NodeMatches(NameMatches(std::string(kPowerTransitions))),
                      ChildrenMatch(UnorderedElementsAre(
                          NodeMatches(AllOf(NameMatches(std::string("0")),
                                            PropertyList(UnorderedElementsAre(
                                                BoolIs(std::string(kPowerState), true),
                                                IntIs(std::string(kCalledAt), 100),
                                                IntIs(std::string(kEffectiveAt), 200)))))))),
                NodeMatches(NameMatches(std::string(kDAIs))),
                NodeMatches(NameMatches(std::string(kRingBuffers)))))));

  called_at = zx::time(300);
  completed_at = zx::time(400);
  recorder()->RecordSocPowerDown(called_at, completed_at);
  ASSERT_THAT(
      GetHierarchy(),
      AllOf(NodeMatches(PropertyList(ElementsAre(BoolIs(std::string(kCurrentPowerState), false)))),
            ChildrenMatch(UnorderedElementsAre(
                AllOf(NodeMatches(NameMatches(std::string(kPowerTransitions))),
                      ChildrenMatch(UnorderedElementsAre(
                          NodeMatches(AllOf(NameMatches(std::string("0")),
                                            PropertyList(UnorderedElementsAre(
                                                BoolIs(std::string(kPowerState), true),
                                                IntIs(std::string(kCalledAt), 100),
                                                IntIs(std::string(kEffectiveAt), 200))))),
                          NodeMatches(AllOf(NameMatches(std::string("1")),
                                            PropertyList(UnorderedElementsAre(
                                                BoolIs(std::string(kPowerState), false),
                                                IntIs(std::string(kCalledAt), 300),
                                                IntIs(std::string(kEffectiveAt), 400)))))))),
                NodeMatches(NameMatches(std::string(kDAIs))),
                NodeMatches(NameMatches(std::string(kRingBuffers)))))));
}

TEST_F(RecorderTest, RingBufferAndDaiPopulation) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  recorder()->PopulateDai("output_dai", 2);

  ASSERT_THAT(
      GetHierarchy(),
      ChildrenMatch(UnorderedElementsAre(
          AllOf(NodeMatches(NameMatches(std::string(kRingBuffers))),
                ChildrenMatch(UnorderedElementsAre(
                    NodeMatches(AllOf(NameMatches(std::string("test_ring_buffer")),
                                      PropertyList(IsSupersetOf({
                                          UintIs(std::string(kElementId), 1),
                                          BoolIs(std::string(kSupportsActiveChannels), true),
                                          BoolIs(std::string(kIsOutgoingStream), true),
                                      }))))))),
          AllOf(NodeMatches(NameMatches(std::string(kDAIs))),
                ChildrenMatch(UnorderedElementsAre(NodeMatches(AllOf(
                    NameMatches(std::string("output_dai")),
                    PropertyList(UnorderedElementsAre(UintIs(std::string(kElementId), 2)))))))))));
}

TEST_F(RecorderTest, RingBufferInstance) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);

  zx::time created_at = zx::time(100);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, created_at);
  ASSERT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kRingBuffers))),
          ChildrenMatch(Contains(AllOf(
              NodeMatches(NameMatches(std::string("test_ring_buffer"))),
              ChildrenMatch(Contains(NodeMatches(AllOf(
                  NameMatches(std::string("instance_0")),
                  PropertyList(UnorderedElementsAre(IntIs(std::string(kCtorTime), 100))))))))))))));

  zx::time destroyed_at = zx::time(200);
  rb_recorder->RecordDestructionTime(destroyed_at);
  ASSERT_THAT(
      GetHierarchy(),
      ChildrenMatch(Contains(AllOf(
          NodeMatches(NameMatches(std::string(kRingBuffers))),
          ChildrenMatch(Contains(AllOf(
              NodeMatches(NameMatches(std::string("test_ring_buffer"))),
              ChildrenMatch(Contains(NodeMatches(AllOf(
                  NameMatches(std::string("instance_0")),
                  PropertyList(UnorderedElementsAre(IntIs(std::string(kCtorTime), 100),
                                                    IntIs(std::string(kDtorTime), 200))))))))))))));
}

TEST_F(RecorderTest, StartAndStop) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  zx::time started_at = zx::time(zx::usec(100).get());
  rb_recorder->RecordStartTime(started_at);

  auto hierarchy = GetHierarchy();
  std::vector<std::string> running_interval_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
  };
  auto running_interval_hierarchy = hierarchy.GetByPath(running_interval_path);
  ASSERT_TRUE(running_interval_hierarchy);
  EXPECT_THAT(*running_interval_hierarchy, NodeMatches(PropertyList(IsSupersetOf({
                                               IntIs(std::string(kStartedAtUsec), 100),
                                           }))));

  zx::time stopped_at = zx::time(zx::usec(200).get());
  rb_recorder->RecordStopTime(stopped_at);
  hierarchy = GetHierarchy();
  running_interval_hierarchy = hierarchy.GetByPath(running_interval_path);
  ASSERT_TRUE(running_interval_hierarchy);
  EXPECT_THAT(*running_interval_hierarchy, NodeMatches(PropertyList(IsSupersetOf({
                                               IntIs(std::string(kStartedAtUsec), 100),
                                               IntIs(std::string(kStoppedAtUsec), 200),
                                               IntIs(std::string(kAudioDuration), 100),
                                           }))));
}

TEST_F(RecorderTest, ActiveChannels) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  zx::time called_at = zx::time(100);
  zx::time completed_at = zx::time(200);
  rb_recorder->RecordActiveChannelsCall(0xff, called_at, completed_at);
  ASSERT_THAT(GetHierarchy(),
              ChildrenMatch(Contains(AllOf(
                  NodeMatches(NameMatches(std::string(kRingBuffers))),
                  ChildrenMatch(Contains(AllOf(
                      NodeMatches(NameMatches(std::string("test_ring_buffer"))),
                      ChildrenMatch(Contains(AllOf(
                          NodeMatches(NameMatches(std::string("instance_0"))),
                          ChildrenMatch(Contains(AllOf(
                              NodeMatches(NameMatches(std::string(kSetActiveChannelsCalls))),
                              ChildrenMatch(Contains(
                                  NodeMatches(AllOf(NameMatches(std::string("0")),
                                                    PropertyList(IsSupersetOf({
                                                        UintIs(std::string(kChannelBitmask), 0xff),
                                                        IntIs(std::string(kCalledAt), 100),
                                                        IntIs(std::string(kEffectiveAt), 200),
                                                    })))))))))))))))))));
}

// If we don't submit buffers, only 'count_buffers_processed' will be populated; all others
// should be absent. Also if we don't start the instance, that tracker should not exist at all.
TEST_F(RecorderTest, BufferTrackerDefaults) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));
  rb_recorder->SetupBufferTracker("test_buffer_tracker", 5, zx::msec(10));

  auto hierarchy = GetHierarchy();
  auto expected_buffers =
      AllOf(NameMatches(std::string("test_buffer_tracker")),
            PropertyList(UnorderedElementsAre(UintIs(std::string(kBuffersProcessedCount), 0))));

  std::vector<std::string> summary_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      std::string(kDiagnosticsSummary),
      "test_buffer_tracker",
  };
  const auto summary_buffer_tracker_hierarchy = hierarchy.GetByPath(summary_buffer_tracker_path);
  ASSERT_TRUE(summary_buffer_tracker_hierarchy);
  EXPECT_THAT(*summary_buffer_tracker_hierarchy, NodeMatches(expected_buffers));

  // This buffer tracker should not exist; the instance never started (never recorded start_time).
  std::vector<std::string> running_instance_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      "test_buffer_tracker",
  };
  const auto running_instance_buffer_tracker_hierarchy =
      hierarchy.GetByPath(running_instance_buffer_tracker_path);
  EXPECT_FALSE(running_instance_buffer_tracker_hierarchy);
}

// Test buffer tracking without monitoring buffer-level.
// Overall count and processing time should still be recorded accurately.
TEST_F(RecorderTest, TrackingWithoutMonitoringBufferLevel) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, false);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  // By setting max_buffer_count to 1, we can easily test empty and full cases.
  rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));

  rb_recorder->RecordStartTime(zx::time(100));
  for (uint32_t i = 0; i < 2; i++) {
    auto task = audio::Subtask("task_" + std::to_string(i), /*collect_thread_metrics*/ true);
    task.Start();
    sleep(1);
    task.Done();
    rb_recorder->RecordTaskMetrics(task.FinalMetrics());
  }
  rb_recorder->RecordStopTime(zx::time(200));

  // Simulate some buffer submissions and completions
  rb_recorder->RecordBufferSubmission();
  usleep(10 * 1000);
  rb_recorder->RecordBufferCompletion();
  usleep(2 * 1000);
  rb_recorder->RecordBufferSubmission();
  usleep(4 * 1000);
  rb_recorder->RecordBufferCompletion();

  auto expected_buffer_tracker = AllOf(
      NameMatches(std::string("test_buffer_tracker")),
      PropertyList(IsSupersetOf(std::vector<::testing::Matcher<const ::inspect::PropertyValue&>>{
          // We processed 2 buffers: 10ms and 4ms.
          UintIs(std::string(kBuffersProcessedCount), 2),
          UintIs(std::string(kBuffersProcessedDurationCumulativeUsec), 20'000),
          UintIs(std::string(kProcessingTimeAvgUsec), testing::Ge(7'000)),
          UintIs(std::string(kProcessingTimeMaxUsec), testing::Ge(10'000)),
          // We were full during each buffer and empty in-between, but this should not be captured.
          UintIs(std::string(kEmptyBufferEpisodesCount), 0),
          UintIs(std::string(kFullBufferEpisodesCount), 0),
      })));

  // The expectation specified above should hold true for the diagnostics summary...
  auto hierarchy = GetHierarchy();
  std::vector<std::string> summary_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      std::string(kDiagnosticsSummary),
      "test_buffer_tracker",
  };
  const auto summary_buffer_tracker_hierarchy = hierarchy.GetByPath(summary_buffer_tracker_path);
  ASSERT_TRUE(summary_buffer_tracker_hierarchy);
  EXPECT_THAT(*summary_buffer_tracker_hierarchy, NodeMatches(expected_buffer_tracker));

  // ...as well as for the running instance.
  std::vector<std::string> running_instance_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      "test_buffer_tracker",
  };
  const auto running_instance_buffer_tracker_hierarchy =
      hierarchy.GetByPath(running_instance_buffer_tracker_path);
  ASSERT_TRUE(running_instance_buffer_tracker_hierarchy);
  EXPECT_THAT(*running_instance_buffer_tracker_hierarchy, NodeMatches(expected_buffer_tracker));
}

TEST_F(RecorderTest, EmptyFullTracking) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  // By setting max_buffer_count to 1, we can easily test empty and full cases.
  rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));

  rb_recorder->RecordStartTime(zx::time(100));
  for (uint32_t i = 0; i < 2; i++) {
    auto task = audio::Subtask("task_" + std::to_string(i), /*collect_thread_metrics*/ true);
    task.Start();
    sleep(1);
    task.Done();
    rb_recorder->RecordTaskMetrics(task.FinalMetrics());
  }
  rb_recorder->RecordStopTime(zx::time(200));

  // Simulate some buffer submissions and completions
  rb_recorder->StartMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferSubmission();  // Our buffer-tracker is now full.
  usleep(1000);

  rb_recorder->RecordBufferCompletion();  // Our buffer-tracker is now empty.
  usleep(200 * 1000);

  rb_recorder->RecordBufferSubmission();  // Our buffer-tracker is now full.
  usleep(3000);

  rb_recorder->RecordBufferCompletion();  // Our buffer-tracker is now empty.
  rb_recorder->StopMonitoringOutstandingBufferCount();

  auto expected_buffer_tracker = AllOf(
      NameMatches(std::string("test_buffer_tracker")),
      PropertyList(IsSupersetOf(std::vector<::testing::Matcher<const ::inspect::PropertyValue&>>{
          // We processed 2 buffers which were 10ms each.
          UintIs(std::string(kBuffersProcessedCount), 2),
          UintIs(std::string(kBuffersProcessedDurationCumulativeUsec), 2ul * 10 * 1000),
          // The buffers were processed in 1ms and 3ms respectively.
          UintIs(std::string(kProcessingTimeAvgUsec), testing::Ge(2000)),
          UintIs(std::string(kProcessingTimeMaxUsec), testing::Ge(3000)),
          // We were empty once,for 200ms between the two buffers.
          UintIs(std::string(kEmptyBufferEpisodesCount), 1),
          UintIs(std::string(kEmptyBufferDurationCumulativeUsec), testing::Ge(200 * 1000)),
          UintIs(std::string(kEmptyBufferDurationMaxUsec), testing::Ge(200 * 1000)),
          // We were full twice, during the 1ms and 3ms buffers, so the max full duration is 3ms.
          UintIs(std::string(kFullBufferEpisodesCount), 2),
          UintIs(std::string(kFullBufferDurationCumulativeUsec), testing::Ge(4 * 1000)),
          UintIs(std::string(kFullBufferDurationMaxUsec), testing::Ge(3 * 1000)),
      })));

  // The expectation specified above should hold true for the diagnostics summary...
  auto hierarchy = GetHierarchy();
  std::vector<std::string> summary_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      std::string(kDiagnosticsSummary),
      "test_buffer_tracker",
  };
  const auto summary_buffer_tracker_hierarchy = hierarchy.GetByPath(summary_buffer_tracker_path);
  ASSERT_TRUE(summary_buffer_tracker_hierarchy);
  EXPECT_THAT(*summary_buffer_tracker_hierarchy, NodeMatches(expected_buffer_tracker));

  // ...as well as for the running instance.
  std::vector<std::string> running_instance_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      "test_buffer_tracker",
  };
  const auto running_instance_buffer_tracker_hierarchy =
      hierarchy.GetByPath(running_instance_buffer_tracker_path);
  ASSERT_TRUE(running_instance_buffer_tracker_hierarchy);
  EXPECT_THAT(*running_instance_buffer_tracker_hierarchy, NodeMatches(expected_buffer_tracker));
}

// Various incorrect calls to buffer-accounting methods. To pass, we just need to not crash.
TEST_F(RecorderTest, BufferErrors) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(123));
  {
    // Setup a buffer tracker that can hold 0 buffers!
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 0, zx::msec(10));
  }
  {
    // Setup a buffer tracker with buffers of zero duration!
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(0));
  }
  {
    // SetupBufferTracker is called twice!
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));
    rb_recorder->SetupBufferTracker("test_buffer_tracker - again?", 1, zx::msec(20));
  }
  {
    // Call StopMonitoringOutstandingBufferCount() before StartMonitoringOutstandingBufferCount()!
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));
    rb_recorder->StopMonitoringOutstandingBufferCount();
  }
  {
    // Call StartMonitoringOutstandingBufferCount() before there even is a buffer tracker.
    rb_recorder->StartMonitoringOutstandingBufferCount();
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));
  }
  {
    // RecordBufferCompletion() decrements the outstanding buffer count below zero.
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));
    rb_recorder->RecordBufferCompletion();
  }
  {
    // RecordBufferSubmission() increments the buffer count beyond the tracker's capacity.
    rb_recorder->SetupBufferTracker("test_buffer_tracker", 1, zx::msec(10));
    rb_recorder->RecordBufferSubmission();
    rb_recorder->RecordBufferSubmission();

    // And don't call StopMonitoringOutstandingBufferCount() to cleanup afterward....
    rb_recorder->StartMonitoringOutstandingBufferCount();
  }
  usleep(2000);
}

// Test the accounting of outstanding buffers (min, max, avg).
// We evaluate for min/max before and after each submission/completion call.
// We take "average outstanding buffers" samples BEFORE acting on a 'RecordBufferCompletion' call.
TEST_F(RecorderTest, BufferLevelAccounting) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  rb_recorder->SetupBufferTracker("test_buffer_tracker", 5, zx::msec(10));

  rb_recorder->RecordStartTime(zx::time(1'000'000));

  // Simulate some buffer submissions and completions
  rb_recorder->RecordBufferSubmission();  // current now 1
  usleep(1000);

  rb_recorder->RecordBufferSubmission();  // current now 2
  usleep(2000);

  rb_recorder->StartMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferSubmission();  // min 2; current now 3; max 3
  usleep(3000);

  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 3; current now 2
  usleep(4000);

  rb_recorder->RecordBufferSubmission();  // current now 3
  usleep(3000);

  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 3; current now 2
  usleep(2000);

  rb_recorder->StopMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 2; current now 1
  usleep(1000);

  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 1; current now 0
  rb_recorder->RecordStopTime(zx::time(20'000'000));

  auto expected_buffer_tracker = AllOf(
      NameMatches(std::string("test_buffer_tracker")),
      PropertyList(IsSupersetOf(std::vector<::testing::Matcher<const ::inspect::PropertyValue&>>{
          UintIs(std::string(kBuffersProcessedCount), 4),
          // Data points: 3, 3, 2, 1. Average = 2.25
          DoubleIs(std::string(kOutstandingBuffersCountAvg), DoubleEq(2.25)),
          UintIs(std::string(kOutstandingBuffersCountMax), 3),
          UintIs(std::string(kOutstandingBuffersCountMin), 2),
          UintIs(std::string(kEmptyBufferEpisodesCount), 0),
          UintIs(std::string(kFullBufferEpisodesCount), 0),
      })));

  auto hierarchy = GetHierarchy();
  std::vector<std::string> summary_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      std::string(kDiagnosticsSummary),
      "test_buffer_tracker",
  };
  const auto summary_buffer_tracker_hierarchy = hierarchy.GetByPath(summary_buffer_tracker_path);
  ASSERT_TRUE(summary_buffer_tracker_hierarchy);
  EXPECT_THAT(*summary_buffer_tracker_hierarchy, NodeMatches(expected_buffer_tracker));

  std::vector<std::string> running_instance_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      "test_buffer_tracker",
  };
  const auto running_instance_buffer_tracker_hierarchy =
      hierarchy.GetByPath(running_instance_buffer_tracker_path);
  ASSERT_TRUE(running_instance_buffer_tracker_hierarchy);
  EXPECT_THAT(*running_instance_buffer_tracker_hierarchy, NodeMatches(expected_buffer_tracker));
}

// A recorder instance maintains separate accounting for each RingBuffer start/stop session.
TEST_F(RecorderTest, BufferAccountingByInstance) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));
  rb_recorder->SetupBufferTracker("test_buffer_tracker", 4, zx::msec(10));

  // Instance 0
  rb_recorder->RecordStartTime(zx::time(1'000'000));
  rb_recorder->RecordBufferSubmission();  // current now 1
  usleep(1000);
  rb_recorder->StartMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferSubmission();  // min 1; current now 2; max 2
  usleep(2000);
  rb_recorder->RecordBufferSubmission();  // current now 3; max 3
  usleep(3000);
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 3; current now 2
  usleep(1000);
  usleep(4000);
  rb_recorder->RecordBufferSubmission();  // current now 3
  usleep(3000);
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 3; current now 2
  usleep(2000);
  rb_recorder->StopMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 2; current now 1
  usleep(1000);
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 1; current now 0
  rb_recorder->RecordStopTime(zx::time(20'000'000));
  // 4 buffs processed; running buff levels 3,3,2,1; min 1; max 3; 0 EMPTY episodes; 0 FULL.
  auto expected_running_instance0_buffer_tracker = AllOf(
      NameMatches(std::string("test_buffer_tracker")),
      PropertyList(IsSupersetOf(std::vector<::testing::Matcher<const ::inspect::PropertyValue&>>{
          UintIs(std::string(kBuffersProcessedCount), 4),  // (3+3+2+1) = 9; avg = 9/4 = 2.25
          DoubleIs(std::string(kOutstandingBuffersCountAvg), DoubleEq(2.25)),
          UintIs(std::string(kOutstandingBuffersCountMax), 3),
          UintIs(std::string(kOutstandingBuffersCountMin), 1),
          UintIs(std::string(kEmptyBufferEpisodesCount), 0),
          UintIs(std::string(kFullBufferEpisodesCount), 0),
      })));

  // Instance 1
  rb_recorder->RecordStartTime(zx::time(21'000'000));
  rb_recorder->RecordBufferSubmission();  // current now 1
  usleep(1000);
  rb_recorder->RecordBufferSubmission();  // current now 2
  usleep(2000);
  rb_recorder->RecordBufferSubmission();  // current now 3
  usleep(3000);
  rb_recorder->StartMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferCompletion();  // max 3; avg_buffs data point: 3; current now 2; min 2
  usleep(4000);
  rb_recorder->RecordBufferSubmission();  // current now 3
  usleep(3000);
  rb_recorder->RecordBufferSubmission();  // current now 4; max 4; FULL episode recorded
  usleep(3000);
  rb_recorder->StopMonitoringOutstandingBufferCount();
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 4; current now 3
  usleep(2000);
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 3; current now 2
  usleep(2000);
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 2; current now 1
  usleep(1000);
  rb_recorder->RecordBufferCompletion();  // avg_buffs data point: 1; current now 0
  rb_recorder->RecordStopTime(zx::time(40'000'000));
  // 5 buffs processed; running buff levels 3,4,3,2,1; min 2; max 4; 0 EMPTY; 1 FULL.
  auto expected_running_instance1_buffer_tracker = AllOf(
      NameMatches(std::string("test_buffer_tracker")),
      PropertyList(IsSupersetOf(std::vector<::testing::Matcher<const ::inspect::PropertyValue&>>{
          UintIs(std::string(kBuffersProcessedCount), 5),  // (3+4+3+2+1) = 13; avg = 13/5 = 2.6
          DoubleIs(std::string(kOutstandingBuffersCountAvg), DoubleEq(2.6)),
          UintIs(std::string(kOutstandingBuffersCountMax), 4),
          UintIs(std::string(kOutstandingBuffersCountMin), 2),
          UintIs(std::string(kEmptyBufferEpisodesCount), 0),
          UintIs(std::string(kFullBufferEpisodesCount), 1),
      })));

  // Summary: 9 buffers; running levels 3,3,2,1,3,4,3,2,1; min 1; max 4; 0 EMPTY; 1 FULL.
  auto expected_summary_buffer_tracker = AllOf(
      NameMatches(std::string("test_buffer_tracker")),
      PropertyList(IsSupersetOf(std::vector<::testing::Matcher<const ::inspect::PropertyValue&>>{
          UintIs(std::string(kBuffersProcessedCount), 9),  // (3+3+2+1+3+4+3+2+1)=22; 22/9=2.44...
          DoubleIs(std::string(kOutstandingBuffersCountAvg), DoubleEq(2.444444444444444)),
          UintIs(std::string(kOutstandingBuffersCountMax), 4),
          UintIs(std::string(kOutstandingBuffersCountMin), 1),
          UintIs(std::string(kEmptyBufferEpisodesCount), 0),
          UintIs(std::string(kFullBufferEpisodesCount), 1),
      })));

  auto hierarchy = GetHierarchy();
  std::vector<std::string> running_instance0_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      "test_buffer_tracker",
  };
  const auto running_instance0_buffer_tracker_hierarchy =
      hierarchy.GetByPath(running_instance0_buffer_tracker_path);
  ASSERT_TRUE(running_instance0_buffer_tracker_hierarchy);
  EXPECT_THAT(*running_instance0_buffer_tracker_hierarchy,
              NodeMatches(expected_running_instance0_buffer_tracker));

  std::vector<std::string> running_instance1_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "1",
      std::string(kDiagnostics),
      "test_buffer_tracker",
  };
  const auto running_instance1_buffer_tracker_hierarchy =
      hierarchy.GetByPath(running_instance1_buffer_tracker_path);
  ASSERT_TRUE(running_instance1_buffer_tracker_hierarchy);
  EXPECT_THAT(*running_instance1_buffer_tracker_hierarchy,
              NodeMatches(expected_running_instance1_buffer_tracker));
  std::vector<std::string> summary_buffer_tracker_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      std::string(kDiagnosticsSummary),
      "test_buffer_tracker",
  };
  const auto summary_buffer_tracker_hierarchy = hierarchy.GetByPath(summary_buffer_tracker_path);
  ASSERT_TRUE(summary_buffer_tracker_hierarchy);
  EXPECT_THAT(*summary_buffer_tracker_hierarchy, NodeMatches(expected_summary_buffer_tracker));
}

TEST_F(RecorderTest, AvgTaskMetrics) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  // Record first set of metrics.
  rb_recorder->RecordStartTime(zx::time(100));
  for (uint32_t i = 0; i < 2; i++) {
    auto task = audio::Subtask("task_" + std::to_string(i), /*collect_thread_metrics*/ true);
    task.Start();
    usleep(1000);
    task.Done();
    rb_recorder->RecordTaskMetrics(task.FinalMetrics());
  }
  rb_recorder->RecordStopTime(zx::time(200));

  auto expected_avg_metrics =
      AllOf(NameMatches(std::string(kAvg)),
            PropertyList(UnorderedElementsAre(
                IntIs(std::string(kWallTimeUsec), testing::Ge(1000)),
                IntIs(std::string(kCpuTimeUsec), testing::Ge(0)),
                IntIs(std::string(kQueueTimeUsec), testing::Ge(0)),
                IntIs(std::string(kPageFaultTimeUsec), testing::Ge(0)),
                IntIs(std::string(kStartToStartIntervalUsec), testing::Ge(1000)),
                IntIs(std::string(kEndToEndIntervalUsec), testing::Ge(1000)))));

  auto hierarchy = GetHierarchy();
  std::vector<std::string> rb_avg_metrics_path = {
      std::string(kRingBuffers), "test_ring_buffer", std::string(kDiagnosticsSummary),
      std::string(kTaskRecords), std::string(kAvg),  std::string(kAvg),
  };
  const auto rb_avg_metrics_hierarchy = hierarchy.GetByPath(rb_avg_metrics_path);
  ASSERT_TRUE(rb_avg_metrics_hierarchy);
  EXPECT_THAT(*rb_avg_metrics_hierarchy, NodeMatches(expected_avg_metrics));

  std::vector<std::string> running_instance_avg_metrics_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      std::string(kTaskRecords),
      std::string(kAvg),
      std::string(kAvg),
  };
  const auto running_instance_avg_metrics_hierarchy =
      hierarchy.GetByPath(running_instance_avg_metrics_path);
  ASSERT_TRUE(running_instance_avg_metrics_hierarchy);
  EXPECT_THAT(*running_instance_avg_metrics_hierarchy, NodeMatches(expected_avg_metrics));
}

TEST_F(RecorderTest, SchedulingDelayMetrics) {
  recorder()->PopulateRingBuffer("test_ring_buffer", 1, true, true);
  auto* rb_recorder = &recorder()->CreateRingBufferInstance(1, zx::time(0));

  // Set the task schedule interval.
  rb_recorder->SetTaskScheduleInterval(zx::msec(1));

  // Record first set of metrics.
  rb_recorder->RecordStartTime(zx::time(100));
  for (uint32_t i = 0; i < 2; i++) {
    auto task = audio::Subtask("task_" + std::to_string(i), /*collect_thread_metrics*/ true);
    task.Start();
    usleep(1500);  // Sleep for 1.5ms to ensure a scheduling delay.
    task.Done();
    rb_recorder->RecordTaskMetrics(task.FinalMetrics());
  }
  rb_recorder->RecordStopTime(zx::time(200));

  auto expected_min_metrics =
      AllOf(NameMatches(std::string(kMin)),
            PropertyList(Contains(IntIs(std::string(kSchedulingDelayUsec), testing::Ge(500)))));

  auto hierarchy = GetHierarchy();
  std::vector<std::string> rb_min_metrics_path = {
      std::string(kRingBuffers), "test_ring_buffer", std::string(kDiagnosticsSummary),
      std::string(kTaskRecords), std::string(kMin),  std::string(kMin),
  };
  const auto rb_min_metrics_hierarchy = hierarchy.GetByPath(rb_min_metrics_path);
  ASSERT_TRUE(rb_min_metrics_hierarchy);
  EXPECT_THAT(*rb_min_metrics_hierarchy, NodeMatches(expected_min_metrics));

  std::vector<std::string> running_instance_min_metrics_path = {
      std::string(kRingBuffers),
      "test_ring_buffer",
      "instance_0",
      std::string(kRunningIntervals),
      "0",
      std::string(kDiagnostics),
      std::string(kTaskRecords),
      std::string(kMin),
      std::string(kMin),
  };
  const auto running_instance_min_metrics_hierarchy =
      hierarchy.GetByPath(running_instance_min_metrics_path);
  ASSERT_TRUE(running_instance_min_metrics_hierarchy);
  EXPECT_THAT(*running_instance_min_metrics_hierarchy, NodeMatches(expected_min_metrics));
}

}  // namespace
}  // namespace audio
