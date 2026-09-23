// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/availability.h>

#if FUCHSIA_API_LEVEL_LESS_THAN(29)

#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.logger/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/syslog/global.h>
#include <lib/syslog/logger.h>
#include <lib/syslog/wire_format.h>
#include <poll.h>
#include <string.h>
#include <unistd.h>

#include <utility>

#include <fbl/string.h>
#include <fbl/string_printf.h>
#include <fbl/unique_fd.h>
#include <sdk/lib/diagnostics/reader/cpp/logs.h>
#include <sdk/lib/syslog/cpp/log_level.h>
#include <zxtest/zxtest.h>

#include "src/lib/diagnostics/log/message/rust/cpp-log-decoder/log_decoder.h"
#include "zircon/system/ulib/syslog/helpers.h"

namespace {

const char* kFileName = syslog::internal::StripPath(__FILE__);
const char* kFilePath = syslog::internal::StripDots(__FILE__);

class FakeLogSink {
 public:
  explicit FakeLogSink(fidl::ServerEnd<fuchsia_logger::LogSink> server_end)
      : loop_(std::make_unique<async::Loop>(&kAsyncLoopConfigNeverAttachToThread)),
        impl_(std::make_shared<Impl>()) {
    loop_->StartThread();

    fidl::BindServer(loop_->dispatcher(), std::move(server_end), impl_);
  }

  FakeLogSink(FakeLogSink&&) = default;
  FakeLogSink& operator=(FakeLogSink&&) = default;

  std::optional<diagnostics::reader::LogsData> ReadLogsData() {
    auto record = impl_->ReadRecord();
    if (record.empty()) {
      return {};
    }
    auto raw_message = fuchsia_decode_log_message_to_json(record.data(), record.size());
    rapidjson::Document document;
    document.Parse(raw_message);
    fuchsia_free_decoded_log_message(raw_message);
    rapidjson::Document log;
    log.CopyFrom(document.GetArray()[0], log.GetAllocator());
    return diagnostics::reader::LogsData(std::move(log));
  }

  // Returns true if a record is available for reading.
  bool WaitForRecord(zx::time deadline) const { return impl_->WaitForRecord(deadline); }

 private:
  class Impl : public fidl::Server<fuchsia_logger::LogSink> {
   public:
    Impl() = default;

    bool WaitForRecord(zx::time deadline) const {
      std::unique_lock lock(mutex_);
      return WaitForRecord(lock, deadline) != 0;
    }

    std::vector<uint8_t> ReadRecord() {
      std::unique_lock lock(mutex_);
      size_t amount = WaitForRecord(lock, zx::time::infinite());
      std::vector<uint8_t> buffer(amount);
      size_t actual;
      if (socket_.read(0, buffer.data(), buffer.size(), &actual) != ZX_OK) {
        return {};
      };
      buffer.resize(actual);
      return buffer;
    }

   private:
    size_t WaitForRecord(std::unique_lock<std::mutex>& lock, zx::time deadline) const {
      condition_.wait(lock, [this] { return socket_.is_valid(); });
      zx_info_socket_t info;
      for (;;) {
        if (socket_.get_info(ZX_INFO_SOCKET, &info, sizeof(info), nullptr, nullptr) != ZX_OK) {
          return 0;
        }
        if (info.rx_buf_available > 0) {
          return info.rx_buf_available;
        }
        if (socket_.wait_one(ZX_SOCKET_READABLE, deadline, nullptr) != ZX_OK) {
          return 0;
        }
      }
    }

    void ConnectStructured(ConnectStructuredRequest& request,
                           ConnectStructuredCompleter::Sync& completer) override {
      std::unique_lock lock(mutex_);
      socket_ = std::move(request.socket());
      condition_.notify_all();
    }

    void WaitForInterestChange(WaitForInterestChangeCompleter::Sync& completer) override {
      ZX_PANIC("Unexpected call to WaitForInterestChange");
    }

    void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_logger::LogSink> metadata,
                               fidl::UnknownMethodCompleter::Sync& completer) override {
      ZX_PANIC("Unexpected call to handle_unknown_method");
    }

    mutable std::mutex mutex_;
    mutable std::condition_variable condition_;
    zx::socket socket_;
  };

  std::unique_ptr<async::Loop> loop_;
  std::shared_ptr<Impl> impl_;
};

