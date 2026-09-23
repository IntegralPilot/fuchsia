// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DRIVER_RUNTIME_BACKED_DISPATCHER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DRIVER_RUNTIME_BACKED_DISPATCHER_H_

#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <memory>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"

namespace display {

// An async_dispatcher_t backed by an `fdf::SynchronizedDispatcher` co-managed
// by the Driver Framework.
//
// We use `fdf::SynchronizedDispatcher` as the backend, because it allows
// dispatching both async tasks and Zircon interrupt requests.
//
// The `fdf::SynchronizedDispatcher` is shut down when the
// `DriverRuntimeBackedDispatcher` is destroyed, or the Driver Framework shuts
// down the driver by which the `fdf::SynchronizedDispatcher` is managed,
// whichever is earlier.
//
// The worker thread which the async dispatcher dispatches its async tasks to is
// managed by the Driver Framework. The dispatcher is not guaranteed to have
// its dedicated thread.
class DriverRuntimeBackedDispatcher final : public Dispatcher {
 public:
  // Factory function that creates a `DriverRuntimeBackedDispatcher` of `name` with
  // the `schedule_role` hint.
  //
  // Preferred for production use.
  //
  // `device` must not be null.
  // `name` must not be empty.
  // `scheduler_role` is a hint. It may or not impact the priority the work
  // scheduler against the dispatcher is handled at. It may or may not impact
  // the ability for other drivers to share Zircon threads with the dispatcher.
  static zx::result<std::unique_ptr<Dispatcher>> Create(std::string_view name,
                                                        std::string_view scheduler_role);

  // Prefer the `Create()` factory method.
  //
  // `fdf_dispatcher` must be valid.
  explicit DriverRuntimeBackedDispatcher(fdf::SynchronizedDispatcher fdf_dispatcher,
                                         std::shared_ptr<libsync::Completion> shutdown_completion);

  ~DriverRuntimeBackedDispatcher() override;

  void Shutdown();

  DriverRuntimeBackedDispatcher(const DriverRuntimeBackedDispatcher&) = delete;
  DriverRuntimeBackedDispatcher(DriverRuntimeBackedDispatcher&&) = delete;
  DriverRuntimeBackedDispatcher& operator=(const DriverRuntimeBackedDispatcher&) = delete;
  DriverRuntimeBackedDispatcher& operator=(DriverRuntimeBackedDispatcher&&) = delete;

  // Dispatcher:
  async_dispatcher_t* async_dispatcher() const override {
    return fdf_dispatcher_.async_dispatcher();
  }

 private:
  fdf::Dispatcher fdf_dispatcher_;
  std::shared_ptr<libsync::Completion> shutdown_completion_;
  bool shut_down_ = false;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DRIVER_RUNTIME_BACKED_DISPATCHER_H_
