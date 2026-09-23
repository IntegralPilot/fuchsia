// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "lib/ld/testing/test-processargs.h"

#include <lib/elfldltl/testing/get-test-data.h>
#include <lib/fdio/fd.h>
#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <limits.h>
#include <zircon/processargs.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <array>
#include <cassert>
#include <functional>
#include <numeric>
#include <span>

#include <gtest/gtest.h>

namespace ld::testing {
namespace {

constexpr size_t kStackAllowance = PTHREAD_STACK_MIN;

}  // namespace

TestProcessArgs& TestProcessArgs::AddHandle(uint32_t info, zx::handle handle) {
  handles_.push_back(std::move(handle));
  handle_info_.push_back(info);
  return *this;
}

TestProcessArgs& TestProcessArgs::AddDuplicateHandle(uint32_t info, zx::unowned_handle ref) {
  zx::handle handle;
  EXPECT_EQ(ref->duplicate(ZX_RIGHT_SAME_RIGHTS, &handle), ZX_OK);
  return AddHandle(info, std::move(handle));
}

TestProcessArgs& TestProcessArgs::AddFd(int fd, zx::handle handle) {
  return AddHandle(PA_HND(PA_FD, fd), std::move(handle));
}

TestProcessArgs& TestProcessArgs::AddFd(int test_fd, fbl::unique_fd local_fd) {
  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_EQ(fdio_fd_transfer(local_fd.release(), &handle), ZX_OK);
  return AddFd(test_fd, zx::handle{handle});
}

TestProcessArgs& TestProcessArgs::AddClonedFd(int test_fd, int local_fd) {
  zx_handle_t handle = ZX_HANDLE_INVALID;
  EXPECT_EQ(fdio_fd_clone(local_fd, &handle), ZX_OK);
  return AddFd(test_fd, zx::handle{handle});
}

TestProcessArgs& TestProcessArgs::AddName(std::string_view name, uint32_t info,
                                          zx::channel handle) {
  return AddHandle(PA_HND(info, static_cast<uint16_t>(next_name())), std::move(handle))
      .AddName(name);
}

TestProcessArgs& TestProcessArgs::AddInProcessTestHandles() {
  return AddProcess(zx::process::self()).AddThread(zx::thread::self());
}

TestProcessArgs& TestProcessArgs::AddProcess(zx::unowned_process process) {
  return AddDuplicateHandle(PA_PROC_SELF, process->borrow());
}

TestProcessArgs& TestProcessArgs::AddThread(zx::unowned_thread thread) {
  return AddDuplicateHandle(PA_THREAD_SELF, thread->borrow());
}

TestProcessArgs& TestProcessArgs::AddLdsvc(zx::channel ldsvc) {
  return AddHandle(PA_LDSVC_LOADER, std::move(ldsvc));
}

TestProcessArgs& TestProcessArgs::AddSelfVmar(zx::vmar vmar) {
  return AddHandle(PA_VMAR_LOADED, std::move(vmar));
}

TestProcessArgs& TestProcessArgs::AddSelfVmar(zx::unowned_vmar vmar) {
  return AddDuplicateHandle(PA_VMAR_LOADED, vmar->borrow());
}

TestProcessArgs& TestProcessArgs::AddAllocationVmar(zx::vmar vmar) {
  return AddHandle(PA_VMAR_ROOT, std::move(vmar));
}

TestProcessArgs& TestProcessArgs::AddAllocationVmar(zx::unowned_vmar vmar) {
  return AddDuplicateHandle(PA_VMAR_ROOT, vmar->borrow());
}

TestProcessArgs& TestProcessArgs::AddExecutableVmo(zx::vmo vmo) {
  return AddHandle(PA_VMO_EXECUTABLE, std::move(vmo));
}

TestProcessArgs& TestProcessArgs::AddStackVmo(zx::vmo vmo) {
  return AddHandle(PA_VMO_STACK, std::move(vmo));
}

void TestProcessArgs::PackBootstrap(zx::unowned_channel bootstrap_sender) {
  PackBootstrap(bootstrap_sender->borrow(), nullptr);
}

std::optional<size_t> TestProcessArgs::GetStackSize() {
  size_t message_size = 0;
  PackBootstrap({}, &message_size);
  return message_size + kStackAllowance;
}

void TestProcessArgs::PackBootstrap(zx::unowned_channel bootstrap_sender,
                                    size_t* count_for_stack_size) {
  ASSERT_EQ(handles_.size(), handle_info_.size());

  size_t handle_count = handles_.size();
  if (count_for_stack_size) {
    ASSERT_FALSE(bootstrap_sender->is_valid());

    // Calculate for PA_VMO_STACK being added later.
    ++handle_count;
  }

  auto on_strings = [this](auto&&... args) {
    return std::invoke(std::forward<decltype(args)>(args)..., args_, env_, names_);
  };

  auto [args_size, env_size, names_size] = on_strings([](auto&&... list) {
    constexpr auto sum_strings = [](uint32_t total, std::string_view str) {
      return total + static_cast<uint32_t>(str.size()) + 1;
    };
    return std::array{std::accumulate(list.begin(), list.end(), uint32_t{}, sum_strings)...};
  });

  constexpr uint32_t info_off = sizeof(zx_proc_args_t);
  const uint32_t args_off = info_off + static_cast<uint32_t>(handle_count * sizeof(uint32_t));
  const uint32_t env_off = args_off + args_size;
  const uint32_t names_off = env_off + env_size;
  const uint32_t message_size = names_off + names_size;

  if (count_for_stack_size) {
    *count_for_stack_size = message_size;
    return;
  }

  std::unique_ptr<char[]> buffer{new char[message_size]};
  new (buffer.get()) zx_proc_args_t{
      .protocol = ZX_PROCARGS_PROTOCOL,
      .version = ZX_PROCARGS_VERSION,
      .handle_info_off = info_off,
      .args_off = args_off,
      .args_num = static_cast<uint32_t>(args_.size()),
      .environ_off = env_off,
      .environ_num = static_cast<uint32_t>(env_.size()),
      .names_off = names_off,
      .names_num = static_cast<uint32_t>(names_.size()),
  };

  uint32_t* info = reinterpret_cast<uint32_t*>(buffer.get() + info_off);
  std::span strings{
      buffer.get() + args_off,
      args_size + env_size + names_size,
  };

  std::ranges::copy(handle_info_, info);

  on_strings([&strings](auto&&... list) mutable {
    auto add_strings = [&strings](auto&& list) {
      for (const auto& str : list) {
        strings = strings.subspan(str.copy(strings.data(), strings.size()));
        strings.front() = '\0';
        strings = strings.subspan(1);
      }
    };
    (add_strings(list), ...);
  });
  ASSERT_TRUE(strings.empty());

  std::vector<zx_handle_t> handles;
  handles.reserve(handles_.size());
  for (auto& handle : handles_) {
    handles.push_back(handle.release());
  }

  handles_.clear();
  handle_info_.clear();
  args_.clear();
  env_.clear();
  names_.clear();

  ASSERT_EQ(bootstrap_sender->write(0, buffer.get(), message_size, handles.data(),
                                    static_cast<uint32_t>(handles.size())),
            ZX_OK);
}

zx::channel TestProcessArgs::MakeBootstrap() {
  zx::channel bootstrap;
  EXPECT_FALSE(bootstrap_sender_);
  if (!bootstrap_sender_) {
    zx_status_t status = zx::channel::create(0, &bootstrap_sender_, &bootstrap);
    EXPECT_EQ(status, ZX_OK);
  }
  return bootstrap;
}

zx::channel TestProcessArgs::PackBootstrap() {
  zx::channel bootstrap;
  if (!bootstrap_sender_) {
    // No channel in use yet.  Make it.
    bootstrap = MakeBootstrap();
  }

  // Pack the current message into the current channel.
  if (bootstrap_sender_) {
    PackBootstrap(bootstrap_sender_.borrow());
  }

  // Return the new channel, or none if packing again into an existing channel.
  return bootstrap;
}

bool TestProcessArgs::empty() const {
  assert(handles_.size() == handle_info_.size());
  return handles_.empty() && args_.empty() && env_.empty() && names_.empty();
}

}  // namespace ld::testing
