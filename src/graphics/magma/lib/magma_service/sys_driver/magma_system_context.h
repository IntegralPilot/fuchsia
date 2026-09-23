// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_CONTEXT_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_CONTEXT_H_

#include <lib/magma/util/status.h>

#include <functional>
#include <memory>

#include "magma_system_buffer.h"
#include "magma_system_semaphore.h"

class CommandBufferHelper;

namespace msd {
class MagmaSystemCommandBuffer;

class MagmaSystemContext {
 public:
  class Owner {
   public:
    virtual std::shared_ptr<MagmaSystemBuffer> LookupBufferForContext(uint64_t id) = 0;
    virtual std::shared_ptr<MagmaSystemSemaphore> LookupSemaphoreForContext(uint64_t id) = 0;
  };

  MagmaSystemContext(Owner* owner, std::unique_ptr<msd::Context> msd_ctx)
      : owner_(owner), msd_ctx_(std::move(msd_ctx)) {}

  magma::Status ExecuteCommandBuffers(std::vector<magma_exec_command_buffer>& command_buffers,
                                      std::vector<magma_exec_resource>& resources,
                                      std::vector<uint64_t>& wait_semaphores_ids,
                                      std::vector<uint64_t>& signal_semaphores_ids, uint64_t flags);
  magma::Status ExecuteInlineCommands(std::vector<magma_inline_command_buffer> commands);

 private:
  msd::Context* msd_ctx() { return msd_ctx_.get(); }

  Owner* owner_;

  std::unique_ptr<msd::Context> msd_ctx_;

  friend class ::CommandBufferHelper;
};
}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_CONTEXT_H_
