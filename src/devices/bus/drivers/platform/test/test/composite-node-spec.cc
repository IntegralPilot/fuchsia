// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>

#include <memory>

#include <ddktl/device.h>

namespace test_composite_node_spec {

class TestCompositeNodeSpec;
using DeviceType = ddk::Device<TestCompositeNodeSpec>;

class TestCompositeNodeSpec : public DeviceType {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent) {
    auto dev = std::make_unique<TestCompositeNodeSpec>(parent);

    zxlogf(INFO, "TestCompositeNodeSpec::Create: test-composite-node-spec");

    auto status = dev->DdkAdd("test-composite-node-spec");
    if (status != ZX_OK) {
      zxlogf(ERROR, "%s: DdkAdd failed: %d", __func__, status);
      return status;
    }

    [[maybe_unused]] auto ptr = dev.release();

    return ZX_OK;
  }

  explicit TestCompositeNodeSpec(zx_device_t* parent) : DeviceType(parent) {}

  void DdkRelease() { delete this; }
};

constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t driver_ops = {};
  driver_ops.version = DRIVER_OPS_VERSION;
  driver_ops.bind = TestCompositeNodeSpec::Create;
  return driver_ops;
}();

}  // namespace test_composite_node_spec

ZIRCON_DRIVER(test_composite_node_spec, test_composite_node_spec::driver_ops, "zircon", "0.1");
