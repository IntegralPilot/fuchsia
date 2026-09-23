// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/mock/fake_address_space.h>
#include <lib/magma_service/mock/mock_bus_mapper.h>
#include <lib/magma_service/test_util/command_buffer_helper_cpp.h>
#include <lib/magma_service/test_util/platform_pci_device_helper.h>
#include <magma_intel_gen_defs.h>

#include <gtest/gtest.h>

#include "command_buffer.h"
#include "lib/magma_service/sys_driver/magma_system_context.h"
#include "msd_intel_context.h"
#include "msd_intel_device.h"
#include "ppgtt.h"
#include "test_command_buffer.h"

namespace {

using AllocatingAddressSpace = FakeAllocatingAddressSpace<GpuMapping, AddressSpace>;

const size_t kPageSize = zx_system_get_page_size();

}  // namespace

class TestHwCommandBuffer : public ::testing::Test {
 public:
  class AddressSpaceOwner : public magma::AddressSpaceOwner {
   public:
    virtual ~AddressSpaceOwner() = default;
    magma::PlatformBusMapper* GetBusMapper() override { return &bus_mapper_; }

   private:
    MockBusMapper bus_mapper_;
  };

  MsdIntelDevice* device() { return static_cast<MsdIntelDevice*>(helper_->dev()->msd_dev()); }

  std::shared_ptr<AddressSpace> exec_address_space() {
    auto context = static_cast<MsdIntelAbiContext*>(helper_->ctx())->ptr();
    return context->exec_address_space();
  }

  void TestMapUnmapResourcesGpu() {
    CreateCommandBuffer();

    auto address_space_owner = std::make_unique<AddressSpaceOwner>();
    auto addr_space =
        std::make_shared<AllocatingAddressSpace>(address_space_owner.get(), 0, 1024 * kPageSize);

    {
      std::vector<msd::MagmaSystemBuffer*>& resources = helper_->resources();
      for (auto& resource : resources) {
        std::shared_ptr<GpuMapping> mapping = addr_space->MapBufferGpu(
            addr_space, static_cast<MsdIntelAbiBuffer*>(resource->msd_buf())->ptr());
        ASSERT_TRUE(mapping);
        EXPECT_TRUE(addr_space->AddMapping(mapping));
      }
    }

    std::vector<std::shared_ptr<GpuMapping>> mappings;
    ASSERT_TRUE(TestCommandBuffer::MapResourcesGpu(cmd_buf_.get(), addr_space, mappings));

    uint32_t i = 0;
    for (auto& map : mappings) {
      gpu_addr_t addr = map->gpu_addr();
      EXPECT_TRUE(addr_space->is_allocated(addr));
      EXPECT_FALSE(addr_space->is_clear(addr));
      EXPECT_GE(addr_space->allocated_size(addr), helper_->resources()[i++]->size());
    }

    TestCommandBuffer::UnmapResourcesGpu(cmd_buf_.get());

    for (auto& map : mappings) {
      gpu_addr_t addr = map->gpu_addr();

      std::vector<std::shared_ptr<GpuMapping>> mappings;
      addr_space->ReleaseBuffer(map->buffer()->platform_buffer(), &mappings);
      EXPECT_EQ(1u, mappings.size());
      EXPECT_EQ(2u, map.use_count());
      mappings.clear();
      EXPECT_EQ(1u, map.use_count());
      EXPECT_TRUE(addr_space->is_allocated(addr));
      map.reset();
      EXPECT_FALSE(addr_space->is_allocated(addr));
    }
  }

