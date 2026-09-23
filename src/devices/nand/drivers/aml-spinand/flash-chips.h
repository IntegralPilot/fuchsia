// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_FLASH_CHIPS_H_
#define SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_FLASH_CHIPS_H_

#include <zircon/types.h>

#include <string_view>

#include "spi-op.h"

namespace amlspinand {

inline constexpr uint16_t kVendorToshiba = 0x98;
inline constexpr uint16_t kDeviceToshibaTC58CVG2S0HRAIJ = 0xed;

constexpr uint32_t kToshibaStatusEcc8Bitflips = (0x3 << 4);

struct NandMemory {
  uint32_t bits_per_cell;
  uint32_t pagesize;
  uint32_t oobsize;
  uint32_t pages_per_eraseblock;
  uint32_t eraseblocks_per_lun;
  uint32_t planes_per_lun;
  uint32_t luns_per_target;
  uint32_t ntargets;
};

struct OobRegion {
  uint32_t offset;
  uint32_t length;
};

struct EccReq {
  uint32_t strength;
  uint32_t step_size;
};

struct FlashChipInfo {
  std::string_view name;
  std::string_view model;
  uint16_t vendor_id;
  uint16_t device_id;
  uint32_t flags;
  uint32_t block_addr_shift;
  NandMemory mem_org;
  OobRegion oob_region;
  EccReq ecc_req;
  SpiOp read_op;
  SpiOp write_op;
};

inline const FlashChipInfo kFlashDevices[] = {
    {
        .name = "Toshiba",
        .model = "TC58CVG2S0HRAIJ 3.3V",
        .vendor_id = kVendorToshiba,
        .device_id = kDeviceToshibaTC58CVG2S0HRAIJ,
        .flags = 0,
        .block_addr_shift = 6,
        .mem_org =
            {
                .bits_per_cell = 1,
                .pagesize = 4096,
                .oobsize = 128,
                .pages_per_eraseblock = 64,
                .eraseblocks_per_lun = 2048,
                .planes_per_lun = 1,
                .luns_per_target = 1,
                .ntargets = 1,
            },
        .oob_region =
            {
                .offset = 2,  // Reserve 2 bytes for the BBM
                .length = 62,
            },
        .ecc_req =
            {
                .strength = 8,
                .step_size = 512,
            },
        .read_op = SpiOp(TransferCmd(OpCmd::kCmdReadBufQuad, 1), TransferAddr(2, 1, 0),
                         TransferPlaceholder(1, 1),
                         TransferData(4, SpiDataDir::kSpiDataIn, 0, nullptr, nullptr)),
        .write_op = SpiOp(TransferCmd(OpCmd::kCmdProgLoadQuad, 1), TransferAddr(2, 1, 0), {},
                          TransferData(4, SpiDataDir::kSpiDataOut, 0, nullptr, nullptr)),
    },
};

}  // namespace amlspinand

#endif  // SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_FLASH_CHIPS_H_
