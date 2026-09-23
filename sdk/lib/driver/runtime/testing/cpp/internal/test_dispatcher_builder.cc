// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/runtime/testing/cpp/internal/test_dispatcher_builder.h>
#include <lib/fdf/testing.h>

namespace fdf_internal {

zx::result<fdf::SynchronizedDispatcher>
TestDispatcherBuilder::CreateUnmanagedSynchronizedDispatcher(
    const void* driver, fdf::SynchronizedDispatcher::Options options, std::string_view name,
    fdf::Dispatcher::ShutdownHandler shutdown_handler) {
  ZX_ASSERT_MSG((options.value & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
                    FDF_DISPATCHER_OPTION_SYNCHRONIZED,
                "options.value=%u, needs to have FDF_DISPATCHER_OPTION_SYNCHRONIZED",
                options.value);

  // We need to create an additional shutdown context in addition to the fdf::Dispatcher
  // object, as the fdf::Dispatcher may be destructed before the shutdown handler
  // is called. This can happen if the raw pointer is released from the fdf::Dispatcher.
  auto dispatcher_shutdown_context =
      std::make_unique<fdf::Dispatcher::DispatcherShutdownContext>(std::move(shutdown_handler));
  fdf_dispatcher_t* dispatcher;
  zx_status_t status =
      fdf_testing_create_unmanaged_dispatcher(driver, options.value, name.data(), name.size(),
                                              dispatcher_shutdown_context->observer(), &dispatcher);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  [[maybe_unused]] auto released = dispatcher_shutdown_context.release();
  return zx::ok(fdf::SynchronizedDispatcher(dispatcher));
}

zx::result<fdf::UnsynchronizedDispatcher>
TestDispatcherBuilder::CreateUnmanagedUnsynchronizedDispatcher(
    const void* driver, fdf::UnsynchronizedDispatcher::Options options, std::string_view name,
    fdf::Dispatcher::ShutdownHandler shutdown_handler) {
  ZX_ASSERT_MSG((options.value & FDF_DISPATCHER_OPTION_SYNCHRONIZATION_MASK) ==
                    FDF_DISPATCHER_OPTION_UNSYNCHRONIZED,
                "options.value=%u, needs to have FDF_DISPATCHER_OPTION_UNSYNCHRONIZED",
                options.value);

  // We need to create an additional shutdown context in addition to the fdf::Dispatcher
  // object, as the fdf::Dispatcher may be destructed before the shutdown handler
  // is called. This can happen if the raw pointer is released from the fdf::Dispatcher.
  auto dispatcher_shutdown_context =
      std::make_unique<fdf::Dispatcher::DispatcherShutdownContext>(std::move(shutdown_handler));
  fdf_dispatcher_t* dispatcher;
  zx_status_t status =
      fdf_testing_create_unmanaged_dispatcher(driver, options.value, name.data(), name.size(),
                                              dispatcher_shutdown_context->observer(), &dispatcher);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  [[maybe_unused]] auto released = dispatcher_shutdown_context.release();
  return zx::ok(fdf::UnsynchronizedDispatcher(dispatcher));
}

}  // namespace fdf_internal
