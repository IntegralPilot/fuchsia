// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MOCK_MOCK_MSD_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_MOCK_MOCK_MSD_H_

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/util/macros.h>
#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_defs.h>
#include <lib/sync/cpp/completion.h>

#include <iterator>
#include <memory>
#include <vector>

// These classes contain default implementations of msd_device_t functionality.
// To override a specific function to contain test logic, inherit from the
// desired class, override the desired function, and pass as the msd_abi object

class MsdMockBuffer : public msd::Buffer {
 public:
  explicit MsdMockBuffer(std::unique_ptr<magma::PlatformBuffer> platform_buf)
      : platform_buf_(std::move(platform_buf)) {}
  ~MsdMockBuffer() override;

  static MsdMockBuffer* cast(msd::Buffer* buf) {
    MAGMA_DASSERT(buf);
    auto buffer = static_cast<MsdMockBuffer*>(buf);
    MAGMA_DASSERT(buffer->magic_ == kMagic);
    return buffer;
  }

  magma::PlatformBuffer* platform_buffer() { return platform_buf_.get(); }

 private:
  std::unique_ptr<magma::PlatformBuffer> platform_buf_;
  static const uint32_t kMagic = 0x6d6b6266;  // "mkbf" (Mock Buffer)
  uint32_t magic_ = kMagic;
};

class MsdMockConnection;

class MsdMockContext : public msd::Context {
 public:
  explicit MsdMockContext(MsdMockConnection* connection) : connection_(connection) {
    magic_ = kMagic;
  }
  ~MsdMockContext() override;

  magma_status_t MsdExecuteCommandBufferWithResources(msd::magma_command_buffer* command_buffer,
                                                      magma_exec_resource* exec_resources,
                                                      msd::Buffer** buffers,
                                                      msd::Semaphore** wait_semaphores,
                                                      msd::Semaphore** signal_semaphores) override {
    last_submitted_exec_resources_.clear();
    for (uint32_t i = 0; i < command_buffer->resource_count; i++) {
      last_submitted_exec_resources_.push_back(MsdMockBuffer::cast(buffers[i]));
    }
    return MAGMA_STATUS_OK;
  }

  static MsdMockContext* cast(msd::Context* ctx) {
    MAGMA_DASSERT(ctx);
    auto context = static_cast<MsdMockContext*>(ctx);
    MAGMA_DASSERT(context->magic_ == kMagic);
    return context;
  }

  std::vector<MsdMockBuffer*>& last_submitted_exec_resources() {
    return last_submitted_exec_resources_;
  }

 private:
  std::vector<MsdMockBuffer*> last_submitted_exec_resources_;

  MsdMockConnection* connection_;
  static const uint32_t kMagic = 0x6d6b6378;  // "mkcx" (Mock Context)
  uint32_t magic_ = kMagic;
};

class MsdMockConnection : public msd::Connection {
 public:
  MsdMockConnection() { magic_ = kMagic; }
  ~MsdMockConnection() override {}

  std::unique_ptr<msd::Context> MsdCreateContext() override {
    return std::make_unique<MsdMockContext>(this);
  }

  virtual void DestroyContext(MsdMockContext* ctx) {}

  magma_status_t MsdMapBuffer(msd::Buffer& buffer, uint64_t gpu_va, uint64_t offset,
                              uint64_t length, uint64_t flags) override {
    return MAGMA_STATUS_OK;
  }
  magma_status_t MsdUnmapBuffer(msd::Buffer& buffer, uint64_t gpu_va) override {
    return MAGMA_STATUS_OK;
  }
  magma_status_t MsdBufferRangeOp(msd::Buffer& buffer, uint32_t options, uint64_t start_offset,
                                  uint64_t length) override {
    return MAGMA_STATUS_OK;
  }
  magma_status_t MsdCreatePerformanceCounterBufferPool(
      uint64_t pool_id, std::unique_ptr<msd::PerfCountPool>* pool_out) override;

  magma_status_t MsdReleasePerformanceCounterBufferPool(
      std::unique_ptr<msd::PerfCountPool> pool) override;

  magma_status_t MsdAddPerformanceCounterBufferOffsetToPool(msd::PerfCountPool& pool,
                                                            msd::Buffer& buffer, uint64_t buffer_id,
                                                            uint64_t buffer_offset,
                                                            uint64_t buffer_size) override {
    return MAGMA_STATUS_OK;
  }

