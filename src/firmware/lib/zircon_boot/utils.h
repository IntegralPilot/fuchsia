// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_ZIRCON_BOOT_UTILS_H_
#define SRC_FIRMWARE_LIB_ZIRCON_BOOT_UTILS_H_

#include <lib/abr/abr.h>

#define METHOD_CALL_ARGS(...) , ##__VA_ARGS__
#define ZIRCON_BOOT_OPS_CALL(zircon_boot_ops, method, ...)                  \
  ({                                                                        \
    assert(zircon_boot_ops->method);                                        \
    zircon_boot_ops->method(zircon_boot_ops METHOD_CALL_ARGS(__VA_ARGS__)); \
  })

// Set to 1 to enable debug logging - requires printf().
#define ZIRCON_BOOT_DEBUG_LOGGING 1
#if ZIRCON_BOOT_DEBUG_LOGGING
#define zircon_boot_dlog(...) printf("zircon_boot: " __VA_ARGS__)
#else
#define zircon_boot_dlog(...)
#endif

#ifdef __cplusplus
extern "C" {
#endif

AbrSlotIndex AbrPeekBootSlot(const AbrOps* abr_ops);

// Returns the size of the ZBI container.
//
// This function checks the ZBI header, looks at a few fields to see if it
// looks correct, and if so returns the full ZBI container size. It only
// looks at the header so the full ZBI does not yet need to exist.
//
// @zbi: buffer containing a ZBI container header; does not need to be aligned.
// @max_size: maximum ZBI size, or 0 to allow any size.
//
// Returns the ZBI size, or 0 if the buffer doesn't look like a ZBI or claims
// to exceed the maximum possible size.
size_t ZbiCheckSize(const void* zbi, size_t max_size);

#ifdef __cplusplus
}
#endif

#endif  // SRC_FIRMWARE_LIB_ZIRCON_BOOT_UTILS_H_
