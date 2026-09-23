// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdint>
#include <memory>

#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"
#include "src/devices/block/drivers/ufs/upiu/descriptors.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"
#include "zircon/errors.h"

namespace ufs {
using namespace ufs_mock_device;

using RequestProcessorTest = UfsTest;

TEST_F(RequestProcessorTest, RequestListCreate) {
  auto request_list =
      RequestList::Create(mock_device_.GetFakeBti().borrow(), kMaxTransferRequestListSize,
                          sizeof(TransferRequestDescriptor));

  // Check list size
  ASSERT_EQ(request_list->GetSlotCount(), kMaxTransferRequestListSize);

  // Check request list slots
  for (uint8_t i = 0; i < kMaxTransferRequestListSize; ++i) {
    auto &slot = request_list->GetSlot(i);
    ASSERT_EQ(slot.state, SlotState::kFree);
    ASSERT_TRUE(slot.command_descriptor_io);
  }
}

TEST_F(RequestProcessorTest, RingRequestDoorbell) {
  dut_->GetTransferRequestProcessor().DisableCompletion();

  // Reuse the command descriptor in admin slot.
  auto slot_num = ReserveAdminSlot();
  ASSERT_TRUE(slot_num.is_ok());

  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kReserved);
    slot.completion_cb = [](zx_status_t) {};
  }

  RingRequestDoorbell<ufs::TransferRequestProcessor>(slot_num.value());
  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kScheduled);
  }

  dut_->GetTransferRequestProcessor().EnableCompletion();
  auto wait_for_slot_freed = [&]() -> bool {
    dut_->GetTransferRequestProcessor().ProcessCompletionOfAdminRequests();
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor()
               .GetRequestListLocked()
               .GetSlot(slot_num.value())
               .state == SlotState::kFree;
  };
  ASSERT_OK(dut_->WaitWithTimeout(wait_for_slot_freed, zx::sec(10),
                                  "Timeout waiting for slot to be freed", zx::msec(100)));
}

TEST_F(RequestProcessorTest, FillDescriptorAndSendRequest) {
  dut_->GetTransferRequestProcessor().DisableCompletion();

  auto slot_num = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_TRUE(slot_num.is_ok());

  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kReserved);
    slot.completion_cb = [](zx_status_t) {};
  }

  DataDirection data_dir = DataDirection::kHostToDevice;
  constexpr uint16_t response_offset = 0x12;
  constexpr uint16_t response_length = 0x34;
  constexpr uint16_t prdt_offset = 0x56;
  constexpr uint16_t prdt_entry_count = 0x78;
  ASSERT_EQ(TransferFillDescriptorAndSendRequest(slot_num.value(), data_dir, response_offset,
                                                 response_length, prdt_offset, prdt_entry_count)
                .status_value(),
            ZX_OK);

  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTransferRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kScheduled);
  }

  dut_->GetTransferRequestProcessor().EnableCompletion();

  // Wait for request slot to be freed.
  auto wait_for_slot_freed = [&]() -> bool {
    dut_->GetTransferRequestProcessor().ProcessCompletionOfAdminRequests();
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor()
               .GetRequestListLocked()
               .GetSlot(slot_num.value())
               .state == SlotState::kFree;
  };
  ASSERT_OK(dut_->WaitWithTimeout(wait_for_slot_freed, zx::sec(10),
                                  "Timeout waiting for slot to be freed", zx::msec(100)));

  // Check Utp Transfer Request Descriptor
  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    auto descriptor = dut_->GetTransferRequestProcessor()
                          .GetRequestListLocked()
                          .GetRequestDescriptor<TransferRequestDescriptor>(slot_num.value());
    EXPECT_EQ(descriptor->command_type(), kCommandTypeUfsStorage);
    EXPECT_EQ(descriptor->data_direction(), data_dir);
    EXPECT_EQ(descriptor->interrupt(), 1U);
    EXPECT_EQ(descriptor->ce(), 0U);                      // Crypto is not supported
    EXPECT_EQ(descriptor->cci(), 0U);                     // Crypto is not supported
    EXPECT_EQ(descriptor->data_unit_number_lower(), 0U);  // Crypto is not supported
    EXPECT_EQ(descriptor->data_unit_number_upper(), 0U);  // Crypto is not supported

    zx_paddr_t paddr = dut_->GetTransferRequestProcessor()
                           .GetRequestListLocked()
                           .GetSlot(slot_num.value())
                           .command_descriptor_io->phys();
    EXPECT_EQ(descriptor->utp_command_descriptor_base_address(), paddr & UINT32_MAX);
    EXPECT_EQ(descriptor->utp_command_descriptor_base_address_upper(),
              static_cast<uint32_t>(paddr >> 32));
    constexpr uint16_t kDwordSize = 4;
    EXPECT_EQ(descriptor->response_upiu_offset(), uint32_t{response_offset / kDwordSize});
    EXPECT_EQ(descriptor->response_upiu_length(), uint32_t{response_length / kDwordSize});
    EXPECT_EQ(descriptor->prdt_offset(), uint32_t{prdt_offset / kDwordSize});
    EXPECT_EQ(descriptor->prdt_length(), uint32_t{prdt_entry_count});
  }
}

