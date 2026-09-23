// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file contains syscall interception code used for testing power-manager.

#ifndef SRC_DEVICES_TESTING_SYSCALL_INTERCEPT_SYSCALL_INTERCEPT_H_
#define SRC_DEVICES_TESTING_SYSCALL_INTERCEPT_SYSCALL_INTERCEPT_H_

#include <lib/async/dispatcher.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>

#include <memory>

#include <sdk/lib/driver/outgoing/cpp/outgoing_directory.h>

namespace syscall_intercept {

class SuspendObserver {
 public:
  // Must be invoked from the async dispatcher's thread context.
  SuspendObserver(const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                  async_dispatcher_t* dispatcher);

  // Must be invoked from the async dispatcher's thread context.
  ~SuspendObserver();
};

}  // namespace syscall_intercept

#endif  // SRC_DEVICES_TESTING_SYSCALL_INTERCEPT_SYSCALL_INTERCEPT_H_
