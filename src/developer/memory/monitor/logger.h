// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_MEMORY_MONITOR_LOGGER_H_
#define SRC_DEVELOPER_MEMORY_MONITOR_LOGGER_H_

#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/inspect/cpp/bounded_list_node.h>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/digest.h"
#include "src/developer/memory/monitor/high_water.h"
#include "src/developer/memory/monitor/memory_monitor_config.h"
#include "src/developer/memory/pressure_signaler/pressure_observer.h"
#include "src/lib/fxl/macros.h"

namespace monitor {

class Logger {
 public:
  using CaptureCb = fit::function<zx_status_t(memory::Capture*)>;
  using DigestCb = fit::function<void(const memory::Capture&, memory::Digest*)>;

  Logger(async_dispatcher_t* dispatcher, std::optional<monitor::HighWater*> high_water,
         CaptureCb capture_cb, DigestCb digest_cb, memory_monitor_config::Config* config,
         inspect::Node node);

  // SetPressureLevel needs to be called at least once for the Logger to start.
  void SetPressureLevel(pressure_signaler::Level l);

 private:
  void Log();
  async_dispatcher_t* dispatcher_;
  std::optional<monitor::HighWater*> high_water_;
  CaptureCb capture_cb_;
  DigestCb digest_cb_;
  memory_monitor_config::Config* config_;
  bool capture_high_water_ = false;
  zx::duration duration_;
  async::TaskClosureMethod<Logger, &Logger::Log> task_{this};
  inspect::Node root_node_;
  inspect::BoundedListNode inspect_bucket_digest_node_;
  std::optional<inspect::StringArray> bucket_names_;
  std::once_flag bucket_names_flag_;

  FXL_DISALLOW_COPY_AND_ASSIGN(Logger);
};

}  // namespace monitor

#endif  // SRC_DEVELOPER_MEMORY_MONITOR_LOGGER_H_