TEST_F(RequestProcessorTest, SendQueryUpiu) {
  ReadAttributeUpiu request(Attributes::bBootLunEn);
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(request);
  ASSERT_OK(response);

  // Check that the Request UPIU is copied into the command descriptor.
  constexpr uint8_t slot_num = kAdminCommandSlotNumber;
  AbstractUpiu command_descriptor([&]() {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor().GetRequestListLocked().GetDescriptorBuffer(slot_num);
  }());
  ASSERT_EQ(memcmp(request.GetData(), command_descriptor.GetData(), sizeof(QueryRequestUpiuData)),
            0);

  // Check response
  ASSERT_EQ(response->GetHeader().trans_code(), UpiuTransactionCodes::kQueryResponse);
  ASSERT_EQ(response->GetHeader().function,
            static_cast<uint8_t>(QueryFunction::kStandardReadRequest));
  ASSERT_EQ(response->GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
  ASSERT_EQ(response->GetHeader().data_segment_length, 0);
  ASSERT_EQ(response->GetHeader().flags, 0);
  ASSERT_EQ(response->GetOpcode(), static_cast<uint8_t>(QueryOpcode::kReadAttribute));
  ASSERT_EQ(response->GetIdn(), static_cast<uint8_t>(Attributes::bBootLunEn));
  ASSERT_EQ(response->GetIndex(), 0);
}

TEST_F(RequestProcessorTest, SendQueryUpiuException) {
  dut_->GetTransferRequestProcessor().DisableCompletion();

  ReadAttributeUpiu request(Attributes::bBootLunEn);
  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));
  auto response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(request);
  ASSERT_EQ(response.status_value(), ZX_ERR_TIMED_OUT);

  dut_->GetTransferRequestProcessor().EnableCompletion();
  dut_->GetTransferRequestProcessor().SetTimeout(kCommandTimeout);
  dut_->GetTransferRequestProcessor().ProcessCompletionOfIoRequests();

  // Hook the query request handler to set a response error
  mock_device_.GetTransferRequestProcessor().SetHook(
      UpiuTransactionCodes::kQueryRequest,
      [](UfsMockDevice &mock_device, CommandDescriptorData command_descriptor_data) {
        QueryRequestUpiuData *request_upiu = reinterpret_cast<QueryRequestUpiuData *>(
            command_descriptor_data.command_upiu_base_addr);
        QueryResponseUpiuData *response_upiu = reinterpret_cast<QueryResponseUpiuData *>(
            command_descriptor_data.response_upiu_base_addr);

        response_upiu->opcode = request_upiu->opcode;
        response_upiu->idn = request_upiu->idn;
        response_upiu->index = request_upiu->index;
        response_upiu->selector = request_upiu->selector;

        // Set response error
        response_upiu->header.response = UpiuHeaderResponseCode::kTargetFailure;

        return mock_device.GetQueryRequestProcessor().HandleQueryRequest(*request_upiu,
                                                                         *response_upiu);
      });

  dut_->GetTransferRequestProcessor().SetTimeout(kCommandTimeout);
  response = dut_->GetTransferRequestProcessor().SendQueryRequestUpiu(request);
  ASSERT_EQ(response.status_value(), ZX_ERR_BAD_STATE);
}

