// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdint>
#include <memory>

#include "src/devices/block/drivers/ufs/upiu/descriptors.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"
#include "unit-lib.h"
#include "zircon/errors.h"

namespace ufs {
using namespace ufs_mock_device;

using TaskManagementRequestProcessorTest = UfsTest;

TEST_F(TaskManagementRequestProcessorTest, RingRequestDoorbell) {
  dut_->GetTaskManagementRequestProcessor().DisableCompletion();

  auto slot_num = ReserveSlot<ufs::TaskManagementRequestProcessor>();
  ASSERT_TRUE(slot_num.is_ok());

  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTaskManagementRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kReserved);
  }

  RingRequestDoorbell<ufs::TaskManagementRequestProcessor>(slot_num.value());
  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTaskManagementRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kScheduled);
  }

  dut_->GetTaskManagementRequestProcessor().EnableCompletion();
  auto wait_for_completion = [&]() -> bool {
    dut_->GetTaskManagementRequestProcessor().ProcessCompletionOfIoRequests();
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    return dut_->GetTaskManagementRequestProcessor()
               .GetRequestListLocked()
               .GetSlot(slot_num.value())
               .result == ZX_OK;
  };
  ASSERT_OK(dut_->WaitWithTimeout(wait_for_completion, zx::sec(10),
                                  "Timeout waiting for completion", zx::msec(100)));
  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    ASSERT_OK(dut_->GetTaskManagementRequestProcessor()
                  .GetRequestListLocked()
                  .GetSlot(slot_num.value())
                  .result);
  }
}

TEST_F(TaskManagementRequestProcessorTest, FillDescriptorAndSendRequest) {
  dut_->GetTaskManagementRequestProcessor().DisableCompletion();

  auto slot_num = ReserveSlot<ufs::TaskManagementRequestProcessor>();
  ASSERT_TRUE(slot_num.is_ok());

  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTaskManagementRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kReserved);
  }

  TaskManagementRequestUpiu request(TaskManagementFunction::kQueryTask, 0, slot_num.value());
  ASSERT_EQ(TaskManagementFillDescriptorAndSendRequest(slot_num.value(), request).status_value(),
            ZX_OK);

  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto &slot =
        dut_->GetTaskManagementRequestProcessor().GetRequestListLocked().GetSlot(slot_num.value());
    ASSERT_EQ(slot.state, SlotState::kScheduled);
  }

  dut_->GetTaskManagementRequestProcessor().EnableCompletion();
  ASSERT_EQ(dut_->GetTaskManagementRequestProcessor().ProcessCompletionOfIoRequests(), 1U);
  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    ASSERT_OK(dut_->GetTaskManagementRequestProcessor()
                  .GetRequestListLocked()
                  .GetSlot(slot_num.value())
                  .result);

    // Check UTP Task Management Request Descriptor
    auto descriptor = dut_->GetTaskManagementRequestProcessor()
                          .GetRequestListLocked()
                          .GetRequestDescriptor<TaskManagementRequestDescriptor>(slot_num.value());
    EXPECT_EQ(descriptor->interrupt(), 1U);
    EXPECT_EQ(descriptor->overall_command_status(), OverallCommandStatus::kSuccess);
  }
}

