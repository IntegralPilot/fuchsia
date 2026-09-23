// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fpromise/result.h>
#include <lib/sync/completion.h>
#include <stddef.h>
#include <stdint.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include "src/storage/lib/buffer/blocking_ring_buffer.h"
#include "src/storage/lib/vfs/cpp/journal/fuzzer_utils.h"
#include "src/storage/lib/vfs/cpp/journal/journal.h"
#include "src/storage/lib/vfs/cpp/journal/superblock.h"

namespace fs {
namespace {

extern "C" int LLVMFuzzerTestOneInput(uint8_t* data, size_t size) {
  FuzzerUtils fuzz_utils(data, size);
  JournalSuperblock info;
  size_t journal_len = fuzz_utils.data_provider()->ConsumeIntegralInRange<size_t>(0, 16);
  size_t writeback_len = fuzz_utils.data_provider()->ConsumeIntegralInRange<size_t>(0, 16);
  std::unique_ptr<storage::BlockingRingBuffer> journal_buffer;
  std::unique_ptr<storage::BlockingRingBuffer> writeback_buffer;
  if (fuzz_utils.FuzzSuperblock(&info) != ZX_OK ||
      fuzz_utils.CreateRingBuffer("journal-writeback-buffer", ReservedVmoid::kJournalVmoid,
                                  journal_len, &journal_buffer) != ZX_OK ||
      fuzz_utils.CreateRingBuffer("data-writeback-buffer", ReservedVmoid::kWritebackVmoid,
                                  writeback_len, &writeback_buffer) != ZX_OK) {
    return 0;
  }
  auto journal_start_block = fuzz_utils.data_provider()->ConsumeIntegral<uint64_t>();
  Journal journal(fuzz_utils.handler(), std::move(info), std::move(journal_buffer),
                  std::move(writeback_buffer), journal_start_block);
  while (fuzz_utils.data_provider()->remaining_bytes() != 0) {
    [[maybe_unused]] zx_status_t status = journal.CommitTransaction(
        {.metadata_operations = fuzz_utils.FuzzOperation(ReservedVmoid::kJournalVmoid),
         .data_promise =
             journal.WriteData(fuzz_utils.FuzzOperation(ReservedVmoid::kWritebackVmoid))});
    sync_completion_t sync_completion;
    journal.schedule_task(
        journal.Sync().then([&sync_completion](fpromise::result<void, zx_status_t>& result) mutable
                                -> fpromise::result<void, zx_status_t> {
          sync_completion_signal(&sync_completion);
          return result;
        }));
    sync_completion_wait(&sync_completion, ZX_TIME_INFINITE);
  }
  return 0;
}

}  // namespace
}  // namespace fs
