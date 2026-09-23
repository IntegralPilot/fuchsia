// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/loop-backed-dispatcher.h"

#include <lib/async-loop/loop.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/threads.h>

#include <string>

#include <fbl/alloc_checker.h>

namespace display {

namespace {

const async_loop_config_t kIrqHandlerAsyncLoopConfig = [] {
  async_loop_config_t loop_config = kAsyncLoopConfigNeverAttachToThread;
  loop_config.irq_support = true;
  return loop_config;
}();

}  // namespace

// static
zx::result<std::unique_ptr<Dispatcher>> LoopBackedDispatcher::Create(
    zx_device_t* device, std::string_view thread_name, std::string_view scheduler_role) {
  ZX_DEBUG_ASSERT(device != nullptr);
  ZX_DEBUG_ASSERT(!thread_name.empty());

  fbl::AllocChecker alloc_checker;
  auto dispatcher = fbl::make_unique_checked<LoopBackedDispatcher>(&alloc_checker);
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> start_thread_result = dispatcher->StartThread(thread_name);
  if (start_thread_result.is_error()) {
    zxlogf(ERROR, "Failed to start async Loop dispatcher thread: %s",
           start_thread_result.status_string());
    return start_thread_result.take_error();
  }

  if (!scheduler_role.empty()) {
    zx::result<> set_scheduler_role_result =
        dispatcher->SetThreadSchedulerRole(scheduler_role, device);
    if (set_scheduler_role_result.is_error()) {
      zxlogf(WARNING, "Failed to set scheduler role: %s",
             set_scheduler_role_result.status_string());
    }
  }
  return zx::ok(std::move(dispatcher));
}

// static
zx::result<std::unique_ptr<Dispatcher>> LoopBackedDispatcher::Create(std::string_view thread_name) {
  ZX_DEBUG_ASSERT(!thread_name.empty());

  fbl::AllocChecker alloc_checker;
  auto dispatcher = fbl::make_unique_checked<LoopBackedDispatcher>(&alloc_checker);
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> start_thread_result = dispatcher->StartThread(thread_name);
  if (start_thread_result.is_error()) {
    zxlogf(ERROR, "Failed to start async Loop dispatcher thread: %s",
           start_thread_result.status_string());
    return start_thread_result.take_error();
  }

  return zx::ok(std::move(dispatcher));
}

LoopBackedDispatcher::LoopBackedDispatcher() : loop_(&kIrqHandlerAsyncLoopConfig) {}

LoopBackedDispatcher::~LoopBackedDispatcher() { loop_.Shutdown(); }

zx::result<> LoopBackedDispatcher::StartThread(std::string_view thread_name) {
  ZX_DEBUG_ASSERT(!thread_name.empty());

  // `std::string_view` is not null-terminated, so we need to copy it to a
  // null-terminated string to use in `Loop::StartThread()`.
  std::string thread_name_string(thread_name);

  return zx::make_result(loop_.StartThread(thread_name_string.c_str(), &loop_thread_));
}

zx::result<> LoopBackedDispatcher::SetThreadSchedulerRole(std::string_view scheduler_role,
                                                          zx_device_t* device) {
  zx_handle_t zx_thread = thrd_get_zx_handle(loop_thread_);
  return zx::make_result(
      device_set_profile_by_role(device, zx_thread, scheduler_role.data(), scheduler_role.size()));
}

}  // namespace display
