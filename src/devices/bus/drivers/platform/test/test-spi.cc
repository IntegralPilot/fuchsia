// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.bus/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.bus/cpp/fidl.h>
#include <fidl/fuchsia.hardware.spi.businfo/cpp/fidl.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>

#include "src/devices/lib/fidl-metadata/spi.h"
#include "test.h"

namespace board_test {
namespace fpbus = fuchsia_hardware_platform_bus;

namespace {
using spi_channel_t = fidl_metadata::spi::Channel;

static const spi_channel_t spi_channels[] = {{.cs = 0,
                                              // VID/PID/DID unused.
                                              .vid = 0,
                                              .pid = 0,
                                              .did = 0}};

}  // namespace

zx_status_t TestBoard::SpiInit() {
  fpbus::Node spi_dev;
  spi_dev.name() = "spi";
  spi_dev.vid() = PDEV_VID_TEST;
  spi_dev.pid() = PDEV_PID_PBUS_TEST;
  spi_dev.did() = PDEV_DID_TEST_SPI;

  std::vector<uint8_t> persisted_spi_bus_metadata;
  {
    zx::result result = fidl_metadata::spi::SpiChannelsToFidl(0, spi_channels);
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to convert spi channels to fidl: %s", result.status_string());
      return result.error_value();
    }
    persisted_spi_bus_metadata = std::move(result.value());
  }

  std::vector<fuchsia_hardware_platform_bus::Metadata> metadata{
      {{.id = fuchsia_hardware_spi_businfo::SpiBusMetadata::kSerializableName,
        .data = std::move(persisted_spi_bus_metadata)}}};

  spi_dev.metadata() = std::move(metadata);

  fdf::Arena arena('TSPI');
  fidl::Arena<> fidl_arena;
  auto result = pbus_.buffer(arena)->NodeAdd(fidl::ToWire(fidl_arena, spi_dev));
  if (!result.ok()) {
    zxlogf(ERROR, "%s: DeviceAdd Spi request failed: %s", __func__,
           result.FormatDescription().data());
    return result.status();
  }
  if (result->is_error()) {
    zxlogf(ERROR, "%s: DeviceAdd Spi failed: %s", __func__,
           zx_status_get_string(result->error_value()));
    return result->error_value();
  }

  return ZX_OK;
}

}  // namespace board_test
