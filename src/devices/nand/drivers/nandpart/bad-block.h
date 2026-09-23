// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_NAND_DRIVERS_NANDPART_BAD_BLOCK_H_
#define SRC_DEVICES_NAND_DRIVERS_NANDPART_BAD_BLOCK_H_

#include <fidl/fuchsia.hardware.nand/cpp/fidl.h>
#include <fuchsia/hardware/nand/c/banjo.h>
#include <fuchsia/hardware/nand/cpp/banjo.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <utility>

namespace nand {

// Interface for interacting with device bad blocks.
class BadBlock {
 public:
  struct Config {
    // Bad block configuration for device.
    fuchsia_hardware_nand::BadBlockConfig bad_block_config;
    // Parent device NAND protocol.
    nand_protocol_t nand_proto;
  };

  static zx::result<std::shared_ptr<BadBlock>> Create(Config config);

  virtual ~BadBlock() = default;

  // Returns a list of bad blocks between [first_block, last_block).
  virtual zx::result<std::vector<uint32_t>> GetBadBlockList(uint32_t first_block,
                                                            uint32_t last_block) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Marks a block bad and updates underlying storage.
  virtual zx_status_t MarkBlockBad(uint32_t block) { return ZX_ERR_NOT_SUPPORTED; }

 protected:
  BadBlock(zx::vmo data_vmo, zx::vmo oob_vmo, std::vector<uint8_t> nand_op)
      : data_vmo_(std::move(data_vmo)),
        oob_vmo_(std::move(oob_vmo)),
        nand_op_(std::move(nand_op)) {}

  // Ensures serialized access.
  std::mutex lock_;

  // VMO with data buffer. Size is dependent on bad block implementation.
  zx::vmo data_vmo_ TA_GUARDED(lock_);
  // VMO with oob buffer. Size is dependent on bad block implementation.
  zx::vmo oob_vmo_ TA_GUARDED(lock_);
  // Operation buffer of size parent_op_size.
  std::vector<uint8_t> nand_op_ TA_GUARDED(lock_);
};

}  // namespace nand

#endif  // SRC_DEVICES_NAND_DRIVERS_NANDPART_BAD_BLOCK_H_
