// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/drivers/msd-arm-mali/src/power_manager.h"

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma/platform/platform_trace.h>

#include "src/graphics/drivers/msd-arm-mali/src/registers.h"

namespace {

// Wait for up to 1 second in increments of 1ms for the power interrupt. The status may change
// without receiving a wakeup.
constexpr size_t kMaxPowerTries = 1000;
constexpr size_t kLogAtPowerTry = 2;

}  // anonymous namespace

PowerManager::PowerManager(Owner* owner, uint64_t default_core_bitmask)
    : owner_(owner), default_cores_(default_core_bitmask) {
  power_state_semaphore_ = magma::PlatformSemaphore::Create();
  // Initialize current set of running cores.
  ReceivedPowerInterrupt();
  auto now = std::chrono::steady_clock::now();
  last_check_time_ = now;
  last_trace_time_ = now;
}

void PowerManager::PowerDownOnIdle() {
  power_down_on_idle_ = true;
  std::lock_guard<std::mutex> lock(active_time_mutex_);
  if (!gpu_active_) {
    PowerDownWhileIdle();
  }
}
void PowerManager::PowerDownWhileIdle() {
  MAGMA_DASSERT(power_down_on_idle_);
  DisableShaders();
  bool success = true;
  if (!WaitForShaderDisable()) {
    success = false;
    MAGMA_LOG(ERROR, "Waiting for shader disable timed out");
  }
  power_down_on_idle_ = false;
  owner_->ReportPowerChangeComplete(false, success);
}

void PowerManager::PowerUpAfterIdle() {
  power_down_on_idle_ = false;

  EnableDefaultCores();
  bool success = true;
  if (!WaitForShaderReady()) {
    success = false;
    MAGMA_LOG(ERROR, "Waiting for shader enable timed out");
  }
  owner_->ReportPowerChangeComplete(true, success);
}

void PowerManager::EnableCores(uint64_t shader_bitmask) {
  TRACE_DURATION("magma:power", "PowerManager::EnableCores", "shader_bitmask",
                 TA_UINT64(shader_bitmask));
  required_cores_ |= shader_bitmask;
  registers::CoreReadyState::WriteState(register_io(), registers::CoreReadyState::CoreType::kShader,
                                        registers::CoreReadyState::ActionType::kActionPowerOn,
                                        shader_bitmask);
  registers::CoreReadyState::WriteState(register_io(), registers::CoreReadyState::CoreType::kL2,
                                        registers::CoreReadyState::ActionType::kActionPowerOn, 1);
  registers::CoreReadyState::WriteState(register_io(), registers::CoreReadyState::CoreType::kTiler,
                                        registers::CoreReadyState::ActionType::kActionPowerOn, 1);
}

void PowerManager::EnableDefaultCores() { EnableCores(default_cores_); }

void PowerManager::DisableShaders() {
  required_cores_ = 0;
  TRACE_DURATION("magma:power", "PowerManager::DisableShaders");
  uint64_t powered_on_shaders = registers::CoreReadyState::ReadBitmask(
                                    register_io(), registers::CoreReadyState::CoreType::kShader,
                                    registers::CoreReadyState::StatusType::kReady) |
                                registers::CoreReadyState::ReadBitmask(
                                    register_io(), registers::CoreReadyState::CoreType::kShader,
                                    registers::CoreReadyState::StatusType::kPowerTransitioning);

  registers::CoreReadyState::WriteState(register_io(), registers::CoreReadyState::CoreType::kShader,
                                        registers::CoreReadyState::ActionType::kActionPowerOff,
                                        powered_on_shaders);
}

void PowerManager::DisableL2() {
  TRACE_DURATION("magma:power", "PowerManager::DisableL2");
  registers::CoreReadyState::WriteState(register_io(), registers::CoreReadyState::CoreType::kL2,
                                        registers::CoreReadyState::ActionType::kActionPowerOff, 1);
  registers::CoreReadyState::WriteState(register_io(), registers::CoreReadyState::CoreType::kTiler,
                                        registers::CoreReadyState::ActionType::kActionPowerOff, 1);
}

