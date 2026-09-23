// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/ktrace_provider/app.h"

#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/fxt/fields.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-engine/instrumentation.h>
#include <lib/trace-provider/provider.h>
#include <lib/zircon-internal/ktrace.h>
#include <lib/zx/channel.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls/log.h>

#include <iterator>

#include <fbl/algorithm.h>

namespace ktrace_provider {
namespace {

struct KTraceCategory {
  const char* name;
  uint32_t group;
  const char* description;
};

constexpr KTraceCategory kGroupCategories[] = {
    {"kernel", KTRACE_GRP_ALL, "All ktrace categories"},
    {"kernel:meta", KTRACE_GRP_META, "Thread and process names"},
    {"kernel:memory", KTRACE_GRP_MEMORY,
     "Memory allocations performed by the kernel, such as heap growth."},
    {"kernel:sched", KTRACE_GRP_SCHEDULER, "Process and thread scheduling information"},
    {"kernel:contention", KTRACE_GRP_CONTENTION, "Emit lock contention duraitons and flows"},
    {"kernel:ipc", KTRACE_GRP_IPC, "Emit an event for each FIDL call"},
    {"kernel:irq", KTRACE_GRP_IRQ, "Emit a duration event for interrupts"},
    {"kernel:probe", KTRACE_GRP_PROBE, "Used for LOCAL_KTRACE events"},
    {"kernel:arch", KTRACE_GRP_ARCH, "Hypervisor vcpus"},
    {"kernel:syscall", KTRACE_GRP_SYSCALL, "Emit an event for each syscall"},
    {"kernel:vm", KTRACE_GRP_VM, "Virtual memory events such as paging, mappings, and accesses"},
    {"kernel:restricted", KTRACE_GRP_RESTRICTED,
     "Duration events for when restricted mode is entered"},
    {"kernel:power", KTRACE_GRP_POWER,
     "Counters and events related to power (DVFS, idle, hotplug, etc...)."},
    {"kernel:oom", KTRACE_GRP_OOM, "Memory pressure and OOM events"},
};

// Meta category to retain current contents of ktrace buffer.
constexpr char kRetainCategory[] = "kernel:retain";

constexpr char kLogCategory[] = "log";

template <typename T>
void LogFidlFailure(const char* rqst_name, const fidl::Result<T>& result) {
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Ktrace FIDL " << rqst_name
                   << " failed: " << result.error_value().status_string();
  } else if (result->status() != ZX_OK) {
    FX_PLOGS(ERROR, result->status()) << "Ktrace " << rqst_name << " failed";
  }
}

zx::result<> RequestKtraceStop(const zx::resource& tracing_resource) {
  return zx::make_result(zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_STOP, 0, nullptr));
}

zx::result<> RequestKtraceRewind(const zx::resource& tracing_resource) {
  return zx::make_result(
      zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_REWIND, 0, nullptr));
}

zx::result<> RequestKtraceStart(const zx::resource& tracing_resource,
                                trace_buffering_mode_t buffering_mode, uint32_t group_mask) {
  return zx::make_result(
      zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_START, group_mask, nullptr));
}

void ForwardBuffer(DrainContext drain_context) {
  const zx::time_monotonic start_time = zx::clock::get_monotonic();

  if (trace_context_t* buffer_context = trace_acquire_context()) {
    auto d = fit::defer([buffer_context]() { trace_release_context(buffer_context); });

    size_t actual;
    if (zx_status_t status =
            zx_ktrace_read(drain_context.tracing_resource.get(), drain_context.buffer.get(), 0,
                           drain_context.buffer_size, &actual);
        status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Failed to read from zx_ktrace open";
      return;
    }
    size_t percent = actual * 100 / drain_context.buffer_size;
    if (actual == drain_context.buffer_size) {
      FX_LOGS(WARNING) << "[ 100% ] Read " << actual << " / " << drain_context.buffer_size
                       << " bytes. May have dropped trace data!";
    } else if (percent > 75) {
      FX_LOGS(WARNING) << "[ " << percent << "% ] Read " << actual << " / "
                       << drain_context.buffer_size << " bytes";
    }

    size_t offset = 0;
    const size_t num_words = actual / 8;
    while (offset < num_words) {
      uint64_t header = drain_context.buffer[offset];
      size_t record_size_words = fxt::RecordFields::RecordSize::Get<size_t>(header);
      fxt::RecordType type = fxt::RecordFields::Type::Get<fxt::RecordType>(header);
      bool is_durable =
          (type == fxt::RecordType::kMetadata || type == fxt::RecordType::kInitialization ||
           type == fxt::RecordType::kString || type == fxt::RecordType::kThread ||
           type == fxt::RecordType::kKernelObject);
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
      void* dst = is_durable
                      ? trace_context_alloc_durable_record(buffer_context, record_size_words * 8)
                      : trace_context_alloc_record(buffer_context, record_size_words * 8);
#else
      void* dst = trace_context_alloc_record(buffer_context, record_size_words * 8);
#endif
      if (dst != nullptr) {
        memcpy(dst, reinterpret_cast<const char*>(drain_context.buffer.get() + offset),
               record_size_words * 8);
        offset += record_size_words;
      } else {
        // We could have failed for two reasons: we failed to allocate space in the buffer, in which
        // we should delay and try again, or the trace is done and we shouldn't try again.
        auto state = trace_state();
        if (state == TRACE_STOPPED || state == TRACE_STOPPING) {
          return;
        }
        break;
      }
    }
  }

  const zx::time_monotonic end_time = zx::clock::get_monotonic();
  const zx::duration read_out_duration = end_time - start_time;

  if (read_out_duration > drain_context.max_readout_time) {
    FX_LOGS(DEBUG) << "Read out exceeded expected worst case execution time: expected="
                   << drain_context.max_readout_time.get()
                   << "ns actual=" << read_out_duration.get() << "ns";
  }

  switch (trace_state()) {
    case TRACE_STOPPED:
    case TRACE_STOPPING:
      return;
    case TRACE_STARTED:
      break;
  }

  // Align the next read out to a multiple of the poll period to ensure a consistent sampling
  // interval, independent of scheduling latency.
  const zx::time now_plus_poll_period = zx::clock::get_monotonic() + drain_context.poll_period;
  const zx::time_monotonic next_poll_time{static_cast<zx_time_t>(
      fbl::round_down(static_cast<uint64_t>(now_plus_poll_period.get()),
                      static_cast<uint64_t>(drain_context.poll_period.get())))};

  async::PostTaskForTime(
      async_get_default_dispatcher(),
      [drain_context = std::move(drain_context)]() mutable {
        ForwardBuffer(std::move(drain_context));
      },
      next_poll_time);
}

}  // namespace

