// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "stack_sampler.h"

#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/suspend_token.h>
#include <zircon/errors.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/object.h>

#include <charconv>
#include <cstdint>
#include <cstring>
#include <vector>

#include <src/lib/unwinder/fuchsia.h>
#include <src/lib/unwinder/registers.h>
#include <src/lib/unwinder/unwind.h>

namespace profiler {

constexpr size_t kStackCaptureSize = 4096ul * 4;  // 16 kib

#if defined(__aarch64__)
// CPSR/SPSR M[4]: set when a thread is executing in AArch32 mode.
constexpr uint64_t kCpsrArch32Mask = uint64_t{1} << 4;
#endif

template <typename T>
bool IsAarch32(const T& state) {
#ifdef __aarch64__
  return state.cpsr & kCpsrArch32Mask;
#else
  return false;
#endif
}

zx::result<> StackSampler::Start(size_t buffer_size_mb) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);

  // TODO(https://fxbug.dev/468419097): Deduplicate watcher code.
  zx::result<> res = targets_.ForEachProcess([this](std::span<const zx_koid_t> job_path,
                                                    const ProcessTarget& p) -> zx::result<> {
    TRACE_DURATION("cpu_profiler", "StackSampler::Start/ForEachProcess");
    std::vector<zx_koid_t> saved_path{job_path.begin(), job_path.end()};
    (void)RefreshMappings(p);

    auto process_watcher = std::make_unique<ProcessWatcher>(
        p.handle.borrow(),
        [saved_path, this](zx_koid_t pid, zx_koid_t tid, zx::thread t) {
          AddThread(saved_path, pid, tid, std::move(t));
        },
        [saved_path, this](zx_koid_t pid, zx_koid_t tid) { RemoveThread(saved_path, pid, tid); });

    auto [it, emplaced] = process_watchers_.emplace(p.pid, std::move(process_watcher));
    if (emplaced) {
      if (zx::result watch_result = it->second->Watch(dispatcher_); watch_result.is_error()) {
        FX_PLOGS(WARNING, watch_result.status_value()) << "Failed to watch process: " << p.pid;
      }
    }
    return zx::ok();
  });

  if (res.is_error()) {
    return res;
  }

  if (auto res = targets_.ForEachJob([this](const JobTarget& target) {
        if (zx::result res = WatchTarget(target); res.is_error()) {
          FX_PLOGS(WARNING, res.status_value())
              << "Failed to watch job [" << target.job_id << "] and its children";
        }
        return zx::ok();
      });
      res.is_error()) {
    return res;
  }

  return zx::make_result(sample_task_.Post(dispatcher_));
}

zx::result<> StackSampler::Stop() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  return zx::make_result(sample_task_.Cancel());
}

void StackSampler::AddThread(std::vector<zx_koid_t> job_path, zx_koid_t pid, zx_koid_t tid,
                             zx::thread t) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  std::string thread_name = profiler::GetThreadName(t);
  zx::result res = targets_.AddThread(
      job_path, pid,
      ThreadTarget{.handle = std::move(t), .tid = tid, .name = std::move(thread_name)});
  if (res.is_error()) {
    FX_PLOGS(ERROR, res.status_value()) << "Failed to add thread to session: " << tid;
  } else {
    zx::result<ProcessTarget*> process_target = targets_.GetProcess(job_path, pid);
    if (process_target.is_ok() && process_target.value() != nullptr) {
      (void)RefreshMappings(*process_target.value());
    }
  }
}

void StackSampler::PopulateRestrictedStateAddrs(const ProcessTarget& target) {
  constexpr char kRestrictedStateVmoPrefix[] = "restricted_state_vmo:";
  constexpr size_t kPrefixLen = sizeof(kRestrictedStateVmoPrefix) - 1;

  for (const zx_info_maps& map : target.cached_mappings) {
    if (strncmp(map.name, kRestrictedStateVmoPrefix, kPrefixLen) == 0) {
      const char* tid_str = map.name + kPrefixLen;
      zx_koid_t tid;
      auto result = std::from_chars(tid_str, map.name + sizeof(map.name), tid);
      if (result.ec != std::errc()) {
        continue;
      }
      auto it = target.threads.find(tid);
      if (it != target.threads.end()) {
        it->second.restricted_state_addr = map.base;
      }
    }
  }
}

