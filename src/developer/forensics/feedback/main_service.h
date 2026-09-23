// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVELOPER_FORENSICS_FEEDBACK_MAIN_SERVICE_H_
#define SRC_DEVELOPER_FORENSICS_FEEDBACK_MAIN_SERVICE_H_

#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/process/lifecycle/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/executor.h>
#include <lib/async/dispatcher.h>
#include <lib/fidl/cpp/interface_handle.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fit/defer.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/sys/cpp/service_directory.h>

#include <memory>
#include <optional>
#include <string>

#include "src/developer/forensics/feedback/annotation_providers.h"
#include "src/developer/forensics/feedback/config.h"
#include "src/developer/forensics/feedback/crash_reports.h"
#include "src/developer/forensics/feedback/feedback_data.h"
#include "src/developer/forensics/feedback/last_reboot.h"
#include "src/developer/forensics/feedback/network_watcher.h"
#include "src/developer/forensics/feedback/system_time_tracker.h"
#include "src/developer/forensics/utils/cobalt/logger.h"
#include "src/developer/forensics/utils/inspect_node_manager.h"
#include "src/developer/forensics/utils/instrumented_binding_set.h"
#include "src/developer/forensics/utils/redact/redactor.h"
#include "src/lib/timekeeper/clock.h"

namespace forensics::feedback {

class MainService {
 public:
  struct Options {
    std::optional<std::string> local_device_id_path;
    std::string graceful_shutdown_info_write_path;
    std::string current_system_time_write_path;
    LastReboot::Options last_reboot_options;
    CrashReports::Options crash_reports_options;
    FeedbackData::Options feedback_data_options;
  };

  MainService(async_dispatcher_t* dispatcher, std::shared_ptr<sys::ServiceDirectory> services,
              timekeeper::Clock* clock, inspect::Node* inspect_root, cobalt::Logger* cobalt,
              const Annotations& startup_annotations,
              fidl::InterfaceRequest<fuchsia::process::lifecycle::Lifecycle> lifecycle_channel,
              std::unique_ptr<RedactorBase> redactor, Options options);

  ~MainService();

  template <typename Protocol>
  ::fidl::InterfaceRequestHandler<Protocol> GetHandler();

 private:
  async_dispatcher_t* dispatcher_;
  async::Executor executor_;
  std::shared_ptr<sys::ServiceDirectory> services_;
  timekeeper::Clock* clock_;
  inspect::Node* inspect_root_;
  cobalt::Logger* cobalt_;
  std::unique_ptr<RedactorBase> redactor_;

  InspectNodeManager inspect_node_manager_;

  AnnotationProviders annotations_;
  NetworkWatcher network_watcher_;

  FeedbackData feedback_data_;
  CrashReports crash_reports_;
  LastReboot last_reboot_;
  async::Loop system_time_tracker_loop_;
  SystemTimeTracker system_time_tracker_;

  InstrumentedBindingSet<fuchsia::feedback::ComponentDataRegister>
      component_data_register_bindings_;
  InstrumentedBindingSet<fuchsia::feedback::CrashReporter> crash_reporter_bindings_;
  InstrumentedBindingSet<fuchsia::feedback::CrashReportingProductRegister> crash_register_bindings_;
  InstrumentedBindingSet<fuchsia::feedback::DataProvider> data_provider_bindings_;
  InstrumentedBindingSet<fuchsia::feedback::LastRebootInfoProvider> last_reboot_info_bindings_;
};

}  // namespace forensics::feedback

#endif  // SRC_DEVELOPER_FORENSICS_FEEDBACK_MAIN_SERVICE_H_
