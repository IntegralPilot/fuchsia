// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/loop-backed-dispatcher-factory.h"

#include <lib/async-loop/loop.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <memory>
#include <string_view>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/dispatcher.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/dispatcher/loop-backed-dispatcher.h"

namespace display {

namespace {

const async_loop_config_t kIrqDispatcherLoopConfig = []() {
  async_loop_config_t loop_config = kAsyncLoopConfigNeverAttachToThread;
  loop_config.irq_support = true;
  return loop_config;
}();

}  // namespace

// static
zx::result<std::unique_ptr<LoopBackedDispatcherFactory>> LoopBackedDispatcherFactory::Create(
    zx_device_t* device) {
  fbl::AllocChecker alloc_checker;
  auto factory = fbl::make_unique_checked<LoopBackedDispatcherFactory>(&alloc_checker, device);
  if (!alloc_checker.check()) {
    zxlogf(ERROR, "Failed to allocate memory for LoopBackedDispatcherFactory");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(factory));
}

LoopBackedDispatcherFactory::LoopBackedDispatcherFactory(zx_device_t* device) : device_(device) {
  ZX_DEBUG_ASSERT(device != nullptr);
}

LoopBackedDispatcherFactory::~LoopBackedDispatcherFactory() = default;

zx::result<std::unique_ptr<Dispatcher>> LoopBackedDispatcherFactory::Create(
    std::string_view name, std::string_view scheduler_role) {
  // `kIrqDispatcherLoopConfig` is used so that the factory creates
  // `Dispatcher`s that can dispatch interrupt handlers.
  return LoopBackedDispatcher::Create(device_, /*thread_name=*/name, scheduler_role);
}

}  // namespace display
