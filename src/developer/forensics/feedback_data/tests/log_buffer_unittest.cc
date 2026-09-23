// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/log_buffer.h"

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace forensics::feedback {
namespace {

using ::testing::IsEmpty;

constexpr fuchsia_logging::LogSeverity kLogInfo = fuchsia_logging::LogSeverity::Info;
constexpr fuchsia_logging::LogSeverity kLogWarning = fuchsia_logging::LogSeverity::Warn;

feedback_data::LogSink::MessageOr ToMessage(const std::string& msg,
                                            fuchsia_logging::LogSeverity severity = kLogInfo,
                                            std::vector<std::string> tags = {"tag1", "tag2"}) {
  return ::fpromise::ok(fuchsia::logger::LogMessage{
      .pid = 100,
      .tid = 101,
      .time = zx::time_boot((zx::sec(1) + zx::msec(10)).get()),
      .severity = severity,
      .dropped_logs = 0,
      .tags = std::move(tags),
      .msg = msg,
  });
}

feedback_data::LogSink::MessageOr ToMessage(const std::string& msg, const zx::duration time) {
  return ::fpromise::ok(fuchsia::logger::LogMessage{
      .pid = 100,
      .tid = 101,
      .time = zx::time_boot(time.to_nsecs()),
      .severity = kLogInfo,
      .dropped_logs = 0,
      .tags = {"tag1", "tag2"},
      .msg = msg,
  });
}

feedback_data::LogSink::MessageOr ToError(const std::string& error) {
  return ::fpromise::error(error);
}

class SimpleRedactor : public RedactorBase {
 public:
  SimpleRedactor() : RedactorBase(inspect::BoolProperty()) {}

 private:
  std::string& Redact(std::string& text) override {
    if (text.find("ERRORS ERR") == text.npos && text.find("Offset") == text.npos) {
      text = "REDACTED";
    }
    return text;
  }
  std::string& RedactJson(std::string& text) override { return Redact(text); }

  std::string UnredactedCanary() const override { return ""; }
  std::string RedactedCanary() const override { return ""; }
};

TEST(LogBufferTest, SafeAfterInterruption) {
  IdentityRedactor redactor(inspect::BoolProperty{});
  LogBuffer buffer(StorageSize::Gigabytes(100), &redactor);
  ASSERT_TRUE(buffer.SafeAfterInterruption());
}

TEST(LogBufferTest, OrderingOnAdd) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Gigabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 0")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 1", zx::sec(20))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
)");

  // Should be deduplicated and before "log 1".
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(18))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(18))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(19))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
)");

  // Should be deduplicated and after "log 1".
  EXPECT_TRUE(buffer.Add(ToMessage("log 3", zx::sec(21))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 3", zx::sec(21))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");

  // Should be after "log 3".
  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
)");

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
)");

  // Should be before "log 3".
  EXPECT_TRUE(buffer.Add(ToMessage("log 4", zx::sec(20))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 4", zx::sec(20))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
[00020.000][00100][00101][tag1, tag2] INFO: log 4
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
)");

  // Should be before "log 3", but not aggregated with other "log 4".
  EXPECT_TRUE(buffer.Add(ToMessage("log 4", zx::sec(20))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
[00020.000][00100][00101][tag1, tag2] INFO: log 4
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 4
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
)");

  // Should be before "log 3".
  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 2")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 4", zx::sec(22))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
[00020.000][00100][00101][tag1, tag2] INFO: log 4
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 4
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
!!! Failed to format chunk: ERRORS ERR 2 !!!
[00022.000][00100][00101][tag1, tag2] INFO: log 4
)");
}

TEST(LogBufferTest, OrderingOnEnforce) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  // 190 bytes is approximately enough to store 3 log messages.
  LogBuffer buffer(StorageSize::Bytes(190), &redactor);

  EXPECT_TRUE(buffer.Add(ToMessage("log 1", zx::sec(20))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 1", zx::sec(20))));

  EXPECT_EQ(buffer.ToString(), R"([00020.000][00100][00101][tag1, tag2] INFO: log 1
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");

  // Should be before "log 1".
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(18))));
  EXPECT_EQ(buffer.ToString(), R"([00018.000][00100][00101][tag1, tag2] INFO: log 2
[00020.000][00100][00101][tag1, tag2] INFO: log 1
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");

  // Should be before "log 1" and not deduplicated against the earlier "log 2"
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(18))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(19))));

  EXPECT_EQ(buffer.ToString(), R"([00018.000][00100][00101][tag1, tag2] INFO: log 2
[00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");

  // Should be deduplicated and after "log 1".
  EXPECT_TRUE(buffer.Add(ToMessage("log 3", zx::sec(21))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 3", zx::sec(21))));

  EXPECT_EQ(buffer.ToString(), R"([00020.000][00100][00101][tag1, tag2] INFO: log 1
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");

  // Should be after "log 3".
  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));

  EXPECT_EQ(buffer.ToString(), R"([00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
)");

  // Should be before "log 3".
  EXPECT_TRUE(buffer.Add(ToMessage("log 4", zx::sec(20))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 4", zx::sec(20))));

  EXPECT_EQ(buffer.ToString(), R"([00020.000][00100][00101][tag1, tag2] INFO: log 4
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00021.000][00100][00101][tag1, tag2] INFO: log 3
!!! MESSAGE REPEATED 1 MORE TIME !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
)");
}

TEST(LogBufferTest, RepeatedMessage) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToMessage("log 1")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 1")));

  // Exact same message, severity and tags: should be deduplicated
  EXPECT_EQ(buffer.ToString(), R"([00001.010][00100][00101][tag1, tag2] INFO: log 1
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");
}

TEST(LogBufferTest, DoNotDeduplicateIfDifferentMessage) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToMessage("log 1")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2")));

  EXPECT_EQ(buffer.ToString(), R"([00001.010][00100][00101][tag1, tag2] INFO: log 1
[00001.010][00100][00101][tag1, tag2] INFO: log 2
)");
}

TEST(LogBufferTest, DoNotDeduplicateIfDifferentSeverity) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToMessage("log 1", kLogInfo)));
  EXPECT_TRUE(buffer.Add(ToMessage("log 1", kLogWarning)));

  EXPECT_EQ(buffer.ToString(), R"([00001.010][00100][00101][tag1, tag2] INFO: log 1
[00001.010][00100][00101][tag1, tag2] WARN: log 1
)");
}

