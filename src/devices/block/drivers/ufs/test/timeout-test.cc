// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>

#include <cstdint>
#include <memory>

#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"
#include "src/devices/block/drivers/ufs/upiu/descriptors.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"
#include "zircon/errors.h"

namespace ufs {
using namespace ufs_mock_device;

constexpr zx::duration kWaitLimit = zx::sec(30);

using TimeoutTest = UfsTest;

TEST_F(TimeoutTest, GetEarliestTimeoutDeadline) {
  constexpr uint8_t kTestLun = 0;
  const uint8_t kMaxSlotCount =
      dut_->GetTransferRequestProcessor().GetSlotCount() - kAdminCommandSlotCount;

  // Disable IoLoop completion
  dut_->GetTransferRequestProcessor().DisableCompletion();

  // If there is no in-flight I/O, ZX_TIME_INFINITE should be returned.
  {
    zx_time_t deadline = dut_->GetTransferRequestProcessor().GetEarliestTimeoutDeadline();
    ASSERT_EQ(ZX_TIME_INFINITE, deadline);
  }

  // If there are in-flight I/Os, it should return the earliest timeout deadline.
  {
    uint8_t cdb_buffer[6] = {};
    auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB*>(cdb_buffer);
    cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

    ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
    for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
      zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
      ASSERT_OK(slot);
      dut_->GetTransferRequestProcessor().SendIoScsiCmd(
          upiu, kTestLun, slot.value(), zx::unowned_vmo(), 0, 0, [](zx_status_t status) {});
    }

    // Request in slot 0 is the earliest issued request.
    zx_time_t slot_0_deadline;
    {
      std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
      slot_0_deadline =
          dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(0).deadline;
      for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
        EXPECT_LE(
            slot_0_deadline,
            dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).deadline);
      }
    }

    // Wait 100 ms for outstanding send requests to complete.
    usleep(100000);

    zx_time_t deadline = dut_->GetTransferRequestProcessor().GetEarliestTimeoutDeadline();
    ASSERT_EQ(slot_0_deadline, deadline);

    dut_->GetTransferRequestProcessor().EnableCompletion();
    ASSERT_EQ(dut_->GetTransferRequestProcessor().ProcessCompletionOfIoRequests(), kMaxSlotCount);
  }
}

TEST_F(TimeoutTest, AsyncCommandTimeout) {
  constexpr uint8_t kTestLun = 0;
  constexpr uint8_t target_task_tag = 0;

  auto lun_id = Ufs::TranslateScsiLunToUfsLun(kTestLun);
  ASSERT_OK(lun_id);

  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));

  uint8_t cdb_buffer[16] = {};
  uint8_t cdb_length;
  auto cdb = reinterpret_cast<scsi::Read10CDB*>(cdb_buffer);
  cdb_length = 10;
  cdb->opcode = scsi::Opcode::READ_10;
  cdb->logical_block_address = 0;
  cdb->transfer_length = 0;
  cdb->set_force_unit_access(false);
  ZX_ASSERT(cdb_length <= sizeof(cdb_buffer));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmo));

  // Emulates a timeout situation. Hook the SCSI command handler to set a response timeout.
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::READ_10,
      [](UfsMockDevice& mock_device, CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius) {
        return zx::error(ZX_ERR_TIMED_OUT);
      });

  ScsiCommandUpiu upiu(cdb_buffer, cdb_length, DataDirection::kDeviceToHost,
                       ufs_mock_device::kMockBlockSize);
  zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
  ASSERT_OK(slot);
  ASSERT_EQ(slot.value(), target_task_tag);
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(upiu, kTestLun, slot.value(), vmo.borrow(), 0,
                                                    ufs_mock_device::kMockBlockSize,
                                                    [](zx_status_t status) {});

  auto wait_for = [&]() -> bool {
    dut_->ProcessIoCompletions();
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor()
               .GetRequestListLocked()
               .GetSlot(target_task_tag)
               .state == SlotState::kTimeout;
  };
  fbl::String timeout_message = "Timeout waiting for SCSI command timeout";
  ASSERT_OK(dut_->WaitWithTimeout(wait_for, kWaitLimit, timeout_message, zx::msec(100)));

  // Check that the timed out command is aborted and not in the request list
  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    ASSERT_EQ(
        dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(target_task_tag).state,
        SlotState::kTimeout);
  }
  // Check that the doorbell bit was cleared via UTRLCLR for the timed-out slot.
  ASSERT_EQ(
      UtrListDoorBellReg::Get().ReadFrom(&dut_->GetMmio()).door_bell() & (1u << target_task_tag),
      0u);
  mock_device_.GetScsiCommandProcessor().Reset();
}