zx::result<FakeLogSink> init_helper(std::span<const char*> tags = {},
                                    fx_log_severity_t severity = FX_LOG_INFO) {
  if (auto endpoints = fidl::CreateEndpoints<fuchsia_logger::LogSink>(); endpoints.is_error()) {
    return endpoints.take_error();
  } else {
    FakeLogSink sink(std::move(endpoints->server));
    fx_logger_config_t config = {
        .min_severity = severity,
        .log_sink_channel = endpoints->client.TakeChannel().release(),
        .tags = tags.data(),
        .num_tags = tags.size(),
    };
    if (zx_status_t status = fx_log_reconfigure(&config); status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(sink));
  }
}

bool ends_with(const char* str, const fbl::String& suffix) {
  size_t str_len = strlen(str);
  size_t suffix_len = suffix.size();
  if (str_len < suffix_len) {
    return false;
  }
  str += str_len - suffix_len;
  return strcmp(str, suffix.c_str()) == 0;
}

void output_compare_helper(FakeLogSink& sink, fx_log_severity_t severity, const char* msg, int line,
                           std::span<const char*> tags = {}) {
  auto message = sink.ReadLogsData();
  ASSERT_TRUE(message);
  ASSERT_EQ(message->metadata().tags.size(), tags.size());
  const char* file = severity > FX_LOG_INFO ? kFilePath : kFileName;
  for (size_t i = 0; i < tags.size(); ++i) {
    ASSERT_EQ(message->metadata().tags[i], tags[i]);
  }
  EXPECT_EQ(message->message(), std::string(msg));
  if (message->metadata().file.has_value()) {
    EXPECT_EQ(message->metadata().file, std::string(file));
  }
  EXPECT_EQ(message->metadata().line, line);
}

TEST(SyslogSocketTests, TestLogSimpleWrite) {
  auto sink = init_helper();
  ASSERT_OK(sink);
  const char* msg = "test message";
  int line = __LINE__ + 1;
  FX_LOG(INFO, nullptr, msg);
  output_compare_helper(*sink, FX_LOG_INFO, msg, line);
}

TEST(SyslogSocketTests, TestLogWrite) {
  auto sink = init_helper();
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_LOGF(INFO, nullptr, "%d, %s", 10, "just some number");
  output_compare_helper(*sink, FX_LOG_INFO, "10, just some number", line);
}

TEST(SyslogSocketTests, TestLogPreprocessedMessage) {
  auto sink = init_helper();
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_LOG(INFO, nullptr, "%d, %s");
  output_compare_helper(*sink, FX_LOG_INFO, "%d, %s", line);
}

TEST(SyslogSocketTests, TestLogSeverity) {
  auto sink = init_helper();
  ASSERT_OK(sink);

  FX_LOG_SET_SEVERITY(WARNING);
  FX_LOGF(INFO, nullptr, "%d, %s", 10, "just some number");
  EXPECT_FALSE(sink->WaitForRecord(zx::time::infinite_past()));

  int line = __LINE__ + 1;
  FX_LOGF(WARNING, nullptr, "%d, %s", 10, "just some number");
  output_compare_helper(*sink, FX_LOG_WARNING, "10, just some number", line);
}

TEST(SyslogSocketTests, TestLogWriteWithTag) {
  auto sink = init_helper();
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_LOGF(INFO, "tag", "%d, %s", 10, "just some string");
  const char* tags[] = {"tag"};
  output_compare_helper(*sink, FX_LOG_INFO, "10, just some string", line, tags);
}

TEST(SyslogSocketTests, TestLogWriteWithGlobalTag) {
  const char* gtags[] = {"gtag"};
  auto sink = init_helper(gtags);
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_LOGF(INFO, "tag", "%d, %s", 10, "just some string");
  const char* tags[] = {"gtag", "tag"};
  output_compare_helper(*sink, FX_LOG_INFO, "10, just some string", line, tags);
}

TEST(SyslogSocketTests, TestLogWriteWithMultiGlobalTag) {
  const char* gtags[] = {"gtag", "gtag2"};
  auto sink = init_helper(gtags);
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_LOGF(INFO, "tag", "%d, %s", 10, "just some string");
  const char* tags[] = {"gtag", "gtag2", "tag"};
  output_compare_helper(*sink, FX_LOG_INFO, "10, just some string", line, tags);
}

