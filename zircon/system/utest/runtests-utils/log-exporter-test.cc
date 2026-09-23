// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/txn_header.h>
#include <lib/zx/time.h>

#include <utility>

#include <fbl/string.h>
#include <fbl/vector.h>
#include <runtests-utils/log-exporter.h>
#include <zxtest/zxtest.h>

namespace runtests {
namespace {

fuchsia_logger::wire::LogMessage CreateLogMessage(fuchsia_logger::wire::LogMessage lm) {
  if (lm.pid == 0) {
    lm.pid = 1024;
  }
  lm.tid = 1034;
  lm.time = zx::time_boot(93892493921);
  lm.severity = static_cast<int32_t>(fuchsia_logger::wire::LogLevelFilter::kInfo);
  return lm;
}

TEST(LogListenerTests, TestLog) {
  auto [listener, listener_request] = fidl::Endpoints<fuchsia_logger::LogListenerSafe>::Create();

  // We expect the log file to be much smaller than this.
  char buf[1024];
  memset(buf, 0, sizeof(buf));
  FILE* buf_file = fmemopen(buf, sizeof(buf), "w");

  // start listener
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  LogExporter log_exporter(
      buf_file, loop.dispatcher(), std::move(listener_request),
      [](zx_status_t status) { EXPECT_STATUS(ZX_ERR_CANCELED, status); }, nullptr);
  fidl::WireClient client{std::move(listener), loop.dispatcher()};

  client
      ->Log(CreateLogMessage({
          .msg = "my message",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());
  {
    fidl::StringView tags[] = {"tag123"};
    client
        ->Log(CreateLogMessage({
            .tags = fidl::VectorView<fidl::StringView>::FromExternal(tags),
            .msg = "my message",

        }))
        .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
          ASSERT_OK(result);
        });
  }
  ASSERT_OK(loop.RunUntilIdle());

  fflush(buf_file);
  ASSERT_STREQ(R"([00093.892493][1024][1034][] INFO: my message
[00093.892493][1024][1034][tag123] INFO: my message
)",
               buf);

  {
    fidl::StringView tags[] = {"tag123", "tag2"};
    client
        ->Log(CreateLogMessage({
            .tags = fidl::VectorView<fidl::StringView>::FromExternal(tags),
            .msg = "my message",
        }))
        .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
          ASSERT_OK(result);
        });
  }
  ASSERT_OK(loop.RunUntilIdle());

  fflush(buf_file);
  ASSERT_STREQ(R"([00093.892493][1024][1034][] INFO: my message
[00093.892493][1024][1034][tag123] INFO: my message
[00093.892493][1024][1034][tag123, tag2] INFO: my message
)",
               buf);
}

TEST(LogListenerTests, TestLogMany) {
  auto [listener, listener_request] = fidl::Endpoints<fuchsia_logger::LogListenerSafe>::Create();

  // We expect the log file to be much smaller than this.
  char buf[1024];
  memset(buf, 0, sizeof(buf));
  FILE* buf_file = fmemopen(buf, sizeof(buf), "w");

  // start listener
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  LogExporter log_exporter(
      buf_file, loop.dispatcher(), std::move(listener_request),
      [](zx_status_t status) { EXPECT_STATUS(ZX_ERR_CANCELED, status); }, nullptr);
  fidl::WireClient client{std::move(listener), loop.dispatcher()};

  {
    fidl::StringView tags[] = {"tag1", "tag2"};
    fuchsia_logger::wire::LogMessage msgs[] = {
        CreateLogMessage({
            .msg = "my message",
        }),
        CreateLogMessage({
            .tags = fidl::VectorView<fidl::StringView>::FromExternal(tags),
            .msg = "my message2",
        }),
    };
    client->LogMany(fidl::VectorView<fuchsia_logger::wire::LogMessage>::FromExternal(msgs))
        .ThenExactlyOnce(
            [](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::LogMany>& result) {
              ASSERT_OK(result);
            });
  }
  ASSERT_OK(loop.RunUntilIdle());
  fflush(buf_file);

  ASSERT_STREQ(R"([00093.892493][1024][1034][] INFO: my message
[00093.892493][1024][1034][tag1, tag2] INFO: my message2
)",
               buf);
  {
    fidl::StringView tags[] = {"tag1"};
    fuchsia_logger::wire::LogMessage msgs[] = {
        CreateLogMessage({
            .tags = fidl::VectorView<fidl::StringView>::FromExternal(tags),
            .msg = "my message",
        }),
    };
    client->LogMany(fidl::VectorView<fuchsia_logger::wire::LogMessage>::FromExternal(msgs))
        .ThenExactlyOnce(
            [](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::LogMany>& result) {
              ASSERT_OK(result);
            });
  }
  ASSERT_OK(loop.RunUntilIdle());

  fflush(buf_file);
  ASSERT_STREQ(R"([00093.892493][1024][1034][] INFO: my message
[00093.892493][1024][1034][tag1, tag2] INFO: my message2
[00093.892493][1024][1034][tag1] INFO: my message
)",
               buf);
}

