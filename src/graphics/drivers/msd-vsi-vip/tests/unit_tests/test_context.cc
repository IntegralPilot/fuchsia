// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma_service/mock/mock_bus_mapper.h>
#include <lib/magma_service/sys_driver/magma_system_buffer.h>
#include <lib/magma_service/sys_driver/magma_system_semaphore.h>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-vsi-vip/src/address_space.h"
#include "src/graphics/drivers/msd-vsi-vip/src/command_buffer.h"
#include "src/graphics/drivers/msd-vsi-vip/src/mapped_batch.h"
#include "src/graphics/drivers/msd-vsi-vip/src/msd_vsi_context.h"
#include "src/graphics/drivers/msd-vsi-vip/src/msd_vsi_driver.h"

// Holds the buffers and semaphores associated with a fake test batch.
class BatchData {
 public:
  static std::pair<std::unique_ptr<MappedBatch>, std::unique_ptr<BatchData>> Create(
      std::shared_ptr<MsdVsiContext> context, uint32_t num_resources) {
    auto driver = std::make_unique<MsdVsiDriver>();
    if (!driver)
      return {};

    std::vector<std::unique_ptr<msd::MagmaSystemSemaphore>> signal_semaphores;
    std::vector<std::unique_ptr<msd::MagmaSystemBuffer>> resources;

    for (unsigned int i = 0; i < num_resources; i++) {
      auto buffer = magma::PlatformBuffer::Create(kResourceSize, "test buffer");
      if (!buffer)
        return {};
      resources.emplace_back(msd::MagmaSystemBuffer::Create(driver.get(), std::move(buffer)));
    }

    for (unsigned int i = 0; i < kNumSignalSemaphores; i++) {
      auto semaphore = magma::PlatformSemaphore::Create();
      if (!semaphore)
        return {};

      uint32_t handle;
      if (!semaphore->duplicate_handle(&handle))
        return {};

      signal_semaphores.emplace_back(msd::MagmaSystemSemaphore::Create(
          driver.get(), zx::event(handle), semaphore->global_id(), /*flags=*/0));
    }

    auto batch_data =
        std::make_unique<BatchData>(std::move(signal_semaphores), std::move(resources));

    return {batch_data->CreateBatch(context), std::move(batch_data)};
  }

  static constexpr uint64_t kResourceSize = 4096;
  static constexpr uint32_t kNumSignalSemaphores = 3;

  BatchData(std::vector<std::unique_ptr<msd::MagmaSystemSemaphore>> signal_semaphores,
            std::vector<std::unique_ptr<msd::MagmaSystemBuffer>> resources)
      : signal_semaphores_(std::move(signal_semaphores)), resources_(std::move(resources)) {}

 private:
  // Returns a new batch created from the BatchData.
  std::unique_ptr<MappedBatch> CreateBatch(std::shared_ptr<MsdVsiContext> context) {
    auto command_buffer = msd::magma_command_buffer{
        .resource_count = static_cast<uint32_t>(resources_.size()),
        .batch_buffer_resource_index = 0,
        .batch_start_offset = 0,
        .wait_semaphore_count = 0,
        .signal_semaphore_count = static_cast<uint32_t>(signal_semaphores_.size()),
        .flags = 0,
    };
    std::vector<magma_exec_resource> resources;
    std::vector<msd::Buffer*> msd_buffers;
    for (auto& buf : resources_) {
      resources.push_back({
          .buffer_id = buf->platform_buffer()->id(),
          .offset = 0,
          .length = kResourceSize,
      });
      msd_buffers.push_back(buf->msd_buf());
    }
    std::vector<msd::Semaphore*> msd_signal_semaphores;
    for (const auto& semaphore : signal_semaphores_) {
      msd_signal_semaphores.emplace_back(semaphore->msd_semaphore());
    }
    return MsdVsiContext::CreateBatch(context, &command_buffer, resources.data(),
                                      msd_buffers.data(), nullptr, msd_signal_semaphores.data());
  }

  std::vector<std::unique_ptr<msd::MagmaSystemSemaphore>> signal_semaphores_;
  std::vector<std::unique_ptr<msd::MagmaSystemBuffer>> resources_;
};

class TestMsdVsiContext : public ::testing::Test {
 public:
  void SetUp() {
    static constexpr uint32_t kAddressSpaceIndex = 1;
    address_space_ = AddressSpace::Create(&mock_address_space_owner_, kAddressSpaceIndex);
    EXPECT_NE(address_space_, nullptr);

    connection_ = std::make_shared<MsdVsiConnection>(&mock_connection_owner_, address_space_,
                                                     0 /* client_id */);
    EXPECT_NE(connection_, nullptr);

    // Buffers are not submitted to hardware, so we just need a mock ringbuffer that the
    // context can map.
    const uint32_t kRingbufferSize = magma::page_size();
    ringbuffer_ = std::make_unique<Ringbuffer>(MsdVsiBuffer::Create(kRingbufferSize, "ringbuffer"));
    EXPECT_NE(ringbuffer_, nullptr);

    context_ = MsdVsiContext::Create(connection_, address_space_, ringbuffer_.get());
    EXPECT_NE(context_, nullptr);
  }

