// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "msd_vsi_device.h"

#include <lib/fit/defer.h>
#include <lib/magma/platform/platform_barriers.h>
#include <lib/magma/platform/platform_logger.h>
#include <lib/magma/platform/platform_mmio.h>
#include <lib/magma/platform/platform_thread.h>
#include <lib/magma/platform/platform_trace.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>

#include <algorithm>
#include <chrono>
#include <iterator>
#include <thread>

#include <fbl/string_printf.h>

#include "address_space_layout.h"
#include "command_buffer.h"
#include "instructions.h"
#include "magma_vendor_queries.h"
#include "msd_vsi_context.h"
#include "registers.h"

static constexpr uint32_t kInterruptIndex = 0;

static constexpr uint32_t kSramMmioIndex = 4;

MsdVsiDevice::~MsdVsiDevice() { Shutdown(); }

bool MsdVsiDevice::Shutdown() {
  CHECK_THREAD_NOT_CURRENT(device_thread_id_);

  DisableInterrupts();

  stop_interrupt_thread_ = true;
  if (interrupt_) {
    interrupt_->Signal();
  }
  if (interrupt_thread_.joinable()) {
    interrupt_thread_.join();
    DLOG("Joined interrupt thread");
  }

  stop_device_thread_ = true;

  if (device_request_semaphore_) {
    device_request_semaphore_->Signal();
  }

  if (device_thread_.joinable()) {
    DLOG("joining device thread");
    device_thread_.join();
    DLOG("joined");
    device_thread_id_ = nullptr;
  }

  // Ensure hardware is idle.
  if (register_io_) {
    return HardwareReset();
  }

  return true;
}

std::unique_ptr<MsdVsiDevice> MsdVsiDevice::Create(void* device_handle, bool enable_suspend) {
  auto device = std::make_unique<MsdVsiDevice>(enable_suspend);
  if (!device->Init(device_handle)) {
    MAGMA_LOG(ERROR, "Failed to initialize device");
    return nullptr;
  }
  return device;
}

bool MsdVsiDevice::Init(void* device_handle) {
  platform_device_ = MsdVsiPlatformDevice::Create(device_handle);
  if (!platform_device_) {
    MAGMA_LOG(ERROR, "Failed to create platform device");
    return false;
  }

  uint32_t mmio_count = platform_device_->platform_device()->GetMmioCount();
  DASSERT(mmio_count > 0);

  std::unique_ptr<magma::PlatformMmio> mmio = platform_device_->platform_device()->CpuMapMmio(0);
  if (!mmio) {
    MAGMA_LOG(ERROR, "failed to map registers");
    return false;
  }

  register_io_ = std::make_unique<VsiRegisterIo>(std::move(mmio), *this);

  device_id_ = registers::ChipId::Get().ReadFrom(register_io()).chip_id();
  customer_id_ = registers::CustomerId::Get().ReadFrom(register_io()).customer_id();
  chip_date_ = registers::ChipDate::Get().ReadFrom(register_io()).chip_date();
  product_id_ = registers::ProductId::Get().ReadFrom(register_io()).product_id();
  eco_id_ = registers::EcoId::Get().ReadFrom(register_io()).eco_id();
  DLOG("Detected vsi chip id 0x%x customer id 0x%x", device_id_, customer_id_);

  if (HasAxiSram()) {
    external_sram_ = platform_device_->platform_device()->GetMmioBuffer(kSramMmioIndex);
    if (!external_sram_) {
      MAGMA_LOG(ERROR, "GetMmioBuffer(%d) failed", kSramMmioIndex);
      return false;
    }

    if (!external_sram_->SetCachePolicy(MAGMA_CACHE_POLICY_WRITE_COMBINING)) {
      MAGMA_LOG(ERROR, "Failed setting cache policy on external SRAM");
      return false;
    }
  }

  if (!IsValidDeviceId()) {
    MAGMA_LOG(ERROR, "Unsupported NPU model 0x%x\n", device_id_);
    return false;
  }

  revision_ = registers::Revision::Get().ReadFrom(register_io()).chip_revision();

  gpu_features_ = std::make_unique<GpuFeatures>(register_io());
  DLOG("NPU features: 0x%x minor features 0x%x 0x%x 0x%x 0x%x 0x%x 0x%x\n",
       gpu_features_->features().reg_value(), gpu_features_->minor_features(0),
       gpu_features_->minor_features(1), gpu_features_->minor_features(2),
       gpu_features_->minor_features(3), gpu_features_->minor_features(4),
       gpu_features_->minor_features(5));
  DLOG("halti5: %d mmu: %d", gpu_features_->halti5(), gpu_features_->has_mmu());

  DLOG(
      "stream count %u register_max %u thread_count %u vertex_cache_size %u shader_core_count "
      "%u pixel_pipes %u vertex_output_buffer_size %u\n",
      gpu_features_->stream_count(), gpu_features_->register_max(), gpu_features_->thread_count(),
      gpu_features_->vertex_cache_size(), gpu_features_->shader_core_count(),
      gpu_features_->pixel_pipes(), gpu_features_->vertex_output_buffer_size());
  DLOG("instruction count %u buffer_size %u num_constants %u varyings_count %u\n",
       gpu_features_->instruction_count(), gpu_features_->buffer_size(),
       gpu_features_->num_constants(), gpu_features_->varyings_count());

  if (Has3dPipe()) {
    if (!gpu_features_->features().pipe_3d()) {
      MAGMA_LOG(ERROR, "NPU has no 3d pipe: features 0x%x\n",
                gpu_features_->features().reg_value());
      return false;
    }
  }

  bus_mapper_ = magma::PlatformBusMapper::Create(
      platform_device_->platform_device()->GetBusTransactionInitiator());
  if (!bus_mapper_) {
    MAGMA_LOG(ERROR, "failed to create bus mapper");
    return false;
  }

  page_table_arrays_ = PageTableArrays::Create(bus_mapper_.get());
  if (!page_table_arrays_) {
    MAGMA_LOG(ERROR, "failed to create page table arrays");
    return false;
  }

  // Add a page to account for ringbuffer overfetch
  uint32_t ringbuffer_size = AddressSpaceLayout::ringbuffer_size() + magma::page_size();
  DASSERT(ringbuffer_size <= AddressSpaceLayout::system_gpu_addr_size());

  auto buffer = MsdVsiBuffer::Create(ringbuffer_size, "ring-buffer");
  buffer->platform_buffer()->SetCachePolicy(MAGMA_CACHE_POLICY_UNCACHED);

  ringbuffer_ =
      std::make_unique<Ringbuffer>(std::move(buffer), AddressSpaceLayout::ringbuffer_size());

  if (!ringbuffer_->MapCpu()) {
    MAGMA_LOG(ERROR, "Failed to map cpu for ringbuffer");
    return false;
  }

  progress_ = std::make_unique<GpuProgress>();

  constexpr uint32_t kFirstSequenceNumber = 0x1;
  sequencer_ = std::make_unique<Sequencer>(kFirstSequenceNumber);

  device_request_semaphore_ = magma::PlatformSemaphore::Create();

  interrupt_ = platform_device_->platform_device()->RegisterInterrupt(kInterruptIndex);
  if (!interrupt_) {
    MAGMA_LOG(ERROR, "Failed to register interrupt");
    return false;
  }

  page_table_slot_allocator_ = std::make_unique<PageTableSlotAllocator>(page_table_arrays_->size());

  bool reset = HardwareReset();
  if (!reset) {
    MAGMA_LOG(ERROR, "Failed to reset hardware");
    return false;
  }

  HardwareInit();

  PowerSuspend();

  return true;
}

