// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/nvme/queue-pair.h"

#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fake-bti/bti.h>
#include <lib/mmio-ptr/fake.h>
#include <lib/sync/completion.h>
#include <lib/zx/bti.h>

#include <gtest/gtest.h>

#include "src/devices/block/drivers/nvme/commands.h"
#include "src/devices/block/drivers/nvme/io-command.h"
#include "src/devices/block/drivers/nvme/registers.h"
#include "src/lib/testing/predicates/status.h"

namespace nvme {

class QueuePairTest : public ::testing::Test {
 public:
  void SetUp() override { ASSERT_OK(fake_bti_create(fake_bti_.reset_and_get_address())); }

 protected:
  TransactionData& txn(QueuePair* pair, size_t id) __TA_NO_THREAD_SAFETY_ANALYSIS {
    return pair->txns_[id];
  }

  zx::bti fake_bti_;
  // We only use the capability register for the doorbell stride, so 0 is fine.
  CapabilityReg caps_ = CapabilityReg::Get().FromValue(0);
  fdf::MmioBuffer mmio_ = fdf_testing::CreateMmioBuffer(
      NVME_REG_DOORBELL_BASE * 0x100, ZX_CACHE_POLICY_UNCACHED_DEVICE, &kMmioOps, this);

  // void doorbell_ring(bool is_completion, size_t queue_id, uint16_t value)
  std::function<void(bool, size_t, uint16_t)> doorbell_ring_;

 private:
  static void Write32(const void* ctx, const mmio_buffer_t& mmio, uint32_t val, zx_off_t offs) {
    offs -= NVME_REG_DOORBELL_BASE;
    offs /= 4;
    bool completion_doorbell = (offs & 1);
    size_t queue_id = offs / 2;
    static_cast<const QueuePairTest*>(ctx)->doorbell_ring_(completion_doorbell, queue_id,
                                                           val & 0xffff);
  }

  static uint32_t Read32(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) {
    ZX_ASSERT(false);
  }

// Define read/write for |bits| that just crashes.
#define STUB_IO_OP(bits)                                                                        \
  static void Write##bits(const void* ctx, const mmio_buffer_t& mmio, uint##bits##_t val,       \
                          zx_off_t offs) {                                                      \
    ZX_ASSERT(false);                                                                           \
  }                                                                                             \
  static uint##bits##_t Read##bits(const void* ctx, const mmio_buffer_t& mmio, zx_off_t offs) { \
    ZX_ASSERT(false);                                                                           \
  }

  STUB_IO_OP(64)
  STUB_IO_OP(16)
  STUB_IO_OP(8)
#undef STUB_IO_OP

  static constexpr fdf::MmioBufferOps kMmioOps = {
      .Read8 = Read8,
      .Read16 = Read16,
      .Read32 = Read32,
      .Read64 = Read64,
      .Write8 = Write8,
      .Write16 = Write16,
      .Write32 = Write32,
      .Write64 = Write64,
  };

  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(QueuePairTest, TestSubmit) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  sync_completion_t rang;
  size_t doorbell_ring_count = 0;
  doorbell_ring_ = [&rang, &pair, &doorbell_ring_count](bool is_completion, size_t queue_id,
                                                        uint16_t value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(doorbell_ring_count + 1, value);
    doorbell_ring_count++;
    Submission* submissions = static_cast<Submission*>(pair->submission().head());
    ASSERT_EQ(0x9fu, submissions[value - 1].opcode());
    sync_completion_signal(&rang);
  };

  Submission s(0x9f);
  ASSERT_OK(pair->Submit(s, std::nullopt, 0, 0));

  sync_completion_wait(&rang, ZX_TIME_INFINITE);

  rang = {};
  s.set_opcode(0x9f);
  ASSERT_OK(pair->Submit(s, std::nullopt, 0, 0));
  sync_completion_wait(&rang, ZX_TIME_INFINITE);
}

TEST_F(QueuePairTest, TestCheckCompletionsNothingReady) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());
  Completion* completions = static_cast<Completion*>(pair->completion().head());
  memset(completions, 0, sizeof(*completions));

  doorbell_ring_ = [](bool, size_t, uint16_t) {
    ZX_ASSERT_MSG(false, "Doorbell should not have been rung");
  };

  Completion* completion = nullptr;
  ASSERT_EQ(pair->CheckForNewCompletion(&completion), ZX_ERR_SHOULD_WAIT);
}

TEST_F(QueuePairTest, TestCheckCompletionsOneReady) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(1u, new_value);
  };
  Submission s(0);
  ASSERT_OK(pair->Submit(s, std::nullopt, 0, 0));

  Completion* completions = static_cast<Completion*>(pair->completion().head());
  memset(completions, 0, sizeof(*completions) * pair->completion().entry_count());
  completions[0].set_command_id(0);
  completions[0].set_phase(1);
  completions[0].set_sq_head(0);

  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_TRUE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(1u, new_value);
  };

  Completion* completion = nullptr;
  ASSERT_EQ(pair->CheckForNewCompletion(&completion), ZX_OK);
  ASSERT_TRUE(completion->status_code_type() == StatusCodeType::kGeneric &&
              completion->status_code() == 0);
  pair->RingCompletionDb();
}