TEST_F(TaskManagementRequestProcessorTest, SendAbortTaskManagementRequest) {
  uint8_t target_lun = 0;
  uint8_t target_task_tag = 0;

  zx::result<TaskManagementServiceResponse> service_response =
      dut_->GetTaskManagementRequestProcessor().GetTaskManagementServiceResponse(
          TaskManagementFunction::kAbortTask, target_lun, target_task_tag);
  ASSERT_EQ(service_response.value(), TaskManagementServiceResponse::kTaskManagementFunctionFailed);

  TaskManagementRequestUpiu request(TaskManagementFunction::kAbortTask, target_lun,
                                    target_task_tag);
  auto response = dut_->GetTaskManagementRequestProcessor().SendTaskManagementRequest(request);
  ASSERT_OK(response);

  // Check that the Request UPIU is copied into the request descriptor.
  constexpr uint8_t slot_num = 0;
  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto descriptor = dut_->GetTaskManagementRequestProcessor()
                          .GetRequestListLocked()
                          .GetRequestDescriptor<TaskManagementRequestDescriptor>(slot_num);
    ASSERT_EQ(memcmp(response->GetData(), descriptor->GetResponseData(),
                     sizeof(TaskManagementResponseUpiuData)),
              0);
  }

  // Check response
  ASSERT_EQ(response->GetHeader().trans_code(),
            static_cast<uint8_t>(UpiuTransactionCodes::kTaskManagementResponse));
  ASSERT_EQ(response->GetHeader().function,
            static_cast<uint8_t>(TaskManagementFunction::kAbortTask));
  ASSERT_EQ(response->GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
  ASSERT_EQ(response->GetHeader().data_segment_length, 0);
  ASSERT_EQ(response->GetHeader().flags, 0);

  ASSERT_EQ(response->GetData<TaskManagementResponseUpiuData>()->output_param1,
            static_cast<uint32_t>(TaskManagementServiceResponse::kTaskManagementFunctionFailed));
}

TEST_F(TaskManagementRequestProcessorTest, SendQueryTaskManagementRequest) {
  uint8_t target_lun = 0;
  uint8_t target_task_tag = 0;

  zx::result<TaskManagementServiceResponse> service_response =
      dut_->GetTaskManagementRequestProcessor().GetTaskManagementServiceResponse(
          TaskManagementFunction::kQueryTask, target_lun, target_task_tag);
  ASSERT_EQ(service_response.value(),
            TaskManagementServiceResponse::kTaskManagementFunctionComplete);

  TaskManagementRequestUpiu request(TaskManagementFunction::kQueryTask, target_lun,
                                    target_task_tag);
  auto response = dut_->GetTaskManagementRequestProcessor().SendTaskManagementRequest(request);
  ASSERT_OK(response);

  // Check that the Request UPIU is copied into the request descriptor.
  constexpr uint8_t slot_num = 0;
  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto descriptor = dut_->GetTaskManagementRequestProcessor()
                          .GetRequestListLocked()
                          .GetRequestDescriptor<TaskManagementRequestDescriptor>(slot_num);
    ASSERT_EQ(memcmp(response->GetData(), descriptor->GetResponseData(),
                     sizeof(TaskManagementResponseUpiuData)),
              0);
  }

  // Check response
  ASSERT_EQ(response->GetHeader().trans_code(),
            static_cast<uint8_t>(UpiuTransactionCodes::kTaskManagementResponse));
  ASSERT_EQ(response->GetHeader().function,
            static_cast<uint8_t>(TaskManagementFunction::kQueryTask));
  ASSERT_EQ(response->GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
  ASSERT_EQ(response->GetHeader().data_segment_length, 0);
  ASSERT_EQ(response->GetHeader().flags, 0);

  ASSERT_EQ(response->GetData<TaskManagementResponseUpiuData>()->output_param1,
            static_cast<uint32_t>(TaskManagementServiceResponse::kTaskManagementFunctionComplete));
}

