// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/task.h>
#include <lib/driver/runtime/testing/cpp/internal/dispatcher.h>
#include <lib/driver/runtime/testing/cpp/internal/test_dispatcher_builder.h>
#include <lib/driver/runtime/testing/cpp/internal/wait_for.h>
#include <lib/fdf/cpp/env.h>
#include <lib/fdf/testing.h>

namespace fdf_internal {

namespace {
uintptr_t owner_gen = 0xdeadbeef;
}

TestSynchronizedDispatcher::TestSynchronizedDispatcher(DispatcherType type)
    : owner_driver_(reinterpret_cast<void*>(owner_gen++)) {
  if (type == DispatcherType::Default) {
    zx::result result =
        StartDefault(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "fdf-default");
    ZX_ASSERT_MSG(result.is_ok(), "Failed to start default dispatcher: %s", result.status_string());
    return;
  }

  if (type == DispatcherType::Managed) {
    zx::result result =
        StartManaged(fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "fdf-managed");
    ZX_ASSERT_MSG(result.is_ok(), "Failed to start managed dispatcher: %s", result.status_string());
    return;
  }

  ZX_ASSERT_MSG(false, "Unknown DispatcherType: %d", static_cast<int>(type));
}

TestSynchronizedDispatcher::~TestSynchronizedDispatcher() {
  // Stop is safe to call multiple times. It returns immediately if Stop has already happened.
  zx::result stop_result = Stop();
  ZX_ASSERT_MSG(stop_result.is_ok(), "Stop failed: %s", stop_result.status_string());
}

zx::result<> TestSynchronizedDispatcher::StartManaged(fdf::SynchronizedDispatcher::Options options,
                                                      std::string_view dispatcher_name) {
  auto dispatcher = fdf_env::DispatcherBuilder::CreateSynchronizedWithOwner(
      owner_driver_, options, dispatcher_name,
      [this](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown_.Signal(); });
  if (dispatcher.is_error()) {
    return dispatcher.take_error();
  }
  dispatcher_ = std::move(dispatcher.value());
  return zx::ok();
}

zx::result<> TestSynchronizedDispatcher::StartDefault(fdf::SynchronizedDispatcher::Options options,
                                                      std::string_view dispatcher_name) {
  // Default dispatchers are created as unmanaged dispatchers so that they are safe to access from
  // the main thread.
  auto dispatcher = fdf_internal::TestDispatcherBuilder::CreateUnmanagedSynchronizedDispatcher(
      owner_driver_, options, dispatcher_name,
      [this](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown_.Signal(); });
  if (dispatcher.is_error()) {
    return dispatcher.take_error();
  }
  dispatcher_ = std::move(dispatcher.value());
  default_dispatcher_setting_.emplace(dispatcher_.get());
  return zx::ok();
}

zx::result<> TestSynchronizedDispatcher::Stop() {
  dispatcher_.ShutdownAsync();
  zx::result<> stop_result = fdf::WaitFor(dispatcher_shutdown_);
  default_dispatcher_setting_.reset();
  return stop_result;
}

const TestSynchronizedDispatcher::DispatcherType kDispatcherDefault =
    TestSynchronizedDispatcher::DispatcherType::Default;

const TestSynchronizedDispatcher::DispatcherType kDispatcherManaged =
    TestSynchronizedDispatcher::DispatcherType::Managed;

}  // namespace fdf_internal
