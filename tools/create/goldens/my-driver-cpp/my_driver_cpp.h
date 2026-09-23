// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_CREATE_GOLDENS_MY_DRIVER_CPP_MY_DRIVER_CPP_H_
#define TOOLS_CREATE_GOLDENS_MY_DRIVER_CPP_MY_DRIVER_CPP_H_

#include <lib/driver/component/cpp/driver_base2.h>

namespace my_driver_cpp {

class MyDriverCpp : public fdf::DriverBase2 {
 public:
  MyDriverCpp() : fdf::DriverBase2("my_driver_cpp") {}

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) override;
};

}  // namespace my_driver_cpp

#endif  // TOOLS_CREATE_GOLDENS_MY_DRIVER_CPP_MY_DRIVER_CPP_H_