  void TestPrepareForExecution(EngineCommandStreamerId id) {
    uint32_t batch_start_offset = 0x10;
    helper_->abi_cmd_buf()->start_offset = batch_start_offset;

    CreateCommandBuffer();

    {
      gpu_addr_t gpu_addr = 0;
      std::vector<msd::MagmaSystemBuffer*>& resources = helper_->resources();
      for (auto& resource : resources) {
        auto buffer = static_cast<MsdIntelAbiBuffer*>(resource->msd_buf())->ptr();
        std::shared_ptr<GpuMapping> mapping;
        EXPECT_TRUE(AddressSpace::MapBufferGpu(exec_address_space(), buffer, gpu_addr, 0,
                                               buffer->platform_buffer()->size() / kPageSize,
                                               &mapping));
        ASSERT_TRUE(mapping);
        EXPECT_TRUE(exec_address_space()->AddMapping(mapping));
        gpu_addr += buffer->platform_buffer()->size() + PerProcessGtt::ExtraPageCount() * kPageSize;
      }
    }

    auto device = static_cast<MsdIntelDevice*>(helper_->dev()->msd_dev());

    ASSERT_TRUE(cmd_buf_->PrepareForExecution());

    auto context = cmd_buf_->GetContext().lock();
    ASSERT_NE(context, nullptr);
    MsdIntelContext* ctx = static_cast<MsdIntelContext*>(context.get());
    ASSERT_NE(ctx, nullptr);

    switch (id) {
      case RENDER_COMMAND_STREAMER:
        EXPECT_TRUE(device->InitContextForEngine(context.get(), device->render_engine_cs()));
        break;
      case VIDEO_COMMAND_STREAMER:
        EXPECT_TRUE(device->InitContextForEngine(context.get(), device->video_command_streamer()));
        break;
    }

    gpu_addr_t gpu_addr;
    EXPECT_TRUE(cmd_buf_->GetGpuAddress(&gpu_addr));
    EXPECT_EQ(batch_start_offset, gpu_addr & (kPageSize - 1));

    // Check that context is initialized correctly
    EXPECT_TRUE(ctx->IsInitializedForEngine(id));
    EXPECT_NE(ctx->get_ringbuffer(id), nullptr);
    EXPECT_NE(ctx->get_context_buffer(id), nullptr);

    // Check that context is mapped correctly
    gpu_addr_t addr;
    EXPECT_TRUE(ctx->GetGpuAddress(id, &addr));
    EXPECT_NE(addr, kInvalidGpuAddr);
    EXPECT_TRUE(ctx->GetRingbufferGpuAddress(id, &addr));
    EXPECT_NE(addr, kInvalidGpuAddr);
    cmd_buf_.reset();
  }

  void TestExecute(uint64_t command_buffer_flags) {
    helper_->set_command_buffer_flags(command_buffer_flags);

    CreateCommandBuffer();

    gpu_addr_t gpu_addr = 0;
    {
      std::vector<msd::MagmaSystemBuffer*>& resources = helper_->resources();
      for (auto& resource : resources) {
        auto buffer = static_cast<MsdIntelAbiBuffer*>(resource->msd_buf())->ptr();
        std::shared_ptr<GpuMapping> mapping;
        EXPECT_TRUE(AddressSpace::MapBufferGpu(exec_address_space(), buffer, gpu_addr, 0,
                                               buffer->platform_buffer()->size() / kPageSize,
                                               &mapping));
        ASSERT_TRUE(mapping);
        EXPECT_TRUE(exec_address_space()->AddMapping(mapping));
        gpu_addr += buffer->platform_buffer()->size() + PerProcessGtt::ExtraPageCount() * kPageSize;
      }
    }

    // Create target buffer and mapping
    auto buffer = std::shared_ptr<MsdIntelBuffer>(MsdIntelBuffer::Create(kPageSize, "test"));
    std::shared_ptr<GpuMapping> target_buffer_mapping;
    EXPECT_TRUE(AddressSpace::MapBufferGpu(exec_address_space(), buffer, gpu_addr, 0,
                                           buffer->platform_buffer()->size() / kPageSize,
                                           &target_buffer_mapping));
    ASSERT_TRUE(target_buffer_mapping);
    EXPECT_TRUE(exec_address_space()->AddMapping(target_buffer_mapping));

    void* target_cpu_addr;
    ASSERT_TRUE(target_buffer_mapping->buffer()->platform_buffer()->MapCpu(&target_cpu_addr));

    gpu_addr_t target_gpu_addr = target_buffer_mapping->gpu_addr();
    *reinterpret_cast<uint32_t*>(target_cpu_addr) = 0;

    auto batch_buf_index = TestCommandBuffer::batch_buffer_resource_index(cmd_buf_.get());
    auto batch_res = TestCommandBuffer::exec_resources(cmd_buf_.get())[batch_buf_index];
    void* batch_cpu_addr;

    ASSERT_TRUE(batch_res.buffer->platform_buffer()->MapCpu(&batch_cpu_addr));
    uint32_t expected_val = 0xdeadbeef;
    uint32_t* batch_ptr = reinterpret_cast<uint32_t*>(batch_cpu_addr);

    static constexpr uint32_t kDwordCount = 4;
    static constexpr bool kUseGlobalGtt = false;
    // store dword
    *batch_ptr++ = (0x20 << 23) | (kDwordCount - 2) | (kUseGlobalGtt ? 1 << 22 : 0);
    *batch_ptr++ = magma::lower_32_bits(target_gpu_addr);
    *batch_ptr++ = magma::upper_32_bits(target_gpu_addr);
    *batch_ptr++ = expected_val;

    // batch end
    *batch_ptr++ = (0xA << 23);

    ASSERT_TRUE(device()->StartDeviceThread());

    cmd_buf_.reset();
    EXPECT_TRUE(helper_->ExecuteAndWait());

    uint32_t target_val = *reinterpret_cast<uint32_t*>(target_cpu_addr);
    EXPECT_EQ(target_val, expected_val);
  }

