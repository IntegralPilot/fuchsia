// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_TEST_DISPATCHER_BUILDER_H_
#define LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_TEST_DISPATCHER_BUILDER_H_

#include <lib/fdf/cpp/dispatcher.h>

namespace fdf_internal {

class TestDispatcherBuilder {
 public:
  // Creates an unmanaged synchronized dispatcher. This dispatcher is not ran on the managed driver
  // runtime thread pool.
  static zx::result<fdf::SynchronizedDispatcher> CreateUnmanagedSynchronizedDispatcher(
      const void* driver, fdf::SynchronizedDispatcher::Options options, std::string_view name,
      fdf::Dispatcher::ShutdownHandler shutdown_handler);

  // Creates an unmanaged unsynchronized dispatcher. This dispatcher is not ran on the managed
  // driver runtime thread pool.
  static zx::result<fdf::UnsynchronizedDispatcher> CreateUnmanagedUnsynchronizedDispatcher(
      const void* driver, fdf::UnsynchronizedDispatcher::Options options, std::string_view name,
      fdf::Dispatcher::ShutdownHandler shutdown_handler);
};

}  // namespace fdf_internal

#endif  // LIB_DRIVER_RUNTIME_TESTING_CPP_INTERNAL_TEST_DISPATCHER_BUILDER_H_
