// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8MM_USDHC_H_
#define SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8MM_USDHC_H_

#include <limits.h>

#include <fbl/algorithm.h>

namespace imx8mm {

constexpr uint32_t kUsdhc1Base = 0x30b40000;
constexpr uint32_t kUsdhc2Base = 0x30b50000;
constexpr uint32_t kUsdhc3Base = 0x30b60000;

inline uint32_t UsdhcSize() {
  return fbl::round_up<uint32_t, uint32_t>(0x10000, zx_system_get_page_size());
}

constexpr uint32_t kUsdhc1Irq = 22 + 32;
constexpr uint32_t kUsdhc2Irq = 23 + 32;
constexpr uint32_t kUsdhc3Irq = 24 + 32;

}  // namespace imx8mm

#endif  // SRC_DEVICES_LIB_NXP_INCLUDE_SOC_IMX8MM_USDHC_H_
