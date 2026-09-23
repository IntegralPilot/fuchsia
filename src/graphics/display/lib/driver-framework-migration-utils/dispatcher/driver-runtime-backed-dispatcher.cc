// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/driver-runtime-backed-dispatcher.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <string_view>

#include <fbl/alloc_checker.h>

namespace display {

// static
zx::result<std::unique_ptr<Dispatcher>> DriverRuntimeBackedDispatcher::Create(
    std::string_view name, std::string_view scheduler_role) {
  ZX_DEBUG_ASSERT(!name.empty());

  fbl::AllocChecker alloc_checker;
  auto shutdown_completion =
      std::shared_ptr<libsync::Completion>(new (&alloc_checker) libsync::Completion());
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<fdf::SynchronizedDispatcher> create_dispatcher_result =
      fdf::SynchronizedDispatcher::Create(
          fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, name,
          [shutdown_completion](fdf_dispatcher_t*) { shutdown_completion->Signal(); },
          scheduler_role);
  if (create_dispatcher_result.is_error()) {
    fdf::error("Failed to create a synchronized dispatcher: {}", create_dispatcher_result);
    return create_dispatcher_result.take_error();
  }

  auto dispatcher = fbl::make_unique_checked<DriverRuntimeBackedDispatcher>(
      &alloc_checker, std::move(create_dispatcher_result).value(), std::move(shutdown_completion));
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(dispatcher));
}

DriverRuntimeBackedDispatcher::DriverRuntimeBackedDispatcher(
    fdf::SynchronizedDispatcher fdf_dispatcher,
    std::shared_ptr<libsync::Completion> shutdown_completion)
    : fdf_dispatcher_(std::move(fdf_dispatcher)),
      shutdown_completion_(std::move(shutdown_completion)) {
  ZX_DEBUG_ASSERT(fdf_dispatcher_.get() != nullptr);
  ZX_DEBUG_ASSERT(shutdown_completion_ != nullptr);
}

void DriverRuntimeBackedDispatcher::Shutdown() {
  if (shut_down_) {
    return;
  }
  shut_down_ = true;
  fdf_dispatcher_.ShutdownAsync();
  shutdown_completion_->Wait();
}

DriverRuntimeBackedDispatcher::~DriverRuntimeBackedDispatcher() { Shutdown(); }

}  // namespace display
