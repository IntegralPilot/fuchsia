// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fdf/cpp/arena.h>
#include <lib/syslog/cpp/macros.h>

#include <format>
#include <vector>

#include <perftest/perftest.h>

#include "src/devices/bin/driver_runtime/microbenchmarks/assert.h"

namespace {

// Measure the time taken to allocate and free a |buffer_size|-byte block.
bool ArenaAllocFreeTest(perftest::RepeatState* state, size_t buffer_size) {
  constexpr uint32_t kTag = 'BNCH';
  fdf::Arena arena(kTag);

  state->DeclareStep("alloc");
  state->DeclareStep("free");
  while (state->KeepRunning()) {
    void* block = arena.Allocate(buffer_size);
    if (!block) {
      return false;
    }
    state->NextStep();
    arena.Free(block);  // Currently a no-op.
  }
  return true;
}

// Measure the time taken to allocate a large block followed by a bunch of small
// ones.
bool ArenaAllocLargeThenSmallTest(perftest::RepeatState* state, size_t large_size) {
  constexpr uint32_t kTag = 'BNCH';
  fdf::Arena arena(kTag);

  state->DeclareStep("one_large_block");
  state->DeclareStep("fifty_small_blocks");
  while (state->KeepRunning()) {
    void* block = arena.Allocate(large_size);
    if (!block) {
      return false;
    }
    state->NextStep();
    for (size_t i = 0; i < 50; i++) {
      void* block = arena.Allocate(32);
      if (!block) {
        return false;
      }
    }
  }
  return true;
}

// Measure the time taken to check whether a block is contained in an arena
// which holds |num_blocks|.
bool ArenaContainsTest(perftest::RepeatState* state, uint32_t num_blocks) {
  constexpr size_t kBlockSizeBytes = 0x1000;

  constexpr uint32_t kTag = 'BNCH';
  fdf::Arena arena(kTag);

  std::vector<uint32_t*> allocated;
  for (uint32_t i = 0; i < num_blocks; i++) {
    auto data = static_cast<uint32_t*>(arena.Allocate(kBlockSizeBytes));
    allocated.push_back(data);
  }

  uint32_t i = 0;

  while (state->KeepRunning()) {
    FX_CHECK(arena.Contains(allocated[i]));
    i++;
    i %= allocated.size();
  }
  return true;
}

void RegisterTests() {
  static const unsigned kBlockSize[] = {
      32, 64, 1024, 8192, 65536,
  };
  for (auto block_size : kBlockSize) {
    auto alloc_free_name = std::format("Arena/AllocFree/{}bytes", block_size);
    perftest::RegisterTest(alloc_free_name.c_str(), ArenaAllocFreeTest, block_size);
  }

  static const unsigned kLargeBlockSize[] = {
      4 * 1024, 8 * 1024, 16 * 1024, 32 * 1024, 64 * 1024,
  };
  for (auto large_size : kLargeBlockSize) {
    auto alloc_large_then_small_name = std::format("Arena/AllocLargeThenSmall/{}bytes", large_size);
    perftest::RegisterTest(alloc_large_then_small_name.c_str(), ArenaAllocLargeThenSmallTest,
                           large_size);
  }

  static const unsigned kNumBlocks[] = {
      1, 4, 16, 32, 1024,
  };
  for (auto num_blocks : kNumBlocks) {
    auto contains_name = std::format("Arena/Contains/{}blocks", num_blocks);
    perftest::RegisterTest(contains_name.c_str(), ArenaContainsTest, num_blocks);
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
