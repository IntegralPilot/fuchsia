// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_DRIVERS_I2C_FAKE_INCOMING_NAMESPACE_H_
#define SRC_DEVICES_I2C_DRIVERS_I2C_FAKE_INCOMING_NAMESPACE_H_

#include <lib/driver/outgoing/cpp/outgoing_directory.h>

#include "fake-i2c-impl.h"

namespace i2c {
class FakeIncomingNamespace {
 public:
  explicit FakeIncomingNamespace(uint64_t max_transfer_size, FakeI2cImpl::OnTransact on_transact =
                                                                 FakeI2cImpl::kDefaultOnTransact)
      : fake_i2c_impl_(max_transfer_size, std::move(on_transact)) {}

  void AddI2cImplService(fidl::ServerEnd<fuchsia_io::Directory> server_end);

  void set_on_transact(FakeI2cImpl::OnTransact on_transact);

 private:
  FakeI2cImpl fake_i2c_impl_;
  fdf::OutgoingDirectory outgoing_{fdf::Dispatcher::GetCurrent()->get()};
};
}  // namespace i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_I2C_FAKE_INCOMING_NAMESPACE_H_