zx::result<> StackSampler::RefreshMappings(const ProcessTarget& target) {
  size_t actual = 0;
  size_t avail = 0;
  zx_status_t status = target.handle.get_info(ZX_INFO_PROCESS_MAPS, nullptr, 0, &actual, &avail);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Process has exited. Failed to get mappings for process: "
                            << target.pid;
    target.cached_mappings.clear();
    return zx::error(status);
  }
  target.cached_mappings.resize(avail);
  status = target.handle.get_info(ZX_INFO_PROCESS_MAPS, target.cached_mappings.data(),
                                  target.cached_mappings.size() * sizeof(zx_info_maps_t), &actual,
                                  &avail);
  if (status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Process has exited. Failed to get mappings for process: "
                            << target.pid;
    target.cached_mappings.clear();
    return zx::error(status);
  }
  if (actual < avail) {
    target.cached_mappings.resize(actual);
  }
  PopulateRestrictedStateAddrs(target);
  return zx::ok();
}

uint64_t StackSampler::GetRestrictedSP(const zx_restricted_state_t& restricted_state) {
#if defined(__aarch64__)
  // On targets running in aarch32 mode, the stack pointer is stored in x13 rather than sp.
  if (IsAarch32(restricted_state)) {
    return restricted_state.x[13];
  }
  return restricted_state.sp;
#elif defined(__x86_64__)
  return restricted_state.rsp;
#elif defined(__riscv)
  return restricted_state.sp;
#else
#error "unsupported architecture"
#endif
}