bool PowerManager::WaitForShaderDisable() {
  TRACE_DURATION("magma:power", "PowerManager::WaitForShaderDisable");
  for (size_t i = 0; i < kMaxPowerTries; i++) {
    uint64_t powered_on = registers::CoreReadyState::ReadBitmask(
                              register_io(), registers::CoreReadyState::CoreType::kShader,
                              registers::CoreReadyState::StatusType::kReady) |
                          registers::CoreReadyState::ReadBitmask(
                              register_io(), registers::CoreReadyState::CoreType::kShader,
                              registers::CoreReadyState::StatusType::kPowerTransitioning);
    if (!powered_on)
      return true;
    const magma::Status status = power_state_semaphore_->Wait(1);
    if (!status.ok() && i == kLogAtPowerTry) {
      TRACE_ALERT("magma", "long-wait");
      MAGMA_LOG(WARNING, "Waiting for shader disable longer than %zu ms", i + 1);
    }
  }
  return false;
}

bool PowerManager::WaitForL2Disable() {
  TRACE_DURATION("magma:power", "PowerManager::WaitForL2Disable");
  for (size_t i = 0; i < kMaxPowerTries; i++) {
    bool powered_on = registers::CoreReadyState::ReadBitmask(
                          register_io(), registers::CoreReadyState::CoreType::kL2,
                          registers::CoreReadyState::StatusType::kReady) |
                      registers::CoreReadyState::ReadBitmask(
                          register_io(), registers::CoreReadyState::CoreType::kL2,
                          registers::CoreReadyState::StatusType::kPowerTransitioning);
    if (!powered_on) {
      MAGMA_LOG(INFO, "L2 disable time was %zu ms", i);
      UpdateReadyStatus();
      return true;
    }
    const magma::Status status = power_state_semaphore_->Wait(1);
    if (!status.ok() && i == kLogAtPowerTry) {
      TRACE_ALERT("magma", "long-wait");
      MAGMA_LOG(WARNING, "Waiting for L2 disable longer than %zu ms", i + 1);
    }
  }
  return false;
}

bool PowerManager::WaitForShaderReady() {
  TRACE_DURATION("magma:power", "PowerManager::WaitForShaderReady");
  for (size_t i = 0; i < kMaxPowerTries; i++) {
    MAGMA_DASSERT(required_cores_);
    // Wait for all required cores to be available, to ensure the hardware is in the expected state
    // later.
    uint64_t powered_on_cores = registers::CoreReadyState::ReadBitmask(
        register_io(), registers::CoreReadyState::CoreType::kShader,
        registers::CoreReadyState::StatusType::kReady);
    if ((powered_on_cores & required_cores_) == required_cores_) {
      UpdateReadyStatus();
      return true;
    }
    // Shader power up takes < 1ms generally.
    const magma::Status status = power_state_semaphore_->Wait(1);
    if (!status.ok() && i == kLogAtPowerTry) {
      TRACE_ALERT("magma", "long-wait");
      MAGMA_LOG(WARNING, "Waiting for shader core 0x%lx enable longer than %zu ms",
                required_cores_ & ~powered_on_cores, i + 1);
    }
  }
  return false;
}

void PowerManager::ReceivedPowerInterrupt() {
  TRACE_DURATION("magma:power", "PowerManager::ReceivedPowerInterrupt");
  UpdateReadyStatus();
  power_state_semaphore_->Signal();
}

void PowerManager::UpdateReadyStatus() {
  TRACE_DURATION("magma:power", "PowerManager::UpdateReadyStatus");
  std::lock_guard<std::mutex> lock(ready_status_mutex_);
  tiler_ready_status_ = registers::CoreReadyState::ReadBitmask(
      register_io(), registers::CoreReadyState::CoreType::kTiler,
      registers::CoreReadyState::StatusType::kReady);
  l2_ready_status_ = registers::CoreReadyState::ReadBitmask(
      register_io(), registers::CoreReadyState::CoreType::kL2,
      registers::CoreReadyState::StatusType::kReady);
}

void PowerManager::UpdateGpuActive(bool active, bool has_pending_work) {
  TRACE_DURATION("magma:power", "PowerManager::UpdateGpuActive", "active", TA_BOOL(active));
  std::lock_guard<std::mutex> lock(active_time_mutex_);
  UpdateGpuActiveLocked(active, has_pending_work);
}

