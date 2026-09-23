// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DISPATCHER_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DISPATCHER_H_

#include <lib/async/dispatcher.h>

namespace display {

// A RAII wrapper of an asynchronous dispatcher and its running environment.
//
// Examples of running environments of async dispatchers include:
// - An `async::Loop` owned by the `Dispatcher` itself, spawning a thread to
//   dispatch all its tasks on.
// - An `fdf::Dispatcher` co-managed by the Driver Framework, running on a
//   background thread owned by the Driver Framework.
//
// # Dispatcher capability
//
// A `display::Dispatcher` must support dispatching both async tasks and async
// interrupt requests.
//
// # Dispatcher availability
//
// `display::Dispatcher` will not actively shut down the async dispatcher's
// running environment until the `display::Dispatcher` is destroyed. However,
// a third party may also shut down the running environment, in which case the
// availability of the async dispatcher will be also determined by the side
// contract between the running environment and the third party.
//
// For example, for a `display::Dispatcher` with an `fdf::Dispatcher` backend,
// the Driver Framework may choose to early-shutdown the `fdf::Dispatcher`,
// which makes the `async_dispatcher_t` unable to dispatch any task, even if
// the `display::Dispatcher` is still alive.
//
// # Thread safety
//
// The `async_dispatcher()` getter must not be used concurrently with
// destruction of the `Dispatcher`.
//
// TODO(https://fxbug.dev/323061435): This class should be only used when the
// display drivers are being migrated to DFv2. Drivers should replace
// `display::Dispatcher` with `fdf::SynchronizedDispatcher` when the migration
// is done.
class Dispatcher {
 public:
  Dispatcher() = default;
  virtual ~Dispatcher() = default;

  Dispatcher(const Dispatcher&) = delete;
  Dispatcher(Dispatcher&&) = delete;
  Dispatcher& operator=(const Dispatcher&) = delete;
  Dispatcher& operator=(Dispatcher&&) = delete;

  virtual async_dispatcher_t* async_dispatcher() const = 0;
};

}  // namespace display

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_DISPATCHER_DISPATCHER_H_
