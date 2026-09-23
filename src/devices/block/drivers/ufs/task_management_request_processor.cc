// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "task_management_request_processor.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>

#include <memory>

#include "src/devices/block/drivers/ufs/task_management_request_descriptor.h"
#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {

zx::result<> TaskManagementRequestProcessor::Init() {
  std::lock_guard<std::mutex> lock(slot_lock_);
  zx_paddr_t paddr = slots_.GetRequestDescriptorPhysicalAddress<TaskManagementRequestDescriptor>(0);
  UtmrListBaseAddressReg::Get().FromValue(paddr & 0xffffffff).WriteTo(&register_);
  UtmrListBaseAddressUpperReg::Get().FromValue(paddr >> 32).WriteTo(&register_);

  if (!HostControllerStatusReg::Get()
           .ReadFrom(&register_)
           .utp_task_management_request_list_ready()) {
    fdf::error("UTP task management request list is not ready\n");
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (UtmrListDoorBellReg::Get().ReadFrom(&register_).door_bell() != 0) {
    fdf::error("UTP task management request list door bell is not ready\n");
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Start UTP task management request list.
  UtmrListRunStopReg::Get().FromValue(1).WriteTo(&register_);

  return zx::ok();
}

uint32_t TaskManagementRequestProcessor::ProcessCompletionOfIoRequests() {
  if (disable_completion_.load(std::memory_order_acquire)) {
    return 0;
  }

  uint32_t active_slots;
  {
    std::lock_guard<std::mutex> lock(slot_lock_);
    active_slots = allocated_slots_mask_;
  }

  if (active_slots == 0) {
    return 0;
  }

  const uint32_t doorbell = ReadDoorBellRegister();
  uint32_t completion_count = 0;
  std::lock_guard<std::mutex> lock(slot_lock_);
  while (active_slots != 0) {
    const uint8_t slot_num = static_cast<uint8_t>(std::countr_zero(active_slots));
    active_slots &= active_slots - 1;

    RequestSlot &request_slot = slots_.GetSlot(slot_num);
    if (request_slot.state == SlotState::kScheduled) {
      if (!(doorbell & (1u << slot_num))) {
        zx::result<> result = zx::ok();
        // Check task management response.
        auto descriptor = slots_.GetRequestDescriptor<TaskManagementRequestDescriptor>(slot_num);
        TaskManagementResponseUpiu response(descriptor->GetResponseData());
        if (response.GetHeader().response != UpiuHeaderResponseCode::kTargetSuccess) {
          fdf::error("UTP task management request command failure: response={}",
                     response.GetHeader().response);
          result = zx::error(ZX_ERR_BAD_STATE);
        }
        request_slot.result = result.status_value();
        sync_completion_signal(&request_slot.complete);

        ++completion_count;
      }
    }
  }
  return completion_count;
}

zx::result<TaskManagementResponseUpiu> TaskManagementRequestProcessor::SendTaskManagementRequest(
    TaskManagementRequestUpiu &request) {
  // TODO(https://fxbug.dev/42075643): Needs to be changed to be compatible with DFv2's dispatcher
  // Since the completion is handled by the I/O thread, submitting a synchronous command from the
  // I/O thread will cause a deadlock.
  // ZX_DEBUG_ASSERT(controller_.GetIoThread() != thrd_current());

  zx::result<uint8_t> slot = ReserveSlot();
  if (slot.is_error()) {
    return zx::error(ZX_ERR_NO_RESOURCES);
  }
  const uint8_t slot_num = slot.value();

  sync_completion_t *complete_signal = nullptr;
  {
    std::lock_guard<std::mutex> lock(slot_lock_);
    RequestSlot &request_slot = slots_.GetSlot(slot_num);
    ZX_DEBUG_ASSERT_MSG(request_slot.state == SlotState::kReserved, "Invalid slot state");

    if (zx::result<> result = FillDescriptorAndSendRequest(slot_num, request); result.is_error()) {
      fdf::error("Failed to send upiu: {}", result);
      if (zx::result<> clear_result = ClearSlotLocked(request_slot); clear_result.is_error()) {
        return clear_result.take_error();
      }
      return result.take_error();
    }
    complete_signal = &request_slot.complete;
  }

  // Wait for completion (WITHOUT holding slot_lock_!).
  TRACE_DURATION("ufs", "SendTaskManagementRequest::sync_completion_wait", "slot", slot_num);
  zx_status_t status = sync_completion_wait(complete_signal, GetTimeout().get());

  std::lock_guard<std::mutex> lock(slot_lock_);
  RequestSlot &request_slot = slots_.GetSlot(slot_num);
  zx_status_t request_result = request_slot.result;

  if (status != ZX_OK) {
    fdf::error("SendTaskManagementRequest timed out: {}", zx_status_get_string(status));
    if (zx::result<> result = ClearSlotLocked(request_slot); result.is_error()) {
      return result.take_error();
    }
    return zx::error(status);
  }
  if (request_result != ZX_OK) {
    if (zx::result<> result = ClearSlotLocked(request_slot); result.is_error()) {
      return result.take_error();
    }
    return zx::error(request_result);
  }

  // Get the UTP task management response UPIU before releasing the slot.
  auto descriptor = slots_.GetRequestDescriptor<TaskManagementRequestDescriptor>(slot_num);
  TaskManagementResponseUpiu response(descriptor->GetResponseData());

  if (zx::result<> result = ClearSlotLocked(request_slot); result.is_error()) {
    return result.take_error();
  }

  return zx::ok(response);
}

zx::result<TaskManagementServiceResponse>
TaskManagementRequestProcessor::GetTaskManagementServiceResponse(TaskManagementFunction function,
                                                                 uint8_t lun, uint8_t task_tag) {
  TaskManagementRequestUpiu query_task(function, lun, task_tag);
  zx::result<TaskManagementResponseUpiu> response = SendTaskManagementRequest(query_task);
  if (response.is_error()) {
    fdf::error("Failed to task management(0x{:x}) command, {}", static_cast<uint8_t>(function),
               response.status_string());
    return response.take_error();
  }
  return zx::ok(static_cast<TaskManagementServiceResponse>(
      response->GetData<TaskManagementResponseUpiuData>()->output_param1));
}

zx::result<> TaskManagementRequestProcessor::FillDescriptorAndSendRequest(
    uint8_t slot, TaskManagementRequestUpiu &request) {
  auto descriptor = slots_.GetRequestDescriptor<TaskManagementRequestDescriptor>(slot);

  // Fill up UTP task management request descriptor.
  CustomMemSet(descriptor, 0, sizeof(TaskManagementRequestDescriptor));
  descriptor->set_interrupt(true);
  // If the command was successful, overwrite |overall_command_status| field with |kSuccess|.
  descriptor->set_overall_command_status(OverallCommandStatus::kInvalid);

  // Copy the UTP task management request UPIU to the descriptor.
  CustomMemCpy(descriptor->GetRequestData(), request.GetData(),
               sizeof(TaskManagementRequestUpiuData));

  if (zx::result<> result = controller_.Notify(NotifyEvent::kSetupTaskManagementRequestList, slot);
      result.is_error()) {
    return result.take_error();
  }
  RingRequestDoorbellLocked(slot);
  return zx::ok();
}

}  // namespace ufs