void PowerManager::UpdateGpuActiveLocked(bool active, bool has_pending_work) {
  TRACE_DURATION("magma:power", "PowerManager::UpdateGpuActiveLocked", "active", TA_BOOL(active));
  has_pending_work_cached_ = has_pending_work;
  if (active || has_pending_work) {
    current_gpu_powerdown_timeout_ = Clock::time_point::max();
  } else if (!active && current_gpu_powerdown_timeout_ == Clock::time_point::max()) {
    // Chosen to avoid performance regressions on benchmarks. 16ms caused 25th-percentile CPU
    // regressions.
    constexpr std::chrono::milliseconds kPowerDownTimeout{30};
    current_gpu_powerdown_timeout_ = Clock::now() + kPowerDownTimeout;
  }
  if (power_down_on_idle_ && !active) {
    PowerDownWhileIdle();
  }
  auto now = std::chrono::steady_clock::now();
  std::chrono::steady_clock::duration total_time = now - last_check_time_;
  constexpr std::chrono::milliseconds kMemoryDuration(100);

  if (active) {
    total_active_time_ += std::chrono::duration_cast<std::chrono::nanoseconds>(total_time).count();
  }

  // Ignore long periods of inactive time.
  if (total_time > kMemoryDuration)
    total_time = kMemoryDuration;

  std::chrono::steady_clock::duration active_time =
      gpu_active_ ? total_time : std::chrono::steady_clock::duration(0);

  constexpr uint32_t kBucketLengthMilliseconds = 50;
  bool coalesced = false;
  if (!time_periods_.empty()) {
    auto start_time = time_periods_.back().end_time - time_periods_.back().total_time;
    if (now - start_time < std::chrono::milliseconds(kBucketLengthMilliseconds)) {
      coalesced = true;
      time_periods_.back().end_time = now;
      time_periods_.back().total_time += total_time;
      time_periods_.back().active_time += active_time;
    }
  }

  if (!coalesced)
    time_periods_.push_back(TimePeriod{now, total_time, active_time});

  while (!time_periods_.empty() && (now - time_periods_.front().end_time > kMemoryDuration)) {
    time_periods_.pop_front();
  }

  if ((now - last_trace_time_) > kMemoryDuration) {
    TRACE_COUNTER("magma", "GPU Utilization", 0, "utilization",
                  [this]() FIT_REQUIRES(active_time_mutex_) {
                    std::chrono::steady_clock::duration total_time_accumulate(0);
                    std::chrono::steady_clock::duration active_time_accumulate(0);
                    for (const auto& period : time_periods_) {
                      total_time_accumulate += period.total_time;
                      active_time_accumulate += period.active_time;
                    }
                    double utilization_value =
                        (total_time_accumulate == std::chrono::steady_clock::duration::zero())
                            ? (0.0)
                            : (double(std::chrono::duration_cast<std::chrono::microseconds>(
                                          active_time_accumulate)
                                          .count()) /
                               double(std::chrono::duration_cast<std::chrono::microseconds>(
                                          total_time_accumulate)
                                          .count()));
                    return utilization_value;
                  }());
    last_trace_time_ = now;
  }

  last_check_time_ = now;
  gpu_active_ = active;
}

void PowerManager::GetGpuActiveInfo(std::chrono::steady_clock::duration* total_time_out,
                                    std::chrono::steady_clock::duration* active_time_out) {
  std::lock_guard<std::mutex> lock(active_time_mutex_);
  UpdateGpuActiveLocked(gpu_active_, has_pending_work_cached_);

  std::chrono::steady_clock::duration total_time_accumulate(0);
  std::chrono::steady_clock::duration active_time_accumulate(0);
  for (auto& period : time_periods_) {
    total_time_accumulate += period.total_time;
    active_time_accumulate += period.active_time;
  }

  *total_time_out = total_time_accumulate;
  *active_time_out = active_time_accumulate;
}

bool PowerManager::GetTotalTime(uint32_t* buffer_out) {
  magma_total_time_query_result result;
  {
    std::lock_guard<std::mutex> lock(active_time_mutex_);
    // Accumulate time since last update.
    UpdateGpuActiveLocked(gpu_active_, has_pending_work_cached_);
    result.monotonic_time_ns =
        std::chrono::duration_cast<std::chrono::nanoseconds>(last_check_time_.time_since_epoch())
            .count();
    result.gpu_time_ns = total_active_time_;
  }

  std::unique_ptr<magma::PlatformBuffer> buffer =
      magma::PlatformBuffer::Create(sizeof(result), "time_query");
  if (!buffer)
    return DRETF(false, "Failed to allocate buffer");
  if (!buffer->Write(&result, 0, sizeof(result)))
    return DRETF(false, "Failed to write result to buffer");
  return DRETF(buffer->duplicate_handle(buffer_out), "Failed to duplicate handle");
}
