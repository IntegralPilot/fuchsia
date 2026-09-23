// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/writeback.h"

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/file_cache.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/storage_buffer.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vnode.h"

namespace f2fs {

Writer::Writer(std::unique_ptr<StorageBufferPool> pool)
    : max_block_address_(pool->Block()->Maxblk()), bcache_mapper_(pool->Block()) {
  pool_ = std::move(pool);
}

Writer::~Writer() {
  Sync();
  executor_.Terminate();
  writeback_executor_.Terminate();
}

void Writer::Sync() {
  sync_completion_t completion;
  ScheduleWriteBlocks(&completion, {}, true);
  ZX_ASSERT_MSG(sync_completion_wait(&completion, zx::sec(kWriteTimeOut).get()) == ZX_OK,
                "Writer::Sync() timeout");
}

zx::result<storage::Operation> Writer::PageToOperation(OwnedStorageBuffer &buffer, Page &page,
                                                       bool &needs_preflush) {
  if (!IsValidBlockAddr(page.GetBlockAddr()) || page.GetBlockAddr() >= max_block_address_) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result index = buffer->Reserve(1);
  if (index.is_error()) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  storage::OperationType type = storage::OperationType::kWrite;
  if (page.IsCommit()) {
    needs_preflush = true;
    type = storage::OperationType::kWriteFua;
    page.ClearCommit();
  } else if (page.IsSync()) {
    type = storage::OperationType::kWriteFua;
    page.ClearSync();
  }

  storage::Operation op = {
      .type = type,
      .vmo_offset = *index,
      .dev_offset = page.GetBlockAddr(),
      .length = 1,
  };
  // Copy data of |page| to |buffer|
  page.Read(buffer->Data(op.vmo_offset));
  return zx::ok(op);
}

std::vector<storage::BufferedOperation> Writer::BuildBufferedOperation(OwnedStorageBuffer &buffer,
                                                                       PageList &pages,
                                                                       PageList &to_submit,
                                                                       bool &needs_preflush) {
  fs::BufferedOperationsBuilder builder;
  NotifyWriteback notifier;
  while (!pages.is_empty()) {
    auto page = pages.pop_front();
    zx::result operation = PageToOperation(buffer, *page, needs_preflush);
    if (operation.is_error()) {
      if (operation.error_value() == ZX_ERR_UNAVAILABLE) {
        // No available buffers. Need to submit pending StorageOperations to free buffers.
        pages.push_front(std::move(page));
        return builder.TakeOperations();
      }
      // If |page| has an invalid addr, notify waiters.
      if (page->IsUptodate()) {
        FX_LOGS(ERROR) << "(" << page->GetKey() << ": " << page->GetVnode().GetKey()
                       << ") page has an invalid addr: " << page->GetBlockAddr() << " "
                       << operation.status_string();
      }
      notifier.ReserveNotify(std::move(page));
      continue;
    }
    to_submit.push_back(std::move(page));
    builder.Add(*operation, &buffer->GetVmoBuffer());
  }
  return builder.TakeOperations();
}

fpromise::promise<> Writer::GetTaskForWriteIO(PageList to_submit, sync_completion_t *completion) {
  return fpromise::make_promise([this, to_submit = std::move(to_submit), completion]() mutable {
    while (!to_submit.is_empty()) {
      PageList submitted;
      OwnedStorageBuffer buffer = pool_->Get(kDefaultBlocksPerSegment);
      bool needs_preflush = false;
      auto operations = BuildBufferedOperation(buffer, to_submit, submitted, needs_preflush);
      if (operations.empty()) {
        break;
      }

      zx_status_t io_status = ZX_OK;
      if (needs_preflush)
        io_status = bcache_mapper_->Flush();
      if (io_status == ZX_OK)
        io_status = bcache_mapper_->RunRequests(operations);

      NotifyWriteback notifier;
      while (!submitted.is_empty()) {
        auto page = submitted.pop_front();
        // The instance of page->GetVnode() is alive as long as it has any writeback pages.
        if (io_status != ZX_OK) {
          if (page->GetVnode().IsMeta() || io_status == ZX_ERR_UNAVAILABLE ||
              io_status == ZX_ERR_PEER_CLOSED) {
            // When it fails to write metadata or the block device is not available,
            // set kCpErrorFlag to enter read-only mode.
            page->fs()->GetSuperblockInfo().SetCpFlags(CpFlag::kCpErrorFlag);
          } else {
            // When IO errors occur with node and data Pages, just set a dirty flag
            // to retry it with another LBA.
            LockedPage locked_page = LockedPage(page, std::try_to_lock);
            if (locked_page) {
              locked_page.SetDirty();
            } else {
              page->SetDirty();
            }
          }
        }
        if (page->GetVnode().IsNode()) {
          fbl::RefPtr<NodePage>::Downcast(page)->SetFsyncMark(false);
        }
        page->ClearColdData();
        notifier.ReserveNotify(std::move(page));
      }
    }
    if (completion) {
      sync_completion_signal(completion);
    }
    return fpromise::ok();
  });
}

void Writer::ScheduleWriteback(fpromise::promise<> task) {
  writeback_executor_.schedule_task(writeback_sequencer_.wrap(std::move(task)));
}

void Writer::ScheduleWriteBlocks(sync_completion_t *completion, PageList pages, bool flush) {
  bool need_flush = completion != nullptr || flush;
  if (!pages.is_empty() || need_flush) {
    std::lock_guard lock(mutex_);
    pages_.splice(pages_.end(), pages);
    // Submit |pages_| when too many pages are waiting for writeback or |need_flush| is set.
    if (need_flush || pages_.size() >= kDefaultBlocksPerSegment) {
      executor_.schedule_task(sequencer_.wrap(GetTaskForWriteIO(std::move(pages_), completion)));
    }
  }
}

void NotifyWriteback::ReserveNotify(fbl::RefPtr<Page> page) {
  VnodeF2fs &vnode = page->GetVnode();
  if (!waiters_.is_empty() && waiters_.front().GetVnode().GetKey() == vnode.GetKey()) {
    waiters_.push_back(std::move(page));
    Notify();
  } else {
    Notify(1);
    waiters_.push_back(std::move(page));
  }
}

void NotifyWriteback::Notify(size_t interval) {
  if (waiters_.size() < interval || waiters_.is_empty()) {
    return;
  }
  auto &page = waiters_.front();
  page.GetFileCache().NotifyWriteback(std::move(waiters_));
}

}  // namespace f2fs
