// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/attachments/system_log.h"

#include <fuchsia/mem/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/fpromise/result.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/syslog/cpp/log_level.h>
#include <lib/zx/time.h>

#include <memory>
#include <string>
#include <utility>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/feedback/attachments/types.h"
#include "src/developer/forensics/feedback_data/constants.h"
#include "src/developer/forensics/feedback_data/log_source.h"
#include "src/developer/forensics/testing/gmatchers.h"
#include "src/developer/forensics/testing/gpretty_printers.h"  // IWYU pragma: keep
#include "src/developer/forensics/testing/stubs/cobalt_logger_factory.h"
#include "src/developer/forensics/testing/stubs/diagnostics_archive.h"
#include "src/developer/forensics/testing/stubs/diagnostics_batch_iterator.h"
#include "src/developer/forensics/testing/unit_test_fixture.h"
#include "src/developer/forensics/utils/errors.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/timekeeper/async_test_clock.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics::feedback {
namespace {

using testing::IsEmpty;
using testing::UnorderedElementsAreArray;

std::string MessageJson(const int id) {
  return fxl::StringPrintf(
      R"JSON(
[
  {
    "metadata": {
      "timestamp": 1234000000000,
      "severity": "INFO",
      "pid": 200,
      "tid": 300,
      "tags": ["tag_%d"]
    },
    "payload": {
      "root": {
        "message": {
          "value": "Message %d"
        }
      }
    }
  }
]
)JSON",
      id, id);
}

std::string MessageJsonWithTimestamp(const int id, const uint64_t timestamp) {
  return fxl::StringPrintf(
      R"JSON(
[
  {
    "metadata": {
      "timestamp": %lu,
      "severity": "INFO",
      "pid": 200,
      "tid": 300,
      "tags": ["tag_%d"]
    },
    "payload": {
      "root": {
        "message": {
          "value": "Message %d"
        }
      }
    }
  }
]
)JSON",
      timestamp, id, id);
}

std::vector<std::string> Messages() { return {MessageJson(1), MessageJson(2), MessageJson(3)}; }

class SystemLogTest : public UnitTestFixture {
 public:
  SystemLogTest()
      : executor_(dispatcher()),
        clock_(dispatcher()),
        cobalt_(dispatcher(), services(), &clock_),
        log_buffer_(feedback_data::kCurrentLogBufferSize, &redactor_),
        system_log_(dispatcher(), services(), &clock_, &redactor_, kActivePeriod, &cobalt_,
                    &log_buffer_) {
    SetUpCobaltServer(std::make_unique<stubs::CobaltLoggerFactory>(dispatcher()));
  }

 protected:
  void SetUpLogServer(std::vector<std::string> messages) {
    log_server_ = std::make_unique<stubs::DiagnosticsArchive>(
        dispatcher(), std::make_unique<stubs::DiagnosticsBatchIteratorNeverRespondsAfterOneBatch>(
                          std::move(messages)));
    InjectServiceProvider(log_server_.get(), feedback_data::kArchiveAccessorName);
  }

  AttachmentData CollectSystemLog(const zx::duration timeout = zx::sec(1)) {
    const uint64_t kTicket = 1234;
    AttachmentData result(Error::kNotSet);
    executor_.schedule_task(
        system_log_.Get(kTicket)
            .and_then([&result](AttachmentData& res) { result = std::move(res); })
            .or_else([] { FX_LOGS(FATAL) << "Bad path"; }));
    RunLoopFor(timeout);
    return result;
  }

  async::Executor& GetExecutor() { return executor_; }

  SystemLog& GetSystemLog() { return system_log_; }

  ::fpromise::promise<AttachmentData> CollectSystemLog(const uint64_t ticket) {
    return system_log_.Get(ticket).or_else([]() -> ::fpromise::result<AttachmentData> {
      FX_LOGS(FATAL) << "Bad path";
      return ::fpromise::error();
    });
  }

  void AddMessageToBuffer(std::string message, const int64_t timestamp) {
    fuchsia::logger::LogMessage log_message;
    log_message.time = zx::time_boot(timestamp);
    log_message.msg = std::move(message);
    log_message.severity = fuchsia::diagnostics::types::Severity::INFO;

    log_buffer_.Add(::fpromise::ok(std::move(log_message)));
  }

  static constexpr zx::duration kActivePeriod = zx::hour(1);
  static constexpr zx::duration kLogTimestamp = zx::sec(1234);

  timekeeper::Clock* Clock() { return &clock_; }
  const stubs::DiagnosticsArchiveBase& LogServer() const { return *log_server_; }

 private:
  async::Executor executor_;
  timekeeper::AsyncTestClock clock_;
  IdentityRedactor redactor_{inspect::BoolProperty()};
  std::unique_ptr<stubs::DiagnosticsArchiveBase> log_server_;
  cobalt::Logger cobalt_;

  LogBuffer log_buffer_;
  SystemLog system_log_;
};