TEST_F(TimeoutTest, AllAsyncCommandsTimeout) {
  constexpr uint8_t kTestLun = 0;
  const uint8_t kMaxSlotCount =
      dut_->GetTransferRequestProcessor().GetSlotCount() - kAdminCommandSlotCount;

  auto lun_id = Ufs::TranslateScsiLunToUfsLun(kTestLun);
  ASSERT_OK(lun_id);

  dut_->GetTransferRequestProcessor().DisableCompletion();
  auto enable_completion =
      fit::defer([this]() { dut_->GetTransferRequestProcessor().EnableCompletion(); });

  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));

  uint8_t cdb_buffer[16] = {};
  uint8_t cdb_length;
  auto cdb = reinterpret_cast<scsi::Read10CDB*>(cdb_buffer);
  cdb_length = 10;
  cdb->opcode = scsi::Opcode::READ_10;
  cdb->logical_block_address = 0;
  cdb->transfer_length = 0;
  cdb->set_force_unit_access(false);
  ZX_ASSERT(cdb_length <= sizeof(cdb_buffer));

  // Emulates a timeout situation. Hook the SCSI command handler to set a response timeout.
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::READ_10,
      [](UfsMockDevice& mock_device, CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius) {
        return zx::error(ZX_ERR_TIMED_OUT);
      });

  auto vmos = std::make_unique<zx::vmo[]>(kMaxSlotCount);

  for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmos[slot_num]));
    ScsiCommandUpiu upiu(cdb_buffer, cdb_length, DataDirection::kDeviceToHost,
                         ufs_mock_device::kMockBlockSize);
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    ASSERT_EQ(slot.value(), slot_num);
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), vmos[slot_num].borrow(), 0, ufs_mock_device::kMockBlockSize,
        [](zx_status_t status) {});
  }

  dut_->GetTransferRequestProcessor().EnableCompletion();
  enable_completion.cancel();

  auto wait_for = [&]() -> bool {
    dut_->ProcessIoCompletions();
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    bool all_timed_out = true;
    for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
      if (dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state !=
          SlotState::kTimeout) {
        all_timed_out = false;
      }
    }
    return all_timed_out;
  };
  fbl::String timeout_message = "Timeout waiting for SCSI command timeout";
  ASSERT_OK(dut_->WaitWithTimeout(wait_for, kWaitLimit, timeout_message, zx::msec(100)));

  // Check that the timed out command.
  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
      EXPECT_EQ(dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state,
                SlotState::kTimeout);
      EXPECT_EQ(dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).result,
                ZX_ERR_TIMED_OUT);
    }
  }
  mock_device_.GetScsiCommandProcessor().Reset();
}