void MsdVsiDevice::HardwareInit() {
  {
    auto reg = registers::PulseEater::Get().ReadFrom(register_io());
    reg.set_disable_internal_dfs(1);
    reg.WriteTo(register_io());
  }

  {
    auto reg = registers::IrqEnable::Get().FromValue(~0);
    reg.WriteTo(register_io());
  }

  {
    auto reg = registers::SecureAhbControl::Get().ReadFrom(register_io());
    reg.set_non_secure_access(1);
    reg.WriteTo(register_io());
  }

  page_table_arrays_->HardwareInit(register_io());
}

void MsdVsiDevice::KillCurrentContext() {
  // Get the context of the batch with the lowest sequence number.
  uint32_t min_seq = UINT_MAX;
  std::shared_ptr<MsdVsiContext> context_to_kill;
  for (unsigned int i = 0; i < kNumEvents; i++) {
    if (events_[i].allocated) {
      uint32_t seq_num = events_[i].mapped_batch->GetSequenceNumber();
      if (seq_num < min_seq) {
        min_seq = seq_num;
        context_to_kill = events_[i].mapped_batch->GetContext().lock();
      }
    }
  }
  if (context_to_kill) {
    context_to_kill->Kill();
  }
}

void MsdVsiDevice::Reset() {
  HardwareReset();

  // Save the pending batches that have been posted to the ringbuffer.
  std::vector<DeferredRequest> pending_batches;
  for (unsigned int i = 0; i < kNumEvents; i++) {
    if (events_[i].allocated) {
      auto context = events_[i].mapped_batch->GetContext().lock();
      if (context && !context->killed()) {
        // Since we are going to reset the hardware state, the TLB should be invalidated.
        // |SubmitCommandBuffer| will determine if flushing is required when switching address
        // spaces.
        pending_batches.emplace_back(
            DeferredRequest{std::move(events_[i].mapped_batch), false /* do_flush */});
      }
      CompleteInterruptEvent(i);
    }
  }

  // Ensure the batches will be requeued in the same order.
  std::sort(pending_batches.begin(), pending_batches.end(),
            [](const DeferredRequest& a, const DeferredRequest& b) {
              return a.batch->GetSequenceNumber() < b.batch->GetSequenceNumber();
            });

  // Prepend these batches to the backlog, which is processed before the device request list.
  request_backlog_.insert(request_backlog_.begin(),
                          std::make_move_iterator(pending_batches.begin()),
                          std::make_move_iterator(pending_batches.end()));

  ringbuffer_->Reset(0);
  configured_address_space_ = nullptr;
  progress_ = std::make_unique<GpuProgress>();

  HardwareInit();
}

void MsdVsiDevice::DisableInterrupts() {
  if (!register_io_) {
    DLOG("Register io was not initialized, skipping disabling interrupts");
    return;
  }
  auto reg = registers::IrqEnable::Get().FromValue(0);
  reg.WriteTo(register_io());
}

void MsdVsiDevice::HangCheckTimeout() {
  std::vector<std::string> dump;
  DumpToString(&dump, false /* fault_present */);

  MAGMA_LOG(WARNING, "Suspected NPU hang:");
  MAGMA_LOG(WARNING, "last_interrupt_timestamp %lu", last_interrupt_timestamp_.load());
  MAGMA_LOG(WARNING, "Power state %u", static_cast<unsigned int>(power_state_));

  for (auto& str : dump) {
    MAGMA_LOG(WARNING, "%s", str.c_str());
  }
  KillCurrentContext();
  Reset();

  ProcessRequestBacklog();
}

void MsdVsiDevice::StartDeviceThread() {
  DASSERT(!device_thread_.joinable());
  device_thread_ = std::thread([this] { this->DeviceThreadLoop(); });
  interrupt_thread_ = std::thread([this] { this->InterruptThreadLoop(); });
}

int MsdVsiDevice::DeviceThreadLoop() {
  magma::PlatformThreadHelper::SetCurrentThreadName("DeviceThread");

  device_thread_id_ = std::make_unique<magma::PlatformThreadId>();
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  DLOG("DeviceThreadLoop starting thread 0x%lx", device_thread_id_->id());

  const char* kRoleName = "fuchsia.graphics.drivers.msd-vsi-vip.device";
  if (!magma::PlatformThreadHelper::SetRole(platform_device_->platform_device()->GetDeviceHandle(),
                                            kRoleName)) {
    MAGMA_LOG(ERROR, "Failed to set device thread role: %s", kRoleName);
    return 0;
  }

  std::unique_lock<std::mutex> lock(device_request_mutex_, std::defer_lock);

  while (!stop_device_thread_) {
    constexpr uint32_t kTimeoutMs = 6000;

    auto timeout = std::chrono::duration_cast<std::chrono::milliseconds>(
        progress_->GetHangcheckTimeout(kTimeoutMs, std::chrono::steady_clock::now()));

    constexpr uint32_t kWaitForSuspendMs = 10;
    // If there are no more command buffers to execute wait before suspending
    if (enable_suspend_) {
      if (progress_->IsIdle() && power_state_ != PowerState::kSuspended) {
        timeout = std::chrono::milliseconds(kWaitForSuspendMs);
      }
    }

    magma::Status status = device_request_semaphore_->Wait(timeout.count());
    switch (status.get()) {
      case MAGMA_STATUS_OK:
        break;
      case MAGMA_STATUS_TIMED_OUT: {
        // Check that there are no pending device requests.
        lock.lock();
        bool empty = device_request_list_.empty();
        lock.unlock();
        if (!empty) {
          break;
        }

        if (enable_suspend_ && timeout == std::chrono::milliseconds(kWaitForSuspendMs)) {
          if (progress_->IsIdle() && power_state_ != PowerState::kSuspended) {
            StopRingBufferAndSuspend();
          }
          break;
        }

        HangCheckTimeout();
      } break;
      default:
        MAGMA_LOG(WARNING, "device_request_semaphore_ Wait failed: %d", status.get());
        DASSERT(false);
        // TODO(https://fxbug.dev/42120893): handle wait errors.
    }

    while (true) {
      lock.lock();
      if (!device_request_list_.size()) {
        lock.unlock();
        break;
      }

      auto request = std::move(device_request_list_.front());
      device_request_list_.pop_front();
      lock.unlock();
      request->ProcessAndReply(this);
    }
  }

  DLOG("DeviceThreadLoop exit");
  return 0;
}

void MsdVsiDevice::EnqueueDeviceRequest(std::unique_ptr<DeviceRequest> request) {
  std::unique_lock<std::mutex> lock(device_request_mutex_);

  // Interrupts are higher priority and placed at the front of the queue in FIFO order
  if (request->RequestType() == InterruptRequest::kRequestType) {
    if (device_request_list_.empty()) {
      device_request_list_.emplace_front(std::move(request));
    } else {
      for (auto it = device_request_list_.begin();; ++it) {
        if (it == device_request_list_.end()) {
          device_request_list_.emplace_back(std::move(request));
          break;
        } else if (it->get()->RequestType() != InterruptRequest::kRequestType) {
          device_request_list_.emplace(it, std::move(request));
          break;
        }
      }
    }
  } else {
    device_request_list_.emplace_back(std::move(request));
  }

  device_request_semaphore_->Signal();
}

