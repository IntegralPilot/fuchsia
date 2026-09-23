// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/block/drivers/nvme/fake/fake-controller.h"

#include <lib/stdcompat/span.h>
#include <lib/zx/clock.h>

#include "src/devices/block/drivers/nvme/commands.h"
#include "src/devices/block/drivers/nvme/registers.h"

namespace fake_nvme {

FakeController::FakeController() { regs_.SetCallbacks(&callbacks_); }

void FakeController::HandleSubmission(size_t queue_id, size_t index, nvme::Submission& submission) {
  // Fill in the completion with data we know will be returned.
  nvme::Completion completion{};
  completion.set_command_id(static_cast<uint16_t>(submission.cid()));
  completion.set_sq_id(static_cast<uint16_t>(queue_id));
  completion.set_sq_head(static_cast<uint16_t>(index));

  // Try and find the command handler. If it is found, run it, otherwise, return an error.
  CommandHandler cmd_handler;
  {
    std::lock_guard<std::mutex> lock(lock_);
    auto& command_set = (queue_id == kAdminQueueId) ? admin_commands_ : io_commands_;
    auto cmd = command_set.find(static_cast<uint8_t>(submission.opcode()));
    if (cmd != command_set.end()) {
      cmd_handler = cmd->second;
    }
  }

  if (cmd_handler) {
    // Find transaction data.
    auto& txn_data = (queue_id == kAdminQueueId) ? nvme_->admin_queue_->txn_data()
                                                 : nvme_->io_queue_->txn_data();
    cmd_handler(submission, txn_data[submission.cid()], completion);
  } else {
    // Command did not exist, return an error.
    completion.set_status_code_type(nvme::StatusCodeType::kGeneric)
        .set_status_code(nvme::GenericStatus::kInvalidOpcode);
  }
  SubmitCompletion(completion);
}

FakeController::QueueState& FakeController::GetQueueState(
    size_t queue_id, std::unordered_map<size_t, QueueState>* queues) {
  std::lock_guard<std::mutex> lock(lock_);
  auto iter = queues->find(queue_id);
  ZX_ASSERT(iter != queues->end());
  ZX_ASSERT(iter->second.queue);
  return iter->second;
}

void FakeController::SubmitCompletion(nvme::Completion& completion) {
  // Find queue.
  QueueState& queue = GetQueueState(completion.sq_id(), &completion_queues_);

  // Check there's space.
  ZX_ASSERT((queue.producer_location + 1) % queue.queue->entry_count() != queue.consumer_location);

  // Mark the completion as ready to be read.
  completion.set_phase(queue.phase);

  // Insert the completion into the queue.
  nvme::Completion* ptr = static_cast<nvme::Completion*>(queue.queue->head());
  ptr[queue.producer_location] = completion;

  // Move forward through the queue.
  queue.producer_location++;
  if (queue.producer_location == queue.queue->entry_count()) {
    queue.producer_location = 0;
    queue.phase ^= 1;
  }

  std::lock_guard<std::mutex> lock(lock_);
  irqs_.find(0)->second.Trigger();
}

zx::result<zx::interrupt> FakeController::GetOrCreateInterrupt(size_t index) {
  std::lock_guard<std::mutex> lock(lock_);
  auto value = irqs_.find(index);
  zx::unowned_interrupt to_clone;
  if (value != irqs_.end()) {
    to_clone = value->second.irq();
  } else {
    zx::interrupt irq;
    zx_status_t status = zx::interrupt::create(zx::resource(), 0, ZX_INTERRUPT_VIRTUAL, &irq);
    if (status != ZX_OK) {
      return zx::error(status);
    }
    to_clone = irqs_.emplace(index, IrqState(std::move(irq))).first->second.irq();
  }

  zx::interrupt out;
  zx_status_t status = to_clone->duplicate(ZX_RIGHT_SAME_RIGHTS, &out);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(out));
}

void FakeController::SetConfig(nvme::ControllerConfigReg& cfg) {
  if (cfg.enabled()) {
    regs_.csts().set_ready(true);
  } else {
    regs_.csts().set_ready(false);
  }
}

void FakeController::UpdateIrqMask(bool enable, nvme::InterruptReg& state) {
  std::lock_guard<std::mutex> lock(lock_);
  uint32_t val = state.reg_value();
  for (size_t i = 0; i < 32; i++) {
    if ((val >> i) & 1) {
      auto entry = irqs_.find(i);
      ZX_ASSERT(entry != irqs_.end());
      if (enable) {
        entry->second.Enable();
      } else {
        entry->second.Disable();
      }
    }
  }
}

void FakeController::RingDoorbell(bool is_submit, size_t queue_id, nvme::DoorbellReg& reg) {
  if (!is_submit) {
    // Completion is easy, just note the new location.
    QueueState& queue = GetQueueState(queue_id, &completion_queues_);
    queue.consumer_location = static_cast<uint16_t>(reg.value());
    return;
  }

  // Submission is a little more complex. For each new submission (between the old and new values of
  // the doorbell), we call HandleSubmission().
  QueueState& queue = GetQueueState(queue_id, &submission_queues_);
  uint16_t old = queue.consumer_location;

  cpp20::span<nvme::Submission> submissions(static_cast<nvme::Submission*>(queue.queue->head()),
                                            queue.queue->entry_count());

  size_t i = old;
  auto next = [&i, &submissions]() {
    i++;
    if (i == submissions.size()) {
      i = 0;
    }
  };

  for (; i != reg.value(); next()) {
    queue.consumer_location = static_cast<uint16_t>(i);
    HandleSubmission(queue_id, i, submissions[i]);
  }
  queue.consumer_location = static_cast<uint16_t>(i);
}

void FakeController::UpdateAdminQueue() {
  AddQueuePair(0, const_cast<nvme::Queue*>(&nvme_->admin_queue_->completion()),
               const_cast<nvme::Queue*>(&nvme_->admin_queue_->submission()));
}

}  // namespace fake_nvme
