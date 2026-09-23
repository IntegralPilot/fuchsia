// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/nvme/queue.h"

#include <lib/zx/bti.h>
#include <zircon/syscalls.h>

namespace nvme {

zx::result<> Queue::Init(zx::unowned_bti bti, uint32_t entries) {
  if (entries * entry_size_ > zx_system_get_page_size()) {
    entries = zx_system_get_page_size() / entry_size_;
  }

  entry_count_ = entries;

  auto buffer_factory = dma_buffer::CreateBufferFactory();
  zx_status_t status = buffer_factory->CreateContiguous(*bti, zx_system_get_page_size(), 0,
                                                        dma_buffer::CacheOptions::kEnabled, &io_);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  memset(io_->virt(), 0, io_->size());
  return zx::ok();
}

}  // namespace nvme
