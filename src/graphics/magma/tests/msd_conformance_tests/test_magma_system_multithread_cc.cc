// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/sys_driver/magma_system_connection.h>
#include <lib/magma_service/sys_driver/magma_system_context.h>
#include <lib/magma_service/sys_driver/magma_system_device.h>
#include <lib/magma_service/test_util/platform_msd_device_helper.h>
#include <magma_intel_gen_defs.h>

#include <thread>
#include <vector>

#include <gtest/gtest.h>

namespace msd {
namespace {
inline uint64_t page_size() { return sysconf(_SC_PAGESIZE); }
}  // namespace

// This test is meant to run on all devices and exercise
// the execution of command buffers from multiple connections
// simultaneously.  So doing requires some device specific knowledge
// (for example what instructions to put into the command buffer);
// and that may not be easily achieved so in practice this test
// may bail out early on some devices.
class TestMultithread {
 public:
  TestMultithread(std::unique_ptr<Driver> driver, std::unique_ptr<MagmaSystemDevice> device)
      : driver_(std::move(driver)), device_(std::move(device)) {}

  void Test(uint32_t num_threads) {
    std::vector<std::thread> threads;

    for (uint32_t i = 0; i < num_threads; i++) {
      std::thread connection_thread(ConnectionThreadEntry, this);
      threads.emplace_back(std::move(connection_thread));
    }

    for (auto& thread : threads) {
      ASSERT_TRUE(thread.joinable());
      thread.join();
    }
  }

  static void ConnectionThreadEntry(TestMultithread* test) {
    return test->ConnectionThreadLoop(100);
  }

  void ConnectionThreadLoop(uint32_t num_iterations) {
    auto connection =
        std::make_unique<MagmaSystemConnection>(device_.get(), device_->msd_dev()->MsdOpen(0));
    ASSERT_NE(connection, nullptr);

    uint64_t extra_page_count;
    EXPECT_EQ(MAGMA_STATUS_OK, device_->msd_dev()->MsdQuery(kMagmaIntelGenQueryExtraPageCount,
                                                            nullptr, &extra_page_count));

    uint32_t context_id = ++context_id_;
    EXPECT_TRUE(connection->CreateContext(context_id));
    auto context = connection->LookupContext(context_id);
    ASSERT_NE(context, nullptr);

    uint64_t gpu_addr = 0;

    for (uint32_t i = 0; i < num_iterations; i++) {
      auto batch_buffer = magma::PlatformBuffer::Create(page_size(), "test");

      zx::handle handle;
      EXPECT_TRUE(batch_buffer->duplicate_handle(&handle));

      uint64_t id = batch_buffer->id();
      EXPECT_TRUE(connection->ImportBuffer(std::move(handle), id));

      InitBatchBufferIntel(batch_buffer.get());

      constexpr uint64_t kMapFlags =
          MAGMA_MAP_FLAG_READ | MAGMA_MAP_FLAG_WRITE | MAGMA_MAP_FLAG_EXECUTE;
      EXPECT_TRUE(connection->MapBuffer(id, gpu_addr, 0, batch_buffer->size(), kMapFlags));
      gpu_addr += batch_buffer->size() + extra_page_count * page_size();

      std::vector<magma_exec_command_buffer> command_buffers(1);
      std::vector<magma_exec_resource> exec_resources(1);
      std::vector<uint64_t> wait_semaphore_ids;
      std::vector<uint64_t> signal_semaphore_ids;
      EXPECT_TRUE(
          InitCommandBuffer(command_buffers.data(), &exec_resources[0], batch_buffer.get()));

      EXPECT_TRUE(context->ExecuteCommandBuffers(
          command_buffers, exec_resources, wait_semaphore_ids, signal_semaphore_ids, /*flags=*/0));
    }
  }

  bool InitCommandBuffer(magma_exec_command_buffer* command_buffer,
                         magma_exec_resource* exec_resource, magma::PlatformBuffer* batch_buffer) {
    command_buffer->resource_index = 0;
    command_buffer->start_offset = 0;

    exec_resource->buffer_id = batch_buffer->id();
    exec_resource->offset = 0;
    exec_resource->length = batch_buffer->size();

    return true;
  }

  void InitBatchBufferIntel(magma::PlatformBuffer* buffer) {
    void* vaddr;
    ASSERT_TRUE(buffer->MapCpu(&vaddr));
    *reinterpret_cast<uint32_t*>(vaddr) = 0xA << 23;
    EXPECT_TRUE(buffer->UnmapCpu());
  }

 private:
  std::unique_ptr<msd::Driver> driver_;
  std::unique_ptr<MagmaSystemDevice> device_;
  uint32_t context_id_ = 0;
};

TEST(MagmaSystem, Multithread) {
  auto driver = msd::Driver::MsdCreate();
  ASSERT_TRUE(driver);

  auto device =
      msd::MagmaSystemDevice::Create(driver.get(), driver->MsdCreateDevice(GetTestDeviceHandle()));
  ASSERT_TRUE(device);

  uint64_t vendor_id;
  ASSERT_TRUE(device->Query(MAGMA_QUERY_VENDOR_ID, &vendor_id));
  // Test only supports Intel GPUs.
  if (vendor_id != 0x8086)
    GTEST_SKIP();

  auto test = std::make_unique<TestMultithread>(std::move(driver), std::move(device));
  ASSERT_TRUE(test);

  test->Test(2);
}

}  // namespace msd
