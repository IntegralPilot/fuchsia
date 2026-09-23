// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_GIGABOOT_SRC_ZIRCON_H_
#define SRC_FIRMWARE_GIGABOOT_SRC_ZIRCON_H_
#include <stdint.h>

#include <efi/boot-services.h>
#include <efi/types.h>

efi_status generate_efi_memory_attributes_table_item(void* ramdisk, const size_t ramdisk_size,
                                                     efi_system_table* sys, const void* mmap,
                                                     size_t memory_map_size, size_t dsize);
#endif  // SRC_FIRMWARE_GIGABOOT_SRC_ZIRCON_H_
