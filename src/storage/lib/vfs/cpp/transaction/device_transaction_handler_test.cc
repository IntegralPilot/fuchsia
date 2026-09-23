// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/transaction/device_transaction_handler.h"

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <vector>

#include <gtest/gtest.h>

#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/operation/operation.h"

#if __has_feature(address_sanitizer) || __has_feature(leak_sanitizer) || \
    __has_feature(hwaddress_sanitizer)
#include <sanitizer/lsan_interface.h>
#endif

namespace {

using storage::BlockBuffer;
using storage::BufferedOperation;
using storage::Operation;
using storage::OperationType;

// Number of device blocks per operation block.
constexpr uint32_t kBlockRatio = 2;

constexpr uint32_t kBlockSize = 512;
constexpr uint32_t kNumBlocks = 64;

class MockBlockDevice : public block_client::FakeBlockDevice {
 public:
  MockBlockDevice() : FakeBlockDevice(kNumBlocks, kBlockSize) {}
  ~MockBlockDevice() override {}

  const std::vector<BlockFifoRequest>& requests() const { return requests_; }

  zx_status_t FifoTransaction(BlockFifoRequest* requests, size_t count) final {
    if (called_) {
      return ZX_ERR_IO_REFUSED;
    }
    called_ = true;
    requests_.assign(requests, requests + count);
    return ZX_OK;
  }

 private:
  std::vector<BlockFifoRequest> requests_;
  bool called_ = false;
};

class MockTransactionHandler : public fs::DeviceTransactionHandler {
 public:
  MockTransactionHandler() = default;
  ~MockTransactionHandler() override {}

  const std::vector<BlockFifoRequest>& GetRequests() const { return device_.requests(); }

  uint64_t BlockNumberToDevice(uint64_t block_num) const final { return block_num * kBlockRatio; }

  zx_status_t RunOperation(const Operation& operation, BlockBuffer* buffer) final {
    return ZX_ERR_NOT_SUPPORTED;
  }

  block_client::BlockDevice* GetDevice() final { return &device_; }

 private:
  MockBlockDevice device_;
};

class TransactionHandlerTest : public testing::Test {
 public:
  void SetUp() final { handler_ = std::make_unique<MockTransactionHandler>(); }

