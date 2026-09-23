// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_WRITEBACK_H_
#define SRC_STORAGE_F2FS_WRITEBACK_H_

#include <lib/fpromise/sequencer.h>

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/file_cache.h"
#include "src/storage/lib/vfs/cpp/journal/background_executor.h"
#include "src/storage/lib/vfs/cpp/transaction/buffered_operations_builder.h"

namespace f2fs {

class StorageBufferPool;
class OwnedStorageBuffer;
class BcacheMapper;

// F2fs flushes dirty pages when the number of dirty pages >= |kMaxDirtyDataPages| if memorypressure
// is unavailable.
constexpr int kMaxDirtyDataPages = 51200;

// This class is final because there might be background threads running when its destructor runs
// and that would be unsafe if this class had overridden virtual methods that might get called from
// those background threads.
class Writer final {
 public:
  Writer(std::unique_ptr<StorageBufferPool>);
  Writer() = delete;
  Writer(const Writer &) = delete;
  Writer &operator=(const Writer &) = delete;
  Writer(Writer &&) = delete;
  Writer &operator=(Writer &&) = delete;
  ~Writer();

  // It schedules |task| on |writeback_executor_|.
  void ScheduleWriteback(fpromise::promise<> task);

  // It inserts |pages| to |pages_|, and calls GetTaskForWriteIO() that makes a task to write
  // pages out to backing storage. Then, it schedules the task on |executor_|. All tasks execute
  // in order they are scheduled by |sequencer_|.
  void ScheduleWriteBlocks(sync_completion_t *completion = nullptr, PageList pages = {},
                           bool flush = true) __TA_EXCLUDES(mutex_);

  // It returns after waiting for the storage operation for all pending pages to complete.
  void Sync();

 private:
  // It returns a task where it builds storage operations from pages and requests the operations to
  // the storage driver server to write the storage operations out to backing storage. On the
  // completion of the storage operations, the task notifies the waiters for writeback pages, and
  // signals |completion| if it is not null. See below for the semantics of |needs_preflush|.
  fpromise::promise<> GetTaskForWriteIO(PageList to_submit, sync_completion_t *completion);
  std::vector<storage::BufferedOperation> BuildBufferedOperation(OwnedStorageBuffer &buffer,
                                                                 PageList &pages,
                                                                 PageList &to_submit,
                                                                 bool &needs_preflush);

  // If the operation requires a pre-flush, |needs_preflush| will be set to true. The caller should
  // initialise `needs_preflush` to false.
  zx::result<storage::Operation> PageToOperation(OwnedStorageBuffer &buffer, Page &page,
                                                 bool &needs_preflush);

  const size_t max_block_address_;
  std::mutex mutex_;
  PageList pages_ __TA_GUARDED(mutex_);
  std::unique_ptr<StorageBufferPool> pool_;
  BcacheMapper *const bcache_mapper_ = nullptr;
  fpromise::sequencer sequencer_;
  fpromise::sequencer writeback_sequencer_;
  // An executor for tasks that write writeback pages to backing storage.
  fs::BackgroundExecutor executor_;
  // An executor for tasks that allocate blocks for dirty pages.
  fs::BackgroundExecutor writeback_executor_;
};

// A helper class to mitigate notification costs
class NotifyWriteback {
 public:
  ~NotifyWriteback() { Notify(1); }
  // It tries to group notification to writeback waiters for the same file.
  // If a new waiter of |page| doesn't belong to the same file as that of |waiters_|, it immediately
  // notifies |waiters_|.
  void ReserveNotify(fbl::RefPtr<Page> page);

 private:
  // It wakes |waiters| up when the number of waiters more than |kNotifyInterval| by default.
  void Notify(size_t interval = kNotifyInterval);
  static constexpr size_t kNotifyInterval = 8;
  PageList waiters_ = {};
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_WRITEBACK_H_
