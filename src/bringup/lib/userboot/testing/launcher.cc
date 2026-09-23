// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/userboot/testing/launcher.h"

#include <fidl/fuchsia.process/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fit/function.h>
#include <lib/ld/processargs.h>
#include <lib/zx/job.h>
#include <lib/zx/result.h>
#include <lib/zx/socket.h>
#include <lib/zx/vmo.h>
#include <zircon/processargs.h>
#include <zircon/status.h>

#include <algorithm>
#include <random>
#include <ranges>

#include <gtest/gtest.h>

namespace userboot::testing {
namespace {

namespace fprocess = fuchsia_process;

zx::result<zx::handle> FromFd(fbl::unique_fd fd) {
  zx::handle handle;
  if (zx_status_t status = fdio_fd_transfer(fd.release(), handle.reset_and_get_address());
      status != ZX_OK) {
    EXPECT_EQ(status, ZX_OK) << "fdio_fd_transfer: " << zx_status_get_string(status);
    return zx::error{status};
  }
  return zx::ok(std::move(handle));
}

// The fprocess::Launcher service packs the legacy <zircon/processargs.h>
// protocol message for the new process.  This includes the the ELF image VMAR
// handle, which is not otherwise available.  It also has duplicates of all the
// other handles we need, so just get them all from there at the same time.
std::vector<zx::handle> InitialHandlesFromLegacyBootstrap(zx::channel bootstrap) {
  using Procargs = ld::ProcessargsBuffer<>;
  Procargs procargs;
  Procargs::HandlesBuffer handle_buffer;
  auto read = procargs.Read(bootstrap.borrow(), handle_buffer);
  EXPECT_TRUE(read.is_ok()) << read.status_string();
  if (read.is_error()) {
    return {};
  }
  bool valid = procargs.Valid(*read);
  EXPECT_TRUE(valid) << "bad procargs message";
  if (!valid) {
    return {};
  }
  std::span handles = std::span{handle_buffer}.subspan(0, read->handles);
  std::span handle_info = procargs.handle_info(read->handles);
  std::vector<zx::handle> result;
  for (auto [raw_handle, info] : std::views::zip(handles, handle_info)) {
    zx::handle handle{raw_handle};
    switch (info) {
      case PA_PROC_SELF:
      case PA_THREAD_SELF:
      case PA_VMAR_ROOT:
      case PA_VMAR_LOADED:
        result.push_back(std::move(handle));
        break;
      default:
        // Others get dropped.
        break;
    }
  }
  return result;
}

class Sender {
 public:
  Sender() = delete;

  explicit Sender(zx::channel channel) : channel_(std::move(channel)) {}

  zx::result<> SendHandles(std::vector<zx::handle> handles) {
    // Permute the order since it's unspecified in the userboot protocol.
    std::ranges::shuffle(handles, random_engine_);
    std::vector<zx_handle_t> raw_handles{
        std::from_range,
        std::views::transform(handles, &zx::handle::release),
    };
    return zx::make_result(  //
        channel_.write(0, nullptr, 0, raw_handles.data(),
                       static_cast<uint32_t>(raw_handles.size())));
  }

 private:
  // The seed is logged and reproducible via gtest machinery.
  static uint32_t Seed() {
    const auto* test = ::testing::UnitTest::GetInstance();
    return std::bit_cast<uint32_t>(test->random_seed());
  }

