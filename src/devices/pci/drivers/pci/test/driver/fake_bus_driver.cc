// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fake_bus_driver.h"

#include <lib/ddk/binding_driver.h>
#include <lib/ddk/platform-defs.h>
#include <lib/fit/defer.h>

#include "src/devices/pci/drivers/pci/config.h"
#include "src/devices/pci/drivers/pci/device.h"
#include "src/devices/pci/drivers/pci/test/driver/driver_tests.h"
#include "src/devices/pci/drivers/pci/test/fakes/fake_config.h"
#include "src/devices/pci/drivers/pci/test/fakes/test_device.h"

namespace pci {

zx_status_t FakeBusDriver::Create(zx_device_t* parent, const char* name, uint8_t start_bus,
                                  uint8_t end_bus, bool is_extended) {
  auto bus_driver =
      std::unique_ptr<FakeBusDriver>(new FakeBusDriver(parent, start_bus, end_bus, is_extended));
  zx_status_t st = bus_driver->DdkAdd(ddk::DeviceAddArgs(name).set_flags(DEVICE_ADD_NON_BINDABLE));
  if (st != ZX_OK) {
    return st;
  }

  auto cleanup = fit::defer([&bus_driver] { bus_driver->DdkAsyncRemove(); });
  st = bus_driver->CreateDevice(bus_driver->test_bdf(), kFakeQuadroDeviceConfig.data(),
                                kFakeQuadroDeviceConfig.max_size());
  if (st != ZX_OK) {
    return st;
  }

  bus_driver->upstream().ConfigureDownstreamDevices();
  cleanup.cancel();
  [[maybe_unused]] void* unused = bus_driver.release();

  return ZX_OK;
}

// Creates a device, seeding the configuration space with a given buffer if provided.
zx_status_t FakeBusDriver::CreateDevice(pci_bdf_t bdf, uint8_t* base_cfg, size_t base_cfg_size,
                                        uint16_t vid, uint16_t did) {
  fdf::MmioView view = bus_.pciroot().ecam().mmio().View(
      GetConfigOffsetInCam(bdf, 0, /*is_extended=*/bus_.pciroot().is_extended()),
      zx_system_get_page_size());
  for (uint32_t off = 0; off < base_cfg_size; off++) {
    view.Write(base_cfg[off], off);
  }

  std::unique_ptr<Config> cfg = std::make_unique<FakeMmioConfig>(bdf, std::move(view));
  cfg->Write(Config::kVendorId, vid);
  cfg->Write(Config::kDeviceId, did);
  return pci::Device::Create(this->zxdev(), std::move(cfg), &upstream(), bus().bdi(),
                             /*has_acpi=*/false, /*has_devicetree=*/false);
}

}  // namespace pci

static zx_status_t fake_pci_bus_driver_bind(void* ctx, zx_device_t* parent) {
  return pci::FakeBusDriver::Create(parent, kFakeBusDriverName, /*start_bus=*/0, /*end_bus=*/1,
                                    /*is_extended=*/false);
}

static const zx_driver_ops_t fake_pci_bus_driver_ops = []() {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = fake_pci_bus_driver_bind;
  return ops;
}();

ZIRCON_DRIVER(fake_pci_bus_driver, fake_pci_bus_driver_ops, "zircon", "0.1");
