// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef BUILD_BAZEL_SDK_TESTS_FUCHSIA_DRIVERS_DRIVER_H_
#define BUILD_BAZEL_SDK_TESTS_FUCHSIA_DRIVERS_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>

namespace example_driver {

class ExampleDriver : public fdf::DriverBase2 {
 public:
  ExampleDriver() : fdf::DriverBase2("example-driver") {}
  virtual ~ExampleDriver() = default;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
};

}  // namespace example_driver

#endif  // BUILD_BAZEL_SDK_TESTS_FUCHSIA_DRIVERS_DRIVER_H_