  zx::channel channel_;
  std::default_random_engine random_engine_{Seed()};
};

}  // namespace

void TestJob::Init() {
  zx_status_t status = zx::job::create(*zx::job::default_job(), 0, &job_);
  EXPECT_EQ(status, ZX_OK) << "zx_job_create: " << zx_status_get_string(status);
}

zx::job TestJob::Get() {
  EXPECT_TRUE(job_) << "Get() called without successful Init()";
  if (!job_) {
    return {};
  }

  zx::job job;
  zx_status_t status = job_.duplicate(ZX_RIGHT_SAME_RIGHTS, &job);
  EXPECT_EQ(status, ZX_OK) << "cannot duplicate job handle: " << zx_status_get_string(status);
  return job;
}

TestJob::~TestJob() {
  if (job_) {
    zx_status_t status = job_.kill();
    EXPECT_EQ(status, ZX_OK) << "Killing test job: " << zx_status_get_string(status);
  }
}

zx::result<Launcher> Launcher::Create() {
  zx::result endpoints = fidl::CreateEndpoints<fprocess::Launcher>();
  EXPECT_TRUE(endpoints.is_ok()) << "Cannot create userboot::testing::Launcher endpoints: "
                                 << endpoints.status_string();
  if (endpoints.is_error()) {
    return endpoints.take_error();
  }

  zx::result<> connect = component::Connect<fprocess::Launcher>(std::move(endpoints->server));
  EXPECT_TRUE(connect.is_ok()) << fidl::DiscoverableProtocolName<fprocess::Launcher> << ": "
                               << connect.status_string();
  if (connect.is_error()) {
    return connect.take_error();
  }

  Launcher launcher;
  launcher.channel_ = endpoints->client.TakeChannel();
  return zx::ok(std::move(launcher));
}

zx::result<zx::process> Launcher::Launch(zx::job job, zx::vmo executable, fbl::unique_fd log_fd,
                                         std::vector<zx::handle> handles) {
  // Check the arguments to bail early for cascading failures.
  if (!job || !executable || !log_fd) {
    return zx::error{ZX_ERR_BAD_HANDLE};
  }

  zx::result log_handle = FromFd(std::move(log_fd));
  if (log_handle.is_error()) {
    return log_handle.take_error();
  }
  zx::handle log_handle_dup;
  zx_status_t status = log_handle->duplicate(ZX_RIGHT_SAME_RIGHTS, &log_handle_dup);
  EXPECT_EQ(status, ZX_OK) << "zx_handle_duplicate: " << zx_status_get_string(status);
  if (status != ZX_OK) {
    return zx::error{status};
  }
  handles.push_back(std::move(log_handle_dup));

  zx::channel bootstrap_send, bootstrap_receive;
  status = zx::channel::create(0, &bootstrap_send, &bootstrap_receive);
  EXPECT_EQ(status, ZX_OK) << "zx_channel_create: " << zx_status_get_string(status);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  zx::process process;
  zx::thread thread;
  uintptr_t entry = 0;
  uintptr_t stack = 0;
  uintptr_t vdso_base = 0;
  std::vector<zx::handle> initial_handles;
  {
    fidl::UnownedClientEnd<fprocess::Launcher> client_end{channel_.borrow()};
    fidl::WireResult reply = fidl::WireCall(client_end)
                                 ->CreateWithoutStarting({
                                     .executable = std::move(executable),
                                     .job = std::move(job),
                                     .name = "userboot",
                                 });
    EXPECT_TRUE(reply.ok()) << "CreateWithoutStarting: " << reply.error();
    if (!reply.ok()) {
      return zx::error{reply.status()};
    }

    EXPECT_EQ(reply.value().status, ZX_OK)
        << "CreateWithoutStarting: " << zx_status_get_string(reply.value().status);
    if (reply.value().status != ZX_OK) {
      return zx::error{reply.value().status};
    }

    auto& data = reply.value().data;
    process = std::move(data->process);
    thread = std::move(data->thread);
    entry = data->entry;
    stack = data->stack;
    vdso_base = data->vdso_base;

    // Most of the handles are in the data.  But the launcher doesn't give us
    // the ELF image VMAR handle; that's only in the bootstrap message it
    // packed.  That also has duplicates of all the other handles we need, so
    // just get them all from there instead.
    initial_handles = InitialHandlesFromLegacyBootstrap(std::move(data->bootstrap));
    if (initial_handles.empty()) {
      return zx::error{ZX_ERR_BAD_STATE};
    }
  }
  initial_handles.push_back(*std::move(log_handle));

  // Send the bootstrap messages.
  Sender sender(std::move(bootstrap_send));

  if (auto send = sender.SendHandles(std::move(initial_handles)); send.is_error()) {
    return send.take_error();
  }

  if (auto send = sender.SendHandles(std::move(handles)); send.is_error()) {
    return send.take_error();
  }

  // Start the process running.
  status = process.start(thread, entry, stack, std::move(bootstrap_receive), vdso_base);
  EXPECT_EQ(status, ZX_OK) << "zx_process_start: " << zx_status_get_string(status);
  if (status != ZX_OK) {
    return zx::error{status};
  }

  return zx::ok(std::move(process));
}

}  // namespace userboot::testing