TEST_F(SystemLogTest, GetTerminatesDueToLogTimestamp) {
  SetUpLogServer(Messages());

  const AttachmentData log = CollectSystemLog();
  EXPECT_THAT(log, AttachmentDataIs(R"([01234.000][00200][00300][tag_1] INFO: Message 1
[01234.000][00200][00300][tag_2] INFO: Message 2
[01234.000][00200][00300][tag_3] INFO: Message 3
)"));
}

TEST_F(SystemLogTest, GetTerminatesDueToForceCompletionWithEmptyLog) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({});

  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  RunLoopUntilIdle();

  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(log, AttachmentDataIs(Error::kDefault));
}

TEST_F(SystemLogTest, GetTerminatesDueToForceCompletion) {
  const uint64_t kTicket = 1234;
  SetUpLogServer(Messages());

  // Prime the clock so log collection won't be completed due to message timestamps.
  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  // Giving some time to actually collect some log data, so that system_log is not empty
  RunLoopUntilIdle();

  // Forcefully terminating log collection
  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([01234.000][00200][00300][tag_1] INFO: Message 1
[01234.000][00200][00300][tag_2] INFO: Message 2
[01234.000][00200][00300][tag_3] INFO: Message 3
)",
                       Error::kDefault));
}

TEST_F(SystemLogTest, ForceCompletionCalledAfterTermination) {
  const uint64_t kTicket = 1234;
  SetUpLogServer(Messages());

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  RunLoopFor(zx::sec(1));

  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);
  EXPECT_THAT(log, AttachmentDataIs(R"([01234.000][00200][00300][tag_1] INFO: Message 1
[01234.000][00200][00300][tag_2] INFO: Message 2
[01234.000][00200][00300][tag_3] INFO: Message 3
)"));
}

TEST_F(SystemLogTest, ActivePeriodExpires) {
  const uint64_t kTicket = 1234;
  SetUpLogServer(Messages());

  AttachmentData log = CollectSystemLog();
  EXPECT_THAT(log, AttachmentDataIs(R"([01234.000][00200][00300][tag_1] INFO: Message 1
[01234.000][00200][00300][tag_2] INFO: Message 2
[01234.000][00200][00300][tag_3] INFO: Message 3
)"));

  // Become disconnected from the server after |kActivePeriod| expires.
  RunLoopFor(kActivePeriod);
  ASSERT_FALSE(LogServer().IsBound());

  log = AttachmentData(Error::kNotSet);

  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  RunLoopUntilIdle();

  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();

  EXPECT_THAT(log, AttachmentDataIs(Error::kDefault));

  ASSERT_TRUE(LogServer().IsBound());
}

TEST_F(SystemLogTest, ActivePeriodResets) {
  const uint64_t kTicket = 1234;
  SetUpLogServer(Messages());

  AttachmentData log = CollectSystemLog(zx::min(1));
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([01234.000][00200][00300][tag_1] INFO: Message 1
[01234.000][00200][00300][tag_2] INFO: Message 2
[01234.000][00200][00300][tag_3] INFO: Message 3
)"));

  RunLoopFor(kActivePeriod / 2);
  ASSERT_TRUE(LogServer().IsBound());

  log = AttachmentData(Error::kNotSet);

  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  RunLoopFor(kActivePeriod / 2);

  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([01234.000][00200][00300][tag_1] INFO: Message 1
[01234.000][00200][00300][tag_2] INFO: Message 2
[01234.000][00200][00300][tag_3] INFO: Message 3
)",
                       Error::kDefault));

  // Connection is still open because active period was reset with the last call to Get
  ASSERT_TRUE(LogServer().IsBound());
}

TEST_F(SystemLogTest, NoCobaltLogsIfNoMessages) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({});

  // Prime the clock so log collection won't be completed due to message timestamps.
  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  // Giving some time to actually collect some log data, so that system_log is not empty
  RunLoopUntilIdle();

  // Forcefully terminating log collection
  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(SystemLogTest, NoCobaltLogsIfNegativeFirstTimestamp) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({});

  AddMessageToBuffer("Message 1", /*timestamp=*/-1000000000);
  AddMessageToBuffer("Message 2", /*timestamp=*/1000000000);
  AddMessageToBuffer("Message 3", /*timestamp=*/2000000000);

  // Prime the clock so log collection won't be completed due to message timestamps.
  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  // Giving some time to actually collect some log data, so that system_log is not empty
  RunLoopUntilIdle();

  // Forcefully terminating log collection
  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();

  // The timestamp formatter expects unsigned integers so -1 becomes a large positive number.
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([1266874888.709][00000][00000][] INFO: Message 1
[00001.000][00000][00000][] INFO: Message 2
[00002.000][00000][00000][] INFO: Message 3
)",
                       Error::kDefault));
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(SystemLogTest, NoCobaltLogsIfUnderCapacity) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({
      MessageJsonWithTimestamp(1, /*timestamp=*/0),
      MessageJsonWithTimestamp(2, /*timestamp=*/1000000000),
      MessageJsonWithTimestamp(3, /*timestamp=*/2000000000),
  });

  // Prime the clock so log collection won't be completed due to message timestamps.
  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  // Giving some time to actually collect some log data, so that system_log is not empty
  RunLoopUntilIdle();

  // Forcefully terminating log collection
  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([00000.000][00200][00300][tag_1] INFO: Message 1
[00001.000][00200][00300][tag_2] INFO: Message 2
[00002.000][00200][00300][tag_3] INFO: Message 3
)",
                       Error::kDefault));
  EXPECT_THAT(ReceivedCobaltEvents(), IsEmpty());
}