int MsdVsiDevice::InterruptThreadLoop() {
  magma::PlatformThreadHelper::SetCurrentThreadName("VSI InterruptThread");
  DLOG("VSI Interrupt thread started");

  const char* kRoleName = "fuchsia.graphics.drivers.msd-vsi-vip.vsi-interrupt";
  if (!magma::PlatformThreadHelper::SetRole(platform_device_->platform_device()->GetDeviceHandle(),
                                            kRoleName)) {
    MAGMA_LOG(ERROR, "Failed to set interrupt thread role: %s", kRoleName);
    return 0;
  }

  while (!stop_interrupt_thread_) {
    interrupt_->Wait();

    if (stop_interrupt_thread_) {
      break;
    }

    last_interrupt_timestamp_ = magma::get_monotonic_ns();

    // In the field (b/280363833) we observe a crash while reading from IrqAck,
    // which indicates the hardware is suspended. This should not be possible
    // because we should not be suspending the hardware while there is work in
    // in progress. To prevent the crash we do PowerOn() here as a temporary measure
    // to reduce the impact of having the driver crash in the field.
    if (power_state() != PowerState::kOn) {
      MAGMA_LOG(ERROR, "Processing Interrupt with power state 0x%x",
                static_cast<unsigned int>(power_state()));
      PowerOn();
    }
    auto irqack = registers::IrqAck::Get().ReadFrom(register_io_.get());
    interrupt_->Complete();

    auto request = std::make_unique<InterruptRequest>(std::move(irqack));
    EnqueueDeviceRequest(std::move(request));
  }
  DLOG("VSI Interrupt thread exiting");
  return 0;
}

magma::Status MsdVsiDevice::ProcessInterrupt(registers::IrqAck irq_status) {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);
  auto mmu_exception = irq_status.mmu_exception();
  auto bus_error = irq_status.bus_error();
  auto value = irq_status.value();
  bool do_dump = false;
  if (mmu_exception) {
    MAGMA_LOG(ERROR, "Interrupt thread received mmu_exception");
    do_dump = true;
  }
  if (bus_error) {
    MAGMA_LOG(ERROR, "Interrupt thread received bus error");
  }

  // Though events complete in order, we may receive a single interrupt for multiple events
  // simultaneously. We should update the ringbuffer head following the event with the
  // highest sequence number.
  uint32_t max_seq_num = 0;
  uint32_t rb_new_head = kInvalidRingbufferOffset;
  // Check which bits are set and complete the corresponding event.
  for (unsigned int i = 0; i < kNumEvents; i++) {
    if (value & (1 << i)) {
      const auto& batch = events_[i].mapped_batch;
      // This should never be null as |WriteInterruptEvent| does not allow it.
      // Ignore it in case it's a spurious interrupt.
      if (!batch) {
        MAGMA_LOG(
            ERROR,
            "Ignoring interrupt, event %u did not have an associated mapped batch, allocated: %d "
            "submitted: %d",
            i, events_[i].allocated, events_[i].submitted);
        do_dump = true;
        continue;
      }

      if (batch->IsCommandBuffer()) {
        auto* buffer = static_cast<CommandBuffer*>(batch.get())->GetBatchBuffer();
        TRACE_VTHREAD_DURATION_END("magma", "Command Buffer", "NPU", buffer->id(),
                                   magma::PlatformTrace::GetCurrentTicks(), "id", buffer->id());
      }

      if (batch->GetSequenceNumber() > max_seq_num) {
        max_seq_num = batch->GetSequenceNumber();
        rb_new_head = events_[i].ringbuffer_offset;
      }
      if (!CompleteInterruptEvent(i)) {
        MAGMA_LOG(ERROR, "Failed to complete event %u", i);
      }
    }
  }
  if (max_seq_num) {
    DASSERT(rb_new_head != kInvalidRingbufferOffset);
    ringbuffer_->update_head(rb_new_head);
    progress_->Completed(max_seq_num, std::chrono::steady_clock::now());
  } else {
    MAGMA_LOG(ERROR, "Interrupt thread did not find any interrupt events");
    do_dump = true;
  }
  if (do_dump) {
    std::vector<std::string> dump;
    DumpToString(&dump, mmu_exception /* fault_present */);
    for (auto& str : dump) {
      MAGMA_LOG(WARNING, "%s", str.c_str());
    }
  }

  if (mmu_exception) {
    KillCurrentContext();
    Reset();
  }

  ProcessRequestBacklog();

  return MAGMA_STATUS_OK;
}

magma::Status MsdVsiDevice::ProcessDumpStatusToLog() {
  std::vector<std::string> dump;
  // Faults are detected on the interrupt thread.
  DumpToString(&dump, false /* fault_present */);
  registers::PrintRegisters(&dump, register_io());
  for (auto& str : dump) {
    MAGMA_LOG(INFO, "%s", str.c_str());
  }
  return MAGMA_STATUS_OK;
}

void MsdVsiDevice::ProcessRequestBacklog() {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  while (!request_backlog_.empty()) {
    uint32_t event_id;
    if (!AllocInterruptEvent(true /* free_on_complete */, &event_id)) {
      // No more events available, we will continue processing after the next interrupt.
      return;
    }
    // Free the interrupt event if submitting fails.
    auto free_event = fit::defer([this, event_id]() { FreeInterruptEvent(event_id); });

    auto request = std::move(request_backlog_.front());
    request_backlog_.pop_front();

    auto context = request.batch->GetContext().lock();
    if (!context) {
      DMESSAGE("No context for batch %lu, IsCommandBuffer=%d", request.batch->GetBatchBufferId(),
               request.batch->IsCommandBuffer());
      // If a batch fails, we will drop it and try the next one.
      continue;
    }
    auto address_space = context->exec_address_space();

    if (!SubmitCommandBuffer(context, address_space->page_table_array_slot(), request.do_flush,
                             std::move(request.batch), event_id)) {
      DMESSAGE("Failed to submit command buffer");
      continue;
    }
    free_event.cancel();
  }
}

bool MsdVsiDevice::IsSuspendSupported() const { return enable_suspend_; }

void MsdVsiDevice::PowerOn() {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);
  if (power_state_ == PowerState::kOn) {
    return;
  }

  auto clock_control = registers::ClockControl::Get().FromValue(0);

  clock_control.set_clk3d_dis(0);
  clock_control.set_clk2d_dis(0);
  clock_control.set_fscale_val(registers::ClockControl::kFscaleOn);
  clock_control.set_fscale_cmd_load(1);
  clock_control.WriteTo(register_io_.get());

  clock_control.set_fscale_cmd_load(0);
  clock_control.WriteTo(register_io_.get());

  power_state_ = PowerState::kOn;

  DLOG("NNA on");
}

void MsdVsiDevice::PowerSuspend() {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  if (!enable_suspend_ || power_state_ == PowerState::kSuspended) {
    return;
  }

  auto clock_control = registers::ClockControl::Get().FromValue(0);

  clock_control.set_clk3d_dis(1);
  clock_control.set_clk2d_dis(1);
  clock_control.set_fscale_val(registers::ClockControl::kFscaleSuspend);
  clock_control.set_fscale_cmd_load(1);
  clock_control.WriteTo(register_io_.get());

  clock_control.set_fscale_cmd_load(0);
  clock_control.WriteTo(register_io_.get());

  power_state_ = PowerState::kSuspended;

  DLOG("NNA suspended");
}

void MsdVsiDevice::StopRingBufferAndSuspend() {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  if (!StopRingbuffer()) {
    MAGMA_LOG(ERROR, "Stop ring buffer for suspend failed");
    DASSERT(false);
  }

  constexpr uint32_t kTimeoutMs = 100;
  if (!WaitUntilIdle(kTimeoutMs)) {
    MAGMA_LOG(WARNING, "Timeout stopping ringbuffer for suspend");
    DASSERT(false);
  } else {
    PowerSuspend();
  }
}

