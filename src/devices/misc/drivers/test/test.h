// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_MISC_DRIVERS_TEST_TEST_H_
#define SRC_DEVICES_MISC_DRIVERS_TEST_TEST_H_

#include <lib/driver/component/cpp/driver_base2.h>

class TestDriver : public fdf::DriverBase2 {
 public:
  TestDriver() : DriverBase2("test_driver") {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  fdf::OwnedChildNode child_;
};

#endif  // SRC_DEVICES_MISC_DRIVERS_TEST_TEST_H_
