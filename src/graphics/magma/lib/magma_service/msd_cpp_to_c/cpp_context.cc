// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "cpp_context.h"

#include <lib/magma/util/macros.h>

#include <vector>

#include "cpp_buffer.h"
#include "cpp_semaphore.h"

namespace msd {

CppContext::CppContext(struct MsdContext* context) : context_(context) { MAGMA_DASSERT(context_); }

CppContext::~CppContext() { msd_context_release(context_); }

magma_status_t CppContext::MsdExecuteCommandBuffers(
    std::vector<magma_exec_command_buffer>& command_buffers,
    std::vector<magma_exec_resource>& resources, std::vector<msd::Buffer*>& buffers,
    std::vector<msd::Semaphore*>& wait_semaphores,
    std::vector<msd::Semaphore*>& signal_semaphores) {
  std::vector<struct MsdBuffer*> msd_buffers;
  msd_buffers.reserve(buffers.size());
  for (auto* buffer : buffers) {
    msd_buffers.push_back(static_cast<CppBuffer*>(buffer)->buffer());
  }

  std::vector<struct MsdSemaphore*> msd_wait_semaphores;
  msd_wait_semaphores.reserve(wait_semaphores.size());
  for (auto* semaphore : wait_semaphores) {
    msd_wait_semaphores.push_back(static_cast<CppSemaphore*>(semaphore)->semaphore());
  }

  std::vector<struct MsdSemaphore*> msd_signal_semaphores;
  msd_signal_semaphores.reserve(signal_semaphores.size());
  for (auto* semaphore : signal_semaphores) {
    msd_signal_semaphores.push_back(static_cast<CppSemaphore*>(semaphore)->semaphore());
  }

  struct MsdCommandDescriptor descriptor = {
      .command_buffer_count = static_cast<uint32_t>(command_buffers.size()),
      .resource_count = static_cast<uint32_t>(resources.size()),
      .wait_semaphore_count = static_cast<uint32_t>(wait_semaphores.size()),
      .signal_semaphore_count = static_cast<uint32_t>(signal_semaphores.size()),
      .command_buffers = command_buffers.data(),
      .exec_resources = resources.data(),
      .buffers = msd_buffers.data(),
      .wait_semaphores = msd_wait_semaphores.data(),
      .signal_semaphores = msd_signal_semaphores.data(),
  };

  return msd_context_execute_command_buffers(context_, &descriptor);
}
}  // namespace msd