bool MsdVsiDevice::AllocInterruptEvent(bool free_on_complete, uint32_t* out_event_id) {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  for (uint32_t i = 0; i < kNumEvents; i++) {
    if (!events_[i].allocated) {
      events_[i].allocated = true;
      events_[i].free_on_complete = free_on_complete;
      *out_event_id = i;
      return true;
    }
  }

  MAGMA_LOG(ERROR, "No events are currently available");
  return false;
}

bool MsdVsiDevice::FreeInterruptEvent(uint32_t event_id) {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  if (event_id >= kNumEvents) {
    MAGMA_LOG(ERROR, "Invalid event id %u", event_id);
    return false;
  }
  if (!events_[event_id].allocated) {
    MAGMA_LOG(ERROR, "Event id %u was not allocated", event_id);
    return false;
  }
  events_[event_id] = {};

  return true;
}

// Writes an event into the end of the ringbuffer.
bool MsdVsiDevice::WriteInterruptEvent(uint32_t event_id, std::unique_ptr<MappedBatch> mapped_batch,
                                       std::shared_ptr<AddressSpace> prev_address_space) {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  if (event_id >= kNumEvents) {
    MAGMA_LOG(ERROR, "Invalid event id %u", event_id);
    return false;
  }
  if (!events_[event_id].allocated) {
    MAGMA_LOG(ERROR, "Event id %u was not allocated", event_id);
    return false;
  }
  if (events_[event_id].submitted) {
    MAGMA_LOG(ERROR, "Event id %u was already submitted", event_id);
    return false;
  }
  if (!mapped_batch) {
    MAGMA_LOG(ERROR, "No mapped batch was provided");
    return false;
  }
  events_[event_id].submitted = true;
  events_[event_id].mapped_batch = std::move(mapped_batch);
  events_[event_id].prev_address_space = prev_address_space;
  MiEvent::write(ringbuffer_.get(), event_id);

  // Save the ringbuffer offset immediately after this event.
  events_[event_id].ringbuffer_offset = ringbuffer_->tail();
  return true;
}

bool MsdVsiDevice::CompleteInterruptEvent(uint32_t event_id) {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  if (event_id >= kNumEvents) {
    MAGMA_LOG(ERROR, "Invalid event id %u", event_id);
    return false;
  }
  if (!events_[event_id].allocated || !events_[event_id].submitted) {
    MAGMA_LOG(ERROR, "Cannot complete event %u, allocated %u submitted %u", event_id,
              events_[event_id].allocated, events_[event_id].submitted);
    return false;
  }
  num_events_completed_++;

  bool free_on_complete = events_[event_id].free_on_complete;
  events_[event_id] = {};
  events_[event_id].allocated = !free_on_complete;

  return true;
}

bool MsdVsiDevice::HardwareReset() {
  DLOG("HardwareReset start");
  // This number was chosen arbitrarily and can be adjusted as needed.
  constexpr uint32_t kMaxResetAttempts = 3;

  bool initial_reset_succeeded = false;
  uint32_t failed_resets = 0;
  while (failed_resets < kMaxResetAttempts) {
    if (initial_reset_succeeded) {
      // We have been seeing issues where a soft reset is not enough to restart the hardware and
      // the hardware is hung forever. A simple fix for this is to fully reset the device's power.
      // The device needs to be idle to do a power reset, so we have to do one soft reset first.
      // The flow is: Soft Reset -> Hard Reset -> Soft Reset.
      magma_status_t status = platform_device_->ResetPower();
      if (status != MAGMA_STATUS_OK) {
        MAGMA_LOG(ERROR, "Failed to reset power");
      }
    }

    // Power on is needed to write to pulse-eater.
    PowerOn();

    // Disable clock gating
    registers::PowerModule::Get().FromValue(0x0).WriteTo(register_io());

    // Disable pulse eater.
    {
      auto pulse = registers::PulseEater::Get().FromValue(0x15b0880);
      pulse.WriteTo(register_io());
      pulse.set_disable(1);
      pulse.WriteTo(register_io());
    }

    auto clock_control = registers::ClockControl::Get().FromValue(0);
    clock_control.set_isolate_gpu(1);
    clock_control.WriteTo(register_io_.get());

    power_state_ = PowerState::kUnknown;

    {
      auto reg = registers::SecureAhbControl::Get().FromValue(0);
      reg.set_reset(1);
      reg.WriteTo(register_io());
    }

    std::this_thread::sleep_for(std::chrono::microseconds(1000));

    clock_control.set_soft_reset(0);
    clock_control.WriteTo(register_io_.get());

    clock_control.set_isolate_gpu(0);
    clock_control.WriteTo(register_io_.get());

    clock_control = registers::ClockControl::Get().ReadFrom(register_io_.get());

    bool is_idle = IsIdle();
    bool is_idle_3d = clock_control.idle_3d();
    // Try again if we did not become idle.
    if (!is_idle || !is_idle_3d) {
      failed_resets += 1;
      MAGMA_LOG(WARNING, "Hardware reset attempt failed %d/%d: is_idle %d is_idle_3d %d",
                failed_resets, kMaxResetAttempts, is_idle, is_idle_3d);
      continue;
    }

    // If we haven't power cycled yet then go back through the loop.
    if (!initial_reset_succeeded) {
      initial_reset_succeeded = true;
      continue;
    }

    DLOG("HardwareReset complete");
    return true;
  }

  MAGMA_LOG(WARNING, "Hardware reset failed fully, returning false");
  return false;
}

bool MsdVsiDevice::IsIdle() {
  return registers::IdleState::Get().ReadFrom(register_io_.get()).IsIdle();
}

bool MsdVsiDevice::StopRingbuffer() {
  if (IsIdle()) {
    return true;
  }
  // Overwrite the last WAIT with an END.
  uint32_t prev_wait_link = ringbuffer_->SubtractOffset(kWaitLinkDwords * sizeof(uint32_t));
  if (!ringbuffer_->Overwrite32(prev_wait_link, MiEnd::kCommandType)) {
    MAGMA_LOG(ERROR, "Failed to overwrite WAIT in ringbuffer");
    return false;
  }

  DLOG("Ringbuffer stopped (0x%X)", prev_wait_link);

  return true;
}

bool MsdVsiDevice::WaitUntilIdle(uint32_t timeout_ms) {
  TRACE_DURATION("magma", "WaitUntilIdle");

  auto start = std::chrono::high_resolution_clock::now();
  while (std::chrono::duration_cast<std::chrono::milliseconds>(
             std::chrono::high_resolution_clock::now() - start)
             .count() < timeout_ms) {
    if (IsIdle()) {
      return true;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(1));
  }
  auto idle_state = registers::IdleState::Get().ReadFrom(register_io_.get()).reg_value();

  MAGMA_LOG(ERROR, "WaitUntilIdle failed, IdleState register: 0x%x", idle_state);
  return false;
}