TEST_F(TaskManagementRequestProcessorTest, SendLogicalUnitResetTaskManagementRequest) {
  uint8_t target_lun = 0;
  uint8_t target_task_tag = 0;

  zx::result<TaskManagementServiceResponse> service_response =
      dut_->GetTaskManagementRequestProcessor().GetTaskManagementServiceResponse(
          TaskManagementFunction::kLogicalUnitReset, target_lun, target_task_tag);
  ASSERT_EQ(service_response.value(),
            TaskManagementServiceResponse::kTaskManagementFunctionComplete);

  TaskManagementRequestUpiu request(TaskManagementFunction::kLogicalUnitReset, target_lun,
                                    target_task_tag);
  auto response = dut_->GetTaskManagementRequestProcessor().SendTaskManagementRequest(request);
  ASSERT_OK(response);

  // Check that the Request UPIU is copied into the request descriptor.
  constexpr uint8_t slot_num = 0;
  {
    std::lock_guard<std::mutex> lock(dut_->GetTaskManagementRequestProcessor().GetSlotLock());
    auto descriptor = dut_->GetTaskManagementRequestProcessor()
                          .GetRequestListLocked()
                          .GetRequestDescriptor<TaskManagementRequestDescriptor>(slot_num);
    ASSERT_EQ(memcmp(response->GetData(), descriptor->GetResponseData(),
                     sizeof(TaskManagementResponseUpiuData)),
              0);
  }

  // Check response
  ASSERT_EQ(response->GetHeader().trans_code(),
            static_cast<uint8_t>(UpiuTransactionCodes::kTaskManagementResponse));
  ASSERT_EQ(response->GetHeader().function,
            static_cast<uint8_t>(TaskManagementFunction::kLogicalUnitReset));
  ASSERT_EQ(response->GetHeader().response, UpiuHeaderResponseCode::kTargetSuccess);
  ASSERT_EQ(response->GetHeader().data_segment_length, 0);
  ASSERT_EQ(response->GetHeader().flags, 0);

  ASSERT_EQ(response->GetData<TaskManagementResponseUpiuData>()->output_param1,
            static_cast<uint32_t>(TaskManagementServiceResponse::kTaskManagementFunctionComplete));
}

TEST_F(TaskManagementRequestProcessorTest, SendTaskManagementRequestException) {
  dut_->GetTaskManagementRequestProcessor().DisableCompletion();

  uint8_t target_lun = 0;
  uint8_t target_task_tag = 0;

  dut_->GetTaskManagementRequestProcessor().SetTimeout(zx::msec(100));

  zx::result<TaskManagementServiceResponse> service_response =
      dut_->GetTaskManagementRequestProcessor().GetTaskManagementServiceResponse(
          TaskManagementFunction::kQueryTask, target_lun, target_task_tag);
  ASSERT_EQ(service_response.status_value(), ZX_ERR_TIMED_OUT);

  TaskManagementRequestUpiu request(TaskManagementFunction::kQueryTask, target_lun,
                                    target_task_tag);
  auto response = dut_->GetTaskManagementRequestProcessor().SendTaskManagementRequest(request);
  ASSERT_EQ(response.status_value(), ZX_ERR_TIMED_OUT);

  dut_->GetTaskManagementRequestProcessor().EnableCompletion();
  dut_->GetTaskManagementRequestProcessor().ProcessCompletionOfIoRequests();

  // Hook the query request handler to set a response error
  mock_device_.GetTaskManagementRequestProcessor().SetHook(
      TaskManagementFunction::kQueryTask,
      [](UfsMockDevice &mock_device, TaskManagementRequestDescriptor &descriptor) {
        TaskManagementResponseUpiuData *response_upiu = descriptor.GetResponseData();

        // Set response error
        response_upiu->header.response = UpiuHeaderResponseCode::kTargetFailure;
        ufs::ufs_mock_device::TaskManagementRequestProcessor::DefaultQueryTaskHandler(mock_device,
                                                                                      descriptor);

        return ZX_OK;
      });

  dut_->GetTaskManagementRequestProcessor().SetTimeout(kCommandTimeout);
  response = dut_->GetTaskManagementRequestProcessor().SendTaskManagementRequest(request);
  ASSERT_EQ(response.status_value(), ZX_ERR_BAD_STATE);
}

TEST_F(TaskManagementRequestProcessorTest, SendTaskManagementRequestWithSlotIsFull) {
  constexpr uint8_t kTestLun = 0;
  const uint8_t kMaxSlotCount = dut_->GetTaskManagementRequestProcessor().GetSlotCount();

  // Reserve all slots.
  for (uint8_t slot_num = 0; slot_num < kMaxSlotCount; ++slot_num) {
    ASSERT_OK(ReserveSlot<ufs::TaskManagementRequestProcessor>());
  }

  uint8_t target_task_tag = 0;
  TaskManagementRequestUpiu request(TaskManagementFunction::kQueryTask, kTestLun, target_task_tag);
  auto response = dut_->GetTaskManagementRequestProcessor().SendTaskManagementRequest(request);
  ASSERT_EQ(response.status_value(), ZX_ERR_NO_RESOURCES);
}

}  // namespace ufs