 protected:
  class MockConnectionOwner : public MsdVsiConnection::Owner {
   public:
    // MsdVsiConnection::Owner
    Ringbuffer* GetRingbuffer() override { return nullptr; }

    magma::Status SubmitBatch(std::unique_ptr<MappedBatch> batch, bool do_flush) override {
      submitted_batch_ids_.push_back(batch->GetBatchBufferId());
      if (submitted_batch_ids_.size() == num_expected_batches_) {
        finished_semaphore_->Signal();
      }
      return MAGMA_STATUS_OK;
    }

    // Signals |finished_semaphore| once |num_expected_batches| are received.
    void SetSignalOnCompletion(uint32_t num_expected_batches,
                               std::shared_ptr<magma::PlatformSemaphore> finished_semaphore) {
      submitted_batch_ids_.clear();
      num_expected_batches_ = num_expected_batches;
      finished_semaphore_ = std::move(finished_semaphore);
    }

    std::vector<uint64_t> GetSubmittedBatchIds() { return submitted_batch_ids_; }

   private:
    std::vector<uint64_t> submitted_batch_ids_;
    uint32_t num_expected_batches_ = 0;
    std::shared_ptr<magma::PlatformSemaphore> finished_semaphore_;
  };

  class MockAddressSpaceOwner : public AddressSpace::Owner {
   public:
    MockAddressSpaceOwner() : bus_mapper_((1ul << (20 - 1))) {}

    magma::PlatformBusMapper* GetBusMapper() override { return &bus_mapper_; }

    void AddressSpaceReleased(AddressSpace* address_space) override {}

   private:
    MockBusMapper bus_mapper_;
  };

  // Submits the requested number of batches and verifies that the connection
  // owner receives the same batches.
  void TestSubmitBatches(uint32_t num_batches, uint32_t num_resources_per_batch);

  MockConnectionOwner mock_connection_owner_;
  MockAddressSpaceOwner mock_address_space_owner_;
  std::shared_ptr<AddressSpace> address_space_;
  std::shared_ptr<MsdVsiConnection> connection_;
  std::shared_ptr<MsdVsiContext> context_;

  std::unique_ptr<Ringbuffer> ringbuffer_;
};

void TestMsdVsiContext::TestSubmitBatches(uint32_t num_batches, uint32_t num_resources_per_batch) {
  auto finished_semaphore =
      std::shared_ptr<magma::PlatformSemaphore>(magma::PlatformSemaphore::Create());
  mock_connection_owner_.SetSignalOnCompletion(num_batches, finished_semaphore);

  // Submit the batches and save the batch ids.
  std::vector<std::unique_ptr<BatchData>> batch_data;
  std::vector<uint64_t> batch_ids;
  for (unsigned int i = 0; i < num_batches; i++) {
    auto [batch, data] = BatchData::Create(context_, num_resources_per_batch);
    ASSERT_NE(batch, nullptr);

    ASSERT_EQ(batch->IsCommandBuffer(), num_resources_per_batch > 0);

    batch_data.push_back(std::move(data));
    batch_ids.push_back(batch->GetBatchBufferId());

    context_->SubmitBatch(std::move(batch));
  }

  // Wait for the batches to be received.
  constexpr uint64_t kTimeoutMs = 1000;
  ASSERT_EQ(MAGMA_STATUS_OK, finished_semaphore->Wait(kTimeoutMs).get());

  // Check the correct batch ids were received.
  auto submitted_batch_ids = mock_connection_owner_.GetSubmittedBatchIds();
  ASSERT_EQ(submitted_batch_ids.size(), batch_data.size());
  for (unsigned int i = 0; i < batch_data.size(); i++) {
    ASSERT_EQ(batch_ids[i], submitted_batch_ids[i]);
  }
}

TEST_F(TestMsdVsiContext, SubmitBatchesNoResources) {
  ASSERT_NO_FATAL_FAILURE(TestSubmitBatches(2 /* num_batches */, 0 /* num_resources_per_batch */));
}

TEST_F(TestMsdVsiContext, SubmitBatchesWithResources) {
  ASSERT_NO_FATAL_FAILURE(TestSubmitBatches(5 /* num_batches */, 2 /* num_resources_per_batch */));
}