bool MsdVsiDevice::LoadInitialAddressSpace(std::shared_ptr<MsdVsiContext> context,
                                           uint32_t address_space_index) {
  // Ensure NNA is on before register access.
  PowerOn();

  // Check if we have already configured an address space and enabled the MMU.
  if (page_table_arrays_->IsEnabled(register_io())) {
    MAGMA_LOG(ERROR, "MMU already enabled");
    return false;
  }

  constexpr uint32_t kPageCount = 1;
  const size_t page_size = zx_system_get_page_size();
  std::unique_ptr<magma::PlatformBuffer> buffer =
      magma::PlatformBuffer::Create(kPageCount * page_size, "address space config");
  if (!buffer) {
    MAGMA_LOG(ERROR, "failed to create buffer");
    return false;
  }

  auto bus_mapping = GetBusMapper()->MapPageRangeBus(buffer.get(), 0, kPageCount);
  if (!bus_mapping) {
    MAGMA_LOG(ERROR, "failed to create bus mapping");
    return false;
  }

  uint32_t* cmd_ptr;
  if (!buffer->MapCpu(reinterpret_cast<void**>(&cmd_ptr))) {
    MAGMA_LOG(ERROR, "failed to map command buffer");
    return false;
  }

  BufferWriter buf_writer(cmd_ptr, magma::to_uint32(buffer->size()), 0);
  auto reg = registers::MmuPageTableArrayConfig::Get().addr();
  MiLoadState::write(&buf_writer, reg, address_space_index);
  MiEnd::write(&buf_writer);

  if (!buffer->UnmapCpu()) {
    MAGMA_LOG(ERROR, "failed to unmap cpu");
    return false;
  }
  if (!buffer->CleanCache(0, page_size * kPageCount, false)) {
    MAGMA_LOG(ERROR, "failed to clean buffer cache");
    return false;
  }

  auto res =
      SubmitCommandBufferNoMmu(bus_mapping->Get()[0], magma::to_uint32(buf_writer.bytes_written()));
  if (!res) {
    MAGMA_LOG(ERROR, "failed to submit command buffer");
    return false;
  }
  constexpr uint32_t kTimeoutMs = 1000;
  if (!WaitUntilIdle(kTimeoutMs)) {
    MAGMA_LOG(ERROR, "failed to wait for device to be idle: addr 0x%08lx size 0x%x",
              bus_mapping->Get()[0], magma::to_uint32(buf_writer.bytes_written()));
    return false;
  }

  page_table_arrays_->Enable(register_io(), true);

  DLOG("Address space loaded, index %u", address_space_index);

  configured_address_space_ = context->exec_address_space();

  return true;
}

bool MsdVsiDevice::SubmitCommandBufferNoMmu(uint64_t bus_addr, uint32_t length,
                                            uint16_t* prefetch_out) {
  if (bus_addr & 0xFFFFFFFF00000000ul) {
    MAGMA_LOG(ERROR, "Can't submit address > 32 bits without mmu: 0x%08lx", bus_addr);
    return false;
  }

  uint32_t prefetch =
      magma::round_up(length, static_cast<uint32_t>(sizeof(uint64_t))) / sizeof(uint64_t);
  if (prefetch & 0xFFFF0000) {
    MAGMA_LOG(ERROR, "Can't submit length %u (prefetch 0x%x)", length, prefetch);
    return false;
  }

  prefetch &= 0xFFFF;
  if (prefetch_out) {
    *prefetch_out = static_cast<uint16_t>(prefetch);
  }

  DLOG("Submitting buffer at bus addr 0x%lx", bus_addr);

  // Ensure NNA is on before register access.
  PowerOn();

  auto reg_cmd_addr = registers::FetchEngineCommandAddress::Get().FromValue(0);
  reg_cmd_addr.set_addr(bus_addr & 0xFFFFFFFF);

  auto reg_cmd_ctrl = registers::FetchEngineCommandControl::Get().FromValue(0);
  reg_cmd_ctrl.set_enable(1);
  reg_cmd_ctrl.set_prefetch(prefetch);

  auto reg_sec_cmd_ctrl = registers::SecureCommandControl::Get().FromValue(0);
  reg_sec_cmd_ctrl.set_enable(1);
  reg_sec_cmd_ctrl.set_prefetch(prefetch);

  reg_cmd_addr.WriteTo(register_io());
  reg_cmd_ctrl.WriteTo(register_io());
  reg_sec_cmd_ctrl.WriteTo(register_io());

  return true;
}

bool MsdVsiDevice::StartRingbuffer(std::shared_ptr<MsdVsiContext> context) {
  if (!IsIdle()) {
    return true;  // Already running and looping on WAIT-LINK.
  }
  DLOG("Starting ringbuffer");

  // On a restart of the RingBuffer the buffer needs resetting.
  ringbuffer_->Reset(0);

  uint64_t rb_gpu_addr;
  bool res = context->exec_address_space()->GetRingbufferGpuAddress(&rb_gpu_addr);
  if (!res) {
    MAGMA_LOG(ERROR, "Could not get ringbuffer NPU address");
    return false;
  }

  const uint16_t kRbPrefetch = 2;
  // Write the initial WAIT-LINK to the ringbuffer. The LINK points back to the WAIT,
  // and will keep looping until the WAIT is replaced with a LINK on command buffer submission.
  uint32_t wait_gpu_addr = magma::to_uint32(rb_gpu_addr + ringbuffer_->tail());
  MiWait::write(ringbuffer_.get());
  MiLink::write(ringbuffer_.get(), kRbPrefetch, wait_gpu_addr);

  auto reg_cmd_addr = registers::FetchEngineCommandAddress::Get().FromValue(0);
  reg_cmd_addr.set_addr(static_cast<uint32_t>(wait_gpu_addr));

  auto reg_cmd_ctrl = registers::FetchEngineCommandControl::Get().FromValue(0);
  reg_cmd_ctrl.set_enable(1);
  reg_cmd_ctrl.set_prefetch(kRbPrefetch);

  auto reg_sec_cmd_ctrl = registers::SecureCommandControl::Get().FromValue(0);
  reg_sec_cmd_ctrl.set_enable(1);
  reg_sec_cmd_ctrl.set_prefetch(kRbPrefetch);

  reg_cmd_addr.WriteTo(register_io());
  reg_cmd_ctrl.WriteTo(register_io());
  reg_sec_cmd_ctrl.WriteTo(register_io());

  DLOG("Ringbuffer started (0x%X)", wait_gpu_addr);

  return true;
}

bool MsdVsiDevice::AddRingbufferWaitLink() {
  uint64_t rb_gpu_addr;
  bool res = configured_address_space_->GetRingbufferGpuAddress(&rb_gpu_addr);
  if (!res) {
    MAGMA_LOG(ERROR, "Failed to get ringbuffer NPU address");
    return false;
  }
  uint32_t wait_gpu_addr = magma::to_uint32(rb_gpu_addr) + ringbuffer_->tail();
  MiWait::write(ringbuffer_.get());
  MiLink::write(ringbuffer_.get(), 2 /* prefetch */, wait_gpu_addr);
  return true;
}

void MsdVsiDevice::LinkRingbuffer(uint32_t wait_link_offset, uint32_t gpu_addr,
                                  uint32_t dest_prefetch) {
  DASSERT(ringbuffer_->IsOffsetPopulated(wait_link_offset));
  // We can assume the instruction was written as 8 contiguous bytes.
  DASSERT(ringbuffer_->IsOffsetPopulated(wait_link_offset + sizeof(uint32_t)));

  // Replace the penultimate WAIT (before the newly added one) with a LINK to the command buffer.
  // We will first modify the second dword which specifies the address,
  // as the hardware may be executing at the address of the current WAIT.
  ringbuffer_->Overwrite32(wait_link_offset + sizeof(uint32_t), gpu_addr);
  magma::barriers::Barrier();
  ringbuffer_->Overwrite32(wait_link_offset, MiLink::kCommandType | dest_prefetch);
  magma::barriers::Barrier();
}

bool MsdVsiDevice::WriteLinkCommand(magma::PlatformBuffer* buf, uint32_t write_offset,
                                    uint16_t link_prefetch, uint32_t link_addr) {
  // Check if we have enough space for the LINK command.
  uint32_t link_instr_size = kInstructionDwords * sizeof(uint32_t);

  if (buf->size() < write_offset + link_instr_size) {
    MAGMA_LOG(ERROR, "Buffer does not have %d free bytes for ringbuffer LINK", link_instr_size);
    return false;
  }

  uint32_t* buf_cpu_addr;
  bool res = buf->MapCpu(reinterpret_cast<void**>(&buf_cpu_addr));
  if (!res) {
    MAGMA_LOG(ERROR, "Failed to map command buffer");
    return false;
  }

  BufferWriter buf_writer(buf_cpu_addr, magma::to_uint32(buf->size()), write_offset);
  MiLink::write(&buf_writer, link_prefetch, link_addr);
  if (!buf->UnmapCpu()) {
    MAGMA_LOG(ERROR, "Failed to unmap command buffer");
    return false;
  }
  return true;
}