  magma_status_t MsdRemovePerformanceCounterBufferFromPool(msd::PerfCountPool& pool,
                                                           msd::Buffer& buffer) override {
    return MAGMA_STATUS_OK;
  }

  magma_status_t MsdDumpPerformanceCounters(msd::PerfCountPool& pool,
                                            uint32_t trigger_id) override {
    return MAGMA_STATUS_OK;
  }

 private:
  static const uint32_t kMagic = 0x6d6b636e;  // "mkcn" (Mock Connection)
  uint32_t magic_ = kMagic;
};

class MsdMockSemaphore : public msd::Semaphore {
 public:
  explicit MsdMockSemaphore(std::unique_ptr<magma::PlatformSemaphore> semaphore)
      : semaphore_(std::move(semaphore)) {}
  ~MsdMockSemaphore() override = default;
  zx_koid_t GetKoid() { return semaphore_->id(); }

 private:
  std::unique_ptr<magma::PlatformSemaphore> semaphore_;
};

class MsdMockDevice : public msd::Device {
 public:
  MsdMockDevice() { magic_ = kMagic; }
  ~MsdMockDevice() override = default;

  void MsdSetMemoryPressureLevel(msd::MagmaMemoryPressureLevel level) override;
  magma_status_t MsdQuery(uint64_t id, zx::vmo* result_buffer_out, uint64_t* result_out) override;
  magma_status_t MsdGetIcdList(std::vector<msd::MsdIcdInfo>* icd_info_out) override;
  std::unique_ptr<msd::Connection> MsdOpen(msd::msd_client_id_t client_id) override {
    return std::make_unique<MsdMockConnection>();
  }

  virtual uint32_t GetDeviceId() { return 0; }

  void WaitForMemoryPressureSignal();

  msd::MagmaMemoryPressureLevel memory_pressure_level() const {
    std::lock_guard lock(level_mutex_);
    return memory_pressure_level_;
  }

 private:
  static const uint32_t kMagic = 0x6d6b6476;  // "mkdv" (Mock Device)
  uint32_t magic_ = kMagic;
  mutable std::mutex level_mutex_;
  msd::MagmaMemoryPressureLevel memory_pressure_level_ = msd::MAGMA_MEMORY_PRESSURE_LEVEL_NORMAL;
  libsync::Completion completion_;
};

class MsdMockDriver : public msd::Driver {
 public:
  MsdMockDriver() { magic_ = kMagic; }
  ~MsdMockDriver() override = default;

  std::unique_ptr<msd::Device> MsdCreateDevice(msd::DeviceHandle* device_data) override {
    return std::make_unique<MsdMockDevice>();
  }

  std::unique_ptr<msd::Buffer> MsdImportBuffer(zx::vmo vmo, uint64_t client_id) override;

  magma_status_t MsdImportSemaphore(zx::handle handle, uint64_t client_id, uint64_t flags,
                                    std::unique_ptr<msd::Semaphore>* out) override;

  static MsdMockDriver* cast(Driver* drv) {
    MAGMA_DASSERT(drv);
    auto driver = static_cast<MsdMockDriver*>(drv);
    MAGMA_DASSERT(driver->magic_ == kMagic);
    return driver;
  }

 private:
  static const uint32_t kMagic = 0x6d6b6472;  // "mkdr" (Mock Driver)
  uint32_t magic_ = kMagic;
};

// There is no buffermanager concept in the msd abi right now, so this class is
// for testing purposes only, making it a little different than the other
// classes in this header

class MsdMockBufferManager {
 public:
  MsdMockBufferManager() = default;

  virtual ~MsdMockBufferManager() = default;

  virtual std::unique_ptr<MsdMockBuffer> CreateBuffer(zx::vmo handle, uint64_t client_id) {
    auto platform_buf = magma::PlatformBuffer::Import(std::move(handle));

    platform_buf->set_local_id(client_id);

    return std::make_unique<MsdMockBuffer>(std::move(platform_buf));
  }

  virtual void DestroyBuffer(MsdMockBuffer* buf) {}

  class ScopedMockBufferManager {
   public:
    explicit ScopedMockBufferManager(std::unique_ptr<MsdMockBufferManager> bufmgr) {
      SetTestBufferManager(std::move(bufmgr));
    }

    ~ScopedMockBufferManager() { SetTestBufferManager(nullptr); }

    MsdMockBufferManager* get();
  };

 private:
  static void SetTestBufferManager(std::unique_ptr<MsdMockBufferManager> bufmgr);
};

#endif
