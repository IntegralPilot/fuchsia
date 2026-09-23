// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_I2C_GMBUS_I2C_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_I2C_GMBUS_I2C_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <threads.h>
#include <zircon/assert.h>

#include <array>

#include "src/graphics/display/drivers/intel-display/hardware-common.h"
#include "src/graphics/display/drivers/intel-display/i2c/gmbus-gpio.h"
#include "src/graphics/display/lib/edid/edid.h"

namespace intel_display {

class GMBusI2c {
 public:
  GMBusI2c(DdiId ddi_id, registers::Platform platform, fdf::MmioBuffer* mmio_space);

  // Checks for the presence of a display device by reading a byte of EDID data.
  //
  // Returns true iff a display is available on the pins connected by the
  // GMBus controller.
  bool ProbeDisplay();

  zx::result<fbl::Vector<uint8_t>> ReadExtendedEdid();

 private:
  // `index` must be non-negative and less than `edid::kMaxEdidBlockCount`.
  zx::result<> ReadEdidBlock(int index, std::span<uint8_t, edid::kBlockSize> edid_block);

  const std::optional<GMBusPinPair> gmbus_pin_pair_;
  const std::optional<GpioPort> gpio_port_;

  // The lock protects the registers this class writes to, not the whole
  // register io space.
  fdf::MmioBuffer* mmio_space_ __TA_GUARDED(lock_);
  mtx_t lock_;

  bool I2cFinish() __TA_REQUIRES(lock_);
  bool I2cWaitForHwReady() __TA_REQUIRES(lock_);
  bool I2cClearNack() __TA_REQUIRES(lock_);
  bool SetDdcSegment(uint8_t block_num) __TA_REQUIRES(lock_);
  bool GMBusRead(uint8_t addr, uint8_t* buf, uint8_t size) __TA_REQUIRES(lock_);
  bool GMBusWrite(uint8_t addr, const uint8_t* buf, uint8_t size) __TA_REQUIRES(lock_);
};

}  // namespace intel_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_INTEL_DISPLAY_I2C_GMBUS_I2C_H_
