// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_SCSI_COMMANDS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_SCSI_COMMANDS_H_

#include <endian.h>
#include <lib/scsi/controller.h>
#include <lib/sync/completion.h>

#include <fbl/intrusive_double_list.h>

#include "upiu_transactions.h"

namespace ufs {

class ScsiCommandUpiu;

// UFS Specification Version 3.1, section 11.3 "Universal Flash Storage SCSI Commands".
class ScsiCommandUpiu : public CommandUpiu {
 protected:
  struct ScsiCommonCDB {
    scsi::Opcode opcode;
  } __PACKED;

 public:
  explicit ScsiCommandUpiu(const uint8_t *cdb, uint8_t cdb_length, DataDirection data_direction,
                           uint32_t transfer_bytes = 0)
      : CommandUpiu(UpiuCommandSetType::kScsi, data_direction), transfer_bytes_(transfer_bytes) {
    constexpr uint32_t kMaxCdbSize = sizeof(GetData<CommandUpiuData>()->cdb);
    ZX_DEBUG_ASSERT(cdb_length <= kMaxCdbSize);
    CustomMemSet(GetData<CommandUpiuData>()->cdb, 0, kMaxCdbSize);
    CustomMemCpy(GetData<CommandUpiuData>()->cdb, cdb, cdb_length);
  }

  explicit ScsiCommandUpiu(const CommandUpiuData &data) : CommandUpiu(data) {}
  ~ScsiCommandUpiu() override = default;

  scsi::Opcode GetOpcode() const { return scsi_cdb_->opcode; }

  // Get the byte size of the PRDT data buffer to send or receive. Returns 0 if there is no buffer
  // to transfer.
  uint32_t GetTransferBytes() const { return transfer_bytes_; }

 private:
  ScsiCommonCDB *scsi_cdb_ = reinterpret_cast<ScsiCommonCDB *>(GetData<CommandUpiuData>()->cdb);
  uint32_t transfer_bytes_ = 0;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_SCSI_COMMANDS_H_
