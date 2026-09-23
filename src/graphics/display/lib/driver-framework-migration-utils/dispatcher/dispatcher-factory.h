// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DISPATCHER_FACTORY_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DISPATCHER_FACTORY_H_

#include <lib/zx/result.h>

#include <memory>
#include <string_view>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"

namespace display {

// A `DispatcherFactory` creates `Dispatcher` instances on which drivers can
// dispatch asynchronous tasks and interrupt handlers.
class DispatcherFactory {
 public:
  DispatcherFactory() = default;
  virtual ~DispatcherFactory() = default;

  DispatcherFactory(const DispatcherFactory&) = delete;
  DispatcherFactory(DispatcherFactory&&) = delete;
  DispatcherFactory& operator=(const DispatcherFactory&) = delete;
  DispatcherFactory& operator=(DispatcherFactory&&) = delete;

  // `name` must not be empty.
  //
  // `scheduler_role` is a hint on scheduling priority of the Dispatcher.
  // It may or not impact the priority the work scheduler against the dispatcher
  // is handled at. It may or may not impact the ability for other drivers to
  // share Zircon threads with the dispatcher.
  virtual zx::result<std::unique_ptr<Dispatcher>> Create(std::string_view name,
                                                         std::string_view scheduler_role) = 0;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DISPATCHER_FACTORY_H_
