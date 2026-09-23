// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_DRIVER_TESTS_TRANSPORT_SCOPED_FAKE_DRIVER_H_
#define LIB_FIDL_DRIVER_TESTS_TRANSPORT_SCOPED_FAKE_DRIVER_H_

#include <lib/fdf/env.h>

namespace fidl_driver_testing {

class ScopedFakeDriver {
 public:
  ScopedFakeDriver() {
    void* driver = reinterpret_cast<void*>(1);
    fdf_env_register_driver_entry(driver);
  }

  ~ScopedFakeDriver() { fdf_env_register_driver_exit(); }
};

}  // namespace fidl_driver_testing

#endif  // LIB_FIDL_DRIVER_TESTS_TRANSPORT_SCOPED_FAKE_DRIVER_H_
