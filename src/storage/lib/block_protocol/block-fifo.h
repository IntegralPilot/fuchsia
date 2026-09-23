// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_PROTOCOL_BLOCK_FIFO_H_
#define SRC_STORAGE_LIB_BLOCK_PROTOCOL_BLOCK_FIFO_H_

#include <fidl/fuchsia.storage.block/cpp/fidl.h>

#include "src/storage/lib/block_protocol/types.h"

// These constants are duplicated in the generated Banjo headers for fuchsia.hardware.block.driver.
#ifndef BLOCK_VMOID_INVALID

constexpr vmoid_t BLOCK_VMOID_INVALID = fuchsia_storage_block::kVmoidInvalid;

constexpr vmoid_t MAX_TXN_GROUP_COUNT = fuchsia_storage_block::kMaxTxnGroupCount;

constexpr uint32_t BLOCK_IO_FLAG_GROUP_ITEM =
    static_cast<uint32_t>(fuchsia_storage_block::BlockIoFlag::kGroupItem);
constexpr uint32_t BLOCK_IO_FLAG_GROUP_LAST =
    static_cast<uint32_t>(fuchsia_storage_block::BlockIoFlag::kGroupLast);
constexpr uint32_t BLOCK_IO_FLAG_FORCE_ACCESS =
    static_cast<uint32_t>(fuchsia_storage_block::BlockIoFlag::kForceAccess);
constexpr uint32_t BLOCK_IO_FLAG_PRE_BARRIER =
    static_cast<uint32_t>(fuchsia_storage_block::BlockIoFlag::kPreBarrier);
constexpr uint32_t BLOCK_IO_FLAG_DECOMPRESS_WITH_ZSTD =
    static_cast<uint32_t>(fuchsia_storage_block::BlockIoFlag::kDecompressWithZstd);
constexpr uint32_t BLOCK_IO_FLAG_INLINE_ENCRYPTION_ENABLED =
    static_cast<uint32_t>(fuchsia_storage_block::BlockIoFlag::kInlineEncryptionEnabled);

constexpr uint8_t BLOCK_OPCODE_TRIM =
    static_cast<uint8_t>(fuchsia_storage_block::BlockOpcode::kTrim);
constexpr uint8_t BLOCK_OPCODE_FLUSH =
    static_cast<uint8_t>(fuchsia_storage_block::BlockOpcode::kFlush);
constexpr uint8_t BLOCK_OPCODE_READ =
    static_cast<uint8_t>(fuchsia_storage_block::BlockOpcode::kRead);
constexpr uint8_t BLOCK_OPCODE_WRITE =
    static_cast<uint8_t>(fuchsia_storage_block::BlockOpcode::kWrite);
constexpr uint8_t BLOCK_OPCODE_CLOSE_VMO =
    static_cast<uint8_t>(fuchsia_storage_block::BlockOpcode::kCloseVmo);

#endif  // BLOCK_VMOID_INVALID
#endif  // SRC_STORAGE_LIB_BLOCK_PROTOCOL_BLOCK_FIFO_H_
