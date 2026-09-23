// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "generic-suspend.h"

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/trace/event.h>
#include <zircon/errors.h>
#include <zircon/syscalls-next.h>
#include <zircon/time.h>

#include "fidl/fuchsia.hardware.power.suspend/cpp/markers.h"
#include "fidl/fuchsia.hardware.power.suspend/cpp/wire_types.h"
#include "fidl/fuchsia.kernel/cpp/markers.h"
#include "fidl/fuchsia.power.observability/cpp/fidl.h"
#include "fidl/fuchsia.power.observability/cpp/natural_types.h"
#include "lib/driver/component/cpp/prepare_stop_completer.h"
#include "lib/driver/incoming/cpp/namespace.h"
#include "lib/fidl/cpp/wire/arena.h"
#include "lib/fidl/cpp/wire/channel.h"
#include "lib/fidl/cpp/wire/vector_view.h"
#include "lib/fidl/cpp/wire/wire_messaging_declarations.h"
#include "zircon/status.h"

namespace suspend {

namespace fobs = fuchsia_power_observability;

namespace {

constexpr char kDeviceName[] = "generic-suspend-device";

constexpr uint64_t kInspectHistorySize = 128;
}  // namespace

GenericSuspend::GenericSuspend()
    : fdf::DriverBase2("generic-suspend"),
      devfs_connector_(fit::bind_member<&GenericSuspend::Serve>(this)) {}

zx::result<zx::resource> GenericSuspend::GetCpuResource(fdf::Namespace& incoming) {
  zx::result resource = incoming.Connect<fuchsia_kernel::CpuResource>();
  if (resource.is_error()) {
    return resource.take_error();
  }

  fidl::WireResult result = fidl::WireCall(resource.value())->Get();
  if (!result.ok()) {
    return zx::error(result.status());
  }

  return zx::ok(std::move(result.value().resource));
}

zx::result<> GenericSuspend::CreateDevfsNode() {
  fidl::Arena arena;
  zx::result connector = devfs_connector_.Bind(dispatcher());
  if (connector.is_error()) {
    fdf::error("Error creating devfs node");
    return connector.take_error();
  }

  auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena).connector(
      std::move(connector.value()));

  auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                  .name(arena, kDeviceName)
                  .devfs_args(devfs.Build())
                  .Build();

  auto controller_endpoints = fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();

  zx::result node_endpoints = fidl::CreateEndpoints<fuchsia_driver_framework::Node>();
  ZX_ASSERT_MSG(node_endpoints.is_ok(), "Failed to create endpoints: %s",
                node_endpoints.status_string());

  fidl::WireResult result = fidl::WireCall(node())->AddChild(
      args, std::move(controller_endpoints.server), std::move(node_endpoints->server));
  if (!result.ok()) {
    fdf::error("Failed to add child {}", result.status_string());
    return zx::error(result.status());
  }
  controller_.Bind(std::move(controller_endpoints.client));
  parent_.Bind(std::move(node_endpoints->client));
  return zx::ok();
}

zx::result<> GenericSuspend::Start(fdf::DriverContext context) {
  exposed_inspector_.emplace(context.CreateInspector(this));
  inspect_events_.emplace(exposed_inspector_->root().CreateChild(fobs::kSuspendEventsNode),
                          kInspectHistorySize);

  fuchsia_hardware_power_suspend::SuspendService::InstanceHandler handler({
      .suspender = suspend_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });

  auto result =
      outgoing()->AddService<fuchsia_hardware_power_suspend::SuspendService>(std::move(handler));
  if (result.is_error()) {
    fdf::error("Failed to add Suspender service {}", result);
    return result.take_error();
  }

  AtStart();

  zx::result resource = GetCpuResource(context.incoming());
  if (!resource.is_ok()) {
    fdf::error("Failed to get CPU Resource: {}", resource);
    return resource.take_error();
  }

  cpu_resource_ = std::move(resource.value());

  zx::result create_devfs_node_result = CreateDevfsNode();
  if (create_devfs_node_result.is_error()) {
    fdf::error("Failed to export to devfs {}", create_devfs_node_result);
    return create_devfs_node_result.take_error();
  }

  fdf::info("Started Generic Suspend Driver");

  return zx::ok();
}

void GenericSuspend::Stop(fdf::StopCompleter completer) { completer(zx::ok()); }

void GenericSuspend::GetSuspendStates(GetSuspendStatesCompleter::Sync& completer) {
  fidl::Arena arena;

  auto suspend_to_idle =
      fuchsia_hardware_power_suspend::wire::SuspendState::Builder(arena).resume_latency(0).Build();
  std::vector<fuchsia_hardware_power_suspend::wire::SuspendState> suspend_states = {
      suspend_to_idle};

  auto resp =
      fuchsia_hardware_power_suspend::wire::SuspenderGetSuspendStatesResponse::Builder(arena)
          .suspend_states(std::move(suspend_states))
          .Build();

  completer.ReplySuccess(resp);
}