std::vector<trace::KnownCategory> GetKnownCategories() {
  std::vector<trace::KnownCategory> known_categories = {
      {.name = kRetainCategory,
       .description = "Retain the previous contents of the buffer instead of clearing it out"},
  };

  for (const auto& category : kGroupCategories) {
    known_categories.emplace_back(category.name, category.description);
  }

  return known_categories;
}

App::App(zx::resource tracing_resource, const fxl::CommandLine& command_line,
         zx::duration poll_period, zx::duration max_readout_time)
    : tracing_resource_(std::move(tracing_resource)),
      poll_period_(poll_period),
      max_readout_time_(max_readout_time) {
  trace_observer_.Start(async_get_default_dispatcher(), [this] {
    if (zx::result res = UpdateState(); res.is_error()) {
      FX_PLOGS(ERROR, res.error_value()) << "Update state failed";
    }
  });
}

zx::result<> App::UpdateState() {
  uint32_t group_mask = 0;
  bool capture_log = false;
  bool retain_current_data = false;
  if (trace_state() == TRACE_STARTED) {
    size_t num_enabled_categories = 0;
    for (const auto& category : kGroupCategories) {
      if (trace_is_category_enabled(category.name)) {
        group_mask |= category.group;
        ++num_enabled_categories;
      }
    }

    // Avoid capturing log traces in the default case by detecting whether all
    // categories are enabled or not.
    capture_log = trace_is_category_enabled(kLogCategory) &&
                  num_enabled_categories != std::size(kGroupCategories);

    // The default case is everything is enabled, but |kRetainCategory| must be
    // explicitly passed.
    retain_current_data = trace_is_category_enabled(kRetainCategory) &&
                          num_enabled_categories != std::size(kGroupCategories);
  }

  if (current_group_mask_ != group_mask || retain_current_data) {
    if (zx::result res = StopKTrace(); res.is_error()) {
      return res.take_error();
    }

    trace_context_t* ctx = trace_acquire_context();
    if (ctx != nullptr) {
      auto d = fit::defer([ctx]() { trace_release_context(ctx); });
      if (zx::result res =
              StartKTrace(group_mask, trace_context_get_buffering_mode(ctx), retain_current_data);
          res.is_error()) {
        return res.take_error();
      }
    }
  }

  if (capture_log) {
    log_importer_.Start();
  } else {
    log_importer_.Stop();
  }
  return zx::ok();
}

zx::result<> App::StartKTrace(uint32_t group_mask, trace_buffering_mode_t buffering_mode,
                              bool retain_current_data) {
  if (!group_mask && !retain_current_data) {
    return zx::ok();  // nothing to trace
  }

  FX_LOGS(INFO) << "Starting ktrace";

  current_group_mask_ = group_mask;

  if (zx::result res = RequestKtraceStop(tracing_resource_); res.is_error()) {
    return res.take_error();
  }
  if (!retain_current_data) {
    if (zx::result res = RequestKtraceRewind(tracing_resource_); res.is_error()) {
      return res.take_error();
    }
    if (zx::result res = RequestKtraceStart(tracing_resource_, buffering_mode, group_mask);
        res.is_error()) {
      return res.take_error();
    }
  }

  // We poll zx_ktrace_read for data while tracing.
  auto drain_context = DrainContext::Create(tracing_resource_, poll_period_, max_readout_time_);
  if (drain_context.is_error()) {
    FX_PLOGS(ERROR, drain_context.error_value()) << "Failed to start reading kernel buffer";
    return drain_context.take_error();
  }
  zx_status_t result = async::PostTask(
      async_get_default_dispatcher(), [drain_context = std::move(drain_context.value())]() mutable {
        ForwardBuffer(std::move(drain_context));
      });
  if (result != ZX_OK) {
    FX_PLOGS(ERROR, result) << "Failed to schedule buffer writer";
    return zx::error(result);
  }

  FX_LOGS(DEBUG) << "Ktrace started";
  return zx::ok();
}

zx::result<> App::StopKTrace() {
  if (current_group_mask_ == 0) {
    return zx::ok();
  }

  auto d = fit::defer([this]() { current_group_mask_ = 0u; });

  FX_LOGS(INFO) << "Stopping ktrace";

  if (zx::result res = RequestKtraceStop(tracing_resource_); res.is_error()) {
    return res;
  }

  return zx::ok();
}

}  // namespace ktrace_provider
