// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/memory_dump.h"

#include <string.h>

#include "src/developer/debug/zxdb/common/checked_math.h"
#include "src/lib/unwinder/error.h"

namespace zxdb {

MemoryDump::MemoryDump() {}
MemoryDump::MemoryDump(std::vector<debug_ipc::MemoryBlock>&& blocks) : blocks_(std::move(blocks)) {}
MemoryDump::~MemoryDump() = default;

bool MemoryDump::AllValid() const {
  if (blocks_.empty())
    return false;

  for (const auto& block : blocks_) {
    if (!block.valid)
      return false;
  }
  return true;
}

unwinder::Error MemoryDump::ReadBytes(uint64_t addr, uint64_t size, void* dest) {
  return ErrToUnwinderError(Read(addr, size, dest));
}

bool MemoryDump::GetByte(uint64_t address, uint8_t* byte) const {
  *byte = 0;

  return Read(address, 1, byte).ok();
}

Err MemoryDump::Read(uint64_t address, size_t size, void* dest) const {
  // Address math needs to be careful to avoid overflow.
  if (blocks_.empty() || address < blocks_.front().address ||
      address > blocks_.back().address + (blocks_.back().size - 1)) {
    return Err("Address out of bounds");
  }

  uint64_t read_end = 0;
  // Subtract one to make sure the last address is also valid.
  if (auto checked_sum = CheckedAdd(address, size - 1)) {
    read_end = *checked_sum;
  } else {
    // Overflow.
    return Err("Address + size overflows");
  }

  // It's expected the set of blocks will be in the 1-3 block making a brute-force search for the
  // block containing the address more efficient than a binary search.
  for (const auto& block : blocks_) {
    uint64_t last_addr = 0;
    if (auto checked_sum = CheckedAdd(block.address, block.size - 1)) {
      last_addr = *checked_sum;
    } else {
      return Err("Block bounds overflows.");
    }

    if (address >= block.address && address <= last_addr) {
      // This block contains |address|.
      if (!block.valid) {
        return Err("Address is contained in an invalid block");
      } else if (read_end > last_addr) {
        return Err("Read too big for containing block");
      }

      memcpy(dest, &block.data[address - block.address], size);
      return Err();
    }
  }
  return Err("No blocks contained address");
}

}  // namespace zxdb
