// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_BCD_H_
#define SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_BCD_H_

#include <cstdint>

uint8_t to_bcd(uint8_t binary) {
  return static_cast<uint8_t>((((binary / 10) << 4) | (binary % 10)));
}

uint8_t from_bcd(uint8_t bcd) { return ((bcd >> 4) * 10) + (bcd & 0xf); }

#endif  // SRC_DEVICES_RTC_DRIVERS_NXP_PCF8563_BCD_H_
