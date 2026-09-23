// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DDK_INCLUDE_LIB_DDK_METADATA_H_
#define SRC_LIB_DDK_INCLUDE_LIB_DDK_METADATA_H_

#include <assert.h>
#include <lib/zbi-format/zbi.h>
#include <stdbool.h>
#include <stdint.h>

// This file contains metadata types for device_get_metadata()
//
// Note: if a metadata type is a FIDL type, then it is always
// serialized using the convention for FIDL data persistence, which
// adds wire format metadata in front of the encoded content.

// Platform board private data (for board driver)
// type: ???
#define DEVICE_METADATA_BOARD_PRIVATE 0x524F426D  // mBOR
static_assert(DEVICE_METADATA_BOARD_PRIVATE == ZBI_TYPE_DRV_BOARD_PRIVATE, "");

// type: display::PanelType (defined in //src/graphics/display/lib/
// device-protocol-display/include/lib/device-protocol/display-panel.h)
#define DEVICE_METADATA_DISPLAY_PANEL_TYPE 0x43505344  // DSPC

// Metadata types that have least significant byte set to lowercase 'd'
// signify private driver data.
// This allows creating metadata types to be defined local to a particular
// driver or driver protocol.
#define DEVICE_METADATA_PRIVATE 0x00000064
#define DEVICE_METADATA_PRIVATE_MASK 0x000000ff

static inline bool is_private_metadata(uint32_t type) {
  return ((type & DEVICE_METADATA_PRIVATE_MASK) == DEVICE_METADATA_PRIVATE);
}

#endif  // SRC_LIB_DDK_INCLUDE_LIB_DDK_METADATA_H_
