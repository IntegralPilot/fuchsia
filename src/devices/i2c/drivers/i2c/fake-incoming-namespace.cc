// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake-incoming-namespace.h"

#include "src/devices/i2c/drivers/i2c/fake-i2c-impl.h"
#include "zxtest/zxtest.h"

namespace i2c {
void FakeIncomingNamespace::AddI2cImplService(fidl::ServerEnd<fuchsia_io::Directory> server_end) {
  auto service_result = outgoing_.AddService<fuchsia_hardware_i2cimpl::Service>(
      fake_i2c_impl_.CreateInstanceHandler());
  ASSERT_OK(service_result);
  ASSERT_OK(outgoing_.Serve(std::move(server_end)));
}

void FakeIncomingNamespace::set_on_transact(FakeI2cImpl::OnTransact on_transact) {
  fake_i2c_impl_.set_on_transact(std::move(on_transact));
}

}  // namespace i2c
