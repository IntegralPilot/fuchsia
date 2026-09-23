// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/monitor/logger.h"

#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/memory/metrics/tests/test_utils.h"
#include "src/lib/testing/loop_fixture/test_loop_fixture.h"

namespace memory {
namespace {
using inspect::testing::ChildrenMatch;
using inspect::testing::NameMatches;
using inspect::testing::NodeMatches;

class LoggerInspectTest : public gtest::TestLoopFixture {};

zx_status_t GetCapture(Capture* capture) {
  TestUtils::CreateCapture(
      capture,
      {
          .kmem =
              {
                  .total_bytes = 1000,
                  .free_bytes = 100,
                  .wired_bytes = 10,
                  .vmo_bytes = 700,
              },
          .vmos =
              {
                  {.koid = 1, .name = "a1", .committed_bytes = 100, .committed_scaled_bytes = 100},
                  {.koid = 2, .name = "b1", .committed_bytes = 200, .committed_scaled_bytes = 200},
                  {.koid = 3, .name = "c1", .committed_bytes = 300, .committed_scaled_bytes = 300},
              },
          .processes =
              {
                  {.koid = 1, .name = "p1", .vmos = {1}},
                  {.koid = 2, .name = "q1", .vmos = {2}},
              },
      });
  return {};
}
void GetDigest(const Capture& capture, Digest* digest) {
  Digester digester({{"A", ".*", "a.*"}, {"B", ".*", "b.*"}});
  digester.Digest(capture, digest);
}

TEST_F(LoggerInspectTest, MemoryBuckets) {
  // |capture_on_pressure_change=true| so that setting the pressure level fills in the buckets.
  // Arbitrary non-zero capture_delay so that the test does not loop for ever.
  memory_monitor_config::Config config{{.capture_on_pressure_change = true,
                                        .critical_capture_delay_s = 1000,
                                        .imminent_oom_capture_delay_s = 1000,
                                        .normal_capture_delay_s = 1000,
                                        .warning_capture_delay_s = 1000}};
  inspect::Inspector inspector;
  monitor::Logger logger{dispatcher(), std::nullopt, &GetCapture,
                         &GetDigest,   &config,      inspector.GetRoot().CreateChild("logger")};

  // Trigger pressure change to capture a log.
  logger.SetPressureLevel(pressure_signaler::kWarning);
  // Let the logger drain its internal task queue.
  RunLoopUntilIdle();

  auto result = inspect::ReadFromVmo(inspector.DuplicateVmo());
  ASSERT_TRUE(result.is_ok());
  auto hierarchy = result.take_value();
  // We expect the following hierarchy:
  //      root
  //        |
  //      logger
  //    /        \
  // buckets      measurements
  //  | (prop.)           | (node)
  // (bucket names)   [measurements]
  // TODO(https://fxbug.dev/394289215): Use inspect StringArray property matcher once available.
  EXPECT_THAT(hierarchy,
              // root
              testing::AllOf(NodeMatches(NameMatches("root")),
                             ChildrenMatch(testing::ElementsAre(
                                 testing::AllOf(NodeMatches(NameMatches("logger")),
                                                ChildrenMatch(testing::ElementsAre(
                                                    // measurements
                                                    NodeMatches(NameMatches("measurements")))))))));
}
}  // namespace
}  // namespace memory
