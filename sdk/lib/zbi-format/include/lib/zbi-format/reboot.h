// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/reboot.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_REBOOT_H_
#define LIB_ZBI_FORMAT_REBOOT_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

// ZBI_TYPE_HW_REBOOT_REASON payload.
typedef uint32_t zbi_hw_reboot_reason_t;

#define ZBI_HW_REBOOT_REASON_UNDEFINED ((zbi_hw_reboot_reason_t)(0u))
#define ZBI_HW_REBOOT_REASON_COLD ((zbi_hw_reboot_reason_t)(1u))
#define ZBI_HW_REBOOT_REASON_WARM ((zbi_hw_reboot_reason_t)(2u))
#define ZBI_HW_REBOOT_REASON_BROWNOUT ((zbi_hw_reboot_reason_t)(3u))
#define ZBI_HW_REBOOT_REASON_WATCHDOG ((zbi_hw_reboot_reason_t)(4u))
#define ZBI_HW_REBOOT_REASON_USER_HARD_RESET ((zbi_hw_reboot_reason_t)(5u))

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_REBOOT_H_