TEST_F(QueuePairTest, TestCheckCompletionsMultipleReady) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  uint16_t expected_doorbell = 1;
  doorbell_ring_ = [&expected_doorbell](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(expected_doorbell, new_value);
    expected_doorbell++;
  };
  Submission s(0);
  {
    ASSERT_OK(pair->Submit(s, std::nullopt, 0, 0));
  }
  {
    ASSERT_OK(pair->Submit(s, std::nullopt, 0, 0));
  }

  Completion* completions = static_cast<Completion*>(pair->completion().head());
  memset(completions, 0, sizeof(*completions) * pair->completion().entry_count());
  completions[0].set_command_id(0);
  completions[0].set_phase(1);
  completions[0].set_sq_head(0);
  completions[1].set_command_id(1);
  completions[1].set_phase(1);
  completions[1].set_sq_head(1);

  // Expect only a single ring of the doorbell.
  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_TRUE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(2, new_value);
  };

  Completion* completion = nullptr;
  ASSERT_EQ(pair->CheckForNewCompletion(&completion), ZX_OK);
  ASSERT_TRUE(completion->status_code_type() == StatusCodeType::kGeneric &&
              completion->status_code() == 0);
  ASSERT_EQ(pair->CheckForNewCompletion(&completion), ZX_OK);
  ASSERT_TRUE(completion->status_code_type() == StatusCodeType::kGeneric &&
              completion->status_code() == 0);
  pair->RingCompletionDb();
}

TEST_F(QueuePairTest, TestCheckCompletionsInactiveTransactionClearsIoCmd) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool, size_t, uint16_t) {};

  IoCommand cmd;
  Submission s(0);
  ASSERT_OK(pair->Submit(s, std::nullopt, 0, 0, &cmd));

  Completion* completions = static_cast<Completion*>(pair->completion().head());
  memset(completions, 0, sizeof(*completions) * pair->completion().entry_count());
  // First completion is valid for command_id 0.
  completions[0].set_command_id(0);
  completions[0].set_phase(1);
  completions[0].set_sq_head(0);

  // Second completion is duplicate/inactive for command_id 0.
  completions[1].set_command_id(0);
  completions[1].set_phase(1);
  completions[1].set_sq_head(0);

  Completion* completion = nullptr;
  IoCommand* io_cmd = nullptr;

  // 1st completion should succeed and return &cmd.
  ASSERT_EQ(pair->CheckForNewCompletion(&completion, &io_cmd), ZX_OK);
  EXPECT_EQ(io_cmd, &cmd);

  // 2nd completion is on an inactive transaction. It should return ZX_ERR_BAD_STATE and set
  // io_cmd to nullptr.
  EXPECT_EQ(pair->CheckForNewCompletion(&completion, &io_cmd), ZX_ERR_BAD_STATE);
  EXPECT_EQ(io_cmd, nullptr);
}

TEST_F(QueuePairTest, TestCheckCompletionsInvalidCommandId) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool, size_t, uint16_t) {};

  Completion* completions = static_cast<Completion*>(pair->completion().head());
  memset(completions, 0, sizeof(*completions) * pair->completion().entry_count());

  // Set command_id equal to txns_.size() (the off-by-one boundary).
  completions[0].set_command_id(static_cast<uint16_t>(pair->submission().entry_count()));
  completions[0].set_phase(1);
  completions[0].set_sq_head(0);

  // Set command_id way out of bounds.
  completions[1].set_command_id(0x9999);
  completions[1].set_phase(1);
  completions[1].set_sq_head(0);

  Completion* completion = nullptr;
  IoCommand dummy_cmd;
  IoCommand* io_cmd = &dummy_cmd;

  // Off-by-one boundary test: command_id == txns_.size()
  EXPECT_EQ(pair->CheckForNewCompletion(&completion, &io_cmd), ZX_ERR_BAD_STATE);
  EXPECT_EQ(io_cmd, nullptr);

  io_cmd = &dummy_cmd;
  // Way out of bounds test
  EXPECT_EQ(pair->CheckForNewCompletion(&completion, &io_cmd), ZX_ERR_BAD_STATE);
  EXPECT_EQ(io_cmd, nullptr);
}

