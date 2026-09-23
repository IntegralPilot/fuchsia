// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_LIB_ZIRCON_BOOT_ZIRCON_RAMBOOT_H_
#define SRC_FIRMWARE_LIB_ZIRCON_BOOT_ZIRCON_RAMBOOT_H_

#include <lib/zircon_boot/zircon_boot.h>

// Tracks the location of the ZBI and vbmeta images in a RAM buffer.
typedef struct {
  const void* zbi;
  size_t zbi_size;

  // vbmeta may be NULL if only the ZBI was provided.
  const void* vbmeta;
  size_t vbmeta_size;
} UnpackedRamImage;

// Boot ops context for RAM-booting.
typedef struct {
  // The original ZirconBootOps for normal disk booting.
  ZirconBootOps* original_ops;
  // Images in RAM.
  UnpackedRamImage ram_image;
} RambootContext;

// Configures a ZirconBootOps to handle RAM-booting.
//
// @ops: the original boot ops provided by the user.
// @image: the image(s) in RAM.
// @size: the size of the contents in RAM.
// @ramboot_context: will be configured for RAM-booting.
// @ramboot_ops: will be configured for RAM-booting.
//
// Returns kBootResultOK on success.
ZirconBootResult SetupRambootOps(ZirconBootOps* ops, const void* image, size_t size,
                                 RambootContext* ramboot_context, ZirconBootOps* ramboot_ops);

#endif  // SRC_FIRMWARE_LIB_ZIRCON_BOOT_ZIRCON_RAMBOOT_H_
