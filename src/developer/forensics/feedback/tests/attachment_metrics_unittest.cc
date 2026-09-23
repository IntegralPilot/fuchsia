// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cctype>
#include <memory>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/attachments/metrics.h"
#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/event.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/lib/timekeeper/test_clock.h"

namespace forensics::feedback {
namespace {

using ::testing::IsEmpty;
using ::testing::UnorderedElementsAreArray;

struct ExpectedMetric {
  std::string key;
  cobalt::TimedOutData metric;
  std::string name;
};

class AttachmentMetricsTest : public UnitTestFixture,
                              public ::testing::WithParamInterface<ExpectedMetric> {
 public:
  AttachmentMetricsTest() : cobalt_(dispatcher(), services(), &clock_) {
    SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));
  }

  cobalt::Logger* Cobalt() { return &cobalt_; }

 private:
  timekeeper::TestClock clock_;
  cobalt::Logger cobalt_;
};

// Converts maps of error to maps of AttachmentValues to get around the fact AttachmentValues can't
// be copied.
Attachments ToAttachments(const std::map<std::string, Error>& errors) {
  Attachments attachments;
  for (const auto& [k, v] : errors) {
    attachments.insert({k, AttachmentValue(v, zx::duration(0))});
  }
  return attachments;
}

INSTANTIATE_TEST_SUITE_P(
    VariousKeys, AttachmentMetricsTest,
    ::testing::ValuesIn(std::vector<ExpectedMetric>({
        {feedback_data::kAttachmentLogKernel, cobalt::TimedOutData::kKernelLog, "KernelLog"},
        {feedback_data::kAttachmentLogSystem, cobalt::TimedOutData::kSystemLog, "SystemLog"},
        {feedback_data::kAttachmentInspect, cobalt::TimedOutData::kInspect, "Inspect"},
    })),
    [](const ::testing::TestParamInfo<ExpectedMetric>& info) { return info.param.name; });

TEST_P(AttachmentMetricsTest, IndividualKeysTimeout) {
  const ExpectedMetric& param = GetParam();

  AttachmentMetrics metrics(Cobalt());
  metrics.LogMetrics(ToAttachments({
      {param.key, Error::kTimeout},
  }));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({cobalt::Event(param.metric)}));
}

TEST_P(AttachmentMetricsTest, IndividualKeysNonTimeout) {
  const ExpectedMetric& param = GetParam();

  AttachmentMetrics metrics(Cobalt());
  metrics.LogMetrics(ToAttachments({
      {param.key, Error::kMissingValue},
  }));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(AttachmentMetricsTest, UnknownKey) {
  AttachmentMetrics metrics(Cobalt());
  metrics.LogMetrics(ToAttachments({
      {"unknown", Error::kTimeout},
  }));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(AttachmentMetricsTest, NonTimeout) {
  AttachmentMetrics metrics(Cobalt());
  metrics.LogMetrics(ToAttachments({
      {"unknown", Error::kTimeout},
  }));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(AttachmentMetricsTest, AllAttachments) {
  AttachmentMetrics metrics(Cobalt());

  metrics.LogMetrics(ToAttachments({
      {feedback_data::kAttachmentLogKernel, Error::kTimeout},
      {feedback_data::kAttachmentLogSystem, Error::kTimeout},
      {feedback_data::kAttachmentInspect, Error::kTimeout},
  }));

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), UnorderedElementsAreArray({
                                          cobalt::Event(cobalt::TimedOutData::kKernelLog),
                                          cobalt::Event(cobalt::TimedOutData::kSystemLog),
                                          cobalt::Event(cobalt::TimedOutData::kInspect),
                                      }));
}

}  // namespace
}  // namespace forensics::feedback