TEST_F(SystemLogTest, BufferSortedBeforeLoggingToCobalt) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({
      MessageJsonWithTimestamp(2, /*timestamp=*/61000000000),
      MessageJsonWithTimestamp(1, /*timestamp=*/1000000000),
      MessageJsonWithTimestamp(3, /*timestamp=*/121000000000),
  });

  // Prime the clock so log collection won't be completed due to message timestamps.
  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  // Giving some time to actually collect some log data, so that system_log is not empty
  RunLoopUntilIdle();

  // Forcefully terminating log collection
  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([00001.000][00200][00300][tag_1] INFO: Message 1
[00061.000][00200][00300][tag_2] INFO: Message 2
[00121.000][00200][00300][tag_3] INFO: Message 3
)",
                       Error::kDefault));
  EXPECT_THAT(
      ReceivedCobaltEvents(),
      UnorderedElementsAreArray({
          cobalt::Event(cobalt::EventType::kInteger, /*syslog_bytes_at_capacity*/ 112u, {}, 147u),
          cobalt::Event(cobalt::EventType::kInteger, /*syslog_duration_at_capacity*/ 113u, {}, 2u),
      }));
}

TEST_F(SystemLogTest, CobaltLogsIfAtCapacity) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({
      MessageJsonWithTimestamp(1, /*timestamp=*/1000000000),
      MessageJsonWithTimestamp(2, /*timestamp=*/61000000000),
      MessageJsonWithTimestamp(3, /*timestamp=*/121000000000),
  });

  // Prime the clock so log collection won't be completed due to message timestamps.
  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  // Giving some time to actually collect some log data, so that system_log is not empty
  RunLoopUntilIdle();

  // Forcefully terminating log collection
  GetSystemLog().ForceCompletion(kTicket, Error::kDefault);

  RunLoopUntilIdle();
  EXPECT_THAT(log, AttachmentDataIs(
                       R"([00001.000][00200][00300][tag_1] INFO: Message 1
[00061.000][00200][00300][tag_2] INFO: Message 2
[00121.000][00200][00300][tag_3] INFO: Message 3
)",
                       Error::kDefault));
  EXPECT_THAT(
      ReceivedCobaltEvents(),
      UnorderedElementsAreArray({
          cobalt::Event(cobalt::EventType::kInteger, /*syslog_bytes_at_capacity*/ 112u, {}, 147u),
          cobalt::Event(cobalt::EventType::kInteger, /*syslog_duration_at_capacity*/ 113u, {}, 2u),
      }));
}

TEST_F(SystemLogTest, GetCalledWithSameTicket) {
  const uint64_t kTicket = 1234;

  // Expect a crash because a ticket cannot be reused.
  ASSERT_DEATH(
      {
        const ::fpromise::promise<AttachmentData> log1 = CollectSystemLog(kTicket);
        const ::fpromise::promise<AttachmentData> log2 = CollectSystemLog(kTicket);
      },
      "Ticket used twice: ");
}

TEST_F(SystemLogTest, AddsSourceMetadata) {
  SetUpLogServer(Messages());

  EXPECT_EQ(CollectSystemLog().Metadata(),
            AttachmentMetadata({{feedback_data::kAttachmentMetadataSourceKey,
                                 feedback_data::kAttachmentMetadataSourceStream}}));
}

TEST_F(SystemLogTest, AddsSourceMetadataOnEmptyLog) {
  const uint64_t kTicket = 1234;
  SetUpLogServer({});

  RunLoopFor(kLogTimestamp + zx::sec(1));

  AttachmentData log(Error::kNotSet);
  GetExecutor().schedule_task(CollectSystemLog(kTicket).and_then(
      [&log](AttachmentData& result) { log = std::move(result); }));

  RunLoopUntilIdle();
  GetSystemLog().ForceCompletion(kTicket, Error::kMissingValue);
  RunLoopUntilIdle();

  EXPECT_EQ(log.Metadata(), AttachmentMetadata({{feedback_data::kAttachmentMetadataSourceKey,
                                                 feedback_data::kAttachmentMetadataSourceStream}}));
}

}  // namespace
}  // namespace forensics::feedback
