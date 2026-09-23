// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_LOOP_BACKED_DISPATCHER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_LOOP_BACKED_DISPATCHER_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/ddk/device.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <string_view>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"

namespace display {

// An async_dispatcher_t backed by an async Loop running on a dedicated thread.
//
// The Loop is owned by the `LoopBackedDispatcher` instance and is shut down
// iff the `LoopBackedDispatcher` is destroyed.
//
// The Loop allows dispatch of async tasks and interrupt requests.
class LoopBackedDispatcher final : public Dispatcher {
 public:
  // Factory function that creates an asynchronous dispatch Loop and starts its
  // worker thread of `thread_name` and as `scheduler_role`.
  //
  // Preferred for production use.
  //
  // `device` must not be null.
  // `thread_name` must not be empty.
  // `scheduler_role` is a hint. It's not an error if `LoopBackedDispatcher` fails to
  // set the worker thread's scheduler role.
  static zx::result<std::unique_ptr<Dispatcher>> Create(zx_device_t* device,
                                                        std::string_view thread_name,
                                                        std::string_view scheduler_role);

  // Factory function that creates an asynchronous dispatch Loop and starts its
  // worker thread of `thread_name` without setting the scheduling role.
  //
  // `thread_name` must not be empty.
  static zx::result<std::unique_ptr<Dispatcher>> Create(std::string_view thread_name);

  // Prefer the `Create()` factory method.
  LoopBackedDispatcher();

  ~LoopBackedDispatcher() override;

  LoopBackedDispatcher(const LoopBackedDispatcher&) = delete;
  LoopBackedDispatcher(LoopBackedDispatcher&&) = delete;
  LoopBackedDispatcher& operator=(const LoopBackedDispatcher&) = delete;
  LoopBackedDispatcher& operator=(LoopBackedDispatcher&&) = delete;

  // Dispatcher:
  async_dispatcher_t* async_dispatcher() const override { return loop_.dispatcher(); }

 private:
  // Must be called exactly once for each valid `LoopBackedDispatcher` instance.
  //
  // `thread_name` must not be empty.
  zx::result<> StartThread(std::string_view thread_name);

  // `scheduler_role` is a hint. It's not an error if `LoopBackedDispatcher` fails to
  // set the worker thread's scheduler role.
  zx::result<> SetThreadSchedulerRole(std::string_view scheduler_role, zx_device_t* device);

  async::Loop loop_;
  thrd_t loop_thread_;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_LOOP_BACKED_DISPATCHER_H_
