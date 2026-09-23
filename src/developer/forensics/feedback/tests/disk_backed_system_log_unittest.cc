// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/disk_backed_system_log.h"

#include <fidl/fuchsia.feedback.internal/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fpromise/result.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/testing/backoff.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/stubs/system_log_recorder.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/cobalt/metrics.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/timekeeper/async_test_clock.h"

namespace forensics::feedback {
namespace {

using ::testing::ElementsAre;
using ::testing::IsEmpty;
using ::testing::Pair;
using ::testing::UnorderedElementsAreArray;

class DiskBackedSystemLogTest : public UnitTestFixture {
 public:
  DiskBackedSystemLogTest()
      : executor_(dispatcher()), clock_(dispatcher()), cobalt_(dispatcher(), services(), &clock_) {
    SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));
  }

 protected:
  async::Executor& GetExecutor() { return executor_; }
  timekeeper::AsyncTestClock& Clock() { return clock_; }
  cobalt::Logger* Cobalt() { return &cobalt_; }
  RedactorBase* Redactor() { return &redactor_; }

  async::Executor executor_;
  timekeeper::AsyncTestClock clock_;
  cobalt::Logger cobalt_;
  IdentityRedactor redactor_{inspect::BoolProperty()};
};

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsSuccess) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs("stub logs content"));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsError) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::error(fuchsia_feedback_internal::RecorderError::kIoError));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kFileReadFailure));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsEmptyLog) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok(""));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kMissingValue));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsMissingLogs) {
  stubs::SystemLogRecorder stub;
  fuchsia_feedback_internal::SystemLogRecorderGetCurrentBootLogsResponse response;
  stub.SetResponse(std::move(response));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kBadValue));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsReconnects) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  RunLoopUntilIdle();
  ASSERT_TRUE(stub.IsBound());

  stub.CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();

  const uint64_t kTicket1 = 1;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket1)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kConnectionError));

  // Wait for backoff (1 sec).
  RunLoopFor(zx::sec(1));

  const uint64_t kTicket2 = 2;
  result = AttachmentData(Error::kNotSet);
  callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket2)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs("stub logs content"));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsResetsBackoffOnSuccess) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  RunLoopUntilIdle();
  ASSERT_TRUE(stub.IsBound());

  stub.CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();

  const uint64_t kTicket1 = 1;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket1)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kConnectionError));

  // Wait for backoff.
  RunLoopFor(zx::sec(1));

  const uint64_t kTicket2 = 2;
  result = AttachmentData(Error::kNotSet);
  callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket2)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs("stub logs content"));

  // Unbind again.
  stub.CloseConnection(ZX_ERR_PEER_CLOSED);
  RunLoopUntilIdle();

  const uint64_t kTicket3 = 3;
  result = AttachmentData(Error::kNotSet);
  callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket3)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kConnectionError));

  // Wait for backoff again. It should be zx::sec(1), not zx::sec(2) because of the intermediate
  // success.
  RunLoopFor(zx::sec(1));

  const uint64_t kTicket4 = 4;
  result = AttachmentData(Error::kNotSet);
  callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket4)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs("stub logs content"));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsDoesNotReconnectOnNotFound) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  RunLoopUntilIdle();
  ASSERT_TRUE(stub.IsBound());

  stub.CloseConnection(ZX_ERR_NOT_FOUND);
  RunLoopUntilIdle();

  const uint64_t kTicket1 = 1;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket1)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kConnectionError));

  // Wait for backoff.
  RunLoopFor(zx::sec(1));

  const uint64_t kTicket2 = 2;
  result = AttachmentData(Error::kNotSet);
  callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket2)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kConnectionError));
  EXPECT_FALSE(stub.IsBound());
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsForceCompletion) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  system_log.ForceCompletion(kTicket, Error::kTimeout);

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kTimeout));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsSourceDisk) {
  fuchsia_feedback_internal::SystemLogMetadata metadata;
  metadata.source(fuchsia_feedback_internal::SystemLogSource::kDisk);

  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"), std::move(metadata));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  GetExecutor().schedule_task(
      system_log.Get(kTicket)
          .and_then([&result](AttachmentData& res) { result = std::move(res); })
          .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  ASSERT_TRUE(result.HasValue());
  EXPECT_THAT(result.Metadata(), ElementsAre(Pair(feedback_data::kAttachmentMetadataSourceKey,
                                                  feedback_data::kAttachmentMetadataSourceDisk)));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsSourceStream) {
  fuchsia_feedback_internal::SystemLogMetadata metadata;
  metadata.source(fuchsia_feedback_internal::SystemLogSource::kStream);

  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"), std::move(metadata));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  GetExecutor().schedule_task(
      system_log.Get(kTicket)
          .and_then([&result](AttachmentData& res) { result = std::move(res); })
          .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  ASSERT_TRUE(result.HasValue());
  EXPECT_THAT(result.Metadata(), ElementsAre(Pair(feedback_data::kAttachmentMetadataSourceKey,
                                                  feedback_data::kAttachmentMetadataSourceStream)));
}

