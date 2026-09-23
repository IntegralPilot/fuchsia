// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Device independent functions to validate partition data and disk images.
// Tools to validate

#include "validation.h"

#include <lib/cksum.h>
#include <lib/zbi-format/kernel.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <span>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>

#include "device-partitioner.h"
#include "pave-logging.h"

namespace paver {

namespace {

// Magic header of ChromeOS kernel verification block.
constexpr std::string_view kChromeOsMagicHeader = "CHROMEOS";

// Magic for Android `boot_x` partition.
// Defined as `BOOT_MAGIC` in AOSP: system/tools/mkbootimg/include/bootimg/bootimg.h
constexpr std::string_view kAndroidBootMagic = "ANDROID!";

// Magic for Android `vendor_boot_x` partition.
// Defined as `VENDOR_BOOT_MAGIC` in AOSP: system/tools/mkbootimg/include/bootimg/bootimg.h
constexpr std::string_view kAndroidVendorBootMagic = "VNDRBOOT";

// Determine if the CRC of the given zbi_header_t is valid.
//
// We require that the "hdr" has "hdr->length" valid bytes after it.
bool ZbiHeaderCrcValid(const zbi_header_t* hdr) {
  // If we don't have the CRC32 flag set, ensure no crc32 value is given.
  if ((hdr->flags & ZBI_FLAGS_CRC32) == 0) {
    return (hdr->crc32 == ZBI_ITEM_NO_CRC32);
  }

  // Otherwise, calculate the CRC.
  return hdr->crc32 == crc32(0, reinterpret_cast<const uint8_t*>(hdr + 1), hdr->length);
}

bool CheckMagic(std::span<const uint8_t> data, std::string_view magic,
                const std::string& kernel_name) {
  if (data.size() < magic.size()) {
    ERROR("%s kernel payload too small.\n", kernel_name.c_str());
    return false;
  }
  if (memcmp(data.data(), magic.data(), magic.size()) != 0) {
    ERROR("%s kernel magic header invalid.\n", kernel_name.c_str());
    return false;
  }

  return true;
}

}  // namespace

bool ExtractZbiPayload(std::span<const uint8_t> data, const zbi_header_t** header,
                       std::span<const uint8_t>* payload) {
  // Validate data header.
  if (data.size() < sizeof(zbi_header_t)) {
    return false;
  }

  // Validate the header.
  const auto zbi_header = reinterpret_cast<const zbi_header_t*>(data.data());
  if (zbi_header->magic != ZBI_ITEM_MAGIC) {
    return false;
  }
  if ((zbi_header->flags & ZBI_FLAGS_VERSION) != ZBI_FLAGS_VERSION) {
    ERROR("ZBI header has invalid version.\n");
    return false;
  }

  // Ensure the data length is valid. We are okay with additional bytes
  // at the end of the data, but not having too few bytes available.
  if (zbi_header->length > data.size() - sizeof(zbi_header_t)) {
    ERROR("Header length length of %u byte(s) exceeds data available of %ld byte(s).\n",
          zbi_header->length, data.size() - sizeof(zbi_header_t));
    return false;
  }

  // Verify CRC.
  if (!ZbiHeaderCrcValid(zbi_header)) {
    ERROR("ZBI payload CRC invalid.\n");
    return false;
  }

  // All good.
  *header = zbi_header;
  *payload = data.subspan(sizeof(zbi_header_t), zbi_header->length);
  return true;
}

bool IsValidKernelZbi(Arch arch, std::span<const uint8_t> data) {
  // Get container header.
  const zbi_header_t* container_header;
  std::span<const uint8_t> container_data;
  if (!ExtractZbiPayload(data, &container_header, &container_data)) {
    ERROR("Kernel payload does not look like a ZBI container");
    return false;
  }

  // Ensure it is of the correct type.
  if (container_header->type != ZBI_TYPE_CONTAINER) {
    ERROR("ZBI container not a container type, or has invalid magic value.\n");
    return false;
  }
  if (container_header->extra != ZBI_CONTAINER_MAGIC) {
    ERROR("ZBI container has invalid magic value.\n");
    return false;
  }

  // Extract kernel.
  const zbi_header_t* kernel_header;
  std::span<const uint8_t> kernel_data;
  if (!ExtractZbiPayload(container_data, &kernel_header, &kernel_data)) {
    return false;
  }

  // Ensure it is of the correct type.
  const uint32_t expected_kernel_type =
      (arch == Arch::kX64) ? ZBI_TYPE_KERNEL_X64 : ZBI_TYPE_KERNEL_ARM64;
  if (kernel_header->type != expected_kernel_type) {
    ERROR("ZBI kernel payload has incorrect type or architecture. Expected %#08x, got %#08x.\n",
          expected_kernel_type, kernel_header->type);
    return false;
  }

  // Ensure payload contains enough data for the kernel header.
  if (kernel_header->length < sizeof(zbi_kernel_t)) {
    ERROR("ZBI kernel payload too small.\n");
    return false;
  }

  return true;
}

bool IsValidAndroidKernel(std::span<const uint8_t> data) {
  return CheckMagic(data, kAndroidBootMagic, "Android");
}

bool IsValidAndroidVendorKernel(std::span<const uint8_t> data) {
  return CheckMagic(data, kAndroidVendorBootMagic, "Android Vendor");
}

bool IsValidChromeOsKernel(std::span<const uint8_t> data) {
  return CheckMagic(data, kChromeOsMagicHeader, "ChromeOS");
}

}  // namespace paver
