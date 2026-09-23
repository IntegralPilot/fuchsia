// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fidl.examples.echo/cpp/wire.h>
#include <lib/ddk/binding_driver.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/driver.h>
#include <lib/fidl/cpp/wire/string_view.h>

#include <string>

#include <ddktl/device.h>
#include <ddktl/unbind-txn.h>

namespace {

class Device;
using DeviceParent = ddk::Device<Device, ddk::Unbindable>;

class Device : public DeviceParent {
 public:
  static zx_status_t Create(void* ctx, zx_device_t* parent) {
    auto device = std::make_unique<Device>(parent);
    auto status = device->Bind();
    if (status != ZX_OK) {
      return status;
    }
    // We've successfully made a fidl call, add a device so the test knows to end.
    status = device->DdkAdd("child");
    if (status == ZX_OK) {
      [[maybe_unused]] auto ptr = device.release();
    }
    return status;
  }

  zx_status_t Bind() {
    auto echo_client = DdkConnectFidlProtocol<fidl_examples_echo::EchoService::Echo>();
    if (echo_client.is_error()) {
      zxlogf(ERROR, "Failed to connect fidl protocol");
      return echo_client.status_value();
    }

    fidl::WireSyncClient client{std::move(*echo_client)};

    constexpr char kInput[] = "Test String";

    auto result = client->EchoString(kInput);
    if (!result.ok()) {
      zxlogf(ERROR, "Failed to call EchoString");
      return result.status();
    }
    if (result.value().response.get() != kInput) {
      zxlogf(ERROR, "Unexpected response: Actual: \"%.*s\", Expected: \"%s\"",
             static_cast<int>(result.value().response.size()), result.value().response.data(),
             kInput);
      return ZX_ERR_INTERNAL;
    }

    zxlogf(INFO, "Recieved: %.*s", (int)result.value().response.size(),
           result.value().response.data());

    auto echo_client2 = DdkConnectFidlProtocol<fidl_examples_echo::EchoService2::Echo>();
    // DFv1 should fail here.
    if (echo_client2.is_ok()) {
      auto result = fidl::WireCall(*echo_client2)->EchoString(kInput);
      // DFv2 should fail here.
      if (result.ok()) {
        zxlogf(ERROR, "Incorrectly able to access %s despite lacking use in manfiest",
               fidl_examples_echo::EchoService2::Name);
        return ZX_ERR_INTERNAL;
      }
    }

    return ZX_OK;
  }

  Device(zx_device_t* parent) : DeviceParent(parent) {}

  void DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }
  void DdkRelease() { delete this; }
};

static constexpr zx_driver_ops_t kDriverOps = []() -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = Device::Create;
  return ops;
}();

}  // namespace

ZIRCON_DRIVER(fidl_service_test_child, kDriverOps, "zircon", "0.1");