zx::result<WakeSourceReport> GenericSuspend::SystemSuspendEnter() {
  // LINT.IfChange
  TRACE_DURATION("power", "generic-suspend:suspend");
  // LINT.ThenChange(//src/performance/lib/trace_processing/metrics/suspend.py)
  WakeSourceReport report;
  const auto entry_count = sizeof(report.entries) / sizeof(report.entries[0]);
  fdf::debug("entry_count requested: {}", entry_count);
  const zx_status_t status =
      zx_system_suspend_enter(cpu_resource_.get(), ZX_TIME_INFINITE, /*options=*/0, &report.header,
                              report.entries, entry_count, &report.actual_entry_count);
  fdf::debug("entry_count actual: {}", report.actual_entry_count);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(report);
}

void GenericSuspend::Suspend(SuspendRequestView request, SuspendCompleter::Sync& completer) {
  fidl::Arena arena;
  auto function_start = zx_clock_get_boot();

  if (!request->has_state_index() || request->state_index() != 0) {
    // This driver only supports one suspend state for now.
    fdf::error("Invalid argument to suspend");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  inspect_events_->CreateEntry([function_start](inspect::Node& n) {
    n.RecordInt(fobs::kSuspendAttemptedAt, function_start);
  });

  fdf::info("on suspend: current monotonic time {}", zx_clock_get_monotonic());
  auto suspend_start = zx_clock_get_boot();
  zx::result<WakeSourceReport> result = SystemSuspendEnter();
  auto suspend_return = zx_clock_get_boot();
  fdf::info("on resume: current monotonic time {}", zx_clock_get_monotonic());

  if (result.is_error()) {
    auto error_value = result.error_value();
    fdf::error("zx_system_suspend_enter failed: {}", zx_status_get_string(error_value));
    inspect_events_->CreateEntry([suspend_return](inspect::Node& n) {
      n.RecordInt(fobs::kSuspendFailedAt, suspend_return);
    });
    completer.ReplyError(error_value);
  } else {
    const auto& header = result.value().header;
    inspect_events_->CreateEntry([suspend_return, &header](inspect::Node& n) {
      n.RecordInt(fobs::kSuspendResumedAt, suspend_return);
      n.RecordInt(fobs::kWakeReasonReportTime, header.report_time);
      n.RecordInt(fobs::kWakeReasonWakeSourcesCount, header.total_wake_sources);
      n.RecordInt(fobs::kWakeReasonWakeSourcesUnreportedCount,
                  header.unreported_wake_report_entries);
    });

    // Copy wake vector results into the response.
    // TODO(b/343229277): Add support for soft_wake_vectors and other wake reports.
    const auto& entries = result.value().entries;
    auto reason_builder =
        fuchsia_hardware_power_suspend::wire::WakeReason::Builder(arena).wake_vectors_type(
            fuchsia_hardware_power_suspend::WakeVectorType::kKoid);
    const auto entry_count = result.value().actual_entry_count;
    const auto entry_count_clipped = std::min(entry_count, kMaxWakeSourceEntriesCount);
    fidl::VectorView<uint64_t> koids(arena, entry_count_clipped);
    for (uint32_t i = 0; i < entry_count_clipped; i++) {
      koids[i] = entries[i].koid;
    }
    reason_builder.wake_vectors(koids).wake_vectors_overflow(entry_count_clipped < entry_count);
    const auto reason = reason_builder.Build();

    // Convert to a response to suspend data.
    auto resp =
        fuchsia_hardware_power_suspend::wire::SuspenderSuspendResponse::Builder(arena)
            .suspend_duration(suspend_return - suspend_start)
            .suspend_overhead(suspend_start - function_start + zx_clock_get_boot() - suspend_return)
            .reason(reason)
            .Build();
    completer.ReplySuccess(resp);
  }
}

void GenericSuspend::ForceLowestPowerMode(ForceLowestPowerModeRequestView request,
                                          ForceLowestPowerModeCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void GenericSuspend::Serve(fidl::ServerEnd<fuchsia_hardware_power_suspend::Suspender> request) {
  suspend_bindings_.AddBinding(dispatcher(), std::move(request), this, fidl::kIgnoreBindingClosure);
}

}  // namespace suspend

// See driver-registration.cc for:
// FUCHSIA_DRIVER_EXPORT(suspend::GenericSuspend);