 protected:
  std::unique_ptr<MockTransactionHandler> handler_;
};

TEST_F(TransactionHandlerTest, RunRequestsNoRequests) {
  std::vector<BufferedOperation> operations;
  EXPECT_EQ(handler_->RunRequests(operations), ZX_OK);
  EXPECT_EQ(0u, handler_->GetRequests().size());
}

TEST_F(TransactionHandlerTest, RunRequestsOneRequest) {
  const vmoid_t kVmoid = 4;
  std::vector<BufferedOperation> operations = {{kVmoid, {OperationType::kWrite, 1, 2, 3}}};
  EXPECT_EQ(handler_->RunRequests(operations), ZX_OK);

  const std::vector<BlockFifoRequest>& requests = handler_->GetRequests();
  EXPECT_EQ(1u, requests.size());
  EXPECT_EQ(1 * kBlockRatio, requests[0].vmo_offset);
  EXPECT_EQ(2 * kBlockRatio, requests[0].dev_offset);
  EXPECT_EQ(3 * kBlockRatio, requests[0].length);
  EXPECT_EQ(kVmoid, requests[0].vmoid);
  EXPECT_EQ(unsigned{BLOCK_OPCODE_WRITE}, requests[0].command.opcode);
}

TEST_F(TransactionHandlerTest, RunRequestsTrim) {
  const vmoid_t kVmoid = 4;
  std::vector<BufferedOperation> operations = {{kVmoid, {OperationType::kTrim, 1, 2, 3}}};
  EXPECT_EQ(handler_->RunRequests(operations), ZX_OK);

  const std::vector<BlockFifoRequest>& requests = handler_->GetRequests();
  EXPECT_EQ(1u, requests.size());
  EXPECT_EQ(1 * kBlockRatio, requests[0].vmo_offset);
  EXPECT_EQ(2 * kBlockRatio, requests[0].dev_offset);
  EXPECT_EQ(3 * kBlockRatio, requests[0].length);
  EXPECT_EQ(kVmoid, requests[0].vmoid);
  EXPECT_EQ(unsigned{BLOCK_OPCODE_TRIM}, requests[0].command.opcode);
}

TEST_F(TransactionHandlerTest, RunRequestsManyRequests) {
  std::vector<BufferedOperation> operations;
  operations.push_back({10, {OperationType::kRead, 11, 12, 13}});
  operations.push_back({20, {OperationType::kRead, 24, 25, 26}});
  operations.push_back({30, {OperationType::kRead, 37, 38, 39}});
  EXPECT_EQ(handler_->RunRequests(operations), ZX_OK);

  const std::vector<BlockFifoRequest>& requests = handler_->GetRequests();
  EXPECT_EQ(3u, requests.size());
  EXPECT_EQ(unsigned{BLOCK_OPCODE_READ}, requests[0].command.opcode);
  EXPECT_EQ(10, requests[0].vmoid);
  EXPECT_EQ(11 * kBlockRatio, requests[0].vmo_offset);
  EXPECT_EQ(12 * kBlockRatio, requests[0].dev_offset);
  EXPECT_EQ(13 * kBlockRatio, requests[0].length);

  EXPECT_EQ(unsigned{BLOCK_OPCODE_READ}, requests[1].command.opcode);
  EXPECT_EQ(20u, requests[1].vmoid);
  EXPECT_EQ(24 * kBlockRatio, requests[1].vmo_offset);
  EXPECT_EQ(25 * kBlockRatio, requests[1].dev_offset);
  EXPECT_EQ(26 * kBlockRatio, requests[1].length);

  EXPECT_EQ(unsigned{BLOCK_OPCODE_READ}, requests[2].command.opcode);
  EXPECT_EQ(30u, requests[2].vmoid);
  EXPECT_EQ(37 * kBlockRatio, requests[2].vmo_offset);
  EXPECT_EQ(38 * kBlockRatio, requests[2].dev_offset);
  EXPECT_EQ(39 * kBlockRatio, requests[2].length);
}

TEST_F(TransactionHandlerTest, RunRequestsFails) {
  std::vector<BufferedOperation> operations = {{0, {OperationType::kWrite, 1, 2, 3}}};
  EXPECT_EQ(handler_->RunRequests(operations), ZX_OK);

  EXPECT_NE(handler_->RunRequests(operations), ZX_OK);
}

TEST_F(TransactionHandlerTest, FlushCallsFlush) {
  handler_->Flush();
  const std::vector<BlockFifoRequest>& requests = handler_->GetRequests();
  EXPECT_EQ(1u, requests.size());
  EXPECT_EQ(unsigned{BLOCK_OPCODE_FLUSH}, requests[0].command.opcode);
}

TEST_F(TransactionHandlerTest, RunRequestsWriteFua) {
  const vmoid_t kVmoid = 4;
  std::vector<BufferedOperation> operations = {{kVmoid, {OperationType::kWriteFua, 1, 2, 3}}};
  EXPECT_EQ(handler_->RunRequests(operations), ZX_OK);

  const std::vector<BlockFifoRequest>& requests = handler_->GetRequests();
  EXPECT_EQ(1u, requests.size());
  EXPECT_EQ(1 * kBlockRatio, requests[0].vmo_offset);
  EXPECT_EQ(2 * kBlockRatio, requests[0].dev_offset);
  EXPECT_EQ(3 * kBlockRatio, requests[0].length);
  EXPECT_EQ(kVmoid, requests[0].vmoid);
  EXPECT_EQ(unsigned{BLOCK_OPCODE_WRITE}, requests[0].command.opcode);
  EXPECT_EQ(unsigned{BLOCK_IO_FLAG_FORCE_ACCESS}, requests[0].command.flags);
}

#if ZX_DEBUG_ASSERT_IMPLEMENTED

using TransactionHandlerCrashTest = TransactionHandlerTest;

TEST_F(TransactionHandlerCrashTest, RunRequestsMixedRequests) {
  std::vector<BufferedOperation> operations;
  operations.push_back({10, {OperationType::kRead, 11, 12, 13}});
  operations.push_back({20, {OperationType::kWrite, 24, 25, 26}});
  ASSERT_DEATH(
      {
#if __has_feature(address_sanitizer) || __has_feature(leak_sanitizer) || \
    __has_feature(hwaddress_sanitizer)
        // Disable LSAN, this thread is expected to leak by way of a crash.
        __lsan::ScopedDisabler _;
#endif
        handler_->RunRequests(operations);
      },
      "");
}

#endif  // ZX_DEBUG_ASSERT_IMPLEMENTED

}  // namespace
