// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BOARD_DRIVERS_X86_INCLUDE_POWER_H_
#define SRC_DEVICES_BOARD_DRIVERS_X86_INCLUDE_POWER_H_

#include <lib/ddk/driver.h>
#include <zircon/types.h>

void poweroff(zx_device_t* device);
zx_status_t suspend_to_ram(zx_device_t* device);

#endif  // SRC_DEVICES_BOARD_DRIVERS_X86_INCLUDE_POWER_H_
