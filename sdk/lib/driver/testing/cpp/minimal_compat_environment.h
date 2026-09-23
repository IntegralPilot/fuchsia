// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TESTING_CPP_MINIMAL_COMPAT_ENVIRONMENT_H_
#define LIB_DRIVER_TESTING_CPP_MINIMAL_COMPAT_ENVIRONMENT_H_

#include <lib/driver/compat/cpp/device_server.h>

namespace fdf_testing {

// A minimal class with a compat device server that can be used as
// the Configuration's EnvironmentType if there are no needed
// customizations to the environment or compat server.
class MinimalCompatEnvironment : public Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    device_server_.Initialize(component::kDefaultInstance);
    return zx::make_result(
        device_server_.Serve(fdf::Dispatcher::GetCurrent()->async_dispatcher(), &to_driver_vfs));

    return zx::ok();
  }

 private:
  compat::DeviceServer device_server_;
};

}  // namespace fdf_testing

#endif  // LIB_DRIVER_TESTING_CPP_MINIMAL_COMPAT_ENVIRONMENT_H_
