// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/fusb302/fusb302-identity.h"

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <cstdint>
#include <utility>

#include "src/devices/power/drivers/fusb302/registers.h"

namespace fusb302 {

Fusb302Identity::Fusb302Identity(fidl::ClientEnd<fuchsia_hardware_i2c::Device>& i2c_channel,
                                 inspect::Node root_node)
    : i2c_(i2c_channel), root_node_(std::move(root_node)) {}

zx::result<> Fusb302Identity::ReadIdentity() {
  auto device_id = DeviceIdReg::ReadFrom(i2c_);

  std::string chip_version;
  chip_version.reserve(16);
  chip_version.append("FUSB302");
  chip_version.push_back(device_id.VersionCharacter());
  chip_version.append("_rev");
  chip_version.push_back(device_id.RevisionCharacter());

  fdf::info("Reporting {} - {}", chip_version.c_str(), device_id.ProductString());

  root_node_.RecordString("Product", device_id.ProductString());
  root_node_.RecordString("Version", chip_version);

  return zx::ok();
}

}  // namespace fusb302
