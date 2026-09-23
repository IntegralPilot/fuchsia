// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_COMMAND_DESCRIPTOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_COMMAND_DESCRIPTOR_H_

#include <zircon/syscalls.h>

#include <hwreg/bitfields.h>

namespace ufs {

// According to the UFSHCI specification, the size of the UTP command descriptor is as follows. The
// size of the transfer request is not limited, a transfer response can be as long as 65535 *
// dwords, and a PRDT can be as long as 65565 * PRDT entry size(16 bytes).
// However, for ease of use, this UFS Driver imposes the following limits. The size of the transfer
// request and the transfer response is 1024 bytes or less. The PRDT region limits the number of
// scatter gathers to 256, using a total of 4096 bytes. Therefore, only 8KB size is allocated for
// the UTP command descriptor.
constexpr size_t kUtpCommandDescriptorSize = 8192;

constexpr size_t kMaxUtpTransferRequestSize = 512;
constexpr size_t kMaxUtpTransferResponseSize = 512;

// To reduce the size of the UTP Command Descriptor(8KB), we must use only 256 PRDT entries.
constexpr uint32_t kMaxPrdtEntryCount = 256;
// Set the PRDT data length to 4KB in order to scatter gather per page.
const uint32_t kPrdtEntryDataLength = zx_system_get_page_size();
// The UFS specification allows for up to 16GB, but we limits the number of PRDT entries to 256, so
// we can use up to 1MiB.
const uint32_t kMaxPrdtDataLength = kMaxPrdtEntryCount * kPrdtEntryDataLength;  // 1MiB

// UFSHCI Specification Version 3.0, section 6.1.2 "UTP Command Descriptor".
struct PhysicalRegionDescriptionTableEntry {
  uint32_t dwords[4];

  // dword 0
  DEF_SUBFIELD(dwords[0], 31, 0, data_base_address);
  // dword 1
  DEF_SUBFIELD(dwords[1], 31, 0, data_base_address_upper);
  // dword 3
  DEF_SUBFIELD(dwords[3], 17, 0, data_byte_count);  // Maximum byte count is 256KB
} __PACKED;
static_assert(sizeof(PhysicalRegionDescriptionTableEntry) == 16,
              "PhysicalRegionDescriptionTableEntry struct must be 16 bytes");
static_assert(
    (kMaxUtpTransferRequestSize + kMaxUtpTransferResponseSize +
     sizeof(PhysicalRegionDescriptionTableEntry) * kMaxPrdtEntryCount) <= kUtpCommandDescriptorSize,
    "The sum of the sizes of the Request, Response, and PRDT must be kUtpCommandDescriptorSize or "
    "less");

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_COMMAND_DESCRIPTOR_H_
