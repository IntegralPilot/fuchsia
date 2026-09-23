// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fuchsia/logger/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>

#include <fbl/string_printf.h>
#include <zxtest/zxtest.h>

class LogTest : public zxtest::Test {};

namespace {

class LogListener : public fuchsia::logger::LogListenerSafe {
 public:
  explicit LogListener(fit::function<void(const fuchsia::logger::LogMessage&)> message_callback,
                       fidl::InterfaceRequest<fuchsia::logger::LogListenerSafe> channel)
      : message_callback_(std::move(message_callback)) {
    binding_.Bind(std::move(channel));
  }

  /// Called for single messages.
  ///
  /// The return value is used for flow control, and implementers should acknowledge receipt of
  /// each message in order to continue receiving future messages.
  void Log(fuchsia::logger::LogMessage log, LogCallback callback) override {
    message_callback_(log);
    callback();
  }

  /// Called when serving cached logs.
  ///
  /// Max logs size per call is `MAX_LOG_MANY_SIZE_BYTES` bytes.
  ///
  /// The return value is used for flow control, and implementers should acknowledge receipt of
  /// each batch in order to continue receiving future messages.
  void LogMany(std::vector<fuchsia::logger::LogMessage> log, LogManyCallback callback) override {
    for (const fuchsia::logger::LogMessage& msg : log) {
      message_callback_(msg);
    }
    callback();
  }

  /// Called when this listener was passed to `DumpLogsSafe()` and all cached logs have been sent.
  void Done() override {}

 private:
  fidl::Binding<fuchsia::logger::LogListenerSafe> binding_{this};
  fit::function<void(const fuchsia::logger::LogMessage&)> message_callback_;
};

TEST_F(LogTest, LegacyFormat) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  constexpr const char payload[] = "test message";
  constexpr const char tag1[] = "test1";
  constexpr const char tag2[] = "test2";
  constexpr size_t line = __LINE__ + 1;
  FX_LOG_KV(INFO, payload, FX_KV("tag", tag1), FX_KV("tag", tag2));
  fuchsia::logger::LogPtr accessor;
  std::unique_ptr context = sys::ComponentContext::Create();
  ASSERT_OK(context->svc()->Connect(accessor.NewRequest()));
  fidl::InterfaceHandle<fuchsia::logger::LogListenerSafe> handle;
  fidl::InterfaceRequest request = handle.NewRequest();
  accessor->ListenSafe(std::move(handle), nullptr);
  LogListener listener(
      [&](const fuchsia::logger::LogMessage& message) {
        auto quit = fit::defer([&] { loop.Quit(); });
        const fbl::String msg = fbl::StringPrintf(
            "[sdk/ctf/tests/fidl/fuchsia.diagnostics/logging_test.cc(%zu)] %s", line, payload);
        ASSERT_EQ(message.msg, msg);
        const std::vector<std::string> tags = {tag1, tag2};
        ASSERT_EQ(message.tags, tags);
      },
      std::move(request));
  loop.Run();
}
}  // namespace
