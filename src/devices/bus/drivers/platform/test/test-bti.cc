// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.btitest/cpp/wire.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <memory>

#include <ddktl/device.h>
#include <ddktl/fidl.h>

namespace {

class TestBti;
using DeviceType =
    ddk::Device<TestBti, ddk::Messageable<fuchsia_hardware_btitest::BtiDevice>::Mixin>;

class TestBti : public DeviceType {
 public:
  explicit TestBti(zx_device_t* parent) : DeviceType(parent) {}

  static zx_status_t Create(void*, zx_device_t* parent);

  void DdkRelease() { delete this; }

  void GetKoid(GetKoidCompleter::Sync& completer) override;
  void Crash(CrashCompleter::Sync&) override { __builtin_abort(); }
};

zx_status_t TestBti::Create(void*, zx_device_t* parent) {
  auto device = std::make_unique<TestBti>(parent);

  zx_status_t status = device->DdkAdd("test-bti");
  if (status != ZX_OK) {
    zxlogf(ERROR, "DdkAdd failed: %d", status);
    return status;
  }
  [[maybe_unused]] auto dummy = device.release();

  return ZX_OK;
}
void TestBti::GetKoid(GetKoidCompleter::Sync& completer) {
  zx::result pdev_client_end =
      DdkConnectFidlProtocol<fuchsia_hardware_platform_device::Service::Device>(parent());
  if (pdev_client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect to platform device: %s", pdev_client_end.status_string());
    completer.Close(pdev_client_end.status_value());
    return;
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  zx::result bti = pdev.GetBti(0);
  if (bti.is_error()) {
    zxlogf(ERROR, "Failed to get bti: %s", bti.status_string());
    completer.Close(bti.status_value());
    return;
  }

  zx_info_handle_basic_t info;
  zx_status_t status = bti->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    completer.Close(status);
    return;
  }

  completer.Reply(info.koid);
}

static zx_driver_ops_t test_driver_ops = {
    .version = DRIVER_OPS_VERSION,
    .bind = TestBti::Create,
};

}  // namespace

ZIRCON_DRIVER(test_bti, test_driver_ops, "zircon", "0.1");
