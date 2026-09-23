// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "driver.h"

#include <lib/driver/component/cpp/driver_export2.h>

namespace example_driver {

zx::result<> ExampleDriver::Start(fdf::DriverContext context) { return zx::ok(); }

}  // namespace example_driver

FUCHSIA_DRIVER_EXPORT2(example_driver::ExampleDriver);
