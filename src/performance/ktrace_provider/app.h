// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_KTRACE_PROVIDER_APP_H_
#define SRC_PERFORMANCE_KTRACE_PROVIDER_APP_H_

#include <lib/sys/cpp/component_context.h>
#include <lib/sys/cpp/service_directory.h>
#include <lib/trace-provider/provider.h>
#include <lib/trace/observer.h>

#include <fbl/unique_fd.h>

#include "src/lib/fxl/command_line.h"
#include "src/performance/ktrace_provider/log_importer.h"

namespace ktrace_provider {

std::vector<trace::KnownCategory> GetKnownCategories();

struct DrainContext {
  DrainContext(zx::time start, zx::resource tracing_resource, zx::duration poll_period,
               zx::duration max_readout_time, size_t buffer_size)
      : start(start),
        tracing_resource(std::move(tracing_resource)),
        poll_period(poll_period),
        max_readout_time(max_readout_time),
        buffer(std::make_unique_for_overwrite<uint64_t[]>(buffer_size / sizeof(uint64_t))),
        buffer_size(buffer_size) {}

  static zx::result<DrainContext> Create(const zx::resource& tracing_resource,
                                         zx::duration poll_period, zx::duration max_readout_time) {
    zx::resource cloned_resource;
    if (zx_status_t status = tracing_resource.duplicate(ZX_RIGHT_SAME_RIGHTS, &cloned_resource);
        status != ZX_OK) {
      return zx::error(status);
    }

    // We need to ask how big our buffer to copy data into needs to be.
    // We ask the kernel using zx_ktrace_read with a nullptr;
    size_t buffer_size = 0;
    if (zx_status_t status = zx_ktrace_read(tracing_resource.get(), nullptr, 0, 0, &buffer_size);
        status != ZX_OK) {
      return zx::error(status);
    }

    return zx::ok(DrainContext{zx::clock::get_monotonic(), std::move(cloned_resource), poll_period,
                               max_readout_time, buffer_size});
  }

  zx::time start;
  zx::resource tracing_resource;
  zx::duration poll_period;
  zx::duration max_readout_time;

  std::unique_ptr<uint64_t[]> buffer;
  size_t buffer_size;
};

class App {
 public:
  App(zx::resource tracing_resource, const fxl::CommandLine& command_line, zx::duration poll_period,
      zx::duration max_readout_time);

 private:
  zx::result<> UpdateState();

  zx::result<> StartKTrace(uint32_t group_mask, trace_buffering_mode_t buffering_mode,
                           bool retain_current_data);
  zx::result<> StopKTrace();

  trace::TraceObserver trace_observer_;
  LogImporter log_importer_;
  uint32_t current_group_mask_ = 0u;
  zx::resource tracing_resource_;
  zx::duration poll_period_;
  zx::duration max_readout_time_;

  App(const App&) = delete;
  App(App&&) = delete;
  App& operator=(const App&) = delete;
  App& operator=(App&&) = delete;
};

}  // namespace ktrace_provider

#endif  // SRC_PERFORMANCE_KTRACE_PROVIDER_APP_H_