TEST_F(RequestProcessorTest, SendNopUpiu) {
  NopOutUpiu nop_out_upiu;
  auto nop_in =
      dut_->GetTransferRequestProcessor().SendRequestUpiu<NopOutUpiu, NopInUpiu>(nop_out_upiu);
  ASSERT_OK(nop_in);

  // Check that the nop out UPIU is copied into the command descriptor.
  constexpr uint8_t slot_num = kAdminCommandSlotNumber;
  AbstractUpiu command_descriptor([&]() {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor().GetRequestListLocked().GetDescriptorBuffer(slot_num);
  }());
  ASSERT_EQ(memcmp(nop_out_upiu.GetData(), command_descriptor.GetData(), sizeof(NopOutUpiuData)),
            0);

  // Check response
  ASSERT_EQ(nop_in->GetHeader().trans_code(), UpiuTransactionCodes::kNopIn);
  ASSERT_EQ(nop_in->GetHeader().function, 0);
  ASSERT_EQ(nop_in->GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
  ASSERT_EQ(nop_in->GetHeader().data_segment_length, 0);
  ASSERT_EQ(nop_in->GetHeader().flags, 0);
}

TEST_F(RequestProcessorTest, SendNopUpiuException) {
  dut_->GetTransferRequestProcessor().DisableCompletion();

  NopOutUpiu nop_out_upiu;
  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));
  auto nop_in =
      dut_->GetTransferRequestProcessor().SendRequestUpiu<NopOutUpiu, NopInUpiu>(nop_out_upiu);
  ASSERT_EQ(nop_in.status_value(), ZX_ERR_TIMED_OUT);

  dut_->GetTransferRequestProcessor().EnableCompletion();
  dut_->ProcessIoCompletions();

  // Enable completion interrupt
  InterruptEnableReg::Get()
      .ReadFrom(mock_device_.GetRegisters())
      .set_utp_transfer_request_completion_enable(true)
      .WriteTo(mock_device_.GetRegisters());

  // Hook the nop out handler to set a response error
  mock_device_.GetTransferRequestProcessor().SetHook(
      UpiuTransactionCodes::kNopOut,
      [](UfsMockDevice &mock_device, CommandDescriptorData command_descriptor_data) {
        NopInUpiuData *nop_in_upiu =
            reinterpret_cast<NopInUpiuData *>(command_descriptor_data.response_upiu_base_addr);
        nop_in_upiu->header.data_segment_length = 0;
        nop_in_upiu->header.flags = 0;
        nop_in_upiu->header.response = UpiuHeaderResponseCode::kTargetFailure;
        return ZX_OK;
      });

  dut_->GetTransferRequestProcessor().SetTimeout(kCommandTimeout);
  nop_in = dut_->GetTransferRequestProcessor().SendRequestUpiu<NopOutUpiu, NopInUpiu>(nop_out_upiu);
  ASSERT_EQ(nop_in.status_value(), ZX_ERR_BAD_STATE);

  // Send Nop-out UPIU when the admin slot is full.
  ASSERT_OK(ReserveAdminSlot());
  nop_in = dut_->GetTransferRequestProcessor().SendRequestUpiu<NopOutUpiu, NopInUpiu>(nop_out_upiu);
  ASSERT_EQ(nop_in.status_value(), ZX_ERR_NO_RESOURCES);
}

TEST_F(RequestProcessorTest, SendRequestUpiuWithAdminSlotIsFull) {
  // Reserve admin slot.
  ASSERT_OK(ReserveAdminSlot());

  // Make request UPIU
  NopOutUpiu nop_out_upiu;
  auto nop_in =
      dut_->GetTransferRequestProcessor().SendRequestUpiu<NopOutUpiu, NopInUpiu>(nop_out_upiu);
  ASSERT_EQ(nop_in.status_value(), ZX_ERR_NO_RESOURCES);
}

