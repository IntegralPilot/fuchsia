// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device.manager.test/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>

#include <vector>

#include <ddktl/device.h>
#include <fbl/alloc_checker.h>

class IsolatedDevMgrTestDriver;
using DeviceType = ddk::Device<IsolatedDevMgrTestDriver,
                               ddk::Messageable<fuchsia_device_manager_test::Metadata>::Mixin>;
class IsolatedDevMgrTestDriver : public DeviceType {
 public:
  IsolatedDevMgrTestDriver(zx_device_t* parent) : DeviceType(parent) {}
  zx_status_t Bind();
  void DdkRelease() { delete this; }

  void GetMetadata(GetMetadataRequestView request, GetMetadataCompleter::Sync& completer);

 private:
  fidl::WireSyncClient<fuchsia_hardware_platform_device::Device> pdev_;
};

void IsolatedDevMgrTestDriver::GetMetadata(GetMetadataRequestView request,
                                           GetMetadataCompleter::Sync& completer) {
  fidl::WireResult result = pdev_->GetMetadata(request->id);
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to send GetMetadata request: %s", result.status_string());
    completer.Close(result.status());
  }
  if (result->is_error()) {
    completer.Close(result->error_value());
  }
  const auto& metadata = result.value()->metadata;
  completer.Reply(fidl::VectorView<uint8_t>::FromExternal(metadata.data(), metadata.size()));
}

zx_status_t IsolatedDevMgrTestDriver::Bind() {
  auto pdev_client = DdkConnectFidlProtocol<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client.is_error()) {
    zxlogf(ERROR, "Failed to connect to platform device: %s", pdev_client.status_string());
    return pdev_client.status_value();
  }
  pdev_.Bind(std::move(pdev_client.value()));

  return DdkAdd("metadata-test");
}

zx_status_t isolateddevmgr_test_bind(void* ctx, zx_device_t* device) {
  fbl::AllocChecker ac;
  auto dev = fbl::make_unique_checked<IsolatedDevMgrTestDriver>(&ac, device);
  if (!ac.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  auto status = dev->Bind();
  if (status == ZX_OK) {
    // devmgr is now in charge of the memory for dev
    [[maybe_unused]] auto ptr = dev.release();
  }
  return status;
}

static constexpr zx_driver_ops_t isolateddevmgr_test_driver_ops = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = isolateddevmgr_test_bind;
  return ops;
}();

ZIRCON_DRIVER(metadata - test, isolateddevmgr_test_driver_ops, "zircon", "0.1");
