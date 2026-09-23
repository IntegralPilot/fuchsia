// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/fsck.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fit/defer.h>
#include <lib/sync/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <memory>
#include <utility>

#include "src/storage/blobfs/blobfs.h"
#include "src/storage/blobfs/blobfs_checker.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/vfs/cpp/paged_vfs.h"

namespace blobfs {

// To run Fsck we mount Blobfs on the given BlockDevice. This requires a dispatcher. This function
// may be called in different contexts where there might not be an easily known dispatcher or none
// set up.
//
// To make this uniform from the caller's perspective, Blobfs is run on a new thread with a
// dedicated dispatcher.
zx_status_t Fsck(std::unique_ptr<block_client::BlockDevice> device, const MountOptions& options) {
  async::Loop loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  if (zx_status_t status = loop.StartThread(); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Cannot initialize dispatch loop";
    return status;
  }

  auto vfs = std::make_unique<fs::PagedVfs>(loop.dispatcher());
  if (auto status = vfs->Init(); status.is_error())
    return status.error_value();

  auto blobfs_or = Blobfs::Create(loop.dispatcher(), std::move(device), vfs.get(), options);
  // This must be declared after blobfs is constructed. The paged vfs' threads must be stopped
  // before blobfs is destroyed.
  auto defer = fit::defer([&] { vfs->TearDown(); });
  if (blobfs_or.is_error()) {
    FX_PLOGS(ERROR, blobfs_or.status_value()) << "Cannot create filesystem for checking";
    return blobfs_or.status_value();
  }
  Blobfs* blobfs = blobfs_or.value().get();

  sync_completion_t completion;
  zx_status_t result = ZX_OK;

  async::PostTask(
      loop.dispatcher(), [dispatcher = loop.dispatcher(), blobfs, &result, &completion]() mutable {
        BlobfsChecker::Options checker_options;
        if (blobfs->writability() == Writability::ReadOnlyDisk) {
          checker_options.repair = false;
        }
        result = BlobfsChecker(blobfs, checker_options).Check() ? ZX_OK : ZX_ERR_IO_DATA_INTEGRITY;
        sync_completion_signal(&completion);
      });

  sync_completion_wait(&completion, ZX_TIME_INFINITE);

  return result;
}

}  // namespace blobfs
