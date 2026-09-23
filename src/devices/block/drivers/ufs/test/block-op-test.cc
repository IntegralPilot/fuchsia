// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <bitset>
#include <future>

#include "src/storage/lib/block_client/cpp/remote_block_device.h"
#include "unit-lib.h"

namespace ufs {
class BlockOpTest : public UfsTest {
 public:
  void SetUp() override {
    UfsTest::SetUp();

    while (true) {
      {
        std::lock_guard<std::mutex> lock(dut_->lock());
        if (!dut_->block_devs().empty() && dut_->block_devs().contains(0) &&
            !dut_->block_devs().at(0).empty()) {
          block_device_ = dut_->block_devs().at(0).at(0).get();
          break;
        }
      }
      zx::nanosleep(zx::deadline_after(zx::msec(1)));
    }
    EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
      auto client_end = driver_test().Connect<fuchsia_hardware_block_volume::Service::Volume>(
          block_device_->DeviceName().c_str());
      ASSERT_OK(client_end);
      auto remote_device_result =
          block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
      ASSERT_OK(remote_device_result);
      client_ = std::move(remote_device_result.value());

      ASSERT_OK(client_->BlockGetInfo(&info_));
    }));
  }

 protected:
  scsi::BlockDevice* block_device_;
  std::unique_ptr<block_client::RemoteBlockDevice> client_;
  fuchsia_storage_block::wire::BlockInfo info_;
};

static void FillRandom(uint8_t* buf, size_t size) {
  for (size_t i = 0; i < size; ++i) {
    buf[i] = static_cast<uint8_t>(rand());
  }
}

TEST_F(BlockOpTest, ReadTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    const uint8_t kTestLun = 0;

    char buf[ufs_mock_device::kMockBlockSize];
    std::strncpy(buf, "test", sizeof(buf));
    ASSERT_OK(mock_device_.BufferWrite(kTestLun, buf, 1, 0));

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&request, 1));

    zx_vaddr_t vaddr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ, 0, vmo, 0,
                                         ufs_mock_device::kMockBlockSize, &vaddr));
    char* mapped_vaddr = reinterpret_cast<char*>(vaddr);
    ASSERT_EQ(std::memcmp(buf, mapped_vaddr, ufs_mock_device::kMockBlockSize), 0);
    ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, ufs_mock_device::kMockBlockSize));
    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, WriteTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    const uint8_t kTestLun = 0;

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));

    zx_vaddr_t vaddr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                         ufs_mock_device::kMockBlockSize, &vaddr));
    char* mapped_vaddr = reinterpret_cast<char*>(vaddr);
    std::strncpy(mapped_vaddr, "test", ufs_mock_device::kMockBlockSize);

    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_WRITE},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&request, 1));

    char buf[ufs_mock_device::kMockBlockSize];
    ASSERT_OK(mock_device_.BufferRead(kTestLun, buf, 1, 0));
    ASSERT_EQ(std::memcmp(mapped_vaddr, buf, ufs_mock_device::kMockBlockSize), 0);
    ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, ufs_mock_device::kMockBlockSize));
    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, FuaWriteTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    const uint8_t kTestLun = 0;

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));

    zx_vaddr_t vaddr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                         ufs_mock_device::kMockBlockSize, &vaddr));
    char* mapped_vaddr = reinterpret_cast<char*>(vaddr);
    std::strncpy(mapped_vaddr, "test", ufs_mock_device::kMockBlockSize);

    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    BlockFifoRequest request = {
        .command =
            {
                .opcode = BLOCK_OPCODE_WRITE,
                .flags = BLOCK_IO_FLAG_FORCE_ACCESS,
            },
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&request, 1));

    // Check that the FUA bit is set.
    ScsiCommandUpiu scsi_upiu([&]() {
      std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
      return *dut_->GetTransferRequestProcessor()
                  .GetRequestListLocked()
                  .GetDescriptorBuffer<CommandUpiuData>(0);
    }());
    scsi::Write10CDB* scsi_cdb =
        reinterpret_cast<scsi::Write10CDB*>(scsi_upiu.GetData<CommandUpiuData>()->cdb);
    ASSERT_EQ(scsi_cdb->force_unit_access(), true);

    char buf[ufs_mock_device::kMockBlockSize];
    ASSERT_OK(mock_device_.BufferRead(kTestLun, buf, 1, 0));

    ASSERT_EQ(std::memcmp(buf, mapped_vaddr, ufs_mock_device::kMockBlockSize), 0);
    ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, ufs_mock_device::kMockBlockSize));
    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, FlushTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_FLUSH},
        .vmoid = BLOCK_VMOID_INVALID,
        .length = 0,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&request, 1));

    // Check that the FLUSH operation is correctly converted to a SYNCHRONIZE CACHE 10 command.
    ScsiCommandUpiu scsi_upiu([&]() {
      std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
      return *dut_->GetTransferRequestProcessor()
                  .GetRequestListLocked()
                  .GetDescriptorBuffer<CommandUpiuData>(0);
    }());
    ASSERT_EQ(scsi_upiu.GetOpcode(), scsi::Opcode::SYNCHRONIZE_CACHE_10);
  }));
}

