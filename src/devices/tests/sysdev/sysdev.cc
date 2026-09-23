// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/sysdev/sysdev.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zx/channel.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <memory>

#include <ddktl/device.h>

namespace {

class Sysdev;
using SysdevType = ddk::Device<Sysdev>;

class Sysdev : public SysdevType {
 public:
  explicit Sysdev(zx_device_t* device) : SysdevType(device) {}

  static zx_status_t Create(void* ctx, zx_device_t* parent, const char* name,
                            zx_handle_t items_svc_handle);

  // Device protocol implementation.
  void DdkRelease() { delete this; }

  zx_status_t MakeComposite();
  zx_status_t AddTestParent();
};

class TestParent;
using TestParentType = ddk::Device<TestParent>;

class TestParent : public TestParentType {
 public:
  explicit TestParent(zx_device_t* device) : TestParentType(device) {}

  static zx_status_t Create(zx_device_t* parent);

  // Device protocol implementation.
  void DdkRelease() { delete this; }
};

zx_status_t TestParent::Create(zx_device_t* parent) {
  auto test_parent = std::make_unique<TestParent>(parent);
  zx_status_t status = test_parent->DdkAdd(ddk::DeviceAddArgs("test")
                                               .set_proto_id(ZX_PROTOCOL_TEST_PARENT)
                                               .set_flags(DEVICE_ADD_ALLOW_MULTI_COMPOSITE));
  if (status != ZX_OK) {
    return status;
  }

  // Now owned by the driver framework.
  [[maybe_unused]] auto ptr = test_parent.release();

  return ZX_OK;
}

zx_status_t Sysdev::Create(void* ctx, zx_device_t* parent, const char* name,
                           zx_handle_t items_svc_handle) {
  zx::channel items_svc(items_svc_handle);
  auto sysdev = std::make_unique<Sysdev>(parent);

  zx_status_t status = sysdev->DdkAdd(ddk::DeviceAddArgs("sys").set_flags(DEVICE_ADD_NON_BINDABLE));
  if (status != ZX_OK) {
    return status;
  }

  ZX_ASSERT(status == ZX_OK);

  status = TestParent::Create(sysdev->zxdev());

  // Now owned by devmgr.
  [[maybe_unused]] auto ptr = sysdev.release();

  return status;
}

static constexpr zx_driver_ops_t driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.create = Sysdev::Create;
  return ops;
}();

}  // namespace

ZIRCON_DRIVER(test_sysdev, driver_ops, "zircon", "0.1");
