// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.runner/cpp/wire_types.h>
#include <fidl/fuchsia.logger/cpp/wire.h>
#include <fuchsia/io/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/component/cpp/tests/test_base.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/binding.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/lib/diagnostics/fake-log-sink/cpp/fake_log_sink.h"

namespace {

namespace fio = fuchsia::io;
namespace frunner = fuchsia_component_runner;

using ::diagnostics::reader::LogsData;
using ::testing::ElementsAre;

constexpr char kName[] = "my-name";
constexpr char kMessage[] = "my-message";
constexpr char kDriverTag[] = "driver";

void CheckLogReadable(fuchsia_logging::FakeLogSink& sink,
                      fuchsia_diagnostics_types::Severity severity) {
  auto data = sink.ReadLogsData();
  ASSERT_TRUE(data);
  const auto& metadata = data->metadata();
  EXPECT_EQ(metadata.severity, severity);
  EXPECT_THAT(metadata.tags, ElementsAre(kDriverTag, kName));
  EXPECT_EQ(data->message(), kMessage);
}

TEST(LoggerTest, CreateAndLog) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  async::Loop ns_loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  ns_loop.StartThread();

  // Set up namespace.
  auto svc = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto ns = fdf::testing::CreateNamespace(std::move(svc.client));
  ASSERT_TRUE(ns.is_ok());

  std::optional<fuchsia_logging::FakeLogSink> log_sink;

  fdf::testing::Directory svc_directory;
  svc_directory.SetOpenHandler([&log_sink](const std::string& path, auto object) {
    EXPECT_EQ(path, fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>);
    ASSERT_FALSE(log_sink);
    log_sink.emplace(FUCHSIA_LOG_INFO,
                     fidl::ServerEnd<fuchsia_logger::LogSink>(object.TakeChannel()));
  });
  fidl::Binding<fio::Directory> svc_binding(&svc_directory);

  fdf::testing::Directory svc_directory2;
  svc_directory2.SetOpenHandler([&ns_loop, &svc_binding](const std::string& path, auto object) {
    EXPECT_EQ(path, ".");
    svc_binding.Bind(object.TakeChannel(), ns_loop.dispatcher());
  });

  fidl::Binding<fio::Directory> svc_binding2(&svc_directory2);
  svc_binding2.Bind(svc.server.TakeChannel(), ns_loop.dispatcher());

  auto logger = fdf::Logger::Create2(*ns, loop.dispatcher(), kName, FUCHSIA_LOG_INFO);
  ASSERT_FALSE(logger->IsNoOp());
  while (!log_sink) {
    loop.RunUntilIdle();
  }

  // Check initial state of logger.
  ASSERT_TRUE(log_sink);
  EXPECT_FALSE(log_sink->WaitForRecord(zx::time::infinite_past()));

  // Check state of logger after writing logs that were below |min_severity|.
  FDF_LOGL(TRACE, *logger, kMessage);
  EXPECT_FALSE(log_sink->WaitForRecord(zx::time::infinite_past()));
  FDF_LOGL(DEBUG, *logger, kMessage);
  EXPECT_FALSE(log_sink->WaitForRecord(zx::time::infinite_past()));

  // Check state of logger after writing logs.
  FDF_LOGL(INFO, *logger, kMessage);
  {
    SCOPED_TRACE("");
    CheckLogReadable(*log_sink, fuchsia_diagnostics_types::Severity::kInfo);
  }
  FDF_LOGL(WARNING, *logger, kMessage);
  {
    SCOPED_TRACE("");
    CheckLogReadable(*log_sink, fuchsia_diagnostics_types::Severity::kWarn);
  }
  FDF_LOGL(ERROR, *logger, kMessage);
  {
    SCOPED_TRACE("");
    CheckLogReadable(*log_sink, fuchsia_diagnostics_types::Severity::kError);
  }
}

TEST(LoggerTest, CreateNoLogSink) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};

  // Setup namespace.
  auto pkg = fidl::CreateEndpoints<fuchsia_io::Directory>();
  EXPECT_EQ(ZX_OK, pkg.status_value());
  auto svc = fidl::Endpoints<fuchsia_io::Directory>::Create();
  fidl::Arena arena;
  fidl::VectorView<frunner::wire::ComponentNamespaceEntry> ns_entries(arena, 2);
  ns_entries[0].Allocate(arena);
  ns_entries[0].set_path(arena, "/pkg").set_directory(std::move(pkg->client));
  ns_entries[1].Allocate(arena);
  ns_entries[1].set_path(arena, "/svc").set_directory(std::move(svc.client));
  auto ns = fdf::Namespace::Create(ns_entries);
  ASSERT_TRUE(ns.is_ok());

  svc.server.TakeChannel().reset();

  // Setup logger.
  auto logger = fdf::Logger::Create2(*ns, loop.dispatcher(), kName, FUCHSIA_LOG_INFO);
  ASSERT_TRUE(logger->IsNoOp());
}