bool MsdVsiDevice::SubmitFlushTlb(std::shared_ptr<MsdVsiContext> context) {
  // It's possible we may need to switch to the address space of |context|. We will use the
  // currently configured address space until the switch occurs. The ringbuffer should already be
  // mapped.
  DASSERT(configured_address_space_);
  uint64_t rb_gpu_addr;
  bool res = configured_address_space_->GetRingbufferGpuAddress(&rb_gpu_addr);
  if (!res) {
    MAGMA_LOG(ERROR, "Failed to get ringbuffer NPU address");
    return false;
  }

  // Save the previous WAIT LINK which will be replaced with a LINK jumping to the new commands.
  uint32_t prev_wait_link = ringbuffer_->SubtractOffset(kWaitLinkDwords * sizeof(uint32_t));

  uint32_t prefetch = kRbInstructionsPerFlush;
  bool switch_address_space =
      configured_address_space_.get() != context->exec_address_space().get();
  if (switch_address_space) {
    // Need to add an additional instruction to load the address space.
    prefetch++;
  }
  // We need to write the new block of ringbuffer instructions contiguously.
  // Since only 30 concurrent events are supported, it should not be possible to run out
  // of space in the ringbuffer.
  bool reserved = ringbuffer_->ReserveContiguous(prefetch * sizeof(uint64_t));
  DASSERT(reserved);

  // Save the gpu address pointing to the new instructions so we can link to it.
  uint32_t new_rb_instructions_start_offset = ringbuffer_->tail();
  uint32_t gpu_addr = magma::to_uint32(rb_gpu_addr + new_rb_instructions_start_offset);

  if (switch_address_space) {
    auto reg = registers::MmuPageTableArrayConfig::Get().addr();
    MiLoadState::write(ringbuffer_.get(), reg,
                       context->exec_address_space()->page_table_array_slot());
    configured_address_space_ = context->exec_address_space();
  }
  auto reg = registers::MmuConfig::Get().addr();
  // The MmuConfig register can also be used to change modes.
  // Instruct the hardware to ignore mode change bits.
  constexpr uint32_t kModeMask = 0x8;
  constexpr uint32_t kFlushAllTlbs = 0x10;
  constexpr uint32_t flush_command = kModeMask | kFlushAllTlbs;
  MiLoadState::write(ringbuffer_.get(), reg, flush_command);
  // These additional bits appear to be needed to ensure the fetch engine waits for any
  // address space change to complete.
  constexpr uint32_t kWaitAddressSpaceChange = 0x3 << 28;
  MiSemaphore::write(ringbuffer_.get(), MiRecipient::FetchEngine, MiRecipient::PixelEngine,
                     kWaitAddressSpaceChange);
  MiStall::write(ringbuffer_.get(), MiRecipient::FetchEngine, MiRecipient::PixelEngine,
                 kWaitAddressSpaceChange);

  if (!AddRingbufferWaitLink()) {
    MAGMA_LOG(ERROR, "Failed to get ringbuffer NPU address");
    return false;
  }

  // Verify the number of instructions we just wrote matches the prefetch value
  // of the user buffer's LINK command.
  DASSERT(new_rb_instructions_start_offset ==
          ringbuffer_->SubtractOffset(prefetch * sizeof(uint64_t)));

  DLOG("Submitting flush TLB command");

  LinkRingbuffer(prev_wait_link, gpu_addr, prefetch);

  return true;
}