TEST(LogListenerTests, TestDroppedLogs) {
  auto [listener, listener_request] = fidl::Endpoints<fuchsia_logger::LogListenerSafe>::Create();

  // We expect the log file to be much smaller than this.
  char buf[1024];
  memset(buf, 0, sizeof(buf));
  FILE* buf_file = fmemopen(buf, sizeof(buf), "w");

  // start listener
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  LogExporter log_exporter(
      buf_file, loop.dispatcher(), std::move(listener_request),
      [](zx_status_t status) { EXPECT_STATUS(ZX_ERR_CANCELED, status); }, nullptr);
  fidl::WireClient client{std::move(listener), loop.dispatcher()};

  client
      ->Log(CreateLogMessage({
          .dropped_logs = 1,
          .msg = "my message1",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());
  client
      ->Log(CreateLogMessage({
          .dropped_logs = 1,
          .msg = "my message2",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());
  client
      ->Log(CreateLogMessage({
          .pid = 1011,
          .dropped_logs = 1,
          .msg = "my message3",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());
  client
      ->Log(CreateLogMessage({
          .pid = 1011,
          .dropped_logs = 1,
          .msg = "my message4",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());
  client
      ->Log(CreateLogMessage({
          .pid = 1011,
          .dropped_logs = 2,
          .msg = "my message5",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());
  client
      ->Log(CreateLogMessage({
          .dropped_logs = 2,
          .msg = "my message6",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());

  fflush(buf_file);
  ASSERT_STREQ(R"([00093.892493][1024][1034][] INFO: my message1
[00093.892493][1024][1034][] WARNING: Dropped logs count:1
[00093.892493][1024][1034][] INFO: my message2
[00093.892493][1011][1034][] INFO: my message3
[00093.892493][1011][1034][] WARNING: Dropped logs count:1
[00093.892493][1011][1034][] INFO: my message4
[00093.892493][1011][1034][] INFO: my message5
[00093.892493][1011][1034][] WARNING: Dropped logs count:2
[00093.892493][1024][1034][] INFO: my message6
[00093.892493][1024][1034][] WARNING: Dropped logs count:2
)",
               buf);
}

TEST(LogListenerTests, TestBadOutputFile) {
  auto [listener, listener_request] = fidl::Endpoints<fuchsia_logger::LogListenerSafe>::Create();

  char buf[1024];
  memset(buf, 0, sizeof(buf));
  FILE* buf_file = fmemopen(buf, sizeof(buf), "r");

  // start listener
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  LogExporter log_exporter(
      buf_file, loop.dispatcher(), std::move(listener_request),
      [](zx_status_t status) { EXPECT_STATUS(ZX_ERR_ACCESS_DENIED, status); }, nullptr);
  fidl::WireClient client{std::move(listener), loop.dispatcher()};

  client
      ->Log(CreateLogMessage({
          .msg = "my message",
      }))
      .ThenExactlyOnce([](fidl::WireUnownedResult<fuchsia_logger::LogListenerSafe::Log>& result) {
        ASSERT_OK(result);
      });
  ASSERT_OK(loop.RunUntilIdle());

  ASSERT_STREQ("", buf);
}

}  // namespace
}  // namespace runtests
