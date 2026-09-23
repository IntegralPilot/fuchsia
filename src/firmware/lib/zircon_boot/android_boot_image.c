// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifdef ZIRCON_BOOT_CUSTOM_SYSDEPS_HEADER
#include <zircon_boot_sysdeps.h>
#else
#include <string.h>
#endif

#include <lib/zircon_boot/android_boot_image.h>

#include "utils.h"

bool zircon_boot_get_android_image_kernel_position(const zircon_boot_android_image_headers* header,
                                                   size_t* kernel_offset, size_t* kernel_size) {
  if (memcmp(header, ZIRCON_BOOT_ANDROID_IMAGE_MAGIC, ZIRCON_BOOT_ANDROID_IMAGE_MAGIC_SIZE)) {
    return false;
  }

  // Extract the kernel offset and size from the header.
  switch (header->v0.header_version) {
    case 0:
      *kernel_offset = header->v0.page_size;
      *kernel_size = header->v0.kernel_size;
      break;
    case 1:
      *kernel_offset = header->v1.page_size;
      *kernel_size = header->v1.kernel_size;
      break;
    case 2:
      *kernel_offset = header->v2.page_size;
      *kernel_size = header->v2.kernel_size;
      break;
    case 3:
      *kernel_offset = 4096;  // Version 3 hardcodes the page size at 4096.
      *kernel_size = header->v3.kernel_size;
      break;
    case 4:
      *kernel_offset = 4096;  // Version 4 hardcodes the page size at 4096.
      *kernel_size = header->v4.kernel_size;
      break;
    default:
      zircon_boot_dlog("Unsupported Android boot image version (%u)\n", header->v0.header_version);
      return false;
  }

  return true;
}

bool zircon_boot_get_android_image_kernel(const void* image, size_t size, const void** kernel,
                                          size_t* kernel_size) {
  if (size < sizeof(zircon_boot_android_image_headers) ||
      ((uintptr_t)image % ANDROID_IMAGE_HEADER_ALIGNMENT != 0)) {
    return false;
  }

  size_t kernel_offset = 0;
  if (!zircon_boot_get_android_image_kernel_position(
          (const zircon_boot_android_image_headers*)image, &kernel_offset, kernel_size)) {
    return false;
  }

  // Finally check that the provided image is actually big enough to hold the
  // header and the kernel.
  size_t required_size = kernel_offset + *kernel_size;
  if (required_size < *kernel_size) {
    zircon_boot_dlog("Kernel size overflow\n");
    return false;
  }
  if (size < required_size) {
    zircon_boot_dlog("Image is too small to contain the reported kernel\n");
    return false;
  }

  *kernel = (const uint8_t*)image + kernel_offset;

  return true;
}
