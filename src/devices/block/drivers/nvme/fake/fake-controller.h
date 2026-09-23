// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_NVME_FAKE_FAKE_CONTROLLER_H_
#define SRC_DEVICES_BLOCK_DRIVERS_NVME_FAKE_FAKE_CONTROLLER_H_

#include <lib/fit/function.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/result.h>

#include <mutex>
#include <unordered_map>
#include <vector>

#include "src/devices/block/drivers/nvme/commands.h"
#include "src/devices/block/drivers/nvme/fake/fake-namespace.h"
#include "src/devices/block/drivers/nvme/fake/fake-registers.h"
#include "src/devices/block/drivers/nvme/nvme.h"
#include "src/devices/block/drivers/nvme/queue-pair.h"
#include "src/devices/block/drivers/nvme/queue.h"

namespace fake_nvme {

constexpr size_t kAdminQueueId = 0;

class FakeController {
 public:
  using CommandHandler = std::function<void(
      nvme::Submission& submission, const nvme::TransactionData& data, nvme::Completion& status)>;
  FakeController();
  // Called when a write to the submission queue doorbell register occurs.
  // queue_id - queue this submission is from
  // index - index of this submission in the queue
  // submission - contents of the submission.
  void HandleSubmission(size_t queue_id, size_t index, nvme::Submission& submission);
  // Called when a submission is finished.
  void SubmitCompletion(nvme::Completion& completion);

  // Add a command handler for the given admin opcode.
  void AddAdminCommand(uint8_t opcode, CommandHandler handler) {
    std::lock_guard<std::mutex> lock(lock_);
    admin_commands_.emplace(opcode, std::move(handler));
  }
  // Add a command handler for the given I/O opcode.
  void AddIoCommand(uint8_t opcode, CommandHandler handler) {
    std::lock_guard<std::mutex> lock(lock_);
    io_commands_.emplace(opcode, std::move(handler));
  }

  // Called when one of the Admin Queue address registers is written to.
  void UpdateAdminQueue();

  // Add a namespace to this controller.
  void AddNamespace(uint32_t nsid, FakeNamespace& ns) {
    std::lock_guard<std::mutex> lock(lock_);
    namespaces_.emplace(nsid, ns);
  }

  // Called by the test fixture to give us a pointer to the driver instance.
  // We use the driver instance to access data buffers and queues since the values written to the
  // register are fake values from fake_bti.
  void SetNvme(nvme::Nvme* nvme) { nvme_ = nvme; }

  void AddQueuePair(size_t queue_id, const nvme::Queue* completion_queue,
                    const nvme::Queue* submission_queue) {
    std::lock_guard<std::mutex> lock(lock_);
    if (completion_queue) {
      completion_queues_.emplace(
          queue_id,
          QueueState{
              .queue = completion_queue,
              .consumer_location = static_cast<uint16_t>(completion_queue->entry_count() - 1),
              .producer_location = 0,
          });
    }
    if (submission_queue) {
      submission_queues_.emplace(queue_id, QueueState{
                                               .queue = submission_queue,
                                               .consumer_location = 0,
                                               .producer_location = 0,
                                           });
    }
  }

  FakeRegisters& registers() { return regs_; }
  nvme::Nvme* nvme() { return nvme_; }
  const std::map<uint32_t, FakeNamespace&>& namespaces() const {
    std::lock_guard<std::mutex> lock(lock_);
    return namespaces_;
  }

  // Returns IRQ number |index|, and creates it if it doesn't yet exist.
  zx::result<zx::interrupt> GetOrCreateInterrupt(size_t index);

 private:
  // Controller-side information about a queue.
  struct QueueState {
    const nvme::Queue* queue;
    // Maximum available slot to fill
    // For completions, this is the value written to completion doorbell.
    // For submissions, this is the index of the last submission we handled.
    uint16_t consumer_location = 0;
    // Next available slot to fill.
    // For completions, this is updated whenever we finish a txn.
    // For submissions, this is the value written to the submission doorbell.
    uint16_t producer_location = 0;
    // Only used by completion queues. Phase bit that should be sit
    // in completion queue entries so that the NVME driver consumes them.
    uint8_t phase = 1;
  };
  // Controller-side information about an interrupt.
  class IrqState {
   public:
    explicit IrqState(zx::interrupt irq) : irq_(std::move(irq)) {}
    // Trigger the interrupt, or mark it as pending if it is disabled.
    void Trigger() {
      if (enabled_) {
        irq_.trigger(0, zx::clock::get_boot());
      } else {
        pending_ = true;
      }
    }
    // Enable the interrupt and trigger any pending interrupts.
    void Enable() {
      enabled_ = true;
      if (pending_) {
        Trigger();
        pending_ = false;
      }
    }
    // Disable the interrupt.
    void Disable() { enabled_ = false; }
    zx::unowned_interrupt irq() { return irq_.borrow(); }

   private:
    // Is this interrupt enabled?
    bool enabled_ = true;
    // Was this interrupt triggered while it was disabled?
    bool pending_ = false;
    // The actual interrupt object.
    zx::interrupt irq_;
  };

  void SetConfig(nvme::ControllerConfigReg& cfg);
  void UpdateIrqMask(bool enable, nvme::InterruptReg& state);
  void RingDoorbell(bool is_submit, size_t queue_id, nvme::DoorbellReg& reg);

  QueueState& GetQueueState(size_t queue_id, std::unordered_map<size_t, QueueState>* queues);

  // Used to maintain integrity of the map containers rather than their contents, which are used to
  // add more elements to the map in certain cases (e.g., processing a queue's command to set up
  // another queue).
  mutable std::mutex lock_;
  std::unordered_map<size_t, QueueState> completion_queues_ TA_GUARDED(lock_);
  std::unordered_map<size_t, QueueState> submission_queues_ TA_GUARDED(lock_);
  std::unordered_map<size_t, IrqState> irqs_ TA_GUARDED(lock_);
  std::unordered_map<uint8_t, CommandHandler> admin_commands_ TA_GUARDED(lock_);
  std::unordered_map<uint8_t, CommandHandler> io_commands_ TA_GUARDED(lock_);
  // This is ordered because "Get Active Namespaces" returns an ordered list of namespaces.
  std::map<uint32_t, FakeNamespace&> namespaces_ TA_GUARDED(lock_);
  nvme::Nvme* nvme_ = nullptr;

  FakeRegisters regs_;
  NvmeRegisterCallbacks callbacks_{
      .set_config = fit::bind_member(this, &FakeController::SetConfig),
      .interrupt_mask_update = fit::bind_member(this, &FakeController::UpdateIrqMask),
      .doorbell_ring = fit::bind_member(this, &FakeController::RingDoorbell),
      .admin_queue_update = fit::bind_member(this, &FakeController::UpdateAdminQueue),
  };
};

}  // namespace fake_nvme

#endif  // SRC_DEVICES_BLOCK_DRIVERS_NVME_FAKE_FAKE_CONTROLLER_H_