TEST_F(BlockOpTest, TrimTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    const uint8_t kTestLun = 0;

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));

    zx_vaddr_t vaddr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                         ufs_mock_device::kMockBlockSize, &vaddr));
    char* mapped_vaddr = reinterpret_cast<char*>(vaddr);
    std::strncpy(mapped_vaddr, "test", ufs_mock_device::kMockBlockSize);

    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    BlockFifoRequest write_request = {
        .command = {.opcode = BLOCK_OPCODE_WRITE},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&write_request, 1));

    char buf[ufs_mock_device::kMockBlockSize];
    ASSERT_OK(mock_device_.BufferRead(kTestLun, buf, 1, 0));
    ASSERT_EQ(std::memcmp(buf, mapped_vaddr, ufs_mock_device::kMockBlockSize), 0);

    BlockFifoRequest trim_request = {
        .command = {.opcode = BLOCK_OPCODE_TRIM},
        .vmoid = BLOCK_VMOID_INVALID,
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&trim_request, 1));

    // Check that the trimmed block is zero.
    ASSERT_OK(mock_device_.BufferRead(kTestLun, buf, 1, 0));

    char zero_buf[ufs_mock_device::kMockBlockSize];
    std::memset(zero_buf, 0, ufs_mock_device::kMockBlockSize);
    ASSERT_EQ(std::memcmp(buf, zero_buf, ufs_mock_device::kMockBlockSize), 0);

    ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, ufs_mock_device::kMockBlockSize));
    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, IoRangeExceptionTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    // Normal I/O. No errors occur.
    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    ASSERT_OK(client_->FifoTransaction(&request, 1));

    // If the I/O length is zero, an invalid args error occurs.
    request.length = 0;
    request.dev_offset = 0;
    EXPECT_EQ(client_->FifoTransaction(&request, 1), ZX_ERR_INVALID_ARGS);

    // If the I/O length exceeds the total block count, an I/O range error occurs.
    request.length = static_cast<uint32_t>(info_.block_count) + 1;
    request.dev_offset = 0;
    EXPECT_EQ(client_->FifoTransaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

    // If the request offset does not fit within total block count, an I/O range error occurs.
    request.length = 1;
    request.dev_offset = static_cast<uint32_t>(info_.block_count);
    EXPECT_EQ(client_->FifoTransaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

    // If the request offset and length does not fit within total block count, an I/O range error
    // occurs.
    request.length = 2;
    request.dev_offset = static_cast<uint32_t>(info_.block_count) - 1;
    EXPECT_EQ(client_->FifoTransaction(&request, 1), ZX_ERR_OUT_OF_RANGE);

    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, TransferSizeTest) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    const uint8_t kTestLun = 0;

    ASSERT_EQ(kMaxTransferSize1MiB, info_.max_transfer_size);

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(kMaxTransferSize1MiB, 0, &vmo));

    zx_vaddr_t vaddr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo, 0,
                                         kMaxTransferSize1MiB, &vaddr));
    uint8_t* mapped_vaddr = reinterpret_cast<uint8_t*>(vaddr);
    FillRandom(mapped_vaddr, kMaxTransferSize1MiB);

    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    auto buffer = std::make_unique<uint8_t[]>(kMaxTransferSize1MiB);
    const uint32_t max_block_count = kMaxTransferSize1MiB / ufs_mock_device::kMockBlockSize;

    // Test on 4KiB, 8KiB, 16KiB, 32KiB, 64KiB, 128KiB, 256KiB, 512KiB, and 1MiB transfer length.
    for (uint32_t block_count = 1; block_count <= max_block_count; block_count *= 2) {
      BlockFifoRequest request = {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .vmoid = owned_vmoid.get(),
          .length = block_count,
          .vmo_offset = 0,
          .dev_offset = 0,
      };
      ASSERT_OK(client_->FifoTransaction(&request, 1));

      std::memset(buffer.get(), 0, kMaxTransferSize1MiB);
      EXPECT_OK(mock_device_.BufferRead(kTestLun, buffer.get(), block_count, 0));

      EXPECT_EQ(
          std::memcmp(buffer.get(), mapped_vaddr, block_count * ufs_mock_device::kMockBlockSize),
          0);
    }

    ASSERT_OK(zx::vmar::root_self()->unmap(vaddr, kMaxTransferSize1MiB));
    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, MultiQueueDepthWriteTest) {
  const uint8_t kTestLun = 0;

  // Test on 1, 2, 4, 8, 16, and 31 queue depth.
  // One of the 32 slots is dedicated to the admin command, so the maximum queue depth is 31.
  std::vector<uint8_t> queue_depth_list{1, 2, 4, 8, 16, 31};
  for (auto queue_depth : queue_depth_list) {
    // Disable IoLoop completion
    dut_->GetTransferRequestProcessor().DisableCompletion();
    dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));

    auto vmos = std::make_unique<zx::vmo[]>(queue_depth);
    auto vaddrs = std::make_unique<zx_vaddr_t[]>(queue_depth);
    std::vector<storage::Vmoid> vmoids(queue_depth);
    std::vector<BlockFifoRequest> requests(queue_depth);

    for (uint32_t i = 0; i < queue_depth; ++i) {
      ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmos[i]));
      ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmos[i], 0,
                                           ufs_mock_device::kMockBlockSize, &vaddrs[i]));
      uint8_t* mapped_vaddr = reinterpret_cast<uint8_t*>(vaddrs[i]);
      FillRandom(mapped_vaddr, ufs_mock_device::kMockBlockSize);

      EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync(
          [&]() { ASSERT_OK(client_->BlockAttachVmo(vmos[i], &vmoids[i])); }));
      requests[i] = {
          .command = {.opcode = BLOCK_OPCODE_WRITE},
          .reqid = i,
          .group = 0,
          .vmoid = vmoids[i].get(),
          .length = 1,
          .vmo_offset = 0,
          .dev_offset = i,
      };
    }

    std::future<zx_status_t> tx_future = std::async(std::launch::async, [&]() {
      return client_->FifoTransaction(requests.data(), queue_depth);
    });

    // Wait until the slot is used up to the desired queue depth.
    constexpr zx::duration kMultiQueueTimeout = zx::sec(30);
    auto wait_for_scheduled = [&]() -> bool {
      driver_test().runtime().RunUntilIdle();
      return GetSlotStateCount(SlotState::kScheduled) == queue_depth;
    };
    fbl::String submission_timeout_message = "Timeout waiting for submission";
    ASSERT_OK(dut_->WaitWithTimeout(wait_for_scheduled, kMultiQueueTimeout,
                                    submission_timeout_message, zx::msec(100)));

    // Wait for mock device write I/O to be completed.
    auto wait_for_completion = [&]() -> bool {
      driver_test().runtime().RunUntilIdle();
      std::bitset<kMaxRequestListSize> notification =
          UtrListCompletionNotificationReg::Get().ReadFrom(&dut_->GetMmio()).notification();
      return notification.count() == queue_depth;
    };
    fbl::String completion_timeout_message = "Timeout waiting for completion";
    ASSERT_OK(dut_->WaitWithTimeout(wait_for_completion, kMultiQueueTimeout,
                                    completion_timeout_message, zx::msec(100)));

    dut_->GetTransferRequestProcessor().EnableCompletion();

    // Wait for request slots to be freed.
    auto wait_for_slots_freed = [&]() -> bool {
      driver_test().runtime().RunUntilIdle();
      return GetSlotStateCount(SlotState::kFree) ==
             dut_->GetTransferRequestProcessor().GetSlotCount();
    };
    fbl::String slots_freed_timeout_message = "Timeout waiting for slots to be freed";
    ASSERT_OK(dut_->WaitWithTimeout(wait_for_slots_freed, kMultiQueueTimeout,
                                    slots_freed_timeout_message, zx::msec(100)));

    while (tx_future.wait_for(std::chrono::milliseconds(10)) != std::future_status::ready) {
      driver_test().runtime().RunUntilIdle();
    }
    ASSERT_OK(tx_future.get());

    auto buf = std::make_unique<uint8_t[]>(ufs_mock_device::kMockBlockSize * queue_depth);
    ASSERT_OK(mock_device_.BufferRead(kTestLun, buf.get(), queue_depth, 0));

    for (uint32_t i = 0; i < queue_depth; ++i) {
      uint8_t* mapped_vaddr = reinterpret_cast<uint8_t*>(vaddrs[i]);
      ASSERT_EQ(std::memcmp(buf.get() + ufs_mock_device::kMockBlockSize * i, mapped_vaddr,
                            ufs_mock_device::kMockBlockSize),
                0);
      ASSERT_OK(zx::vmar::root_self()->unmap(vaddrs[i], ufs_mock_device::kMockBlockSize));
      EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync(
          [&]() { ASSERT_OK(client_->BlockDetachVmo(std::move(vmoids[i]))); }));
      vmos[i].reset();
    }
  }
}