TEST_F(DiskBackedSystemLogTest, ForceCompletionUnknownTicketIsNoOp) {
  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  // Should not crash or fail when calling ForceCompletion on an unknown ticket.
  system_log.ForceCompletion(9999, Error::kTimeout);
}

TEST_F(DiskBackedSystemLogTest, NullServiceDirectory) {
  DiskBackedSystemLog system_log(dispatcher(), /*system_log_recorder_services=*/nullptr,
                                 std::make_unique<MonotonicBackoff>(), Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kConnectionError));
}

TEST_F(DiskBackedSystemLogTest, DestructionResolvesPromise) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  auto system_log = std::make_unique<DiskBackedSystemLog>(
      dispatcher(), services(), std::make_unique<MonotonicBackoff>(), Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log->Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  // Destroy system_log while the request is in flight.
  system_log.reset();

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(result, AttachmentDataIs(Error::kLogicError));
}

TEST_F(DiskBackedSystemLogTest, DuplicateTicketPanics) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  GetExecutor().schedule_task(
      system_log.Get(kTicket).and_then([](AttachmentData&) {}).or_else([] {}));

  ASSERT_DEATH({ system_log.Get(kTicket); }, "Ticket used twice");
}

TEST_F(DiskBackedSystemLogTest, ConcurrentRequestsIndependentResolution) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket1 = 1;
  const uint64_t kTicket2 = 2;

  AttachmentData result1(Error::kNotSet);
  bool callback1_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket1)
                                  .and_then([&result1, &callback1_called](AttachmentData& res) {
                                    result1 = std::move(res);
                                    callback1_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  AttachmentData result2(Error::kNotSet);
  bool callback2_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket2)
                                  .and_then([&result2, &callback2_called](AttachmentData& res) {
                                    result2 = std::move(res);
                                    callback2_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  // Force completion on ticket 1 before running the loop.
  system_log.ForceCompletion(kTicket1, Error::kTimeout);

  RunLoopUntilIdle();

  EXPECT_TRUE(callback1_called);
  EXPECT_THAT(result1, AttachmentDataIs(Error::kTimeout));

  EXPECT_TRUE(callback2_called);
  EXPECT_THAT(result2, AttachmentDataIs("stub logs content"));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsCobaltLogsIfAtCapacity) {
  stubs::SystemLogRecorder stub;
  fuchsia_feedback_internal::SystemLogMetadata metadata;
  metadata.first_timestamp(zx::time_boot(0) + zx::sec(1));
  metadata.last_timestamp(zx::time_boot(0) + zx::sec(121));
  stub.SetResponse(fit::ok(R"([00001.000][00200][00300][tag_1] INFO: Message 1
[00061.000][00200][00300][tag_2] INFO: Message 2
[00121.000][00200][00300][tag_3] INFO: Message 3
)"),
                   std::move(metadata));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(ReceivedCobaltEvents(),
              UnorderedElementsAreArray({
                  cobalt::Event(cobalt::EventType::kInteger,
                                cobalt_registry::kSyslogBytesAtCapacityMetricId, {}, 147u),
                  cobalt::Event(cobalt::EventType::kInteger,
                                cobalt_registry::kSyslogDurationAtCapacityMetricId, {}, 2u),
              }));
}

TEST_F(DiskBackedSystemLogTest, GetCurrentBootLogsNoCobaltLogsIfUnderCapacity) {
  stubs::SystemLogRecorder stub;
  fuchsia_feedback_internal::SystemLogMetadata metadata;
  metadata.first_timestamp(zx::time_boot(0));
  metadata.last_timestamp(zx::time_boot(0) + zx::sec(2));
  stub.SetResponse(fit::ok(R"([00000.000][00200][00300][tag_1] INFO: Message 1
[00001.000][00200][00300][tag_2] INFO: Message 2
[00002.000][00200][00300][tag_3] INFO: Message 3
)"),
                   std::move(metadata));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(DiskBackedSystemLogTest, AddsSourceMetadata) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::ok("stub logs content"));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_EQ(result.Metadata(),
            AttachmentMetadata({{feedback_data::kAttachmentMetadataSourceKey,
                                 feedback_data::kAttachmentMetadataSourceDisk}}));
}

TEST_F(DiskBackedSystemLogTest, AddsSourceMetadataOnError) {
  stubs::SystemLogRecorder stub;
  stub.SetResponse(fit::error(fuchsia_feedback_internal::RecorderError::kIoError));
  InjectServiceProvider(&stub);

  DiskBackedSystemLog system_log(dispatcher(), services(), std::make_unique<MonotonicBackoff>(),
                                 Redactor(), Cobalt());

  const uint64_t kTicket = 1234;
  AttachmentData result(Error::kNotSet);
  bool callback_called = false;
  GetExecutor().schedule_task(system_log.Get(kTicket)
                                  .and_then([&result, &callback_called](AttachmentData& res) {
                                    result = std::move(res);
                                    callback_called = true;
                                  })
                                  .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));

  RunLoopUntilIdle();
  EXPECT_TRUE(callback_called);
  EXPECT_EQ(result.Metadata(),
            AttachmentMetadata({{feedback_data::kAttachmentMetadataSourceKey,
                                 feedback_data::kAttachmentMetadataSourceDisk}}));
}

}  // namespace
}  // namespace forensics::feedback
