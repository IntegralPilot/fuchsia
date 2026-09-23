// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/tests/driver-multiname-test/parent_device.h"

#include <lib/ddk/binding_driver.h>

#include "src/devices/tests/driver-multiname-test/child_device.h"

namespace parent_device {

void ParentDevice::DdkInit(ddk::InitTxn txn) { txn.Reply(ZX_OK); }

void ParentDevice::DdkRelease() { delete this; }

void ParentDevice::AddDevice(AddDeviceCompleter::Sync& completer) {
  std::unique_ptr child = std::make_unique<child_device::ChildDevice>(zxdev());

  zx_status_t status = child->DdkAdd("duplicate");
  if (status == ZX_OK) {
    [[maybe_unused]] child_device::ChildDevice* ptr = child.release();
  }
  completer.Reply(zx::make_result(status));
}

static zx_driver_ops_t parent_device_driver_ops = {
    .version = DRIVER_OPS_VERSION,
    .bind =
        [](void* ctx, zx_device_t* dev) {
          std::unique_ptr device = std::make_unique<ParentDevice>(dev);
          zx_status_t status = device->DdkAdd(
              ddk::DeviceAddArgs("parent_device").set_flags(DEVICE_ADD_NON_BINDABLE));
          if (status == ZX_OK) {
            [[maybe_unused]] ParentDevice* ptr = device.release();
          }
          return status;
        },
};

}  // namespace parent_device

ZIRCON_DRIVER(ParentDevice, parent_device::parent_device_driver_ops, "zircon", "0.1");
