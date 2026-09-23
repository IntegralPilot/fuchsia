// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "generic-suspend-for-tests.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <memory>

#include <sdk/lib/driver/outgoing/cpp/outgoing_directory.h>

#include "src/devices/testing/syscall-intercept/syscall-intercept.h"

namespace suspend {

void GenericSuspendForTests::AtStart() {
  suspend_observer_ =
      std::make_unique<syscall_intercept::SuspendObserver>(outgoing(), dispatcher());
}

}  // namespace suspend

FUCHSIA_DRIVER_EXPORT2(suspend::GenericSuspendForTests);
