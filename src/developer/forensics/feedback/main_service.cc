// Copyright 2021 The Fuchsia Authors.All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback/main_service.h"

#include <fuchsia/feedback/cpp/fidl.h>
#include <fuchsia/hardware/power/statecontrol/cpp/fidl.h>
#include <fuchsia/process/lifecycle/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>
#include <optional>
#include <utility>

#include "src/developer/forensics/feedback/annotations/device_id_provider.h"
#include "src/developer/forensics/feedback/annotations/provider.h"
#include "src/developer/forensics/feedback/constants.h"
#include "src/developer/forensics/feedback/reboot_log/graceful_shutdown_info.h"
#include "src/developer/forensics/feedback/redactor_factory.h"
#include "src/developer/forensics/feedback/stop_signals.h"
#include "src/developer/forensics/utils/cobalt/logger.h"

namespace forensics::feedback {
namespace {

using fuchsia::hardware::power::statecontrol::ShutdownWatcherRegister_RegisterWatcher_Result;

std::unique_ptr<CachedAsyncAnnotationProvider> MakeDeviceIdProvider(
    const std::optional<std::string>& local_device_id_path, async_dispatcher_t* dispatcher,
    const std::shared_ptr<sys::ServiceDirectory>& services) {
  if (local_device_id_path.has_value()) {
    FX_LOGS(INFO) << "Using local device id provider";
    return std::make_unique<LocalDeviceIdProvider>(local_device_id_path.value());
  }

  FX_LOGS(INFO) << "Using remote device id provider";
  return std::make_unique<RemoteDeviceIdProvider>(dispatcher, services,
                                                  AnnotationProviders::AnnotationProviderBackoff());
}

std::set<std::string> RemoveExcludedAnnotations(const std::set<std::string>& default_annotations,
                                                const std::set<std::string>& excluded_annotations) {
  std::set<std::string> difference;
  std::set_difference(default_annotations.begin(), default_annotations.end(),
                      excluded_annotations.begin(), excluded_annotations.end(),
                      std::inserter(difference, difference.begin()));
  return difference;
}

}  // namespace

MainService::MainService(
    async_dispatcher_t* dispatcher, const std::shared_ptr<sys::ServiceDirectory> services,
    timekeeper::Clock* clock, inspect::Node* inspect_root, cobalt::Logger* cobalt,
    const Annotations& startup_annotations,
    fidl::InterfaceRequest<fuchsia::process::lifecycle::Lifecycle> lifecycle_channel,
    std::unique_ptr<RedactorBase> redactor, Options options)
    : dispatcher_(dispatcher),
      executor_(dispatcher),
      services_(services),
      clock_(clock),
      inspect_root_(inspect_root),
      cobalt_(cobalt),
      redactor_(std::move(redactor)),
      inspect_node_manager_(inspect_root),
      annotations_(
          dispatcher_, services_,
          /*allowlist=*/
          RemoveExcludedAnnotations(
              options.feedback_data_options.snapshot_config.default_annotations,
              options.feedback_data_options.snapshot_exclusion_config.excluded_annotations),
          options.feedback_data_options.snapshot_exclusion_config.excluded_annotations,
          startup_annotations,
          MakeDeviceIdProvider(options.local_device_id_path, dispatcher_, services_)),
      network_watcher_(dispatcher, *services),
      feedback_data_(dispatcher_, services_, clock_, inspect_root_, cobalt_, redactor_.get(),
                     annotations_.GetAnnotationManager(), std::move(options.feedback_data_options)),
      crash_reports_(dispatcher_, services_, clock_, inspect_root_,
                     annotations_.GetAnnotationManager(), feedback_data_.DataProvider(),
                     options.crash_reports_options),
      last_reboot_(dispatcher_, cobalt_, redactor_.get(), crash_reports_.CrashReporter(),
                   std::move(options.last_reboot_options)),
      system_time_tracker_loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
      system_time_tracker_(system_time_tracker_loop_.dispatcher(), clock_,
                           kSystemTimeTrackerWritePeriod,
                           std::move(options.current_system_time_write_path)),
      component_data_register_bindings_(dispatcher_, annotations_.ComponentDataRegister(),
                                        &inspect_node_manager_,
                                        "/fidl/fuchsia.feedback.ComponentDataRegister"),
      crash_reporter_bindings_(dispatcher_, crash_reports_.CrashReporter(), &inspect_node_manager_,
                               "/fidl/fuchsia.feedback.CrashReporter"),
      crash_register_bindings_(dispatcher_, crash_reports_.CrashRegister(), &inspect_node_manager_,
                               "/fidl/fuchsia.feedback.CrashReportingProductRegister"),
      data_provider_bindings_(dispatcher_, feedback_data_.DataProvider(), &inspect_node_manager_,
                              "/fidl/fuchsia.feedback.DataProvider"),
      last_reboot_info_bindings_(dispatcher_, last_reboot_.LastRebootInfoProvider(),
                                 &inspect_node_manager_,
                                 "/fidl/fuchsia.feedback.LastRebootInfoProvider") {
  executor_.schedule_task(
      WaitForLifecycleStop(dispatcher_, std::move(lifecycle_channel))
          .and_then([this](LifecycleStopSignal& signal) {
            FX_LOGS(INFO) << "Received stop signal; stopping upload, but not exiting "
                             "to continue persisting new reports and logs";

            crash_reports_.ShutdownImminent();
            feedback_data_.ShutdownImminent(
                fit::defer_callback([s = std::move(signal)]() mutable { s.Respond(); }));
          })
          .or_else([](const Error& error) {
            FX_LOGS(ERROR) << "Won't receive lifecycle signal: " << ToString(error);
          }));

  fidl::InterfaceHandle<fuchsia::hardware::power::statecontrol::ShutdownWatcher> handle;
  executor_.schedule_task(
      WaitForShutdownInfo(dispatcher_, handle.NewRequest())
          .and_then([this, path = options.graceful_shutdown_info_write_path](
                        GracefulShutdownInfoSignal& signal) {
            const GracefulShutdownAction action = signal.Action();
            const std::vector<GracefulShutdownReason> reasons = signal.Reasons();
            FX_LOGS(INFO) << std::format("Received shutdown action '{}' with reasons '{}'",
                                         ToString(action), ToRawStrings(reasons));

            WriteGracefulShutdownInfo(action, reasons, path);
            system_time_tracker_.RecordSystemShutdownSignal();
            signal.Respond();
          })
          .or_else([](const Error& error) {
            FX_LOGS(ERROR) << "Won't receive shutdown options: " << ToString(error);
          }));

  // We register ourselves with ShutdownWatcher using a fire-and-forget request that gives
  // an endpoint to a long-lived connection we maintain.
  //
  // The ack response is ignored because the failures aren't expected unless the system is in a dire
  // state.
  services->Connect<fuchsia::hardware::power::statecontrol::ShutdownWatcherRegister>()
      ->RegisterWatcher(std::move(handle),
                        [](const ShutdownWatcherRegister_RegisterWatcher_Result& result) {});

  network_watcher_.Register(
      fit::bind_member(&crash_reports_, &CrashReports::SetNetworkIsReachable));

  // Perform periodic writes on a separate thread so that Feedback can still function if the
  // filesystem gets stuck.
  if (const zx_status_t status = system_time_tracker_loop_.StartThread("system-time-tracker");
      status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Failed to start writer thread for SystemTimeTracker";
  }

  system_time_tracker_.Start();
}

MainService::~MainService() { system_time_tracker_loop_.Shutdown(); }

template <>
::fidl::InterfaceRequestHandler<fuchsia::feedback::LastRebootInfoProvider>
MainService::GetHandler() {
  return [this](::fidl::InterfaceRequest<fuchsia::feedback::LastRebootInfoProvider> request) {
    last_reboot_info_bindings_.AddBinding(std::move(request));
  };
}

template <>
::fidl::InterfaceRequestHandler<fuchsia::feedback::CrashReporter> MainService::GetHandler() {
  return [this](::fidl::InterfaceRequest<fuchsia::feedback::CrashReporter> request) {
    crash_reporter_bindings_.AddBinding(std::move(request));
  };
}

template <>
::fidl::InterfaceRequestHandler<fuchsia::feedback::CrashReportingProductRegister>
MainService::GetHandler() {
  return
      [this](::fidl::InterfaceRequest<fuchsia::feedback::CrashReportingProductRegister> request) {
        crash_register_bindings_.AddBinding(std::move(request));
      };
}

template <>
::fidl::InterfaceRequestHandler<fuchsia::feedback::ComponentDataRegister>
MainService::GetHandler() {
  return [this](::fidl::InterfaceRequest<fuchsia::feedback::ComponentDataRegister> request) {
    component_data_register_bindings_.AddBinding(std::move(request));
  };
}

template <>
::fidl::InterfaceRequestHandler<fuchsia::feedback::DataProvider> MainService::GetHandler() {
  return [this](::fidl::InterfaceRequest<fuchsia::feedback::DataProvider> request) {
    data_provider_bindings_.AddBinding(std::move(request));
  };
}

}  // namespace forensics::feedback
