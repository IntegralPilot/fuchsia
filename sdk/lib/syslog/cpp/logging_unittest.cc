// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <functional>
#include <mutex>
#include <optional>
#include <string>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "lib/syslog/cpp/log_level.h"
#ifndef __Fuchsia__
#include "host/encoder.h"
#endif
#include <zircon/types.h>

#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"
#include "src/lib/uuid/uuid.h"

#ifdef __Fuchsia__
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/dispatcher.h>
#include <lib/async/wait.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/structured_backend/cpp/log_connection.h>
#include <lib/syslog/structured_backend/cpp/logger.h>
#include <lib/zx/socket.h>

#include <cinttypes>

#include <rapidjson/document.h>

#include "src/lib/diagnostics/fake-log-sink/cpp/fake_log_sink.h"
#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/string_printf.h"
#endif

namespace fuchsia_logging {

namespace {
std::chrono::high_resolution_clock::time_point mock_time = std::chrono::steady_clock::now();

using ::testing::HasSubstr;

}  // namespace

namespace {
class LoggingFixture : public ::testing::Test {
 public:
  LoggingFixture() : old_severity_(GetMinLogSeverity()), old_stderr_(dup(STDERR_FILENO)) {}
  ~LoggingFixture() {
    LogSettingsBuilder builder;
    builder.WithMinLogSeverity(old_severity_);
    builder.BuildAndInitialize();
  }

