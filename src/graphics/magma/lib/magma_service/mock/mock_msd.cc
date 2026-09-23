// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mock_msd.h"

#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma_service/msd.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>

#include <vector>

std::unique_ptr<MsdMockBufferManager> g_bufmgr;

msd::DeviceHandle* GetTestDeviceHandle() { return nullptr; }

// static
std::unique_ptr<msd::Driver> msd::Driver::MsdCreate() { return std::make_unique<MsdMockDriver>(); }

void MsdMockDevice::MsdSetMemoryPressureLevel(msd::MagmaMemoryPressureLevel level) {
  std::lock_guard lock(level_mutex_);
  memory_pressure_level_ = level;
  completion_.Signal();
}

magma_status_t MsdMockDevice::MsdQuery(uint64_t id, zx::vmo* result_buffer_out,
                                       uint64_t* result_out) {
  switch (id) {
    case MAGMA_QUERY_DEVICE_ID:
      *result_out = GetDeviceId();
      break;

    default:
      return MAGMA_STATUS_INVALID_ARGS;
  }

  if (result_buffer_out)
    *result_buffer_out = {};

  return MAGMA_STATUS_OK;
}

magma_status_t MsdMockDevice::MsdGetIcdList(std::vector<msd::MsdIcdInfo>* icd_info_out) {
  icd_info_out->clear();

  // Hardcode results.
  const char* kResults[] = {"a", "b"};
  for (uint32_t i = 0; i < std::size(kResults); i++) {
    msd::MsdIcdInfo info{};
    info.component_url = kResults[i];
    info.support_flags = msd::ICD_SUPPORT_FLAG_VULKAN;
    icd_info_out->push_back(info);
  }
  return MAGMA_STATUS_OK;
}

void MsdMockDevice::WaitForMemoryPressureSignal() { completion_.Wait(); }

std::unique_ptr<msd::Buffer> MsdMockDriver::MsdImportBuffer(zx::vmo handle, uint64_t client_id) {
  if (!g_bufmgr)
    g_bufmgr.reset(new MsdMockBufferManager());

  return g_bufmgr->CreateBuffer(std::move(handle), client_id);
}

MsdMockBuffer::~MsdMockBuffer() { g_bufmgr->DestroyBuffer(this); }

void MsdMockBufferManager::SetTestBufferManager(std::unique_ptr<MsdMockBufferManager> bufmgr) {
  g_bufmgr = std::move(bufmgr);
}

MsdMockBufferManager* MsdMockBufferManager::ScopedMockBufferManager::get() {
  return g_bufmgr.get();
}

MsdMockContext::~MsdMockContext() { connection_->DestroyContext(this); }

magma_status_t MsdMockDriver::MsdImportSemaphore(zx::handle handle, uint64_t client_id,
                                                 uint64_t flags,
                                                 std::unique_ptr<msd::Semaphore>* out) {
  auto semaphore = magma::PlatformSemaphore::Import(std::move(handle), flags);
  if (!semaphore)
    return MAGMA_STATUS_INVALID_ARGS;

  semaphore->set_local_id(client_id);
  *out = std::make_unique<MsdMockSemaphore>(std::move(semaphore));
  return MAGMA_STATUS_OK;
}

class MsdMockPool : public msd::PerfCountPool {
 public:
  ~MsdMockPool() override = default;
};

magma_status_t MsdMockConnection::MsdCreatePerformanceCounterBufferPool(
    uint64_t pool_id, std::unique_ptr<msd::PerfCountPool>* pool_out) {
  *pool_out = std::make_unique<MsdMockPool>();
  return MAGMA_STATUS_OK;
}

magma_status_t MsdMockConnection::MsdReleasePerformanceCounterBufferPool(
    std::unique_ptr<msd::PerfCountPool> pool) {
  pool.reset();
  return MAGMA_STATUS_OK;
}
