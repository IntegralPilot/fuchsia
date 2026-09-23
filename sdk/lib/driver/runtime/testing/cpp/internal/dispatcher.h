// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_DISPATCHER_H_
#define LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_DISPATCHER_H_

#include <lib/driver/runtime/testing/cpp/internal/default_dispatcher_setting.h>
#include <lib/driver/runtime/testing/cpp/sync_helpers.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/testing.h>
#include <lib/sync/cpp/completion.h>

namespace fdf_internal {

// A RAII wrapper around an fdf::SynchronizedDispatcher that is meant for testing.
// There are two types of TestSynchronizedDispatcher: Default and managed. Both types
// are described in length down below at |kDispatcherDefault| and |kDispatcherManaged|.
class TestSynchronizedDispatcher {
 public:
  // The type of the dispatcher.
  // See |fdf::kDispatcherDefault| and |fdf::kDispatcherManaged| for a full description of each.
  enum class DispatcherType {
    Default,
    Managed,
  };

  // This will start the underlying dispatcher with the given type. If the underlying
  // dispatcher fails to start, this constructor will throw an assert.
  explicit TestSynchronizedDispatcher(DispatcherType type);

  // Don't allow moving since raw pointers to this are captured.
  TestSynchronizedDispatcher(TestSynchronizedDispatcher&& other) = delete;
  TestSynchronizedDispatcher& operator=(TestSynchronizedDispatcher&& other) = delete;

  // Initiates and waits for shutdown of the dispatcher.
  // When the destructor returns the shutdown has completed.
  ~TestSynchronizedDispatcher();

  const fdf::SynchronizedDispatcher& driver_dispatcher() { return dispatcher_; }
  async_dispatcher_t* dispatcher() { return dispatcher_.async_dispatcher(); }

  const void* owner() const { return owner_driver_; }

 private:
  // Start a managed dispatcher. Once this returns successfully the dispatcher is available to be
  // used for queueing and running tasks.
  //
  // This dispatcher will be ran on the managed driver runtime thread pool.
  //
  // This MUST be called from the main test thread.
  zx::result<> StartManaged(fdf::SynchronizedDispatcher::Options options,
                            std::string_view dispatcher_name);

  // Start an unmanaged dispatcher, and set it as the default dispatcher for the driver runtime.
  // Once this returns successfully the dispatcher is available to be used for queueing and running
  // tasks.
  //
  // This dispatcher is not going to run on the managed driver runtime thread pool,
  // therefore it must be manually ran using |fdf_testing_run_until_idle|, or any of the helpers
  // that wrap that behavior (eg: |RunOnDispatcherSync|, |WaitFor|).
  //
  // This MUST be called from the main test thread.
  zx::result<> StartDefault(fdf::SynchronizedDispatcher::Options options,
                            std::string_view dispatcher_name);

  // This will stop the dispatcher and wait until it stops.
  // When this function returns, the dispatcher is stopped.
  // Safe to call multiple times. It will return immediately if Stop has already happened
  //
  // This MUST be called from the main test thread.
  zx::result<> Stop();

  std::optional<fdf_internal::DefaultDispatcherSetting> default_dispatcher_setting_;
  fdf::SynchronizedDispatcher dispatcher_;
  libsync::Completion dispatcher_shutdown_;
  const void* owner_driver_;
};

// Starts a driver dispatcher that becomes the default driver dispatcher for the current thread.
// This dispatcher is not handled by the managed driver runtime thread pool, but instead must be
// manually told to run using the |fdf_testing_run_until_idle| call, or the provided helpers like
// |RunOnDispatcherSync| and |WaitFor|.
//
// Objects that live on default dispatchers can be accessed directly from the test.
//
// You can only create 1 default dispatcher at a time.
extern const TestSynchronizedDispatcher::DispatcherType kDispatcherDefault;

// Starts a driver dispatcher that is handled by the managed driver runtime thread pool.
// It is good practice to create an |fdf_testing::DriverRuntimeEnv| instance before
// creating managed dispatchers. This serves 3 purposes:
// - It serves as a visual reminder to test authors that the driver runtime is managing
//   the managed thread-pool.
// - An initial thread is created on the thread pool to avoid deadlocking in some scenarios.
// - Exercises the cleanup path during teardown.
//
// Objects that live on a managed dispatcher should be wrapped with an
// |async_patterns::TestDispatcherBound|.
extern const TestSynchronizedDispatcher::DispatcherType kDispatcherManaged;

}  // namespace fdf_internal

#endif  // LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_DISPATCHER_H_