 private:
  RawLogSeverity old_severity_;
  int old_stderr_;
};

using LoggingFixtureDeathTest = LoggingFixture;
#ifdef __Fuchsia__
constexpr std::string SeverityToString(const fuchsia_diagnostics_types::Severity severity) {
  switch (severity) {
    case fuchsia_diagnostics_types::Severity::kTrace:
      return "TRACE";
    case fuchsia_diagnostics_types::Severity::kDebug:
      return "DEBUG";
    case fuchsia_diagnostics_types::Severity::kInfo:
      return "INFO";
    case fuchsia_diagnostics_types::Severity::kWarn:
      return "WARN";
    case fuchsia_diagnostics_types::Severity::kError:
      return "ERROR";
    case fuchsia_diagnostics_types::Severity::kFatal:
      return "FATAL";
    default:
      return "INVALID";
  }
}

std::string Format(const std::optional<diagnostics::reader::LogsData>& data) {
  if (!data) {
    return "<invalid>\n";
  }
  std::string result = std::format(
      "[{:05}.{:03}][{:05}][{:05}][{}] {}: [{}({})]", data->metadata().timestamp / 1000000000ULL,
      (data->metadata().timestamp / 1000000ULL) % 1000ULL, data->metadata().pid.value_or(0),
      data->metadata().tid.value_or(0), fxl::JoinStrings(data->metadata().tags, ", "),
      SeverityToString(data->metadata().severity), data->metadata().file.value_or(""),
      data->metadata().line.value_or(0));
  if (!data->message().empty()) {
    result.append(std::format(" {}", data->message()));
  }
  for (const auto& key : data->keys()) {
    result.append(std::format(" {}=", key.name()));
    switch (key.format()) {
      case inspect::PropertyFormat::kInt:
        result.append(std::format("{}", key.Get<inspect::IntPropertyValue>().value()));
        break;
      case inspect::PropertyFormat::kUint:
        result.append(std::format("{}", key.Get<inspect::UintPropertyValue>().value()));
        break;
      case inspect::PropertyFormat::kDouble:
        result.append(std::format("{}", key.Get<inspect::DoublePropertyValue>().value()));
        break;
      case inspect::PropertyFormat::kString:
        result.push_back('"');
        for (char c : key.Get<inspect::StringPropertyValue>().value()) {
          if (c == '\"') {
            result.push_back('\\');
          }
          result.push_back(c);
        }
        result.push_back('"');
        break;
      case inspect::PropertyFormat::kBool:
        if (key.Get<inspect::BoolPropertyValue>().value()) {
          result.append("true");
        } else {
          result.append("false");
        }
        break;
      default:
        ADD_FAILURE() << "Unexpected type for " << key.name() << ": "
                      << static_cast<uint8_t>(key.format());
    }
  }
  result.append("\n");
  return result;
}

#endif

#ifdef __Fuchsia__
using LogState = FakeLogSink;
LogState SetupLogs(bool wait_for_initial_interest = true) { return FakeLogSink(); }

std::string ReadLogs(FakeLogSink& log_sink) {
  std::string result;
  while (log_sink.WaitForRecord(zx::time::infinite_past())) {
    result.append(Format(log_sink.ReadLogsData()));
  }
  return result;
}
#else

struct TestLogState {
  files::ScopedTempDir temp_dir;
  std::string log_file;
};

using LogState = std::unique_ptr<TestLogState>;

LogState SetupLogs(bool wait_for_initial_interest = true) {
  auto state = std::make_unique<TestLogState>();
  {
    std::string log_file_out;
    state->temp_dir.NewTempFile(&log_file_out);
    state->log_file = std::move(log_file_out);
  }
  LogSettingsBuilder builder;
  builder.WithLogFile(state->log_file);
  builder.BuildAndInitialize();
  return state;
}

std::string ReadLogs(LogState& state) {
  std::string log;
  files::ReadFileToString(state->log_file, &log);
  return log;
}
#endif

TEST_F(LoggingFixture, Log) {
  LogState state = SetupLogs();

  int error_line = __LINE__ + 1;
  FX_LOGS(ERROR) << "something at error";

  int info_line = __LINE__ + 1;
  FX_LOGS(INFO) << "and some other at info level";

  std::string log = ReadLogs(state);

  EXPECT_THAT(log, HasSubstr("ERROR: [sdk/lib/syslog/cpp/logging_unittest.cc(" +
                             std::to_string(error_line) + ")] something at error"));

  EXPECT_THAT(log, HasSubstr("INFO: [logging_unittest.cc(" + std::to_string(info_line) +
                             ")] and some other at info level"));
}

TEST_F(LoggingFixture, LogFirstN) {
  constexpr int kLimit = 5;
  constexpr int kCycles = 20;
  constexpr const char* kLogMessage = "Hello";
  static_assert(kCycles > kLimit);

  LogState state = SetupLogs();

  for (int i = 0; i < kCycles; ++i) {
    FX_LOGS_FIRST_N(ERROR, kLimit) << kLogMessage;
  }

  std::string log = ReadLogs(state);

  int count = 0;
  size_t pos = 0;
  while ((pos = log.find(kLogMessage, pos)) != std::string::npos) {
    ++count;
    ++pos;
  }
  EXPECT_EQ(kLimit, count);
}

TEST_F(LoggingFixture, LogT) {
  LogState state = SetupLogs();

  int error_line = __LINE__ + 1;
  FX_LOGST(ERROR, "first") << "something at error";

  int info_line = __LINE__ + 1;
  FX_LOGST(INFO, "second") << "and some other at info level";

  std::string log = ReadLogs(state);

  EXPECT_THAT(log, HasSubstr("first] ERROR: [sdk/lib/syslog/cpp/logging_unittest.cc(" +
                             std::to_string(error_line) + ")] something at error"));

  EXPECT_THAT(log, HasSubstr("second] INFO: [logging_unittest.cc(" + std::to_string(info_line) +
                             ")] and some other at info level"));
}

TEST_F(LoggingFixtureDeathTest, CheckFailed) { ASSERT_DEATH(FX_CHECK(false), ""); }

#if defined(__Fuchsia__)
TEST_F(LoggingFixture, Plog) {
  FakeLogSink sink;

  FX_PLOGS(ERROR, ZX_OK) << "should be ok";
  FX_PLOGS(ERROR, ZX_ERR_ACCESS_DENIED) << "got access denied";

  EXPECT_THAT(sink.ReadLogsData()->message(), HasSubstr("should be ok: 0 (ZX_OK)"));
  EXPECT_THAT(sink.ReadLogsData()->message(),
              HasSubstr("got access denied: -30 (ZX_ERR_ACCESS_DENIED)"));
}

TEST_F(LoggingFixture, PlogT) {
  FakeLogSink sink;

  int line1 = __LINE__ + 1;
  FX_PLOGST(ERROR, "abcd", ZX_OK) << "should be ok";

  int line2 = __LINE__ + 1;
  FX_PLOGST(ERROR, "qwerty", ZX_ERR_ACCESS_DENIED) << "got access denied";

  EXPECT_THAT(Format(sink.ReadLogsData()),
              HasSubstr("abcd] ERROR: [sdk/lib/syslog/cpp/logging_unittest.cc(" +
                        std::to_string(line1) + ")] should be ok: 0 (ZX_OK)"));
  EXPECT_THAT(
      Format(sink.ReadLogsData()),
      HasSubstr("qwerty] ERROR: [sdk/lib/syslog/cpp/logging_unittest.cc(" + std::to_string(line2) +
                ")] got access denied: -30 (ZX_ERR_ACCESS_DENIED)"));
}
#endif  // defined(__Fuchsia__)

TEST_F(LoggingFixture, SLog) {
  LogState state = SetupLogs(false);
  std::string log_id = uuid::Generate();

  int line1 = __LINE__ + 1;
  FX_LOG_KV(ERROR, nullptr, FX_KV("some_msg", "String log"));

  int line2 = __LINE__ + 1;
  FX_LOG_KV(ERROR, nullptr, FX_KV("some_msg", 42));

  int line4 = __LINE__ + 1;
  FX_LOG_KV(ERROR, "msg", FX_KV("first", 42), FX_KV("second", "string"));

  int line5 = __LINE__ + 1;
  FX_LOG_KV(ERROR, "String log");

  int line6 = __LINE__ + 1;
  FX_LOG_KV(ERROR, nullptr, FX_KV("float", 0.25f));

  int line7 = __LINE__ + 1;
  FX_LOG_KV(ERROR, "String with quotes", FX_KV("value", "char is '\"'"));

  std::string log = ReadLogs(state);
  EXPECT_THAT(log, HasSubstr("ERROR: [" + std::string("sdk/lib/syslog/cpp/logging_unittest.cc") +
                             "(" + std::to_string(line1) + ")] some_msg=\"String log\""));
  EXPECT_THAT(log, HasSubstr("ERROR: [" + std::string("sdk/lib/syslog/cpp/logging_unittest.cc") +
                             "(" + std::to_string(line2) + ")] some_msg=42"));
  EXPECT_THAT(log, HasSubstr("ERROR: [" + std::string("sdk/lib/syslog/cpp/logging_unittest.cc") +
                             "(" + std::to_string(line4) + ")] msg first=42 second=\"string\""));
  EXPECT_THAT(log, HasSubstr("ERROR: [" + std::string("sdk/lib/syslog/cpp/logging_unittest.cc") +
                             "(" + std::to_string(line5) + ")] String log"));
  EXPECT_THAT(log, HasSubstr("ERROR: [" + std::string("sdk/lib/syslog/cpp/logging_unittest.cc") +
                             "(" + std::to_string(line6) + ")] float=0.25"));

  EXPECT_THAT(log,
              HasSubstr("ERROR: [" + std::string("sdk/lib/syslog/cpp/logging_unittest.cc") + "(" +
                        std::to_string(line7) + ")] String with quotes value=\"char is '\\\"'\""));
}

TEST_F(LoggingFixture, BackendDirect) {
  LogState state = SetupLogs(false);

  {
    LogBufferBuilder builder(LogSeverity::Error);
    auto buffer =
        builder.WithFile("foo.cc", 42).WithMsg("Log message").WithCondition("condition").Build();
    buffer.WriteKeyValue("tag", "fake tag");
    EXPECT_EQ(FlushToGlobalLogger(buffer).status_value(), ZX_OK);
  }
  LogBufferBuilder builder(LogSeverity::Error);
  auto buffer =
      builder.WithMsg("fake message").WithCondition("condition").WithFile("foo.cc", 42).Build();
  buffer.WriteKeyValue("tag", "fake tag");
  buffer.WriteKeyValue("foo", static_cast<int64_t>(42));
  EXPECT_EQ(FlushToGlobalLogger(buffer).status_value(), ZX_OK);

  std::string log = ReadLogs(state);
  EXPECT_THAT(log, HasSubstr("ERROR: [foo.cc(42)] Check failed: condition. Log message\n"));
  EXPECT_THAT(log, HasSubstr("ERROR: [foo.cc(42)] Check failed: condition. fake message foo=42\n"));
}

TEST_F(LoggingFixture, MacroCompilationTest) {
  uint8_t zero = 0;
  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<uint16_t>(zero)));
  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<uint32_t>(zero)));
  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<uint64_t>(zero)));
  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<size_t>(zero)));

  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<int16_t>(zero)));
  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<int32_t>(zero)));
  FX_LOG_KV(DEBUG, "test log", FX_KV("key", static_cast<int64_t>(zero)));
}