TEST_F(RequestProcessorTest, SendRequestUsingSlot) {
  constexpr uint8_t kTestLun = 0;

  auto slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(slot);

  // Send scsi command with SendIoScsiCmd()
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  sync_completion_t complete;
  zx_status_t completion_status = ZX_OK;
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(
      upiu, kTestLun, slot.value(), zx::unowned_vmo(), 0, 0,
      [&complete, &completion_status](zx_status_t status) {
        completion_status = status;
        sync_completion_signal(&complete);
      });
  ASSERT_OK(sync_completion_wait(&complete, ZX_TIME_INFINITE));
  ASSERT_OK(completion_status);

  // Check that the SCSI UPIU is copied into the command descriptor.
  AbstractUpiu command_descriptor([&]() {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor().GetRequestListLocked().GetDescriptorBuffer(
        slot.value());
  }());

  ASSERT_EQ(memcmp(upiu.GetData(), command_descriptor.GetData(), sizeof(CommandUpiuData)), 0);

  // Check response
  const uint16_t response_offset = upiu.GetResponseOffset();
  void *response_buf;
  {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    response_buf = dut_->GetTransferRequestProcessor().GetRequestListLocked().GetDescriptorBuffer(
        slot.value(), response_offset);
  }
  ResponseUpiu response(response_buf);
  EXPECT_EQ(response.GetHeader().trans_code(), UpiuTransactionCodes::kResponse);
  EXPECT_EQ(response.GetHeader().status, static_cast<uint8_t>(scsi::StatusCode::GOOD));
  EXPECT_EQ(response.GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
}

TEST_F(RequestProcessorTest, SendRequestUsingSlotTimeout) {
  constexpr uint8_t kTestLun = 0;

  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(10));
  auto cleanup =
      fit::defer([this]() { dut_->GetTransferRequestProcessor().SetTimeout(kCommandTimeout); });

  auto slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(slot);

  // Hook TEST_UNIT_READY to time out.
  mock_device_.GetScsiCommandProcessor().SetHook(
      scsi::Opcode::TEST_UNIT_READY,
      [](UfsMockDevice &mock_device, CommandUpiuData &command_upiu, ResponseUpiuData &response_upiu,
         cpp20::span<PhysicalRegionDescriptionTableEntry> &prdt_upius) {
        return zx::error(ZX_ERR_TIMED_OUT);
      });

  // Send scsi command with SendIoScsiCmd()
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  sync_completion_t complete;
  zx_status_t completion_status = ZX_OK;
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(
      upiu, kTestLun, slot.value(), zx::unowned_vmo(), 0, 0,
      [&complete, &completion_status](zx_status_t status) {
        completion_status = status;
        sync_completion_signal(&complete);
      });

  zx::nanosleep(zx::deadline_after(zx::msec(20)));
  dut_->GetTransferRequestProcessor().ProcessCompletionOfIoRequests();

  ASSERT_OK(sync_completion_wait(&complete, ZX_TIME_INFINITE));
  ASSERT_EQ(completion_status, ZX_ERR_TIMED_OUT);
}

TEST_F(RequestProcessorTest, SendScsiUpiu) {
  constexpr uint8_t kTestLun = 0;

  // Send scsi command with SendScsiUpiu()
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  auto response_or = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_OK(response_or);

  // Check that the SCSI UPIU is copied into the command descriptor.
  AbstractUpiu command_descriptor([&]() {
    std::lock_guard<std::mutex> lock(dut_->GetTransferRequestProcessor().GetSlotLock());
    return dut_->GetTransferRequestProcessor().GetRequestListLocked().GetDescriptorBuffer(
        kAdminCommandSlotNumber);
  }());
  ASSERT_EQ(memcmp(upiu.GetData(), command_descriptor.GetData(), sizeof(CommandUpiuData)), 0);

  // Check response
  ResponseUpiu response(response_or->GetData());
  EXPECT_EQ(response.GetHeader().trans_code(), UpiuTransactionCodes::kResponse);
  EXPECT_EQ(response.GetHeader().status, static_cast<uint8_t>(scsi::StatusCode::GOOD));
  EXPECT_EQ(response.GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
}

TEST_F(RequestProcessorTest, SendScsiUpiuTimeout) {
  constexpr uint8_t kTestLun = 0;

  dut_->GetTransferRequestProcessor().DisableCompletion();
  dut_->GetTransferRequestProcessor().SetTimeout(zx::msec(100));

  // Send scsi command with SendScsiUpiu()
  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  auto response_or = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_EQ(response_or.status_value(), ZX_ERR_TIMED_OUT);
}

TEST_F(RequestProcessorTest, SendScsiUpiuWithAdminSlotIsFull) {
  constexpr uint8_t kTestLun = 0;

  ASSERT_OK(ReserveAdminSlot());

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
  auto response = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_EQ(response.status_value(), ZX_ERR_NO_RESOURCES);
}

TEST_F(RequestProcessorTest, SendScsiUpiuWithSlotIsFull) {
  const uint8_t kMaxSlotCount =
      dut_->GetTransferRequestProcessor().GetSlotCount() - kAdminCommandSlotCount;

  // Reserve all slots.
  for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
    ASSERT_OK(ReserveSlot<ufs::TransferRequestProcessor>());
  }

  auto slot = dut_->GetTransferRequestProcessor().ReserveSlot();
  ASSERT_EQ(slot.status_value(), ZX_ERR_NO_RESOURCES);
}

TEST_F(RequestProcessorTest, SendAdminScsiCmdWithSlotIsFull) {
  constexpr uint8_t kTestLun = 0;
  const uint8_t kMaxSlotCount =
      dut_->GetTransferRequestProcessor().GetSlotCount() - kAdminCommandSlotCount;

  // Reserve all I/O slots.
  for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
    ASSERT_OK(ReserveSlot<ufs::TransferRequestProcessor>());
  }

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);

  // Admin command should succeed even if all I/O slots are full.
  auto response = dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, kTestLun);
  ASSERT_OK(response);
}

