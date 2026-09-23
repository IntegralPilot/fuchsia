// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef BUILD_BAZEL_SDK_TESTS_FUCHSIA_PACKAGING_FUCHSIA_PACKAGE_CC_DRIVER_CC_TEST_DRIVER_H_
#define BUILD_BAZEL_SDK_TESTS_FUCHSIA_PACKAGING_FUCHSIA_PACKAGE_CC_DRIVER_CC_TEST_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>

namespace cc_test_driver {

class CCTestDriver : public fdf::DriverBase2 {
 public:
  CCTestDriver() : fdf::DriverBase2("cc-test-driver") {}
  virtual ~CCTestDriver() = default;

  zx::result<> Start(fdf::DriverContext context) override;

 private:
};

}  // namespace cc_test_driver

#endif  // BUILD_BAZEL_SDK_TESTS_FUCHSIA_PACKAGING_FUCHSIA_PACKAGE_CC_DRIVER_CC_TEST_DRIVER_H_
