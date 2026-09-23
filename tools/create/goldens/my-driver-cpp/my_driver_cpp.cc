// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/create/goldens/my-driver-cpp/my_driver_cpp.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

namespace my_driver_cpp {

zx::result<> MyDriverCpp::Start(fdf::DriverContext context) {
  return zx::ok();
}

void MyDriverCpp::Stop(fdf::StopCompleter completer) {
  completer(zx::ok());
}

}  // namespace my_driver_cpp

FUCHSIA_DRIVER_EXPORT2(my_driver_cpp::MyDriverCpp);
