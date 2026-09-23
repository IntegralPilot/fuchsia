// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_CONTEXT_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_CONTEXT_H_

#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_c.h>
#include <lib/zx/vmo.h>

namespace msd {
class CppContext : public msd::Context {
 public:
  explicit CppContext(struct MsdContext* context);

  ~CppContext() override;

  magma_status_t MsdExecuteCommandBuffers(std::vector<magma_exec_command_buffer>& command_buffers,
                                          std::vector<magma_exec_resource>& resources,
                                          std::vector<msd::Buffer*>& buffers,
                                          std::vector<msd::Semaphore*>& wait_semaphores,
                                          std::vector<msd::Semaphore*>& signal_semaphores) override;

 private:
  struct MsdContext* context_;
};

}  // namespace msd
#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MSD_CPP_TO_C_CPP_CONTEXT_H_
