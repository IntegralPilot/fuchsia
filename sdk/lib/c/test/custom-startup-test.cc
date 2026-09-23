// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "custom-startup-test.h"

// This is a test component that's really just a test harness for running the
// static PIE implemented in static-pie-custom-startup-test.cc and feeding it
// the trivial custom bootstrap protocol described in the common header.  The
// harness just delivers gtest failures if the static PIE crashes or fails to
// complete its side of the custom protocol before it exits.
//
// Though it starts the test process directly via zx::process::start to use the
// custom test protocol, this still uses the fuchsia.process.Launcher service.
// It doesn't need any special process creation privilege, just the component
// routing for that FIDL service.

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.process/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/io.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/zx/job.h>
#include <lib/zx/result.h>
#include <lib/zx/socket.h>
#include <lib/zx/vmo.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include <array>
#include <span>
#include <vector>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

namespace fio = fuchsia_io;
namespace fprocess = fuchsia_process;

class LibcCustomStartupTests : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result endpoints = fidl::CreateEndpoints<fprocess::Launcher>();
    ASSERT_TRUE(endpoints.is_ok()) << endpoints.status_string();
    zx_status_t status =
        fdio_service_connect_by_name(fidl::DiscoverableProtocolName<fprocess::Launcher>,
                                     endpoints->server.TakeChannel().release());
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    launcher_.Bind(std::move(endpoints->client));
  }

  void TearDown() override {
    if (job_) {
      zx_status_t status = job_.kill();
      EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);
    }
  }

  void GetJob(zx::job& job) {
    ASSERT_FALSE(job_);

    zx_status_t status = zx::job::create(*zx::job::default_job(), 0, &job_);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

    status = job_.duplicate(ZX_RIGHT_SAME_RIGHTS, &job);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  static void GetExecutable(const char* file, zx::vmo& vmo) {
    constexpr fio::wire::Flags kFlags =
        fio::wire::Flags::kProtocolFile | fio::wire::kPermReadable | fio::wire::kPermExecutable;
    fbl::unique_fd fd;
    zx_status_t status =
        fdio_open3_fd(file, static_cast<uint64_t>(kFlags), fd.reset_and_get_address());
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    status = fdio_get_vmo_exec(fd.get(), vmo.reset_and_get_address());
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  auto& launcher() { return launcher_; }

  static void Start(const fprocess::wire::ProcessStartData& data, zx::channel bootstrap) {
    zx_status_t status = data.process.start(data.thread, data.entry, data.stack,
                                            std::move(bootstrap), data.vdso_base);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

 private:
  fidl::WireSyncClient<fprocess::Launcher> launcher_;
  zx::job job_;
};

TEST_F(LibcCustomStartupTests, CustomProtocol) {
  fprocess::wire::LaunchInfo info = {.name = "custom-startup-test-child"};
  ASSERT_NO_FATAL_FAILURE(GetJob(info.job));
  ASSERT_NO_FATAL_FAILURE(
      GetExecutable("/pkg/bin/static-pie-custom-startup-test", info.executable));

  fidl::WireResult reply = launcher()->CreateWithoutStarting(std::move(info));
  ASSERT_TRUE(reply.ok()) << reply.error();
  ASSERT_EQ(reply.value().status, ZX_OK) << zx_status_get_string(reply.value().status);

  // Create the channel for the custom protocol.
  zx::channel bootstrap_parent, bootstrap_child;
  zx_status_t status = zx::channel::create(0, &bootstrap_parent, &bootstrap_child);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  // Start the child running.  It will wait for its bootstrap message.
  ASSERT_NO_FATAL_FAILURE(Start(*reply.value().data, std::move(bootstrap_child)));

  // Duplicate the process handle so it can be transferred.
  zx::process process;
  status = reply.value().data->process.duplicate(ZX_RIGHT_SAME_RIGHTS, &process);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  std::array<zx_handle_t, kMessageHandles> handles;
  handles[kProcessSelfHandle] = reply.value().data->process.release();
  handles[kThreadSelfHandle] = reply.value().data->thread.release();
  handles[kAllocationVmarHandle] = reply.value().data->root_vmar.release();

  // Decode the service's processargs message just enough for kImageVarHandle.
  std::vector<std::byte> procargs_buffer;
  std::vector<zx_handle_t> procargs_raw_handles;
  procargs_buffer.resize(ZX_CHANNEL_MAX_MSG_BYTES);
  procargs_raw_handles.resize(ZX_CHANNEL_MAX_MSG_HANDLES);
  uint32_t actual_bytes, actual_handles;
  status = reply.value().data->bootstrap.read(
      0, procargs_buffer.data(), procargs_raw_handles.data(),
      static_cast<uint32_t>(procargs_buffer.size()),
      static_cast<uint32_t>(procargs_raw_handles.size()), &actual_bytes, &actual_handles);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  procargs_buffer.resize(actual_bytes);
  procargs_raw_handles.resize(actual_handles);
  std::vector<zx::handle> procargs_handles;
  procargs_handles.reserve(actual_handles);
  for (zx_handle_t h : procargs_raw_handles) {
    procargs_handles.emplace_back(h);
  }

  zx_proc_args_t procargs_header;
  ASSERT_GT(procargs_buffer.size(), sizeof(procargs_header));
  memcpy(&procargs_header, procargs_buffer.data(), sizeof(procargs_header));
  ASSERT_EQ(procargs_header.protocol, ZX_PROCARGS_PROTOCOL);
  ASSERT_EQ(procargs_header.version, ZX_PROCARGS_VERSION);
  ASSERT_LT(procargs_header.handle_info_off, procargs_buffer.size());
  ASSERT_EQ(procargs_header.handle_info_off % sizeof(uint32_t), 0u);
  ASSERT_GE(procargs_buffer.size() - procargs_header.handle_info_off,
            procargs_handles.size() * sizeof(uint32_t));
  std::span procargs_handle_info{
      reinterpret_cast<const uint32_t*>(procargs_buffer.data() + procargs_header.handle_info_off),
      procargs_handles.size()};

  for (size_t i = 0; i < procargs_handles.size(); ++i) {
    if (PA_HND_TYPE(procargs_handle_info[i]) == PA_VMAR_LOADED) {
      handles[kImageVarHandle] = procargs_handles[i].release();
      break;
    }
  }

  // Finally, make a log socket to pass in.
  zx::socket log_parent, log_child;
  status = zx::socket::create(ZX_SOCKET_STREAM, &log_parent, &log_child);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  handles[kLogHandle] = log_child.release();

  // The message in our trivial custom protocol is ready to send.
  status = bootstrap_parent.write(0, kPing.data(), kPing.size(), handles.data(), handles.size());
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  // Now wait for the process to reply and exit.
  std::string log_text;
  while (true) {
    std::vector<zx_wait_item_t> wait_items;
    std::vector<fit::function<void(zx_signals_t)>> on_item;

    if (process) {
      wait_items.push_back({
          .handle = process.get(),
          .waitfor = ZX_PROCESS_TERMINATED,
      });
      on_item.push_back([&process](zx_signals_t pending) {
        if (pending & ZX_PROCESS_TERMINATED) {
          zx_info_process_t info;
          zx_status_t status =
              process.get_info(ZX_INFO_PROCESS, &info, sizeof(info), nullptr, nullptr);
          ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
          ASSERT_TRUE(info.flags & ZX_INFO_PROCESS_FLAG_EXITED);
          process.reset();
          EXPECT_EQ(info.return_code, 0);
        }
      });
    }

    if (bootstrap_parent) {
      wait_items.push_back({
          .handle = bootstrap_parent.get(),
          .waitfor = ZX_CHANNEL_READABLE | ZX_CHANNEL_PEER_CLOSED,
      });
      on_item.push_back([&bootstrap_parent](zx_signals_t pending) {
        if (pending & ZX_CHANNEL_READABLE) {
          std::array<char, kPong.size()> buffer;
          uint32_t actual;
          zx_status_t status =
              bootstrap_parent.read(0, buffer.data(), nullptr, buffer.size(), 0, &actual, nullptr);
          ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
          bootstrap_parent.reset();
          EXPECT_EQ(actual, kPong.size());
          EXPECT_EQ(std::string_view(buffer.data(), actual), kPong);
        }
        if (pending & ZX_CHANNEL_PEER_CLOSED) {
          EXPECT_FALSE(bootstrap_parent);  // Should have gotten pong already.
          bootstrap_parent.reset();
        }
      });
    }

    if (log_parent) {
      wait_items.push_back({
          .handle = log_parent.get(),
          .waitfor = ZX_SOCKET_READABLE | ZX_SOCKET_PEER_CLOSED,
      });
      on_item.push_back([&log_parent, &log_text](zx_signals_t pending) {
        if (pending & ZX_SOCKET_READABLE) {
          std::string buffer;
          buffer.resize(1024);
          size_t actual;
          zx_status_t status = log_parent.read(0, buffer.data(), buffer.size(), &actual);
          ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
          buffer.resize(actual);
          log_text += buffer;
        }
        if (pending & ZX_SOCKET_PEER_CLOSED) {
          log_parent.reset();
        }
      });
    }

    if (wait_items.empty()) {
      break;
    }

    status = zx::handle::wait_many(wait_items.data(), static_cast<uint32_t>(wait_items.size()),
                                   zx::time::infinite());
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
    for (size_t i = 0; i < wait_items.size(); ++i) {
      on_item[i](wait_items[i].pending);
    }
  }

  EXPECT_TRUE(log_text.starts_with("{{{")) << log_text;
  EXPECT_TRUE(log_text.ends_with(kLog)) << log_text;
}

}  // anonymous namespace
