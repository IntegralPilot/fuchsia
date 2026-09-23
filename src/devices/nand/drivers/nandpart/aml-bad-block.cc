// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-bad-block.h"

#include <fuchsia/hardware/nand/c/banjo.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/sync/completion.h>
#include <stdlib.h>

#include <fbl/algorithm.h>

namespace nand {

namespace {

constexpr uint32_t kBadBlockTableMagic = 0x7462626E;  // "nbbt"

struct BlockOperationContext {
  sync_completion_t* completion_event;
  zx_status_t status;
};

void CompletionCallback(void* cookie, zx_status_t status, nand_operation_t* op) {
  auto* ctx = static_cast<BlockOperationContext*>(cookie);

  fdf::debug("Completion status: {}", zx_status_get_string(status));
  ctx->status = status;
  sync_completion_signal(ctx->completion_event);
}

}  // namespace

zx::result<std::shared_ptr<AmlBadBlock>> AmlBadBlock::Create(Config config) {
  // Query parent to get its nand_info_t and size for nand_operation_t.
  nand_info_t nand_info;
  size_t parent_op_size;
  config.nand_proto.ops->query(config.nand_proto.ctx, &nand_info, &parent_op_size);

  if (nand_info.page_size == 0) {
    fdf::error("Page size cannot be zero");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (nand_info.num_blocks == 0) {
    fdf::error("Number of blocks cannot be zero");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::vector<uint8_t> nand_op(parent_op_size, 0);

  // Allocate VMOs.
  const uint32_t table_len = fbl::round_up(nand_info.num_blocks, nand_info.page_size);
  zx::vmo data_vmo;
  zx_status_t status = zx::vmo::create(table_len, 0, &data_vmo);
  if (status != ZX_OK) {
    fdf::error("Failed to create VMO for bad block table: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  const uint32_t bbt_page_count = table_len / nand_info.page_size;
  zx::vmo oob_vmo;
  status = zx::vmo::create(sizeof(OobMetadata) * bbt_page_count, 0, &oob_vmo);
  if (status != ZX_OK) {
    fdf::error("Failed to create VMO for oob metadata: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  // Map them.
  constexpr uint32_t kPermissions = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
  uintptr_t vaddr_table;
  status = zx::vmar::root_self()->map(kPermissions, 0, data_vmo, 0, table_len, &vaddr_table);
  if (status != ZX_OK) {
    fdf::error("Failed to map VMO for bad block table: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  uintptr_t vaddr_oob;
  status = zx::vmar::root_self()->map(kPermissions, 0, oob_vmo, 0,
                                      sizeof(OobMetadata) * bbt_page_count, &vaddr_oob);
  if (status != ZX_OK) {
    fdf::error("Failed to map VMO for oob metadata: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  std::span<BlockStatus> bad_block_table(reinterpret_cast<BlockStatus*>(vaddr_table), table_len);
  std::span<OobMetadata> oob_(reinterpret_cast<OobMetadata*>(vaddr_oob), bbt_page_count);
  auto bad_block =
      std::make_unique<AmlBadBlock>(std::move(data_vmo), std::move(oob_vmo), std::move(nand_op),
                                    config, nand_info, bad_block_table, oob_);
  return zx::ok(std::move(bad_block));
}

zx_status_t AmlBadBlock::EraseBlock(uint32_t block) {
  sync_completion_t completion;
  BlockOperationContext op_ctx = {.completion_event = &completion, .status = ZX_ERR_INTERNAL};
  auto* nand_op = reinterpret_cast<nand_operation_t*>(nand_op_.data());
  nand_op->erase.command = NAND_OP_ERASE;
  nand_op->erase.first_block = block;
  nand_op->erase.num_blocks = 1;
  nand_.Queue(nand_op, CompletionCallback, &op_ctx);

  // Wait on completion.
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
  return op_ctx.status;
}

zx_status_t AmlBadBlock::GetNewBlock() {
  for (;;) {
    // Find a block with the least number of PE cycles.
    uint16_t least_pe_cycles = UINT16_MAX;
    std::optional<size_t> index;
    for (size_t i = 0; i < block_list_.size(); i++) {
      const BlockListEntry& block_entry = block_list_[i];
      if (block_entry.valid &&
          (!block_entry_index_.has_value() || i != block_entry_index_.value()) &&
          block_entry.program_erase_cycles < least_pe_cycles) {
        least_pe_cycles = block_entry.program_erase_cycles;
        index = i;
      }
    }
    if (!index.has_value()) {
      fdf::error("Unable to find a valid block to store BBT into");
      return ZX_ERR_NOT_FOUND;
    }
    BlockListEntry& block_entry = block_list_[index.value()];

    // Make sure we aren't trying to write to a bad block.
    const uint32_t block = block_entry.block;
    if (bad_block_table_[block] != kNandBlockGood) {
      // Try again.
      block_entry.valid = false;
      continue;
    }

    // Erase the block before using it.
    const zx_status_t status = EraseBlock(block);
    if (status != ZX_OK) {
      fdf::error("Failed to erase block {}, marking bad", block);
      // Mark the block as bad and try again.
      bad_block_table_[block] = kNandBlockBad;
      block_entry.valid = false;
      continue;
    }

    fdf::info("Moving BBT to block {}", block);
    block_entry_index_.emplace(index.value());
    block_entry.program_erase_cycles++;
    page_ = 0;
    return ZX_OK;
  }
}

zx_status_t AmlBadBlock::WritePages(uint32_t nand_page, uint32_t num_pages) {
  sync_completion_t completion;
  BlockOperationContext op_ctx = {.completion_event = &completion, .status = ZX_ERR_INTERNAL};

  auto* nand_op = reinterpret_cast<nand_operation_t*>(nand_op_.data());
  nand_op->rw.command = NAND_OP_WRITE;
  nand_op->rw.data_vmo = data_vmo_.get();
  nand_op->rw.oob_vmo = oob_vmo_.get();
  nand_op->rw.length = num_pages;
  nand_op->rw.offset_nand = nand_page;
  nand_op->rw.offset_data_vmo = 0;
  nand_op->rw.offset_oob_vmo = 0;
  nand_.Queue(nand_op, CompletionCallback, &op_ctx);

  // Wait on completion.
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
  return op_ctx.status;
}

zx_status_t AmlBadBlock::WriteBadBlockTable(bool use_new_block) {
  const uint32_t bbt_page_count = BbtPageCount();

  for (;;) {
    {
      if (!block_entry_index_.has_value()) {
        fdf::error("Missing block entry");
        return ZX_ERR_BAD_STATE;
      }
      const BlockListEntry& block_entry = block_list_[block_entry_index_.value()];
      if (use_new_block || bad_block_table_[block_entry.block] != kNandBlockGood ||
          page_ + bbt_page_count >= nand_info_.pages_per_block) {
        // Current BBT is in a bad block, or it is full, so we must find a new one.
        use_new_block = false;
        fdf::info("Finding a new block to store BBT into");
        const zx_status_t status = GetNewBlock();
        if (status != ZX_OK) {
          return status;
        }
      }
    }

    {
      if (!block_entry_index_.has_value()) {
        fdf::error("Missing block entry");
        return ZX_ERR_BAD_STATE;
      }
      const BlockListEntry& block_entry = block_list_[block_entry_index_.value()];
      // Perform write.
      for (size_t i = 0; i < bbt_page_count; ++i) {
        OobMetadata& oob = oob_[i];
        oob.magic = kBadBlockTableMagic;
        oob.program_erase_cycles = block_entry.program_erase_cycles;
        oob.generation = generation_;
      }

      const uint32_t block = block_entry.block;
      const uint32_t nand_page = (block * nand_info_.pages_per_block) + page_;
      const zx_status_t status = WritePages(nand_page, bbt_page_count);
      if (status != ZX_OK) {
        fdf::error("BBT write failed. Marking {} bad and trying again", block);
        bad_block_table_[block] = kNandBlockBad;
        continue;
      }
      fdf::debug("BBT write to block {} pages [{}, {}) successful", block, page_,
                 page_ + bbt_page_count);
      break;
    }
  }

  page_ += bbt_page_count;
  generation_++;
  return ZX_OK;
}

zx_status_t AmlBadBlock::ReadPages(uint32_t nand_page, uint32_t num_pages) {
  sync_completion_t completion;
  BlockOperationContext op_ctx = {.completion_event = &completion, .status = ZX_ERR_INTERNAL};
  auto* nand_op = reinterpret_cast<nand_operation_t*>(nand_op_.data());
  nand_op->rw.command = NAND_OP_READ;
  nand_op->rw.data_vmo = data_vmo_.get();
  nand_op->rw.oob_vmo = oob_vmo_.get();
  nand_op->rw.length = num_pages;
  nand_op->rw.offset_nand = nand_page;
  nand_op->rw.offset_data_vmo = 0;
  nand_op->rw.offset_oob_vmo = 0;
  nand_.Queue(nand_op, CompletionCallback, &op_ctx);

  // Wait on completion.
  sync_completion_wait(&completion, ZX_TIME_INFINITE);
  return op_ctx.status;
}

zx_status_t AmlBadBlock::FindBadBlockTable() {
  fdf::debug("Finding bad block table");

  if (sizeof(OobMetadata) > nand_info_.oob_size) {
    fdf::error("OOB is too small. Need {}, found {}", sizeof(OobMetadata), nand_info_.oob_size);
    return ZX_ERR_NOT_SUPPORTED;
  }

  fdf::debug("Starting in block {}. Ending in block {}.", config_.table_start_block(),
             config_.table_end_block());

  const uint32_t blocks = config_.table_end_block() - config_.table_start_block();
  if (blocks == 0 || blocks > block_list_.size()) {
    // Driver assumption that no more than |kBlockListMax| blocks will be dedicated for BBT use.
    fdf::error("Unsupported number of blocks used for BBT.");
    return ZX_ERR_NOT_SUPPORTED;
  }

  // First find the block the BBT lives in.
  const uint32_t bbt_page_count = BbtPageCount();

  size_t valid_blocks = 0;
  block_entry_index_.reset();
  uint32_t block = config_.table_start_block();
  for (; block <= config_.table_end_block(); block++) {
    //  Attempt to read up to 6 entries to see if block is valid.
    uint32_t nand_page = block * nand_info_.pages_per_block;
    zx_status_t status = ZX_ERR_INTERNAL;
    for (uint32_t i = 0; i < 6 && status != ZX_OK; i++, nand_page += bbt_page_count) {
      status = ReadPages(nand_page, 1);
    }
    if (status != ZX_OK) {
      // This block is untrustworthy. Do not add it to the block list.
      // TODO(surajmalhotra): Should we somehow mark this block as bad or
      // try erasing it?
      fdf::error("Unable to read any pages in block {}", block);
      continue;
    }

    fdf::debug("Successfully read block {}.", block);

    block_list_[valid_blocks].block = block;
    block_list_[valid_blocks].valid = true;

    // If block has valid BBT entries, see if it has the latest entries.
    if (oob_[0].magic == kBadBlockTableMagic) {
      if (oob_[0].generation >= generation_) {
        fdf::debug("Block {} has valid BBT entries!", block);
        block_entry_index_.emplace(valid_blocks);
        generation_ = oob_[0].generation;
      }
      block_list_[valid_blocks].program_erase_cycles = oob_[0].program_erase_cycles;
    } else if (oob_[0].magic == 0xFFFFFFFF) {
      // Page is erased.
      block_list_[valid_blocks].program_erase_cycles = 0;
    } else {
      fdf::error("Block {} is neither erased, nor contains a valid entry!", block);
      block_list_[valid_blocks].program_erase_cycles = oob_[0].program_erase_cycles;
    }

    valid_blocks++;
  }

  if (!block_entry_index_.has_value()) {
    fdf::error("No valid BBT entries found!");
    // TODO(surajmalhotra): Initialize the BBT by reading the factory bad
    // blocks.
    return ZX_ERR_INTERNAL;
  }

  for (size_t idx = valid_blocks - 1; idx < block_list_.size(); idx++) {
    block_list_[idx].valid = false;
  }

  const BlockListEntry& block_entry = block_list_[block_entry_index_.value()];
  fdf::debug("Finding last BBT in block {}", block_entry.block);

  // Next find the last valid BBT entry in block.
  bool found_one = false;
  bool latest_entry_bad = true;
  uint32_t page = 0;
  bool break_loop = false;
  for (; page + bbt_page_count <= nand_info_.pages_per_block; page += bbt_page_count) {
    zx_status_t status = ZX_OK;
    // Check that all pages in current bbt_page_count are valid.
    fdf::debug("Reading pages [{}, {})", page, page + bbt_page_count);
    const uint32_t nand_page = (block_entry.block * nand_info_.pages_per_block) + page;
    status = ReadPages(nand_page, bbt_page_count);
    if (status != ZX_OK) {
      // It's fine for entries to be unreadable as long as future ones are
      // readable.
      fdf::debug("Unable to read page {}", page);
      latest_entry_bad = true;
      continue;
    }
    for (size_t i = 0; i < bbt_page_count; i++) {
      if (oob_[i].magic != kBadBlockTableMagic) {
        // Last BBT entry in table was found, so quit looking at future entries.
        fdf::debug("Page {} does not contain valid BBT entry", page + i);
        break_loop = true;
        break;
      }
    }
    if (break_loop) {
      break;
    }
    // Store latest complete BBT.
    fdf::debug("BBT entry in pages ({}, {}] is valid", page, page + bbt_page_count);
    latest_entry_bad = false;
    found_one = true;
    page_ = page;
    generation_ = static_cast<uint16_t>(oob_[0].generation + 1);
  }

  if (!found_one) {
    fdf::error("Unable to find a valid copy of the bad block table");
    return ZX_ERR_NOT_FOUND;
  }

  if (page + bbt_page_count <= nand_info_.pages_per_block || latest_entry_bad) {
    // Last iteration failed to read valid copy of BBT (that's how loop exited),
    // so we need to reread the BBT.
    const uint32_t nand_page = (block_entry.block * nand_info_.pages_per_block) + page_;
    const zx_status_t status = ReadPages(nand_page, bbt_page_count);
    if (status != ZX_OK) {
      fdf::error("Unable to re-read latest copy of bad block table");
      return status;
    }
    for (size_t i = 0; i < bbt_page_count; i++) {
      if (oob_[i].magic != kBadBlockTableMagic) {
        fdf::error("Latest copy of bad block table no longer valid?");
        return ZX_ERR_INTERNAL;
      }
    }

    if (latest_entry_bad) {
      fdf::error("Latest entry in block {} is invalid. Moving bad block file.", block_entry.block);
      constexpr bool kUseNewBlock = true;
      const zx_status_t status = WriteBadBlockTable(kUseNewBlock);
      if (status != ZX_OK) {
        return status;
      }
    } else {
      // Page needs to point to next available slot.
      fdf::info("Latest BBT entry found in pages [{}, {})", page_, page + bbt_page_count);
      page_ += bbt_page_count;
    }
  }

  table_valid_ = true;
  return ZX_OK;
}

zx::result<std::vector<uint32_t>> AmlBadBlock::GetBadBlockList(uint32_t first_block,
                                                               uint32_t last_block) {
  // Account for an off-by-one error in the bootloader.
  last_block++;

  const std::lock_guard lock(lock_);
  if (!table_valid_) {
    const zx_status_t status = FindBadBlockTable();
    if (status != ZX_OK) {
      return zx::error(status);
    }
  }

  if (first_block >= nand_info_.num_blocks || last_block > nand_info_.num_blocks) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Scan BBT for bad block list.
  size_t bad_block_count = 0;
  for (uint32_t block = first_block; block < last_block; block++) {
    if (bad_block_table_[block] != kNandBlockGood) {
      bad_block_count += 1;
    }
  }

  // Early return if no bad blocks found.
  if (bad_block_count == 0) {
    return zx::ok(std::vector<uint32_t>());
  }

  // Copy list.
  std::vector<uint32_t> bad_blocks;
  bad_blocks.reserve(bad_block_count);
  for (uint32_t block = first_block; block < last_block; block++) {
    if (bad_block_table_[block] != kNandBlockGood) {
      bad_blocks.emplace_back(block);
    }
  }

  return zx::ok(std::move(bad_blocks));
}

zx_status_t AmlBadBlock::MarkBlockBad(uint32_t block) {
  const std::lock_guard lock(lock_);
  if (!table_valid_) {
    const zx_status_t status = FindBadBlockTable();
    if (status != ZX_OK) {
      return status;
    }
  }

  if (block > nand_info_.num_blocks) {
    return ZX_ERR_OUT_OF_RANGE;
  }

  // Early return if block is already marked bad.
  if (bad_block_table_[block] != kNandBlockGood) {
    return ZX_OK;
  }
  bad_block_table_[block] = kNandBlockBad;

  constexpr bool kNoUseNewBlock = false;
  return WriteBadBlockTable(kNoUseNewBlock);
}

uint32_t AmlBadBlock::BbtPageCount() const {
  ZX_DEBUG_ASSERT(bad_block_table_.size() % nand_info_.page_size == 0);
  return static_cast<uint32_t>(bad_block_table_.size()) / nand_info_.page_size;
}

}  // namespace nand