// When submitting a command buffer, we modify the following:
//  1) add a LINK from the command buffer to the end of the ringbuffer
//  2) add an EVENT and WAIT-LINK pair to the end of the ringbuffer
//  3) modify the penultimate WAIT in the ringbuffer to LINK to the command buffer
bool MsdVsiDevice::SubmitCommandBuffer(std::shared_ptr<MsdVsiContext> context,
                                       uint32_t address_space_index, bool do_flush,
                                       std::unique_ptr<MappedBatch> mapped_batch,
                                       uint32_t event_id) {
  if (context->killed()) {
    MAGMA_LOG(ERROR, "Context killed");
    return false;
  }

  auto kill_context = fit::defer([context]() { context->Kill(); });

  // Ensure NNA is on before register access.
  PowerOn();

  // Check if we have loaded an address space and enabled the MMU.
  bool initial_address_space_loaded = page_table_arrays_->IsEnabled(register_io());
  if (!initial_address_space_loaded) {
    if (!LoadInitialAddressSpace(context, address_space_index)) {
      MAGMA_LOG(ERROR, "Failed to load initial address space");
      return false;
    }
  }
  // Check if we have started the ringbuffer WAIT-LINK loop.
  if (IsIdle()) {
    if (!StartRingbuffer(context)) {
      MAGMA_LOG(ERROR, "Failed to start ringbuffer");
      return false;
    }
  }
  // Check if we need to switch address spaces. We should also save this copy before
  // any possible address space switch happens in |SubmitFlushTlb|.
  auto prev_address_space = configured_address_space_;
  // We always save the last address space the ringbuffer was mapped to, as we need
  // to keep the previous address space alive until the switch is completed by the hardware.
  DASSERT(prev_address_space);
  bool switch_address_space = prev_address_space.get() != context->exec_address_space().get();
  do_flush |= switch_address_space;
  if (do_flush && !SubmitFlushTlb(context)) {
    MAGMA_LOG(ERROR, "Failed to submit flush tlb command");
    return false;
  }
  uint64_t rb_gpu_addr;
  bool res = context->exec_address_space()->GetRingbufferGpuAddress(&rb_gpu_addr);
  if (!res) {
    MAGMA_LOG(ERROR, "Failed to get ringbuffer NPU address");
    return false;
  }
  uint32_t gpu_addr = magma::to_uint32(mapped_batch->GetGpuAddress());
  uint32_t length = magma::to_uint32(magma::round_up(mapped_batch->GetLength(), sizeof(uint64_t)));

  // Number of new commands to be added to the ringbuffer - EVENT WAIT LINK.
  const uint16_t kRbPrefetch = kRbInstructionsPerBatch;
  uint32_t prev_wait_link = ringbuffer_->SubtractOffset(kWaitLinkDwords * sizeof(uint32_t));

  // We need to write the new block of ringbuffer instructions contiguously.
  // Since only 30 concurrent events are supported, it should not be possible to run out
  // of space in the ringbuffer.
  bool reserved = ringbuffer_->ReserveContiguous(kRbPrefetch * sizeof(uint64_t));
  DASSERT(reserved);

  // Calculate where to jump to after completion of the command buffer.
  // This will point to EVENT WAIT LINK.
  uint32_t rb_complete_addr = magma::to_uint32(rb_gpu_addr + ringbuffer_->tail());

  bool is_cmd_buf = mapped_batch->IsCommandBuffer();
  if (is_cmd_buf) {
    auto* command_buf = static_cast<CommandBuffer*>(mapped_batch.get());
    magma::PlatformBuffer* buf = command_buf->GetBatchBuffer();

    TRACE_VTHREAD_DURATION_BEGIN("magma", "Command Buffer", "NPU", buf->id(),
                                 magma::PlatformTrace::GetCurrentTicks(), "id", buf->id());

    uint32_t write_offset = command_buf->GetBatchBufferWriteOffset();

    // Write a LINK at the end of the command buffer that links back to the ringbuffer.
    if (!WriteLinkCommand(buf, write_offset, kRbPrefetch, rb_complete_addr)) {
      MAGMA_LOG(ERROR, "Failed to write LINK from command buffer to ringbuffer");
      return false;
    }
    // Increment the command buffer length to account for the LINK command size.
    length += (kInstructionDwords * sizeof(uint32_t));

    auto prev_executed_context = prev_executed_context_.lock();
    if (!prev_executed_context || (prev_executed_context != context)) {
      auto csb = command_buf->GetContextStateBufferResource();
      if (csb) {
        auto csb_mapping = command_buf->GetContextStateBufferMapping();
        DASSERT(csb_mapping);
        // |gpu_addr| and |length| currently point to the command buffer which the ringbuffer
        // will be linking to at the end of this function. We want the ringbuffer to link
        // to the CSB instead, and the CSB to link to the command buffer.
        uint32_t cmd_buf_prefetch =
            magma::round_up(length, static_cast<uint32_t>(sizeof(uint64_t))) / sizeof(uint64_t);
        if (cmd_buf_prefetch & 0xFFFF0000) {
          MAGMA_LOG(ERROR, "Can't submit length %u (prefetch 0x%x)", length, cmd_buf_prefetch);
          return false;
        }
        // Write a LINK at the end of the context state buffer that links to the command buffer.
        uint32_t csb_length = magma::to_uint32(magma::round_up(csb->length, sizeof(uint64_t)));
        bool res = WriteLinkCommand(csb->buffer->platform_buffer(),
                                    magma::to_uint32(csb_length + csb->offset) /* write_offset */,
                                    static_cast<uint16_t>(cmd_buf_prefetch), gpu_addr);
        if (!res) {
          MAGMA_LOG(ERROR, "Failed to write LINK from context state buffer to command buffer");
          return false;
        }
        // Update the address the ringbuffer will link to.
        gpu_addr = magma::to_uint32(csb_mapping->gpu_addr());
        length = csb_length + (kInstructionDwords * sizeof(uint32_t));  // Additional LINK size.
      }
    }
  } else {
    // If there is no command buffer, we link directly to the new ringbuffer commands.
    gpu_addr = rb_complete_addr;
    length = kRbPrefetch * sizeof(uint64_t);
  }

  uint32_t prefetch =
      magma::round_up(length, static_cast<uint32_t>(sizeof(uint64_t))) / sizeof(uint64_t);
  if (prefetch & 0xFFFF0000) {
    MAGMA_LOG(ERROR, "Can't submit length %u (prefetch 0x%x)", length, prefetch);
    return false;
  }

  // Write the new commands to the end of the ringbuffer.
  // When adding new instructions, make sure to modify |kRbInstructionsPerBatch| accordingly.
  // Add an EVENT to the end to the ringbuffer.
  uint32_t new_rb_instructions_start = ringbuffer_->tail();
  if (!WriteInterruptEvent(event_id, std::move(mapped_batch), prev_address_space)) {
    MAGMA_LOG(ERROR, "Failed to write interrupt event %u\n", event_id);
    return false;
  }
  // Add a new WAIT-LINK to the end of the ringbuffer.
  if (!AddRingbufferWaitLink()) {
    MAGMA_LOG(ERROR, "Failed to add WAIT-LINK to ringbuffer");
    return false;
  }
  // Verify the number of instructions we just wrote matches the prefetch value
  // of the user buffer's LINK command.
  DASSERT(new_rb_instructions_start ==
          ringbuffer_->SubtractOffset(kRbInstructionsPerBatch * sizeof(uint64_t)));

  DLOG("Submitting buffer at NPU addr 0x%x", gpu_addr);

  LinkRingbuffer(prev_wait_link, gpu_addr, prefetch);

  // Save the context of the last executed command buffer. Since any command buffer may modify
  // hardware state, we should update this even if no command state buffer was provided.
  if (is_cmd_buf) {
    prev_executed_context_ = context;
  }

  kill_context.cancel();

  return true;
}

std::vector<MappedBatch*> MsdVsiDevice::GetInflightBatches() {
  std::vector<MappedBatch*> inflight_batches;
  inflight_batches.reserve(kNumEvents);
  for (unsigned i = 0; i < kNumEvents; i++) {
    if (events_[i].submitted) {
      DASSERT(events_[i].mapped_batch != nullptr);
      inflight_batches.push_back(events_[i].mapped_batch.get());
    }
  }
  // Sort the batches by sequence number, as the event ids may not correspond to the actual
  // ordering.
  std::sort(inflight_batches.begin(), inflight_batches.end(),
            [](const MappedBatch* a, const MappedBatch* b) {
              return a->GetSequenceNumber() < b->GetSequenceNumber();
            });

  return inflight_batches;
}

void MsdVsiDevice::DumpStatusToLog() { EnqueueDeviceRequest(std::make_unique<DumpRequest>()); }

magma::Status MsdVsiDevice::SubmitBatch(std::unique_ptr<MappedBatch> batch, bool do_flush) {
  DLOG("SubmitBatch");
  CHECK_THREAD_NOT_CURRENT(device_thread_id_);

  EnqueueDeviceRequest(std::make_unique<BatchRequest>(std::move(batch), do_flush));
  return MAGMA_STATUS_OK;
}

magma::Status MsdVsiDevice::ProcessBatch(std::unique_ptr<MappedBatch> batch, bool do_flush) {
  CHECK_THREAD_IS_CURRENT(device_thread_id_);

  auto context = batch->GetContext().lock();
  if (!context) {
    MAGMA_LOG(ERROR, "No context for batch %lu, IsCommandBuffer=%d", batch->GetBatchBufferId(),
              batch->IsCommandBuffer());
    return MAGMA_STATUS_INTERNAL_ERROR;
  }

  uint32_t sequence_number = sequencer_->next_sequence_number();
  batch->SetSequenceNumber(sequence_number);
  progress_->Submitted(sequence_number, std::chrono::steady_clock::now());

  uint32_t event_id;
  if (!AllocInterruptEvent(true /* free_on_complete */, &event_id)) {
    DLOG("No events remaining, deferring execution of command buffer until next interrupt");
    // Not an error, just need to wait for a pending command buffer to complete.
    request_backlog_.emplace_back(DeferredRequest{std::move(batch), do_flush});
    return MAGMA_STATUS_OK;
  }
  if (!SubmitCommandBuffer(context, context->exec_address_space()->page_table_array_slot(),
                           do_flush, std::move(batch), event_id)) {
    FreeInterruptEvent(event_id);
    MAGMA_LOG(ERROR, "Failed to submit command buffer");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }

  return MAGMA_STATUS_OK;
}

std::unique_ptr<MsdVsiConnection> MsdVsiDevice::OpenVsiConnection(msd::msd_client_id_t client_id) {
  uint32_t page_table_array_slot;
  if (!page_table_slot_allocator_->Alloc(&page_table_array_slot)) {
    MAGMA_LOG(ERROR, "couldn't allocate page table slot");
    return nullptr;
  }

  auto address_space = AddressSpace::Create(this, page_table_array_slot);
  if (!address_space) {
    MAGMA_LOG(ERROR, "failed to create address space");
    return nullptr;
  }

  page_table_arrays_->AssignAddressSpace(page_table_array_slot, address_space.get());

  return std::make_unique<MsdVsiConnection>(this, std::move(address_space), client_id);
}

