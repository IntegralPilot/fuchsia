// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/driver.h>

#include <ddktl/device.h>

namespace composite {

class Composite;

using DeviceType = ddk::Device<Composite>;

class Composite : public DeviceType {
 public:
  explicit Composite(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Bind(void* ctx, zx_device_t* device) {
    auto dev = std::make_unique<Composite>(device);

    auto status = dev->DdkAdd("composite");
    if (status != ZX_OK) {
      return status;
    }

    [[maybe_unused]] auto ptr = dev.release();
    return ZX_OK;
  }

  void DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }
  void DdkRelease() { delete this; }
};

static zx_driver_ops_t kDriverOps = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Composite::Bind;
  return ops;
}();

}  // namespace composite

ZIRCON_DRIVER(composite, composite::kDriverOps, "zircon", "0.1");