TEST_F(QueuePairTest, TestSubmitWithDataOnePage) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(1u, new_value);
  };
  zx::vmo data_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &data_vmo));
  Submission s(0xa9);
  {
    ASSERT_OK(pair->Submit(s, data_vmo.borrow(), 0, zx_system_get_page_size()));
  }

  Submission* submitted = static_cast<Submission*>(pair->submission().head());
  ASSERT_EQ(0u, submitted->data_transfer_mode());
  ASSERT_EQ(0u, submitted->fused());
  ASSERT_EQ(0xa9u, submitted->opcode());
  ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, submitted->data_pointer[0]);
  ASSERT_EQ(0u, submitted->data_pointer[1]);
  TransactionData& txn_data = txn(pair.value().get(), 0);
  ASSERT_FALSE(txn_data.prp_buffer);
  ASSERT_TRUE(txn_data.active);
}

TEST_F(QueuePairTest, TestSubmitWithDataTwoPages) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(1u, new_value);
  };
  zx::vmo data_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &data_vmo));
  Submission s(0xa9);
  {
    ASSERT_OK(pair->Submit(s, data_vmo.borrow(), 0, zx_system_get_page_size()));
  }

  Submission* submitted = static_cast<Submission*>(pair->submission().head());
  ASSERT_EQ(0u, submitted->data_transfer_mode());
  ASSERT_EQ(0u, submitted->fused());
  ASSERT_EQ(0xa9u, submitted->opcode());
  ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, submitted->data_pointer[0]);
  ASSERT_EQ(0u, submitted->data_pointer[1]);
  TransactionData& txn_data = txn(pair.value().get(), 0);
  ASSERT_FALSE(txn_data.prp_buffer);
  ASSERT_TRUE(txn_data.active);
}

// Not using QueuePair::PreparePrpList() for now. See QueuePair::kMaxTransferPages.
TEST_F(QueuePairTest, DISABLED_TestSubmitWithDataManyPages) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(1u, new_value);
  };
  zx::vmo data_vmo;
  constexpr size_t kNumPages = 4;
  ASSERT_OK(zx::vmo::create(kNumPages * zx_system_get_page_size(), 0, &data_vmo));
  Submission s(0xa9);
  {
    ASSERT_OK(pair->Submit(s, data_vmo.borrow(), 0, kNumPages * zx_system_get_page_size()));
  }

  Submission* submitted = static_cast<Submission*>(pair->submission().head());
  ASSERT_EQ(0u, submitted->data_transfer_mode());
  ASSERT_EQ(0u, submitted->fused());
  ASSERT_EQ(0xa9u, submitted->opcode());
  ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, submitted->data_pointer[0]);
  ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, submitted->data_pointer[1]);
  TransactionData& txn_data = txn(pair.value().get(), 0);
  ASSERT_TRUE(txn_data.prp_buffer);
  ASSERT_TRUE(txn_data.active);
  uint64_t* prps = static_cast<uint64_t*>(txn_data.prp_buffer->virt());
  for (size_t i = 0; i < kNumPages - 1; i++) {
    ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, prps[i]);
  }
  ASSERT_EQ(0u, prps[kNumPages - 1]);
}

// Not using QueuePair::PreparePrpList() for now. See QueuePair::kMaxTransferPages.
TEST_F(QueuePairTest, DISABLED_TestSubmitWithMultiPagePrp) {
  auto pair = QueuePair::Create(fake_bti_.borrow(), 0, 100, caps_, mmio_, /*prealloc_prp=*/false);
  ASSERT_OK(pair.status_value());

  doorbell_ring_ = [](bool is_completion, size_t queue_id, uint16_t new_value) {
    ASSERT_FALSE(is_completion);
    ASSERT_EQ(0u, queue_id);
    ASSERT_EQ(1u, new_value);
  };
  zx::vmo data_vmo;
  const size_t addr_per_page = zx_system_get_page_size() / sizeof(uint64_t);
  const size_t kNumAddresses = addr_per_page + 10;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size() * kNumAddresses, 0, &data_vmo));
  Submission s(0xa9);
  {
    ASSERT_OK(pair->Submit(s, data_vmo.borrow(), 0, zx_system_get_page_size() * kNumAddresses));
  }

  Submission* submitted = static_cast<Submission*>(pair->submission().head());
  ASSERT_EQ(0u, submitted->data_transfer_mode());
  ASSERT_EQ(0u, submitted->fused());
  ASSERT_EQ(0xa9u, submitted->opcode());
  ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, submitted->data_pointer[0]);
  ASSERT_EQ(uint64_t{FAKE_BTI_PHYS_ADDR}, submitted->data_pointer[1]);
  TransactionData& txn_data = txn(pair.value().get(), 0);
  ASSERT_TRUE(txn_data.prp_buffer);
  ASSERT_TRUE(txn_data.active);
  uint64_t* prps = static_cast<uint64_t*>(txn_data.prp_buffer->virt());
  for (size_t i = 0; i < kNumAddresses; i++) {
    ZX_ASSERT_MSG(FAKE_BTI_PHYS_ADDR == prps[i], "PRP %zu had wrong value", i);
  }
  ASSERT_EQ(0u, prps[kNumAddresses]);
}
}  // namespace nvme