std::unique_ptr<msd::Connection> MsdVsiDevice::MsdOpen(msd::msd_client_id_t client_id) {
  auto connection = OpenVsiConnection(client_id);
  if (connection) {
    return std::make_unique<MsdVsiAbiConnection>(std::move(connection));
  } else {
    MAGMA_LOG(ERROR, "failed to open vsi connection");
    return nullptr;
  }
}

magma_status_t MsdVsiDevice::ChipIdentity(magma_vsi_vip_chip_identity* out_identity) {
  if (!IsValidDeviceId()) {
    // TODO(https://fxbug.dev/42113659): Read hardcoded values from features database instead.
    MAGMA_LOG(ERROR, "unhandled device id 0x%x", device_id());
    return MAGMA_STATUS_UNIMPLEMENTED;
  }

  memset(out_identity, 0, sizeof(*out_identity));
  out_identity->chip_model = device_id();
  out_identity->chip_revision = revision();
  out_identity->chip_date = chip_date();

  out_identity->stream_count = gpu_features_->stream_count();
  out_identity->pixel_pipes = gpu_features_->pixel_pipes();
  out_identity->resolve_pipes = 0x0;
  out_identity->instruction_count = gpu_features_->instruction_count();
  out_identity->num_constants = gpu_features_->num_constants();
  out_identity->varyings_count = gpu_features_->varyings_count();
  out_identity->gpu_core_count = 0x1;

  out_identity->product_id = product_id();
  out_identity->chip_flags = 0x4;
  out_identity->eco_id = eco_id();
  out_identity->customer_id = customer_id();
  return MAGMA_STATUS_OK;
}

magma_status_t MsdVsiDevice::ChipOption(magma_vsi_vip_chip_option* out_option) {
  if (!IsValidDeviceId()) {
    // TODO(https://fxbug.dev/42113659): Read hardcoded values from features database instead.
    MAGMA_LOG(ERROR, "unhandled device id 0x%x", device_id());
    return MAGMA_STATUS_UNIMPLEMENTED;
  }

  memset(out_option, 0, sizeof(*out_option));
  out_option->gpu_profiler = false;
  out_option->allow_fast_clear = false;
  out_option->power_management = false;
  out_option->enable_mmu = true;
  out_option->compression = kVsiVipCompressionOptionNone;
  out_option->usc_l1_cache_ratio = 0;
  out_option->secure_mode = kVsiVipSecureModeNormal;
  return MAGMA_STATUS_OK;
}

magma_status_t MsdVsiDevice::QuerySram(zx::vmo* out_sram) {
  if (!external_sram_) {
    MAGMA_LOG(ERROR, "Device has no external SRAM");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }

  // TODO(https://fxbug.dev/42149674): this may fail due to delays in handling client VMO release
  if (external_sram_->HasChildren()) {
    MAGMA_LOG(ERROR, "External SRAM has children");
    return MAGMA_STATUS_ACCESS_DENIED;
  }

  void* ptr;
  if (!external_sram_->MapCpu(&ptr)) {
    MAGMA_LOG(ERROR, "MapCpu failed");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }

  // Wipe any previous content
  memset(ptr, 0, external_sram_->size());

  // Client looks for phys addr in the first few bytes
  std::optional<uint64_t> sram_base = platform_device_->GetExternalSramPhysicalBase();
  if (!sram_base.has_value()) {
    external_sram_->UnmapCpu();
    MAGMA_LOG(ERROR, "Could not get external sram physical base");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }
  *reinterpret_cast<uint64_t*>(ptr) = sram_base.value();

  external_sram_->UnmapCpu();

  uint32_t handle;
  if (!external_sram_->CreateChild(&handle)) {
    MAGMA_LOG(ERROR, "CreateChild failed");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }

  *out_sram = zx::vmo(handle);

  return MAGMA_STATUS_OK;
}

magma_status_t MsdVsiDevice::DataToBuffer(const char* name, void* data, uint64_t size,
                                          zx::vmo* result_buffer_out) {
  auto buffer = magma::PlatformBuffer::Create(size, name);
  if (!buffer) {
    MAGMA_LOG(ERROR, "Failed to allocate buffer");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }
  if (!buffer->Write(data, 0, size)) {
    MAGMA_LOG(ERROR, "Failed to write result to buffer");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }

  uint32_t result_buffer;

  if (!buffer->duplicate_handle(&result_buffer)) {
    MAGMA_LOG(ERROR, "Failed to duplicate handle");
    return MAGMA_STATUS_INTERNAL_ERROR;
  }
  *result_buffer_out = zx::vmo(result_buffer);

  return MAGMA_STATUS_OK;
}

magma_status_t MsdVsiDevice::MsdQuery(uint64_t id, zx::vmo* result_buffer_out,
                                      uint64_t* result_out) {
  switch (id) {
    case MAGMA_QUERY_VENDOR_ID:
      *result_out = MAGMA_VENDOR_ID_VSI;
      break;

    case MAGMA_QUERY_VENDOR_VERSION:
      *result_out = MAGMA_VENDOR_VERSION_VSI;
      break;

    case MAGMA_QUERY_DEVICE_ID:
      *result_out = device_id();
      break;

    case MAGMA_QUERY_IS_TOTAL_TIME_SUPPORTED:
      *result_out = 0;
      break;

    case kMsdVsiVendorQueryClientGpuAddrRange: {
      uint32_t size_in_pages = AddressSpaceLayout::client_gpu_addr_size() / magma::page_size();
      DASSERT(size_in_pages * magma::page_size() == AddressSpaceLayout::client_gpu_addr_size());
      uint32_t base_in_pages = AddressSpaceLayout::client_gpu_addr_base() / magma::page_size();
      DASSERT(base_in_pages * magma::page_size() == AddressSpaceLayout::client_gpu_addr_base());
      *result_out =
          static_cast<uint64_t>(base_in_pages) | (static_cast<uint64_t>(size_in_pages) << 32);
      break;
    }

    case kMsdVsiVendorQueryChipIdentity: {
      magma_vsi_vip_chip_identity chip_id;
      magma_status_t status = ChipIdentity(&chip_id);
      if (status != MAGMA_STATUS_OK)
        return status;

      return DataToBuffer("chip_identity", &chip_id, sizeof(chip_id), result_buffer_out);
    }

    case kMsdVsiVendorQueryChipOption: {
      magma_vsi_vip_chip_option chip_opt;
      magma_status_t status = ChipOption(&chip_opt);
      if (status != MAGMA_STATUS_OK)
        return status;

      return DataToBuffer("chip_option", &chip_opt, sizeof(chip_opt), result_buffer_out);
    }

    case kMsdVsiVendorQueryExternalSram:
      return QuerySram(result_buffer_out);

    default:
      MAGMA_LOG(ERROR, "unhandled id %" PRIu64, id);
      return MAGMA_STATUS_INVALID_ARGS;
  }

  return MAGMA_STATUS_OK;
}

void MsdVsiDevice::MsdDumpStatus(uint32_t dump_type) { DumpStatusToLog(); }

magma_status_t MsdVsiDevice::MsdGetIcdList(std::vector<msd::MsdIcdInfo>* icd_info_out) {
  const char* kSuffixes[] = {"_test", ""};
  auto& icd_info = *icd_info_out;
  icd_info.clear();
  icd_info.resize(std::size(kSuffixes));
  for (uint32_t i = 0; i < std::size(kSuffixes); i++) {
    icd_info[i].component_url = fbl::StringPrintf(
        "fuchsia-pkg://fuchsia.com/libopencl_vsi_vip%s#meta/opencl.cm", kSuffixes[i]);
    icd_info[i].support_flags = msd::ICD_SUPPORT_FLAG_OPENCL;
  }
  return MAGMA_STATUS_OK;
}