TEST(LoggerTest, SetSeverity) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  async::Loop ns_loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  ns_loop.StartThread();

  // Setup namespace.
  auto svc = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto ns = fdf::testing::CreateNamespace(std::move(svc.client));
  ASSERT_TRUE(ns.is_ok());

  // Setup logger.
  std::optional<fuchsia_logging::FakeLogSink> log_sink;

  fdf::testing::Directory svc_directory;
  svc_directory.SetOpenHandler([&log_sink](const std::string& path, auto object) {
    EXPECT_EQ(path, fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>);
    ASSERT_FALSE(log_sink);
    log_sink.emplace(FUCHSIA_LOG_INFO,
                     fidl::ServerEnd<fuchsia_logger::LogSink>(object.TakeChannel()));
  });
  fidl::Binding<fio::Directory> svc_binding(&svc_directory);

  fdf::testing::Directory svc_directory2;
  svc_directory2.SetOpenHandler([&ns_loop, &svc_binding](const std::string& path, auto object) {
    EXPECT_EQ(path, ".");
    svc_binding.Bind(object.TakeChannel(), ns_loop.dispatcher());
  });
  fidl::Binding<fio::Directory> svc_binding2(&svc_directory2);

  svc_binding2.Bind(svc.server.TakeChannel(), ns_loop.dispatcher());

  auto logger = fdf::Logger::Create2(*ns, loop.dispatcher(), kName, FUCHSIA_LOG_INFO);
  ASSERT_FALSE(logger->IsNoOp());
  while (!log_sink) {
    loop.RunUntilIdle();
  }

  // Check initial state of logger.
  ASSERT_TRUE(log_sink);
  EXPECT_FALSE(log_sink->WaitForRecord(zx::time::infinite_past()));

  // Check state of logger after writing logs that were above or equal to the default
  // severity.
  FDF_LOGL(INFO, *logger, kMessage);
  {
    SCOPED_TRACE("");
    CheckLogReadable(*log_sink, fuchsia_diagnostics_types::Severity::kInfo);
  }
  FDF_LOGL(WARNING, *logger, kMessage);
  {
    SCOPED_TRACE("");
    CheckLogReadable(*log_sink, fuchsia_diagnostics_types::Severity::kWarn);
  }

  // Check severity after setting it.
  log_sink->SetSeverity(FUCHSIA_LOG_WARNING);

  for (;;) {
    loop.RunUntilIdle();

    if (logger->GetSeverity() == FUCHSIA_LOG_WARNING) {
      break;
    }

    // Unfortunately, setting the severity involves two async loops on different threads: a FIDL
    // message has to pass from the server and the client. It's not ideal, but the easiest way to
    // test this is just to sleep and then test again.
    usleep(1000);
  }

  // Check state of logger after writing logs that were below min severity.
  FDF_LOGL(INFO, *logger, kMessage);
  EXPECT_FALSE(log_sink->WaitForRecord(zx::time::infinite_past()));
  FDF_LOGL(WARNING, *logger, kMessage);
  {
    SCOPED_TRACE("");
    CheckLogReadable(*log_sink, fuchsia_diagnostics_types::Severity::kWarn);
  }
}

TEST(LoggerTest, LogVeryLongMessage) {
  async::Loop loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  async::Loop ns_loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  ns_loop.StartThread();

  // Set up namespace.
  auto svc = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto ns = fdf::testing::CreateNamespace(std::move(svc.client));
  ASSERT_TRUE(ns.is_ok());

  std::optional<fuchsia_logging::FakeLogSink> log_sink;

  fdf::testing::Directory svc_directory;
  svc_directory.SetOpenHandler([&log_sink](const std::string& path, auto object) {
    EXPECT_EQ(path, fidl::DiscoverableProtocolName<fuchsia_logger::LogSink>);
    ASSERT_FALSE(log_sink);
    log_sink.emplace(FUCHSIA_LOG_INFO,
                     fidl::ServerEnd<fuchsia_logger::LogSink>(object.TakeChannel()));
  });
  fidl::Binding<fio::Directory> svc_binding(&svc_directory);

  fdf::testing::Directory svc_directory2;
  svc_directory2.SetOpenHandler([&ns_loop, &svc_binding](const std::string& path, auto object) {
    EXPECT_EQ(path, ".");
    svc_binding.Bind(object.TakeChannel(), ns_loop.dispatcher());
  });

  fidl::Binding<fio::Directory> svc_binding2(&svc_directory2);
  svc_binding2.Bind(svc.server.TakeChannel(), ns_loop.dispatcher());

  auto logger = fdf::Logger::Create2(*ns, loop.dispatcher(), kName, FUCHSIA_LOG_INFO);
  ASSERT_FALSE(logger->IsNoOp());
  while (!log_sink) {
    loop.RunUntilIdle();
  }

  // Log a very long message.
  std::string long_msg(2000, 'a');
  logger->log(FUCHSIA_LOG_INFO, std::source_location::current(), "{}", long_msg);

  auto data = log_sink->ReadLogsData();
  ASSERT_TRUE(data);
  const auto& metadata = data->metadata();
  EXPECT_EQ(metadata.severity, fuchsia_diagnostics_types::Severity::kInfo);
  EXPECT_THAT(metadata.tags, ElementsAre(kDriverTag, kName));

  std::string message = data->message();
  EXPECT_LT(message.size(), 2000u);
  EXPECT_GE(message.size(), 1020u);

  if (!message.empty() && message.back() == '\0') {
    message.pop_back();
  }
  EXPECT_TRUE(message.ends_with("..."));
}

}  // namespace
