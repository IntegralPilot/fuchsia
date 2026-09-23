// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_TESTING_MOCK_MEMORY_H_
#define SRC_LIB_UNWINDER_TESTING_MOCK_MEMORY_H_

#include <map>
#include <vector>

#include "src/lib/unwinder/memory.h"

namespace unwinder {

// A fake implementation of Memory that stores literal bytes.
// It is called "MockMemory" to align with zxdb naming conventions.
//
// Example usage:
//   MockMemory mock_memory;
//   uint64_t val = 0xdeadbeef;
//   mock_memory.AddMemory(0x1000, std::vector<uint8_t>(
//       reinterpret_cast<uint8_t*>(&val),
//       reinterpret_cast<uint8_t*>(&val) + sizeof(val)));
class MockMemory : public Memory {
 public:
  void AddMemory(uint64_t addr, std::vector<uint8_t> data) { regions_[addr] = std::move(data); }

  Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    for (const auto& [base_addr, data] : regions_) {
      if (addr >= base_addr && addr + size <= base_addr + data.size()) {
        memcpy(dst, data.data() + (addr - base_addr), size);
        return Success();
      }
    }
    return Error("Invalid memory access at 0x%zx", addr);
  }

 private:
  std::map<uint64_t, std::vector<uint8_t>> regions_;
};

// A fake implementation of AsyncMemory::Delegate.
//
// Used to test asynchronous unwinding flows. Automatically executes callbacks
// synchronously to simplify test execution.
class MockAsyncMemoryDelegate : public AsyncMemory::Delegate {
 public:
  void AddMemory(uint64_t addr, std::vector<uint8_t> data) {
    mem_.AddMemory(addr, std::move(data));
  }

  Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    return mem_.ReadBytes(addr, size, dst);
  }

  void FetchMemoryRanges(std::vector<std::pair<uint64_t, uint32_t>> ranges,
                         fit::callback<void()> cb) override {
    cb();
  }

  void PostTask(fit::callback<void()> cb) override { cb(); }

 private:
  MockMemory mem_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_TESTING_MOCK_MEMORY_H_