TEST_F(BlockOpTest, ReadFailureCompletesWithoutAssertion) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    // Emulates a failure in SCSI READ_10.
    mock_device_.GetScsiCommandProcessor().SetHook(
        scsi::Opcode::READ_10, [](ufs_mock_device::UfsMockDevice& mock_device,
                                  CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
                                  cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius) {
          return zx::error(ZX_ERR_IO);
        });

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    EXPECT_NE(client_->FifoTransaction(&request, 1), ZX_OK);

    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
    mock_device_.GetScsiCommandProcessor().Reset();
  }));
}

TEST_F(BlockOpTest, WriteFailureCompletesWithoutAssertion) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    // Emulates a failure in SCSI WRITE_10.
    mock_device_.GetScsiCommandProcessor().SetHook(
        scsi::Opcode::WRITE_10, [](ufs_mock_device::UfsMockDevice& mock_device,
                                   CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
                                   cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius) {
          return zx::error(ZX_ERR_IO);
        });

    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid));

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_WRITE},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    EXPECT_NE(client_->FifoTransaction(&request, 1), ZX_OK);

    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
    mock_device_.GetScsiCommandProcessor().Reset();
  }));
}

TEST_F(BlockOpTest, ReadPinFailureCompletesWithoutAssertion) {
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync([&]() {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
    // A read operation transfers data from the device to host memory via DMA, which requires
    // ZX_RIGHT_WRITE on the VMO for BTI pinning. A read-only VMO will cause pinning to fail.
    zx::vmo ro_vmo;
    ASSERT_OK(vmo.duplicate(ZX_RIGHT_READ | ZX_RIGHT_MAP | ZX_RIGHT_TRANSFER | ZX_RIGHT_DUPLICATE,
                            &ro_vmo));
    storage::Vmoid owned_vmoid;
    ASSERT_OK(client_->BlockAttachVmo(ro_vmo, &owned_vmoid));

    BlockFifoRequest request = {
        .command = {.opcode = BLOCK_OPCODE_READ},
        .vmoid = owned_vmoid.get(),
        .length = 1,
        .vmo_offset = 0,
        .dev_offset = 0,
    };
    EXPECT_NE(client_->FifoTransaction(&request, 1), ZX_OK);

    ASSERT_OK(client_->BlockDetachVmo(std::move(owned_vmoid)));
  }));
}

