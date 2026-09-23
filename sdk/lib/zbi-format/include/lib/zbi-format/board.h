// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// DO NOT EDIT. Generated from FIDL library
//   zbi (//sdk/fidl/zbi/board.fidl)
// by zither, a Fuchsia platform tool.

#ifndef LIB_ZBI_FORMAT_BOARD_H_
#define LIB_ZBI_FORMAT_BOARD_H_

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

#define ZBI_BOARD_NAME_LEN ((uint64_t)(32u))

// ZBI_TYPE_PLATFORM_ID payload.
typedef struct {
  uint32_t vid;
  uint32_t pid;
  char board_name[32];
} zbi_platform_id_t;

// ZBI_TYPE_DRV_BOARD_INFO payload.
typedef struct {
  uint32_t revision;
} zbi_board_info_t;

#if defined(__cplusplus)
}
#endif

#endif  // LIB_ZBI_FORMAT_BOARD_H_