 private:
  void CreateCommandBuffer() {
    msd::magma_command_buffer command_buffer = {
        .resource_count = static_cast<uint32_t>(helper_->resources().size()),
        .batch_buffer_resource_index = helper_->abi_cmd_buf()->resource_index,
        .batch_start_offset = helper_->abi_cmd_buf()->start_offset,
        .wait_semaphore_count = CommandBufferHelper::kWaitSemaphoreCount,
        .signal_semaphore_count = CommandBufferHelper::kSignalSemaphoreCount,
    };

    cmd_buf_ = CommandBuffer::Create(
        static_cast<MsdIntelAbiContext*>(helper_->ctx())->ptr(), &command_buffer,
        helper_->abi_resources(), helper_->msd_resources().data(), helper_->msd_wait_semaphores(),
        helper_->msd_signal_semaphores());
  }

  void SetUp() override { helper_ = CommandBufferHelper::Create(); }

  std::unique_ptr<msd::MagmaSystemBuffer> buffer_;
  std::unique_ptr<CommandBuffer> cmd_buf_;
  std::unique_ptr<CommandBufferHelper> helper_;
};

TEST_F(TestHwCommandBuffer, MapUnmapResourcesGpu) {
  TestHwCommandBuffer::TestMapUnmapResourcesGpu();
}

class TestHwCommandBuffer_RenderCommandStreamer : public TestHwCommandBuffer {};

TEST_F(TestHwCommandBuffer_RenderCommandStreamer, PrepareForExecution) {
  TestHwCommandBuffer::TestPrepareForExecution(RENDER_COMMAND_STREAMER);
}

TEST_F(TestHwCommandBuffer_RenderCommandStreamer, Execute) {
  TestHwCommandBuffer::TestExecute(kMagmaIntelGenCommandBufferForRender);
}

class TestHwCommandBuffer_VideoCommandStreamer : public TestHwCommandBuffer {};

TEST_F(TestHwCommandBuffer_VideoCommandStreamer, PrepareForExecution) {
  TestHwCommandBuffer::TestPrepareForExecution(VIDEO_COMMAND_STREAMER);
}

TEST_F(TestHwCommandBuffer_VideoCommandStreamer, Execute) {
  TestHwCommandBuffer::TestExecute(kMagmaIntelGenCommandBufferForVideo);
}
