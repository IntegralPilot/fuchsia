// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_REQUEST_DESCRIPTOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_REQUEST_DESCRIPTOR_H_

#include <hwreg/bitfields.h>

namespace ufs {

constexpr uint8_t kCommandTypeUfsStorage = 0x1;

// UFSHCI Specification Version 3.0, section 6.1.1 "UTP Transfer Request Descriptor".
enum DataDirection {
  kNone = 0,
  kHostToDevice,
  kDeviceToHost,
};

enum OverallCommandStatus {
  kSuccess = 0x0,
  kInvalidCommandTableAttributes = 0x01,
  kInvalidPrdtAttributes = 0x02,
  kMismatchDataBufferSize = 0x03,
  kMismatchResponseUpiuSize = 0x04,
  kCommunicationFailureWithinUicLayers = 0x05,
  kAborted = 0x06,
  kHostControllerFatalError = 0x07,
  kDeviceFatalError = 0x08,
  kInvalidCryptoConfiguration = 0x09,
  kGeneralCryptoError = 0x0a,
  kInvalid = 0xf,
};

// UFSHCI Specification Version 3.0, section 6.1.1 "UTP Transfer Request Descriptor".
struct TransferRequestDescriptor {
  uint32_t dwords[8];

  // dword 0
  DEF_SUBFIELD(dwords[0], 31, 28, command_type);
  DEF_SUBBIT(dwords[0], 27, ru);
  DEF_SUBFIELD(dwords[0], 26, 25, data_direction);
  DEF_SUBBIT(dwords[0], 24, interrupt);
  DEF_SUBBIT(dwords[0], 23, ce);
  DEF_SUBFIELD(dwords[0], 7, 0, cci);
  // dword 1
  DEF_SUBFIELD(dwords[1], 31, 0, data_unit_number_lower);
  // dword 2
  DEF_ENUM_SUBFIELD(dwords[2], OverallCommandStatus, 7, 0, overall_command_status);
  // dword 3
  DEF_SUBFIELD(dwords[3], 31, 0, data_unit_number_upper);
  // dword 4
  DEF_SUBFIELD(dwords[4], 31, 0, utp_command_descriptor_base_address);
  // dword 5
  DEF_SUBFIELD(dwords[5], 31, 0, utp_command_descriptor_base_address_upper);
  // dword 6
  DEF_SUBFIELD(dwords[6], 31, 16, response_upiu_offset);
  DEF_SUBFIELD(dwords[6], 15, 0, response_upiu_length);
  // dword 7
  DEF_SUBFIELD(dwords[7], 31, 16, prdt_offset);
  DEF_SUBFIELD(dwords[7], 15, 0, prdt_length);  // The number of entries in the PRDT.

} __PACKED;
static_assert(sizeof(TransferRequestDescriptor) == 32,
              "TransferRequestDescriptor struct must be 32 bytes");

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_REQUEST_DESCRIPTOR_H_
