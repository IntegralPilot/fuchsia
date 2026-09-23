// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE_COMMANDS_RECORD_H_
#define SRC_PERFORMANCE_TRACE_COMMANDS_RECORD_H_

#include <lib/async/cpp/wait.h>
#include <lib/zx/process.h>
#include <lib/zx/time.h>

#include <memory>
#include <optional>
#include <ostream>
#include <string>
#include <unordered_map>
#include <vector>

#include "src/lib/fxl/memory/weak_ptr.h"
#include "src/performance/lib/trace_converters/chromium_exporter.h"
#include "src/performance/trace/cmd_utils.h"
#include "src/performance/trace/command.h"
#include "src/performance/trace/tracer.h"

namespace tracing {

class RecordCommand : public CommandWithController {
 public:
  struct Options {
    bool Setup(const fxl::CommandLine&);

    std::string test_name;
    std::string app;
    std::vector<std::string> args;
    std::vector<std::string> categories;
    zx::duration duration = zx::sec(kDefaultDurationSeconds);
    bool detach = false;
    bool decouple = false;
    bool spawn = false;
    bool return_child_result = true;
    std::optional<std::string> environment_name;
    uint32_t buffer_size_megabytes = kDefaultBufferSizeMegabytes;
    std::vector<ProviderSpec> provider_specs;
    fuchsia_tracing::BufferingMode buffering_mode = kDefaultBufferingMode;
    bool binary = false;

    std::string output_file_name = kDefaultOutputFileName;
    std::string benchmark_results_file;
    std::string test_suite;
    std::unordered_map<std::string, Action> trigger_specs;
  };

  static Info Describe();

  explicit RecordCommand();

 protected:
  void Start(const fxl::CommandLine& command_line) override;

 private:
  void TerminateTrace(int32_t return_code);
  void DoneTrace();
  void LaunchComponentApp();
  void LaunchSpawnedApp();
  void StartTimer();
  void OnSpawnedAppExit(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                        const zx_packet_signal_t* signal);
  void KillSpawnedApp();
  void OnAlert(std::string alert_name);

  async_dispatcher_t* dispatcher_;
  zx::process spawned_app_;
  async::WaitMethod<RecordCommand, &RecordCommand::OnSpawnedAppExit> wait_spawned_app_;

  std::unique_ptr<std::ofstream> binary_out_;
  // TODO(https://fxbug.dev/42097007): Remove |exporter_|.
  std::unique_ptr<ChromiumExporter> exporter_;

  std::unique_ptr<Tracer> tracer_;

  bool tracing_ = false;
  int32_t return_code_ = 0;
  Options options_;

  fxl::WeakPtrFactory<RecordCommand> weak_ptr_factory_;
};

}  // namespace tracing

#endif  // SRC_PERFORMANCE_TRACE_COMMANDS_RECORD_H_