TEST(StructuredLogging, LOGS) {
  std::string str;
  // 5mb log shouldn't crash
  str.resize(1000 * 5000);
  memset(str.data(), 's', str.size() - 1);
  FX_LOGS(INFO) << str;
}

#ifndef __Fuchsia__
TEST(StructuredLogging, Remaining) {
  LogSettingsBuilder builder;
  std::string log_file;
  files::ScopedTempDir temp_dir;
  ASSERT_TRUE(temp_dir.NewTempFile(&log_file));
  builder.WithLogFile(log_file);
  builder.BuildAndInitialize();
  LogBufferBuilder builder2(LogSeverity::Info);
  auto buffer = builder2.WithFile("test", 5).WithMsg("test_msg").Build();
  auto header = internal::MsgHeader::CreatePtr(&buffer);
  auto initial = header->RemainingSpace();
  header->WriteChar('t');
  ASSERT_EQ(header->RemainingSpace(), initial - 1);
  header->WriteString("est");
  ASSERT_EQ(header->RemainingSpace(), initial - 4);
}

TEST(StructuredLogging, FlushAndReset) {
  LogBufferBuilder builder(LogSeverity::Info);
  auto buffer = builder.WithFile("test", 5).WithMsg("test_msg").Build();
  auto header = internal::MsgHeader::CreatePtr(&buffer);
  auto initial = header->RemainingSpace();
  header->WriteString("test");
  ASSERT_EQ(header->RemainingSpace(), initial - 4);
  header->FlushAndReset();
  ASSERT_EQ(header->RemainingSpace(),
            LogBuffer::data_size() - 2);  // last byte reserved for NULL terminator
}
#endif