TEST_F(TimeoutTest, PartialAsyncCommandsTimeout) {
  constexpr uint8_t kTestLun = 0;
  const uint8_t kMaxSlotCount =
      dut_->GetTransferRequestProcessor().GetSlotCount() - kAdminCommandSlotCount;
  const uint8_t kTimeoutCount = kMaxSlotCount / 2;

  auto lun_id = Ufs::TranslateScsiLunToUfsLun(kTestLun);
  ASSERT_OK(lun_id);

  dut_->GetTransferRequestProcessor().DisableCompletion();
  auto enable_completion =
      fit::defer([this]() { dut_->GetTransferRequestProcessor().EnableCompletion(); });

  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));

  uint8_t cdb_buffer[16] = {};
  uint8_t cdb_length;
  auto cdb = reinterpret_cast<scsi::Read10CDB*>(cdb_buffer);
  cdb_length = 10;
  cdb->opcode = scsi::Opcode::READ_10;
  cdb->logical_block_address = 0;
  cdb->transfer_length = 0;
  cdb->set_force_unit_access(false);
  ZX_ASSERT(cdb_length <= sizeof(cdb_buffer));

  // Emulates a timeout situation. Hook the SCSI command handler to set a response timeout.
  // This hook is only affects the READ_10 command.
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::READ_10,
      [](UfsMockDevice& mock_device, CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius) {
        return zx::error(ZX_ERR_TIMED_OUT);
      });

  auto vmos = std::make_unique<zx::vmo[]>(kMaxSlotCount);

  // Execute READ_10 commands to timeout.
  cdb->opcode = scsi::Opcode::READ_10;
  for (uint8_t slot_num = 0; slot_num < kTimeoutCount; ++slot_num) {
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmos[slot_num]));
    ScsiCommandUpiu upiu(cdb_buffer, cdb_length, DataDirection::kDeviceToHost,
                         ufs_mock_device::kMockBlockSize);
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    ASSERT_EQ(slot.value(), slot_num);
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), vmos[slot_num].borrow(), 0, ufs_mock_device::kMockBlockSize,
        [](zx_status_t status) {});
  }

  // Execute WRITE_10 commands to succeed.
  cdb->opcode = scsi::Opcode::WRITE_10;
  cdb->transfer_length = htobe16(1);
  for (uint8_t slot_num = kTimeoutCount; slot_num < kMaxSlotCount; ++slot_num) {
    ASSERT_OK(zx::vmo::create(ufs_mock_device::kMockBlockSize, 0, &vmos[slot_num]));
    ScsiCommandUpiu upiu(cdb_buffer, cdb_length, DataDirection::kHostToDevice,
                         ufs_mock_device::kMockBlockSize);
    zx::result<uint8_t> slot = dut_->GetTransferRequestProcessor().ReserveSlot();
    ASSERT_OK(slot);
    ASSERT_EQ(slot.value(), slot_num);
    dut_->GetTransferRequestProcessor().SendIoScsiCmd(
        upiu, kTestLun, slot.value(), vmos[slot_num].borrow(), 0, ufs_mock_device::kMockBlockSize,
        [](zx_status_t status) {});
  }

  dut_->GetTransferRequestProcessor().EnableCompletion();
  enable_completion.cancel();

  auto wait_for = [&]() -> bool {
    dut_->ProcessIoCompletions();
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    bool all_done = true;
    for (uint8_t slot_num = 0; slot_num < kTimeoutCount; ++slot_num) {
      if (dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state !=
          SlotState::kTimeout) {
        all_done = false;
      }
    }
    for (uint8_t slot_num = kTimeoutCount; slot_num < kMaxSlotCount; ++slot_num) {
      if (dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state !=
          SlotState::kFree) {
        all_done = false;
      }
    }
    return all_done;
  };
  fbl::String timeout_message = "Timeout waiting for SCSI command timeout or completion";
  ASSERT_OK(dut_->WaitWithTimeout(wait_for, kWaitLimit, timeout_message, zx::msec(100)));

  // Check that the timed out command.
  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    for (uint8_t slot_num = 0; slot_num < kTimeoutCount; ++slot_num) {
      EXPECT_EQ(dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state,
                SlotState::kTimeout);
      EXPECT_EQ(dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).result,
                ZX_ERR_TIMED_OUT);
    }

    // Check that the completed command.
    for (uint8_t slot_num = kTimeoutCount; slot_num < kMaxSlotCount; ++slot_num) {
      EXPECT_EQ(dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).state,
                SlotState::kFree);
      EXPECT_EQ(dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num).result,
                ZX_OK);
    }
  }
  mock_device_.GetScsiCommandProcessor().Reset();
}

TEST_F(TimeoutTest, AdminCommandTimeoutClearsCompletionCallback) {
  // Emulate a timeout situation for admin command.
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::TEST_UNIT_READY,
      [](UfsMockDevice& mock_device, CommandUpiuData& command_upiu, ResponseUpiuData& response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry>& prdt_upius) {
        return zx::error(ZX_ERR_TIMED_OUT);
      });

  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(10));

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB*>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;
  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);

  zx::result result = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, 0);
  EXPECT_TRUE(result.is_error());
  EXPECT_EQ(result.status_value(), ZX_ERR_TIMED_OUT);

  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    auto& slot =
        dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(kAdminCommandSlotNumber);
    EXPECT_EQ(slot.state, SlotState::kTimeout);
    // Completion callback must be reset so there are no lingering references to the wait context.
    EXPECT_FALSE(slot.completion_cb);
  }

  mock_device_.GetScsiCommandProcessor().Reset();
}

}  // namespace ufs
