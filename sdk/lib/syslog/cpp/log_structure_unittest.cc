// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/syslog/structured_backend/cpp/log_buffer.h>
#include <lib/zx/socket.h>

#include <atomic>
#include <cstring>
#include <thread>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/diagnostics/fake-log-sink/cpp/fake_log_sink.h"
#include "src/lib/files/file.h"
#include "src/lib/files/scoped_temp_dir.h"

namespace fuchsia_logging {

TEST(StructuredLogging, Log) {
  FX_LOG_KV(WARNING, "test_log", FX_KV("foo", "bar"));
  constexpr std::string_view kStringView = "string_view";
  FX_LOG_KV(WARNING, "test_log", FX_KV("foo", kStringView));
  const std::string kString = "string";
  FX_LOG_KV(WARNING, "test_log", FX_KV("foo", kString));
  // TODO(https://fxbug.dev/42135333): Figure out how to verify this appropriately.
}

class SideEffectTracker {
 public:
  explicit SideEffectTracker(bool* output) { *output = true; }
  operator int64_t() { return 0; }
};

TEST(StructuredLogging, NoSideEffectsIfLoggingIsDisabled) {
  bool called = false;
  FX_LOG_KV(DEBUG, "test", FX_KV("a", static_cast<int64_t>(SideEffectTracker(&called))));
  ASSERT_FALSE(called);
  FX_LOG_KV(INFO, "test", FX_KV("a", static_cast<int64_t>(SideEffectTracker(&called))));
  ASSERT_TRUE(called);
}

template <typename T>
static std::optional<std::string_view> ToStringView(T input) {
  return input;
}

TEST(StructuredLogging, NullSafeStringView) {
  // Construct from nullptr directly.
  ASSERT_EQ(ToStringView(internal::NullSafeStringView(nullptr)), std::nullopt);
  // Construct from nullptr via const char*.
  ASSERT_EQ(ToStringView(internal::NullSafeStringView(static_cast<const char*>(nullptr))),
            std::nullopt);
  // Construct from std::string
  ASSERT_EQ(ToStringView(internal::NullSafeStringView(std::string("test"))),
            std::string_view("test"));
  // Construct from non-null const char*
  ASSERT_EQ(ToStringView(internal::NullSafeStringView("test")), std::string_view("test"));
}

// Test to validate that SetLogSettings and log initialization is thread-safe.
TEST(StructuredLogging, ThreadInitialization) {
  // TODO(bbosak): Convert to actual stress test.
  auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
  FakeLogSink log_sink(Info, std::move(endpoints->server));
  LogSettingsBuilder().WithLogSink(endpoints->client.TakeChannel().release()).BuildAndInitialize();
  auto start = zx_clock_get_monotonic();
  std::atomic_bool running = true;
  std::thread thread_a([&]() {
    while (running) {
      auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
      FakeLogSink log_sink(Info, std::move(endpoints->server));
      LogSettingsBuilder()
          .WithLogSink(endpoints->client.TakeChannel().release())
          .BuildAndInitialize();
    }
  });
  std::thread thread_b([&]() {
    while (running) {
      FX_LOG_KV(WARNING, "test_log", FX_KV("foo", "bar"));
    }
  });
  while (true) {
    auto duration = (zx_clock_get_monotonic() - start);
    if (duration > ZX_SEC(4)) {
      running = false;
      break;
    }

    auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>();
    FakeLogSink log_sink(Info, std::move(endpoints->server));
    LogSettingsBuilder()
        .WithLogSink(endpoints->client.TakeChannel().release())
        .BuildAndInitialize();
    FX_LOG_KV(WARNING, "test_log", FX_KV("foo", "bar"));
  }
  thread_a.join();
  thread_b.join();
  LogSettingsBuilder builder;
  builder.BuildAndInitialize();
}

TEST(StructuredLogging, BackendDirect) {
  {
    LogBufferBuilder builder(LogSeverity::Warn);
    auto buffer =
        builder.WithFile("foo.cc", 42).WithCondition("condition").WithMsg("fake tag").Build();
    [[maybe_unused]] zx::result<> result = fuchsia_logging::FlushToGlobalLogger(buffer);
  }
  LogBufferBuilder builder(LogSeverity::Warn);
  auto buffer =
      builder.WithFile("foo.cc", 42).WithCondition("condition").WithMsg("fake tag").Build();
  buffer.WriteKeyValue("foo", static_cast<int64_t>(42));
  buffer.WriteKeyValue("bar", true);
  ASSERT_EQ(fuchsia_logging::FlushToGlobalLogger(buffer).status_value(), ZX_OK);
  // TODO(https://fxbug.dev/42135333): Figure out how to verify this appropriately.
}

TEST(StructuredLogging, Overflow) {
  std::vector<char> very_large_string;
  very_large_string.resize(1000 * 1000);
  memset(very_large_string.data(), 5, very_large_string.size());
  very_large_string[very_large_string.size() - 1] = 0;
  {
    LogBufferBuilder builder(LogSeverity::Warn);
    auto buffer =
        builder.WithFile("foo.cc", 42).WithCondition("condition").WithMsg("fake tag").Build();
    [[maybe_unused]] zx::result<> result = fuchsia_logging::FlushToGlobalLogger(buffer);
  }
  LogBufferBuilder builder(LogSeverity::Warn);
  auto buffer =
      builder.WithFile("foo.cc", 42).WithCondition("condition").WithMsg("fake tag").Build();
  buffer.WriteKeyValue("foo", static_cast<int64_t>(42));
  buffer.WriteKeyValue("bar", very_large_string.data());

  ASSERT_EQ(fuchsia_logging::FlushToGlobalLogger(buffer).status_value(), ZX_ERR_INVALID_ARGS);
}

TEST(StructuredLogging, LOGS) {
  std::string str;
  // 5mb log shouldn't crash
  str.resize(1000 * 5000);
  memset(str.data(), 's', str.size() - 1);
  FX_LOGS(INFO) << str;
}

}  // namespace fuchsia_logging