TEST_F(BlockOpTest, StopWithPendingRequest) {
  // 1. Reserve all available transfer slots so that new block I/O requests
  // cannot obtain a slot and must remain in |pending_commands_|.
  const uint8_t max_slot_count =
      dut_->GetTransferRequestProcessor().GetSlotCount() - kAdminCommandSlotCount;
  for (uint8_t slot_num = 0; slot_num < max_slot_count; ++slot_num) {
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
  }

  // 2. Attach a VMO to the block client.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));
  storage::Vmoid owned_vmoid;
  EXPECT_OK(driver_test().RunOnBackgroundDispatcherSync(
      [&]() { ASSERT_OK(client_->BlockAttachVmo(vmo, &owned_vmoid)); }));

  // 3. Issue a block write request asynchronously.
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_WRITE},
      .vmoid = owned_vmoid.get(),
      .length = 1,
      .vmo_offset = 0,
      .dev_offset = 0,
  };

  std::future<zx_status_t> tx_future =
      std::async(std::launch::async, [&]() { return client_->FifoTransaction(&request, 1); });

  // Wait until the request has been queued into pending_commands_.
  while (dut_->GetPendingCommandsCount() == 0) {
    zx::nanosleep(zx::deadline_after(zx::msec(1)));
  }

  // 4. Stop the driver. Stop() drains pending_commands_ first (completing the pending
  // request with ZX_ERR_IO_NOT_PRESENT) before calling BlockDevice::ShutdownAsync().
  // If ShutdownAsync() was called first, it would hang indefinitely waiting for the pending
  // request to finish.
  driver_stopped_ = true;
  EXPECT_OK(driver_test().StopDriver());

  // The transaction should have completed with an error.
  EXPECT_NE(tx_future.get(), ZX_OK);

  // Release VMO ID since the block server is already destroyed.
  (void)owned_vmoid.TakeId();
}
}  // namespace ufs
