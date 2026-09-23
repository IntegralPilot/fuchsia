// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/scheduler/role.h>
#include <lib/trace-provider/provider.h>

#include <memory>

#include "lib/syslog/cpp/log_settings.h"
#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/monitor/imminent_oom_observer.h"
#include "src/developer/memory/monitor/monitor.h"

namespace {
std::optional<fidl::Client<fuchsia_hardware_ram_metrics::Device>> GetRamDevice(
    async_dispatcher_t* dispatcher) {
  // Look for optional RAM device that provides bandwidth measurement interface.
  zx::result device =
      component::SyncServiceMemberWatcher<fuchsia_hardware_ram_metrics::Service::Device>()
          .GetNextInstance(true);
  if (device.is_error()) {
    FX_LOGS(INFO) << "CANNOT collect memory bandwidth measurements. error_code: "
                  << device.status_string();
    return std::nullopt;
  }
  FX_LOGS(INFO) << "Will collect memory bandwidth measurements.";
  return fidl::Client<fuchsia_hardware_ram_metrics::Device>(std::move(*device), dispatcher);
}
}  // namespace

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  // TODO(https://fxbug.dev/508683746): Remove this once no longer needed.
  fuchsia_logging::LogSettingsBuilder log_settings;
  log_settings.WithTags({"memory_monitor"});
  log_settings.BuildAndInitialize();
  FX_LOGS(DEBUG) << argv[0] << ": starting";

  trace::TraceProviderWithFdio trace_provider(loop.dispatcher(), monitor::Monitor::kTraceName);
  // Lower the priority.
  zx_status_t status = fuchsia_scheduler::SetRoleForThisThread("fuchsia.memory-monitor.main");
  FX_CHECK(status == ZX_OK) << "Set scheduler role status: " << zx_status_get_string(status);

  auto maker = memory::CaptureMaker::Create(memory::CreateDefaultOS());
  if (maker.is_error()) {
    FX_LOGS(ERROR) << "Error getting capture state: " << zx_status_get_string(maker.error_value());
    exit(EXIT_FAILURE);
  }

  zx::result pressure_client_end = component::Connect<fuchsia_memorypressure::Provider>();
  if (!pressure_client_end.is_ok()) {
    FX_LOGS(ERROR) << "Error connecting to FIDL fuchsia.memorypressure.Provider: "
                   << pressure_client_end.status_string();
    exit(-1);
  }
  auto pressure_provider = fidl::Client{std::move(*pressure_client_end), loop.dispatcher()};

  auto root_job_client_end = component::Connect<fuchsia_kernel::RootJobForInspect>();
  if (!root_job_client_end.is_ok()) {
    FX_LOGS(ERROR) << "Error connecting to root job: " << root_job_client_end.error_value();
    exit(-1);
  }

  auto root_job = fidl::WireCall(*root_job_client_end)->Get();
  if (root_job.status() != ZX_OK) {
    FX_LOGS(ERROR) << "Error getting root job: " << root_job.status();
    exit(-1);
  }
  zx_handle_t imminent_oom_event_handle;
  zx_status_t imminent_oom_status = zx_system_get_event(
      root_job->job.get(), ZX_SYSTEM_EVENT_IMMINENT_OUT_OF_MEMORY, &imminent_oom_event_handle);
  if (imminent_oom_status != ZX_OK) {
    FX_LOGS(ERROR) << "zx_system_get_event [IMMINENT-OOM] returned "
                   << zx_status_get_string(imminent_oom_status);
    exit(-1);
  }
  monitor::ImminentOomEventObserver imminent_oom_observer{imminent_oom_event_handle};

  std::optional<fidl::Client<fuchsia_metrics::MetricEventLoggerFactory>> factory;
  {
    zx::result client_end = component::Connect<fuchsia_metrics::MetricEventLoggerFactory>();
    if (!client_end.is_ok()) {
      FX_LOGS(ERROR) << "Unable to get metrics.MetricEventLoggerFactory.";
    } else {
      factory.emplace(std::move(*client_end), loop.dispatcher());
    }
  }
  auto app = std::make_unique<monitor::Monitor>(
      loop.dispatcher(), memory_monitor_config::Config::TakeFromStartupHandle(), std::move(*maker),
      std::move(pressure_provider), &imminent_oom_observer, std::move(factory),
      GetRamDevice(loop.dispatcher()));
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());
  zx::result result = outgoing.AddProtocol<fuchsia_memory_inspection::Collector>(std::move(app));
  FX_CHECK(result.is_ok());

  result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve the outgoing directory from startup info: "
                   << result.status_string();
    return -1;
  }
  loop.Run();
  FX_LOGS(DEBUG) << argv[0] << ": exiting";
  return 0;
}
