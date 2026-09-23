// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8MM_I2C_H_
#define SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8MM_I2C_H_

#include <limits.h>
#include <stdint.h>
#include <zircon/types.h>

#include <fbl/algorithm.h>

namespace imx8mm {

constexpr uint32_t kI2c1Base = 0x30a20000;
constexpr uint32_t kI2c2Base = 0x30a30000;
constexpr uint32_t kI2c3Base = 0x30a40000;
constexpr uint32_t kI2c4Base = 0x30a50000;

inline uint32_t I2cSize() {
  return fbl::round_up<uint32_t, uint32_t>(0x10000, zx_system_get_page_size());
}

// 32 is ARM64 SPI (Shared Peripheral Interrupt) start offset
constexpr uint32_t kI2c1Irq = 35 + 32;
constexpr uint32_t kI2c2Irq = 36 + 32;
constexpr uint32_t kI2c3Irq = 37 + 32;
constexpr uint32_t kI2c4Irq = 38 + 32;

}  // namespace imx8mm

#endif  // SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8MM_I2C_H_