TEST(SyslogSocketTests, TestGetTags) {
  const char* tags[] = {"gtag", "gTag"};
  auto sink = init_helper(tags);
  ASSERT_OK(sink);
  std::vector<std::string_view> logger_tags;
  fx_logger_get_tags(
      fx_log_get_logger(),
      [](void* context, const char* tag) {
        static_cast<decltype(&logger_tags)>(context)->push_back(tag);
      },
      &logger_tags);
  EXPECT_EQ(logger_tags.size(), std::size(tags));
  EXPECT_EQ(logger_tags[0], tags[0]);
  EXPECT_EQ(logger_tags[1], tags[1]);
}

TEST(SyslogSocketTests, TestLogFallback) {
  const char* gtags[] = {"gtag", "gtag2"};
  auto sink = init_helper(gtags);
  ASSERT_OK(sink);

  int pipefd[2];
  EXPECT_EQ(pipe2(pipefd, O_NONBLOCK), 0);
  fbl::unique_fd fd_to_close1(pipefd[0]);
  fbl::unique_fd fd_to_close2(pipefd[1]);
  fx_logger_activate_fallback(fx_log_get_logger(), pipefd[0]);

  int line = __LINE__ + 1;
  FX_LOGF(INFO, "tag", "%d, %s", 10, "just some string");

  char buf[256];
  size_t n = read(pipefd[1], buf, sizeof(buf));
  EXPECT_GT(n, 0u);
  buf[n] = 0;
  EXPECT_TRUE(
      ends_with(buf, fbl::StringPrintf("[gtag, gtag2, tag] INFO: [%s(%d)] 10, just some string\n",
                                       kFileName, line)),
      "%s", buf);
}

TEST(SyslogSocketTests, TestVlogSimpleWrite) {
  auto sink = init_helper({}, 1);  // 1 is INFO-1
  ASSERT_OK(sink);
  const char* msg = "test message";
  int line = __LINE__ + 1;
  FX_VLOG(1, nullptr, msg);
  output_compare_helper(*sink, (FX_LOG_INFO - 1), msg, line);
}

TEST(SyslogSocketTests, TestWriteWithNullptrFile) {
  // Ensure that we support nullptr filenames. See b/350577005 for justification.
  auto sink = init_helper();
  ASSERT_OK(sink);
  const char* msg = "test message";
  fx_logger_t* logger = fx_log_get_logger();
  fx_logger_log(logger, FX_LOG_INFO, nullptr, msg);
  output_compare_helper(*sink, FX_LOG_INFO, msg, 0);
}

TEST(SyslogSocketTests, TestVlogWrite) {
  auto sink = init_helper({}, 1);  // 1 is INFO-1
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_VLOGF(1, nullptr, "%d, %s", 10, "just some number");
  output_compare_helper(*sink, (FX_LOG_INFO - 1), "10, just some number", line);
}

TEST(SyslogSocketTests, TestVlogWriteWithTag) {
  auto sink = init_helper({}, 5);  // INFO-5
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_VLOGF(5, "tag", "%d, %s", 10, "just some string");
  const char* tags[] = {"tag"};
  output_compare_helper(*sink, (FX_LOG_INFO - 5), "10, just some string", line, tags);
}

TEST(SyslogSocketTests, TestLogVerbosity) {
  auto sink = init_helper();
  ASSERT_OK(sink);

  FX_VLOGF(10, nullptr, "%d, %s", 10, "just some number");
  EXPECT_FALSE(sink->WaitForRecord(zx::time::infinite_past()));

  FX_VLOGF(1, nullptr, "%d, %s", 10, "just some number");
  EXPECT_FALSE(sink->WaitForRecord(zx::time::infinite_past()));

  FX_LOG_SET_VERBOSITY(1);  // INFO - 1
  int line = __LINE__ + 1;
  FX_VLOGF(1, nullptr, "%d, %s", 10, "just some number");
  output_compare_helper(*sink, (FX_LOG_INFO - 1), "10, just some number", line);
}

TEST(SyslogSocketTests, TestLogReconfiguration) {
  // Initialize with no tags.
  auto sink = init_helper();
  ASSERT_OK(sink);
  int line = __LINE__ + 1;
  FX_LOG(INFO, NULL, "Hi");
  output_compare_helper(*sink, FX_LOG_INFO, "Hi", line);

  // Now reconfigure the logger and add tags.
  const char* tags[] = {"tag1", "tag2"};

  auto new_sink = init_helper(tags);
  ASSERT_OK(new_sink);

  line = __LINE__ + 1;
  FX_LOG(INFO, NULL, "Hi");
  output_compare_helper(*new_sink, FX_LOG_INFO, "Hi", line, tags);
}

}  // namespace

#endif  // FUCHSIA_API_LEVEL_LESS_THAN(29)
