// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8M_GPIO_H_
#define SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8M_GPIO_H_

#include <stdint.h>
#include <zircon/types.h>

#define IMX_GPIO_PIN(port, index) (((port - 1) * 32) + index)

namespace imx8m {

constexpr uint8_t kMaxGpiosPerPort = 32;
constexpr uint8_t kMaxGpioPorts = 5;
// an interrupt number for each 16 pins of 32 pin gpio controller
constexpr uint8_t kInterruptsPerPort = 2;
constexpr uint8_t kGpiosPerInterrupt = 16;

constexpr zx_off_t kGpioDataReg = 0x00;
constexpr zx_off_t kGpioDirReg = 0x04;
constexpr zx_off_t kGpioPadStatusReg = 0x08;
constexpr zx_off_t kGpioInterruptConfReg1 = 0x0c;
constexpr zx_off_t kGpioInterruptConfReg2 = 0x10;
constexpr zx_off_t kGpioInterruptMaskReg = 0x14;
constexpr zx_off_t kGpioInterruptStatusReg = 0x18;
constexpr zx_off_t kGpioEdgeSelectReg = 0x1c;

constexpr uint32_t kGpioInterruptConfBitCount = 0x2;
constexpr uint32_t kGpioInterruptLowLevel = 0x0;
constexpr uint32_t kGpioInterruptHighLevel = 0x1;
constexpr uint32_t kGpioInterruptRisingEdge = 0x2;
constexpr uint32_t kGpioInterruptFallingEdge = 0x3;

// All the pad/alt-function combination doesn't necessarily have
// input register to be configured so in that case input_reg_offset
// is set to 0 in metadata. gpio controller driver doesn't write input
// register if input_reg_offset is set to 0.
struct PinConfigEntry {
  uint32_t mux_reg_offset;
  uint32_t mux_val;
  uint32_t input_reg_offset;
  uint32_t input_val;
  uint32_t conf_reg_offset;
  uint32_t conf_val;
};

// Some of the gpio ports have less than kMaxGpiosPerPort
// pins so maintain supported pin count for each gpio port
// so that when GPIO API is called, we can validate pin
// number.
struct GpioPortInfo {
  uint32_t pin_count;
};

struct PinConfigMetadata {
  uint32_t pin_config_entry_count;
  PinConfigEntry pin_config_entry[kMaxGpioPorts * kMaxGpiosPerPort] = {};
  GpioPortInfo port_info[kMaxGpioPorts] = {};
};

}  // namespace imx8m

#endif  // SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8M_GPIO_H_
