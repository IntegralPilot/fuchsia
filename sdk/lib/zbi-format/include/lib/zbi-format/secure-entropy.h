// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/secure-entropy.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_SECURE_ENTROPY_H_
#define LIB_ZBI_FORMAT_SECURE_ENTROPY_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

// ZBI_TYPE_SECURE_ENTROPY item subtypes (for zbi_header_t.extra)
typedef uint32_t zbi_secure_entropy_t;

// Contents are used to seed the kernel's PRNG.
#define ZBI_SECURE_ENTROPY_GENERAL ((zbi_secure_entropy_t)(0u))

// Contents are used by early boot, before the kernel is fully
// operational.
#define ZBI_SECURE_ENTROPY_EARLY_BOOT ((zbi_secure_entropy_t)(1u))

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_SECURE_ENTROPY_H_