void StackSampler::CollectSamples(async_dispatcher_t* dispatcher, async::TaskBase* task,
                                  zx_status_t status) {
  zx::ticks start_time = zx::ticks::now();
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (status != ZX_OK) {
    return;
  }

  (void)targets_.ForEachProcess([this](std::span<const zx_koid_t>,
                                       const ProcessTarget& target) -> zx::result<> {
    TRACE_DURATION("cpu_profiler", "StackSampler::CollectSamples/ForEachProcess");

    for (const auto& [tid, thread] : target.threads) {
      zx_info_thread_t thread_info;
      zx_status_t status = thread.handle.get_info(ZX_INFO_THREAD, &thread_info, sizeof(thread_info),
                                                  nullptr, nullptr);
      if (status != ZX_OK) {
        FX_PLOGS(ERROR, status) << "unable to get thread info for thread " << thread.handle.get()
                                << ", skipping";
        continue;  // Skip this thread.
      }

      // Skip threads that are not actively running or blocked
      if (thread_info.state != ZX_THREAD_STATE_RUNNING) {
        continue;
      }

      // Suspend thread
      zx::suspend_token suspend_token;
      status = thread.handle.suspend(&suspend_token);
      if (status != ZX_OK) {
        // skip terminited thread
        continue;
      }

      // IPI Profiling Target States and Sampling Strategy:
      //
      // 1. Userspace: IPI executes immediately. (Action: SAMPLE)
      // 2. Kernel (Interruptible): IPI executes immediately. (Action: SAMPLE)
      // 3. Kernel (Uninterruptible): IPI execution delayed until interrupts are
      //    re-enabled. Captured to reflect heavy kernel workloads. (Action: SAMPLE)
      // 4. Context Switch / Blocking: Thread is yielding the CPU. We only sample
      //    if we catch the exact state entering the block. Otherwise, we skip
      //    to avoid capturing off-CPU behavior. (Action: CONDITIONAL SKIP)
      //
      // Poll until the thread handles the suspend request.
      // We yield briefly if it is still running, and break immediately if it blocks.
      // The timeout is kept short (2ms) to avoid blocking the dispatcher for too long.
      zx::time max_loop_dur = zx::clock::get_monotonic() + zx::msec(2);
      bool thread_suspended = false;

      while (zx::clock::get_monotonic() < max_loop_dur) {
        zx_info_thread_t info;
        status = thread.handle.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr);
        if (status != ZX_OK) {
          break;
        }
        if (info.state == ZX_THREAD_STATE_SUSPENDED) {
          thread_suspended = true;
          break;
        }
        if (info.state == ZX_THREAD_STATE_RUNNING) {
          // Wait for the thread to suspend, or time out after 100µs.
          // This is edge-triggered, so it returns immediately if the signal is asserted.
          zx_signals_t observed;
          status = thread.handle.wait_one(ZX_THREAD_SUSPENDED, zx::deadline_after(zx::usec(100)),
                                          &observed);
          if (status == ZX_OK) {
            thread_suspended = true;
            break;
          }
        } else {
          // Thread blocked before we could sample it, skip
          break;
        }
      }

      zx::ticks tick_suspended = zx::ticks::now();

      if (!thread_suspended) {
        continue;
      }

      // Get registers
      zx_thread_state_general_regs_t regs;
      if (thread.handle.read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs)) != ZX_OK) {
        continue;
      }

      unwinder::Registers registers = unwinder::FromFuchsiaRegisters(regs);

      uint64_t sp = 0;
#ifdef __aarch64__
      // For a thread executing in AArch32 mode (CPSR M[4] set), the stack
      // pointer is R13, reported in r[13]; the sp field holds SP_EL0, which
      // AArch32 execution never updates and is therefore stale.
      if (IsAarch32(regs)) {
        sp = regs.r[13];
      } else
#endif
      {
        if (registers.GetSP(sp).has_err()) {
          continue;
        }
      }

      BufferedStackMemory stack_memory(target.handle.borrow(), {}, target.cached_mappings);
      stack_memory.CaptureStack(sp, kStackCaptureSize);

      if (thread.restricted_state_addr.has_value()) {
        uint64_t restricted_base = thread.restricted_state_addr.value();

        // Capture the memory containing the restricted state struct
        if (stack_memory.CaptureStack(restricted_base, sizeof(zx_restricted_state_t)) == ZX_OK) {
          zx_restricted_state_t restricted_state;
          // Read from local buffer
          if (!stack_memory.ReadBytes(restricted_base, sizeof(restricted_state), &restricted_state)
                   .has_err()) {
            sp = GetRestrictedSP(restricted_state);
            stack_memory.CaptureStack(sp, kStackCaptureSize);
          } else {
            thread.restricted_state_addr.reset();
          }
        } else {
          thread.restricted_state_addr.reset();
        }
      }
      suspend_token.reset();
      zx::ticks tick_resume = zx::ticks::now();

      // Group registers and memory chunks into a single chunk
      zx::ticks sample_time = zx::ticks::now();
      if (sample_cb_) {
        std::vector<uint8_t> sample_memory;

        // Append registers headers
        uint64_t regs_size = sizeof(regs);
        sample_memory.insert(sample_memory.end(), reinterpret_cast<uint8_t*>(&regs_size),
                             reinterpret_cast<uint8_t*>(&regs_size) + sizeof(regs_size));
        sample_memory.insert(sample_memory.end(), reinterpret_cast<uint8_t*>(&regs),
                             reinterpret_cast<uint8_t*>(&regs) + sizeof(regs));

        // Append all memory chunks
        for (const auto& chunk : stack_memory.GetChunks()) {
          uint64_t base = chunk.base;
          uint64_t size = chunk.data.size();
          sample_memory.insert(sample_memory.end(), reinterpret_cast<uint8_t*>(&base),
                               reinterpret_cast<uint8_t*>(&base) + sizeof(base));
          sample_memory.insert(sample_memory.end(), reinterpret_cast<uint8_t*>(&size),
                               reinterpret_cast<uint8_t*>(&size) + sizeof(size));
          sample_memory.insert(sample_memory.end(), chunk.data.begin(), chunk.data.end());
        }

        sample_cb_({.pid = target.pid,
                    .tid = tid,
                    .stack = {},
                    .timestamp = sample_time,
                    .stack_memory = sample_memory});
      }

      inspecting_durations_.push_back(tick_resume - tick_suspended);
    }
    return zx::ok();
  });

  zx::time end_time = zx::clock::get_monotonic();
  zx::duration total_dur = end_time - zx::time(start_time.get());
  FX_LOGS(DEBUG) << "CollectSamples pass finished in " << total_dur.to_msecs() << " ms";

  zx::duration period = zx::msec(10);
  if (!sample_specs_.empty() && sample_specs_[0].period().has_value()) {
    period = zx::nsec(sample_specs_[0].period().value());
  }
  sample_task_.PostDelayed(dispatcher_, period);
}
}  // namespace profiler
