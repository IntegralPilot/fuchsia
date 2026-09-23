// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_DRIVERS_I2C_FAKE_I2C_IMPL_H_
#define SRC_DEVICES_I2C_DRIVERS_I2C_FAKE_I2C_IMPL_H_

#include <fidl/fuchsia.hardware.i2cimpl/cpp/driver/wire.h>

namespace i2c {
class FakeI2cImpl : public fdf::WireServer<fuchsia_hardware_i2cimpl::Device> {
 public:
  using OnTransact =
      std::function<void(TransactRequestView, fdf::Arena&, TransactCompleter::Sync&)>;

  static const OnTransact kDefaultOnTransact;

  explicit FakeI2cImpl(uint64_t max_transfer_size, OnTransact on_transact = kDefaultOnTransact)
      : on_transact_(std::move(on_transact)), max_transfer_size_(max_transfer_size) {}

  fuchsia_hardware_i2cimpl::Service::InstanceHandler CreateInstanceHandler();

  void set_on_transact(OnTransact on_transact) { on_transact_ = std::move(on_transact); }

  // Protocol methods.
  void GetMaxTransferSize(fdf::Arena& arena, GetMaxTransferSizeCompleter::Sync& completer) override;
  void SetBitrate(SetBitrateRequestView request, fdf::Arena& arena,
                  SetBitrateCompleter::Sync& completer) override;
  void Transact(TransactRequestView request, fdf::Arena& arena,
                TransactCompleter::Sync& completer) override;
  void handle_unknown_method(fidl::UnknownMethodMetadata<fuchsia_hardware_i2cimpl::Device> metadata,
                             fidl::UnknownMethodCompleter::Sync& completer) override {}  // No-op.

 private:
  OnTransact on_transact_;
  uint64_t max_transfer_size_;
  fdf::ServerBindingGroup<fuchsia_hardware_i2cimpl::Device> bindings_;
};
}  // namespace i2c

#endif  // SRC_DEVICES_I2C_DRIVERS_I2C_FAKE_I2C_IMPL_H_