#ifdef __Fuchsia__

TEST(LogConnection, Basic) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
  FakeLogSink log_sink(FUCHSIA_LOG_INFO, std::move(endpoints->server));

  auto result = internal::LogConnection::Create(std::move(endpoints->client));
  ASSERT_EQ(result.status_value(), ZX_OK);

  auto [connection, min_severity] = *std::move(result);

  ASSERT_TRUE(connection.IsValid());

  LogBuffer buffer;
  buffer.BeginRecord(FUCHSIA_LOG_INFO, {}, {}, "foo", 1, 2, 3);
  ASSERT_EQ(connection.FlushBuffer(buffer).status_value(), ZX_OK);

  std::vector<uint8_t> record = log_sink.ReadRecord();

  std::span<const uint8_t> span = buffer.EndRecord();
  ASSERT_EQ(record.size(), span.size());
  EXPECT_EQ(memcmp(record.data(), span.data(), record.size()), 0);
}

TEST(LogConnection, EncodingError) {
  auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
  FakeLogSink log_sink(FUCHSIA_LOG_INFO, std::move(endpoints->server));

  auto result = internal::LogConnection::Create(std::move(endpoints->client));
  ASSERT_EQ(result.status_value(), ZX_OK);

  auto [connection, min_severity] = *std::move(result);

  ASSERT_TRUE(connection.IsValid());

  LogBuffer buffer;
  std::string message;
  message.resize(sizeof internal::LogBufferData::data, 'a');

  // This should result in an invalid message because it's too big.
  buffer.BeginRecord(FUCHSIA_LOG_INFO, {}, 0, message, 0, 1, 2);

  EXPECT_EQ(connection.FlushBuffer(buffer).status_value(), ZX_ERR_INVALID_ARGS);
}

TEST(Logger, LocalLogger) {
  Logger logger;

  // `logger` should be destroyed after `loop`.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);

  ASSERT_EQ(loop.StartThread(), ZX_OK);

  auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
  FakeLogSink log_sink(FUCHSIA_LOG_INFO, std::move(endpoints->server));

  struct Context {
    std::mutex mutex;
    std::condition_variable condition;
    RawLogSeverity severity = 0;
  } context;

  auto logger_result = Logger::Create(RawLogSettings{
      .log_sink = endpoints->client.TakeChannel().release(),
      .dispatcher = loop.dispatcher(),
      .severity_change_callback =
          +[](void* context_in, RawLogSeverity s) {
            Context* context = static_cast<Context*>(context_in);
            std::unique_lock lock(context->mutex);
            context->severity = s;
            context->condition.notify_all();
          },
      .severity_change_callback_context = &context,
  });
  ASSERT_EQ(logger_result.status_value(), ZX_OK);
  logger = *std::move(logger_result);

  LogBuffer buffer;
  buffer.BeginRecord(FUCHSIA_LOG_INFO, {}, {}, "foo", 1, 2, 3);

  ASSERT_EQ(logger.FlushBuffer(buffer).status_value(), ZX_OK);

  std::vector<uint8_t> record = log_sink.ReadRecord();

  std::span<const uint8_t> span = buffer.EndRecord();
  ASSERT_EQ(record.size(), span.size());
  EXPECT_EQ(memcmp(record.data(), span.data(), record.size()), 0);

  // Make sure a severity update gets through.
  log_sink.SetSeverity(FUCHSIA_LOG_WARNING);
  std::unique_lock lock(context.mutex);
  context.condition.wait(lock, [&] { return context.severity == FUCHSIA_LOG_WARNING; });

  EXPECT_EQ(logger.GetMinSeverity(), FUCHSIA_LOG_WARNING);
}

#endif

}  // namespace
}  // namespace fuchsia_logging
