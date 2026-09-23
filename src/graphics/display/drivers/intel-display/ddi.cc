// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/intel-display/ddi.h"

#include <zircon/assert.h>

#include <span>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/pci-ids.h"

namespace intel_display {

std::span<const DdiId> GetDdiIds(uint16_t device_id) {
  if (is_skl(device_id)) {
    return DdiIds<registers::Platform::kSkylake>();
  }
  if (is_kbl(device_id)) {
    return DdiIds<registers::Platform::kKabyLake>();
  }
  if (is_tgl(device_id)) {
    return DdiIds<registers::Platform::kTigerLake>();
  }
  if (is_test_device(device_id)) {
    return DdiIds<registers::Platform::kTestDevice>();
  }
  ZX_ASSERT_MSG(false, "Device id (%04x) not supported", device_id);
}

}  // namespace intel_display