TEST_F(RequestProcessorTest, SendIoScsiCmdFailureCallsCallback) {
  constexpr uint8_t kTestLun = 0;
  zx::result<uint8_t> slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(slot);

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  // Create a 4KB VMO, but specify an offset/length beyond its size so that pinning fails.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kDeviceToHost,
                       zx_system_get_page_size());

  bool callback_called = false;
  zx_status_t callback_status = ZX_OK;
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(
      upiu, kTestLun, slot.value(), vmo.borrow(), zx_system_get_page_size() * 2,
      zx_system_get_page_size(), [&](zx_status_t status) {
        callback_called = true;
        callback_status = status;
      });

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(callback_status, ZX_ERR_OUT_OF_RANGE);

  // Verify that the slot was released on failure and can be reserved again.
  zx::result<uint8_t> new_slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(new_slot);
  EXPECT_EQ(new_slot.value(), slot.value());
}

TEST_F(RequestProcessorTest, SendIoScsiCmdInvalidVmoCallsCallback) {
  constexpr uint8_t kTestLun = 0;
  zx::result<uint8_t> slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(slot);

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  // With transfer bytes > 0 but invalid VMO, SendIoScsiCmd should fail and invoke the callback with
  // ZX_ERR_BAD_HANDLE.
  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kDeviceToHost, 1024);

  bool callback_called = false;
  zx_status_t callback_status = ZX_OK;
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(upiu, kTestLun, slot.value(), zx::unowned_vmo(),
                                                    0, 1024, [&](zx_status_t status) {
                                                      callback_called = true;
                                                      callback_status = status;
                                                    });

  EXPECT_TRUE(callback_called);
  EXPECT_EQ(callback_status, ZX_ERR_BAD_HANDLE);

  // Verify that the slot was released on failure and can be reserved again.
  zx::result<uint8_t> new_slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(new_slot);
  EXPECT_EQ(new_slot.value(), slot.value());
}

TEST_F(RequestProcessorTest, ZeroLengthWithValidVmo) {
  constexpr uint8_t kTestLun = 0;
  zx::result<uint8_t> slot = ReserveSlot<ufs::TransferRequestProcessor>();
  ASSERT_OK(slot);

  uint8_t cdb_buffer[6] = {};
  auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
  cdb->opcode = scsi::Opcode::TEST_UNIT_READY;

  ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kDeviceToHost, 0);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0, &vmo));

  bool callback_called = false;
  zx_status_t callback_status = ZX_ERR_INTERNAL;
  dut_->GetTransferRequestProcessor().SendIoScsiCmd(upiu, kTestLun, slot.value(), vmo.borrow(), 0,
                                                    0, [&](zx_status_t status) {
                                                      callback_called = true;
                                                      callback_status = status;
                                                    });

  // Hardware completion
  dut_->GetTransferRequestProcessor().ProcessCompletionOfIoRequests();

  EXPECT_TRUE(callback_called);
  EXPECT_OK(callback_status);
}

TEST_F(RequestProcessorTest, SyncRequestPanicsOnAdminWorkerDispatcher) {
  ASSERT_DEATH(
      {
        libsync::Completion done;
        async::PostTask(dut_->admin_worker_dispatcher()->async_dispatcher(), [&]() {
          uint8_t cdb_buffer[6] = {};
          auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
          cdb->opcode = scsi::Opcode::TEST_UNIT_READY;
          ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
          [[maybe_unused]] auto result =
              dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, 0);
          done.Signal();
        });
        done.Wait();
      },
      "Synchronous UFS request cannot be issued from inside IO or Admin worker dispatcher thread!");
}

TEST_F(RequestProcessorTest, SyncRequestPanicsOnIoWorkerDispatcher) {
  ASSERT_DEATH(
      {
        libsync::Completion done;
        async::PostTask(dut_->io_worker_dispatcher()->async_dispatcher(), [&]() {
          uint8_t cdb_buffer[6] = {};
          auto cdb = reinterpret_cast<scsi::TestUnitReadyCDB *>(cdb_buffer);
          cdb->opcode = scsi::Opcode::TEST_UNIT_READY;
          ScsiCommandUpiu upiu(cdb_buffer, sizeof(*cdb), DataDirection::kNone);
          [[maybe_unused]] auto result =
              dut_->GetTransferRequestProcessor().SendAdminScsiCmd(upiu, 0);
          done.Signal();
        });
        done.Wait();
      },
      "Synchronous UFS request cannot be issued from inside IO or Admin worker dispatcher thread!");
}

}  // namespace ufs