TEST(LogBufferTest, DoNotDeduplicateIfDifferentTags) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToMessage("log 1", kLogInfo, {"tag1", "tag2"})));
  EXPECT_TRUE(buffer.Add(ToMessage("log 1", kLogInfo, {"tag1"})));

  EXPECT_EQ(buffer.ToString(), R"([00001.010][00100][00101][tag1, tag2] INFO: log 1
[00001.010][00100][00101][tag1] INFO: log 1
)");
}

TEST(LogBufferTest, TimestampZeroOnFirstError) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 1 !!!
)");
}

TEST(LogBufferTest, RepeatedError) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));
  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 1 !!!
!!! MESSAGE REPEATED 1 MORE TIME !!!
)");
}

TEST(LogBufferTest, DoNotDeduplicateIfDifferentError) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));
  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 2")));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 1 !!!
!!! Failed to format chunk: ERRORS ERR 2 !!!
)");
}

TEST(LogBufferTest, RedactsLogs) {
  SimpleRedactor redactor;

  LogBuffer buffer(StorageSize::Megabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToMessage("log 1")));

  EXPECT_TRUE(buffer.Add(ToMessage("log 2")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2")));

  EXPECT_TRUE(buffer.Add(ToMessage("log 3")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 3")));

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 1")));

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 2")));
  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 2")));

  EXPECT_TRUE(buffer.Add(ToMessage("log 4")));

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 3")));

  EXPECT_TRUE(buffer.Add(ToMessage("log 4")));

  EXPECT_EQ(buffer.ToString(), R"([00001.010][00100][00101][REDACTED, REDACTED] INFO: REDACTED
!!! MESSAGE REPEATED 5 MORE TIMES !!!
!!! Failed to format chunk: ERRORS ERR 1 !!!
!!! Failed to format chunk: ERRORS ERR 2 !!!
!!! MESSAGE REPEATED 1 MORE TIME !!!
[00001.010][00100][00101][REDACTED, REDACTED] INFO: REDACTED
!!! Failed to format chunk: ERRORS ERR 3 !!!
[00001.010][00100][00101][REDACTED, REDACTED] INFO: REDACTED
)");
}

TEST(LogBufferTest, NotifyInterruption) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Gigabytes(100), &redactor);

  EXPECT_TRUE(buffer.Add(ToError("ERRORS ERR 0")));
  EXPECT_TRUE(buffer.Add(ToMessage("log 1", zx::sec(20))));

  EXPECT_EQ(buffer.ToString(), R"(!!! Failed to format chunk: ERRORS ERR 0 !!!
[00020.000][00100][00101][tag1, tag2] INFO: log 1
)");

  // Should be clear the buffer.
  buffer.NotifyInterruption();

  EXPECT_THAT(buffer.ToString(), IsEmpty());

  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(18))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(18))));
  EXPECT_TRUE(buffer.Add(ToMessage("log 2", zx::sec(19))));

  EXPECT_EQ(buffer.ToString(), R"([00018.000][00100][00101][tag1, tag2] INFO: log 2
!!! MESSAGE REPEATED 2 MORE TIMES !!!
)");
}

TEST(LogBufferTest, RunsActions) {
  IdentityRedactor redactor(inspect::BoolProperty{});

  LogBuffer buffer(StorageSize::Gigabytes(100), &redactor);

  bool run1{false};
  buffer.ExecuteAfter(zx::time_boot(0), [&run1] { run1 = true; });

  bool run2{false};
  buffer.ExecuteAfter(zx::time_boot(0), [&run2] { run2 = true; });

  bool run3{false};
  buffer.ExecuteAfter(zx::time_boot(zx::sec(5).to_nsecs()), [&run3] { run3 = true; });

  bool run4{false};
  buffer.ExecuteAfter(zx::time_boot(zx::sec(5).to_nsecs()), [&run4] { run4 = true; });

  bool run5{false};
  buffer.ExecuteAfter(zx::time_boot(zx::sec(7).to_nsecs()), [&run5] { run5 = true; });

  bool run6{false};
  buffer.ExecuteAfter(zx::time_boot(zx::sec(30).to_nsecs()), [&run6] { run6 = true; });

  buffer.Add(ToMessage("unused", zx::sec(0)));

  EXPECT_TRUE(run1);
  EXPECT_TRUE(run2);
  EXPECT_FALSE(run3);
  EXPECT_FALSE(run4);
  EXPECT_FALSE(run5);
  EXPECT_FALSE(run6);

  buffer.Add(ToMessage("unused", zx::sec(10)));

  EXPECT_TRUE(run1);
  EXPECT_TRUE(run2);
  EXPECT_TRUE(run3);
  EXPECT_TRUE(run4);
  EXPECT_TRUE(run5);
  EXPECT_FALSE(run6);

  buffer.NotifyInterruption();

  EXPECT_TRUE(run1);
  EXPECT_TRUE(run2);
  EXPECT_TRUE(run3);
  EXPECT_TRUE(run4);
  EXPECT_TRUE(run5);
  EXPECT_TRUE(run6);
}

}  // namespace
}  // namespace forensics::feedback
