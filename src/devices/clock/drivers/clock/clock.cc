// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "clock.h"

#include <lib/ddk/metadata.h>
#include <lib/driver/component/cpp/driver_export.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/metadata/cpp/metadata.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/defer.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <memory>

#include <bind/fuchsia/clock/cpp/bind.h>
#include <bind/fuchsia/cpp/bind.h>

#include "src/devices/clock/drivers/clock/clock_parser.h"

namespace {

// The amount of historical data we store to emit for inspect. Currently these are chosen
// with the heuristic of average 1 change every minute, in order to support up to 1 hours
// of data at this rate. This rate in reality will vary between clocks, and the rate of state
// changes and frequency changes will likely be different too. We can use these as a start point
// and eventually change it if we need. Each rate entry will have a uint32 timestamp along with a
// byte. And each state change will have a uint32 timestamp and a bit. So for calculating the
// amount of bytes needed for these per clock it will be:
// (kInspectRateEntries * 5) + ((kInspectEnableEntries / 8) * ((8 * 4) + 1)
// (60 * 5) + ((60 / 8) * ((8 * 4) + 1)) = 0.5475 kB
// If we assume a high number of clocks like 100, the total comes to 54.75 kB (kilobytes).
constexpr size_t kInspectRateEntries = 60;
constexpr size_t kInspectEnableEntries = 60;

struct ClockState {
  bool enabled;
  uint64_t rate_hz;
};

inline int64_t GetCurrentMsec() {
  return zx::duration(zx::clock::get_boot().to_timespec()).to_msecs();
}

template <typename T>
inline std::vector<power_observability::internal::DataPoint<T>> ReconstructSeries(
    power_observability::internal::TimestampedBuffer<T>& buffer) {
  std::vector<power_observability::internal::DataPoint<T>> series;
  buffer.ForEachDataPoint([&](const power_observability::internal::DataPoint<T>& data_point) {
    series.push_back(data_point);
  });
  return series;
}

// Tries to get the enabled and rate. If any fails just set the default (false, 0).
ClockState TryGetClockState(fdf::UnownedClientEnd<fuchsia_hardware_clockimpl::ClockImpl> clock_impl,
                            uint32_t id) {
  ClockState state{.enabled = false, .rate_hz = 0};

  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult enabled_result = fdf::WireCall(clock_impl).buffer(arena)->IsEnabled(id);
  if (enabled_result.ok() && enabled_result->is_ok()) {
    state.enabled = enabled_result.value()->enabled;
  }

  fdf::WireUnownedResult rate_result = fdf::WireCall(clock_impl).buffer(arena)->GetRate(id);
  if (rate_result.ok() && rate_result->is_ok()) {
    state.rate_hz = rate_result.value()->hz;
  }

  return state;
}

fuchsia_hardware_clockimpl::ClockIdsMetadata ConvertMetadata(
    clock_metadata::ClockMetadata generic) {
  std::vector<fuchsia_hardware_clockimpl::ClockNodeDescriptor> clock_nodes;
  for (auto& c : generic.clock_nodes) {
    fuchsia_hardware_clockimpl::ClockNodeDescriptor node;
    node.clock_id(c.id);
    node.node_id(c.node_id);
    if (c.name) {
      node.name(std::move(*c.name));
    }
    clock_nodes.push_back(std::move(node));
  }
  return {{.clock_nodes = std::move(clock_nodes)}};
}

}  // namespace

void ClockDevice::Enable(EnableCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->Enable(id_);
  if (!result.ok()) {
    fdf::error("Failed to send Enable request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to enable clock {}: {}", id_, zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  inspect_enable_buffer_->AddEntry(true, GetCurrentMsec());

  completer.ReplySuccess();
}

void ClockDevice::Disable(DisableCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->Disable(id_);
  if (!result.ok()) {
    fdf::error("Failed to send Disable request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to disable clock {}: {}", id_, zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  inspect_enable_buffer_->AddEntry(false, GetCurrentMsec());

  completer.ReplySuccess();
}

void ClockDevice::IsEnabled(IsEnabledCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->IsEnabled(id_);
  if (!result.ok()) {
    fdf::error("Failed to send IsEnabled request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to check if clock {} is enabled: {}", id_,
               zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result.value()->enabled);
}

void ClockDevice::SetRate(SetRateRequestView request, SetRateCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  clock_impl_.buffer(arena)
      ->SetRate(id_, request->hz)
      .ThenExactlyOnce([this, completer = completer.ToAsync(), hz = request->hz](
                           fdf::WireUnownedResult<fuchsia_hardware_clockimpl::ClockImpl::SetRate>&
                               result) mutable {
        if (!result.ok()) {
          fdf::error("Failed to send SetRate request to clock {}: {}", id_, result.status_string());
          completer.ReplyError(result.status());
          return;
        }
        if (result->is_error()) {
          fdf::error("Failed to set rate for clock {}: {}", id_,
                     zx_status_get_string(result->error_value()));
          completer.ReplyError(result->error_value());
          return;
        }

        inspect_rate_buffer_->AddEntry(parent_->GetDataForRate(hz), GetCurrentMsec());
        completer.ReplySuccess();
      });
}

void ClockDevice::QuerySupportedRate(QuerySupportedRateRequestView request,
                                     QuerySupportedRateCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result =
      clock_impl_.sync().buffer(arena)->QuerySupportedRate(id_, request->hz_in);
  if (!result.ok()) {
    fdf::error("Failed to send QuerySupportedRate request to clock {}: {}", id_,
               result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    // TODO(b/426652785): ZX_ERR_OUT_OF_RANGE is a sentinel value that means that no suitable rate
    // could be found.
    if (result->error_value() != ZX_ERR_OUT_OF_RANGE) {
      fdf::error("Failed to query supported rate for clock {}: {}", id_,
                 zx_status_get_string(result->error_value()));
    }
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result.value()->hz);
}

void ClockDevice::GetRate(GetRateCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->GetRate(id_);
  if (!result.ok()) {
    fdf::error("Failed to send GetRate request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to get rate rate for clock {}: {}", id_,
               zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result.value()->hz);
}

void ClockDevice::SetInput(SetInputRequestView request, SetInputCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->SetInput(id_, request->idx);
  if (!result.ok()) {
    fdf::error("Failed to send SetInput request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to set input for clock {}: {}", id_,
               zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess();
}

void ClockDevice::GetNumInputs(GetNumInputsCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->GetNumInputs(id_);
  if (!result.ok()) {
    fdf::error("Failed to send GetNumInputs request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to get number of inputs for clock {}: {}", id_,
               zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result.value()->n);
}

void ClockDevice::GetInput(GetInputCompleter::Sync& completer) {
  fdf::Arena arena{'CLOC'};
  fdf::WireUnownedResult result = clock_impl_.sync().buffer(arena)->GetInput(id_);
  if (!result.ok()) {
    fdf::error("Failed to send GetInput request to clock {}: {}", id_, result.status_string());
    completer.ReplyError(result.status());
    return;
  }
  if (result->is_error()) {
    fdf::error("Failed to get input for clock {}: {}", id_,
               zx_status_get_string(result->error_value()));
    completer.ReplyError(result->error_value());
    return;
  }

  completer.ReplySuccess(result.value()->index);
}

void ClockDevice::GetProperties(GetPropertiesCompleter::Sync& completer) {
  completer.Reply(id_, fidl::StringView::FromExternal(name_));
}

void ClockDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_clock::Clock> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fdf::error("Unexpected Clock FIDL call: 0x{:x}", metadata.method_ordinal);
}

zx_status_t ClockDevice::Init(const std::shared_ptr<fdf::Namespace>& incoming,
                              const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                              std::optional<int32_t> node_id,
                              const fidl::ClientEnd<fuchsia_driver_framework::Node>& parent_node,
                              bool report_initial_conditions) {
  zx::result clock_impl = incoming->Connect<fuchsia_hardware_clockimpl::Service::Device>();
  if (clock_impl.is_error()) {
    fdf::error("Failed to connect to the clock-impl FIDL protocol: {}", clock_impl);
    return clock_impl.status_value();
  }

  if (report_initial_conditions) {
    auto state = TryGetClockState(clock_impl.value().borrow(), id_);
    inspect_rate_buffer_->AddEntry(parent_->GetDataForRate(state.rate_hz), GetCurrentMsec());
    inspect_enable_buffer_->AddEntry(state.enabled, GetCurrentMsec());
  }

  clock_impl_.Bind(std::move(clock_impl.value()), fdf::Dispatcher::GetCurrent()->get());

  if (!node_id.has_value()) {
    child_name_ = std::format("clock-{}", id_);
  } else {
    child_name_ = std::format("clock-{}_{}", id_, node_id.value());
  }

  auto node_offers = std::vector{
      fdf::MakeOffer2<fuchsia_hardware_clock::Service>(child_name_),
  };

  std::vector<fuchsia_driver_framework::NodeProperty2> node_properties{
      fdf::MakeProperty2(bind_fuchsia::SERVICE, "fuchsia.hardware.clock.Service"),
  };

  if (node_id.has_value()) {
    node_properties.push_back(
        fdf::MakeProperty2(bind_fuchsia::ID, static_cast<uint32_t>(node_id.value())));
  } else {
    node_properties.push_back(fdf::MakeProperty2(bind_fuchsia::ID, id_));
  }

  fuchsia_hardware_clock::Service::InstanceHandler instance_handler{
      {.clock = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                        fidl::kIgnoreBindingClosure)}};
  zx::result result = outgoing->AddService<fuchsia_hardware_clock::Service>(
      std::move(instance_handler), child_name_);
  if (result.is_error()) {
    fdf::error("Failed to add clock service to outgoing directory: {}", result);
    return result.status_value();
  }

  fuchsia_hardware_clock::DebugService::InstanceHandler debug_instance_handler{
      {.device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                         fidl::kIgnoreBindingClosure)}};
  result = outgoing->AddService<fuchsia_hardware_clock::DebugService>(
      std::move(debug_instance_handler), child_name_);
  if (result.is_error()) {
    fdf::error("Failed to add clock debug service to outgoing directory: {}", result);
    return result.status_value();
  }

  zx::result node = fdf::AddChild(parent_node, *fdf::Logger::GlobalInstance(), child_name_,
                                  node_properties, node_offers);
  if (node.is_error()) {
    fdf::error("Failed to create child node: {}", node);
    return node.status_value();
  }

  child_node_.Bind(std::move(node.value()), fdf::Dispatcher::GetCurrent()->async_dispatcher());
  child_node_->WaitForDriver().Then(fit::bind_member<&ClockDevice::WaitForDriverCompleted>(this));

  return ZX_OK;
}

bool ClockDevice::pending_driver() const { return pending_driver_; }

std::string_view ClockDevice::child_name() const { return child_name_; }

void ClockDevice::WaitForDriverCompleted(
    fidl::WireUnownedResult<fuchsia_driver_framework::NodeController::WaitForDriver>& result) {
  // Not much we can do in most of these failure cases, so just set these for all results before
  // returning.
  auto deferred = fit::defer([this]() {
    pending_driver_ = false;
    parent_->CheckIfReady();
  });

  if (!result.ok()) {
    fdf::error("Failed call to WaitForDriver for clock {}", child_name_);
    return;
  }

  if (result->is_error()) {
    fdf::error("WaitForDriver returned error for clock {}", child_name_);
    return;
  }

  switch (result->value()->Which()) {
    case fuchsia_driver_framework::wire::DriverResult::Tag::kDriverStartedNodeToken: {
      fdf::debug("Driver using clock {} has started.", child_name_);
      break;
    }
    case fuchsia_driver_framework::wire::DriverResult::Tag::kMatchError: {
      fdf::info("Clock {} did not match a driver/composite.", child_name_);
      break;
    }
    case fuchsia_driver_framework::wire::DriverResult::Tag::kStartError: {
      fdf::warn("Driver using clock {} failed to start.", child_name_);
      break;
    }
    default: {
      fdf::error("Clock {} unrecognized driver result type.", child_name_);
    }
  }
}

zx::result<> ClockDriver::Start(fdf::DriverContext context) {
  component_inspector_.emplace(context.CreateInspector(this));
  auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  zx::result clock_impl = incoming->Connect<fuchsia_hardware_clockimpl::Service::Device>();
  if (clock_impl.is_error()) {
    fdf::error("Failed to connect to the clock-impl FIDL protocol: {}", clock_impl);
    return clock_impl.take_error();
  }

  std::unordered_set<uint32_t> reported_initial_conditions;

  std::optional<fuchsia_hardware_clockimpl::InitMetadata> metadata;
  {
    zx::result result =
        fdf_metadata::GetMetadataIfExists<fuchsia_hardware_clockimpl::InitMetadata>(incoming);
    if (result.is_error()) {
      fdf::error("Failed to get metadata: {}", result);
      return result.take_error();
    }
    metadata = std::move(result.value());
  }
  if (metadata.has_value()) {
    zx_status_t status = ConfigureClocks(metadata.value(), std::move(clock_impl.value()),
                                         reported_initial_conditions);
    if (status != ZX_OK) {
      fdf::error("Failed to configure clocks: {}", zx_status_get_string(status));
      return zx::error(status);
    }

    auto props = std::vector{
        fdf::MakeProperty2(bind_fuchsia::INIT_STEP, bind_fuchsia_clock::BIND_INIT_STEP_CLOCK)};

    auto result = AddChild("clock-init", props, {});
    if (result.is_error()) {
      fdf::error("Failed to add child: {}", result);
      return result.take_error();
    }
    clock_init_child_node_ = std::move(result.value());
  } else {
    fdf::info("No init metadata provided");
  }

  zx_status_t status = CreateClockDevices(reported_initial_conditions, incoming);
  if (status != ZX_OK) {
    fdf::error("Failed to create clock devices: {}", zx_status_get_string(status));
    return zx::error(status);
  }

  component_inspector_->root().RecordLazyNode(
      "power_observability_state_recorders",
      fit::bind_member<&ClockDriver::PowerObservabilityInspectCallback>(this));

  return zx::ok();
}

void ClockDriver::CheckIfReady() {
  bool ready = true;
  for (auto& clock : clock_devices_) {
    if (clock->pending_driver()) {
      fdf::debug("clock {} not started yet.", clock->child_name());
      ready = false;
    }
  }

  if (ready) {
    // All children are either successfully started completely, or failed to bind to anything.
    // This is where we can notify the impl driver.
    fdf::info("Core clock has received all requests from children clocks.");
  }
}

// Generates Inspect data for power observability. This intends to match the exact format generated
// by the library at `//sdk/lib/power/state_recorder` which will allow us to use the same tooling.
//
// The reason for not using that library currently is that it stores all live data directly in the
// inspect library instead of populating it lazily. That causes it to use too much memory and limits
// the history we get out of it. This instead uses a space efficient storage that it can convert
// into the inspect format when needed.
//
// The data is structured as follows:
// {
//   "power_observability_state_recorders": {
//     "clock_rates:<name_or_id>:0x<id>": {
//       "metadata": {
//         "name": "clock_rate",
//         "type": "numeric",
//         "units": "hz",
//         "range": {
//           "min_inc": <minimum rate across all clocks>,
//           "max_inc": <maximum rate across all clocks>
//         }
//       },
//       "history": {
//         "0": { "@time": <timestamp>, "value": <rate in Hz> },
//         "1": { "@time": <timestamp>, "value": <rate in Hz> },
//         ...
//       }
//     },
//     "clock_states:<name_or_id>:0x<id>": {
//       "metadata": {
//         "name": "clock_state",
//         "type": "enum",
//         "states": {
//           "disabled": 0,
//           "enabled": 1
//         }
//       },
//       "history": {
//         "0": { "@time": <timestamp>, "value": <0 or 1> },
//         "1": { "@time": <timestamp>, "value": <0 or 1> },
//         ...
//       }
//     },
//     ...
//   }
// }
fpromise::promise<inspect::Inspector> ClockDriver::PowerObservabilityInspectCallback() {
  inspect::Inspector inspector;

  uint64_t rate_min = std::numeric_limits<int64_t>::max();
  uint64_t rate_max = 0;

  // The min/max is over ALL of our clocks.
  for (const auto& [rate, _] : rate_to_index_table_) {
    rate_min = std::min(rate, rate_min);
    rate_max = std::max(rate, rate_max);
  }

  for (const auto& [id, rate_buffer] : inspect_rate_buffers_) {
    auto rate_data = ReconstructSeries(*rate_buffer);

    std::string name_or_id;
    if (id_to_name_.contains(id)) {
      name_or_id = id_to_name_[id];
    } else {
      name_or_id = std::to_string(id);
    }

    inspector.GetRoot().RecordChild(
        std::format("clock_rates:{}:0x{:x}", name_or_id, id),
        [this, rate_data, rate_min, rate_max](inspect::Node& clock_rates_node) {
          clock_rates_node.RecordChild(
              "metadata", [rate_min, rate_max](inspect::Node& metadata_node) {
                metadata_node.RecordString("name", "clock_rate");
                metadata_node.RecordString("type", "numeric");
                metadata_node.RecordString("units", "hz");

                metadata_node.RecordChild("range", [rate_min, rate_max](inspect::Node& range_node) {
                  range_node.RecordInt("min_inc", static_cast<int64_t>(rate_min));
                  range_node.RecordInt("max_inc", static_cast<int64_t>(rate_max));
                });
              });

          clock_rates_node.RecordChild(
              "history", [this, rate_data](inspect::Node& history_node) mutable {
                int curr = 0;
                for (const auto& data : rate_data) {
                  history_node.RecordChild(
                      std::format("{}", curr++), [this, timestamp = data.timestamp_ns,
                                                  data = data.value](inspect::Node& entry_node) {
                        entry_node.RecordInt("@time", static_cast<int64_t>(timestamp));
                        entry_node.RecordInt("value", static_cast<int64_t>(GetRateForData(data)));
                      });
                }
              });
        });
  }

  for (const auto& [id, enabled_buffer] : inspect_enable_buffers_) {
    auto enabled_data = ReconstructSeries(*enabled_buffer);

    std::string name_or_id;
    if (id_to_name_.contains(id)) {
      name_or_id = id_to_name_[id];
    } else {
      name_or_id = std::to_string(id);
    }

    inspector.GetRoot().RecordChild(
        std::format("clock_states:{}:0x{:x}", name_or_id, id),
        [enabled_data](inspect::Node& clock_rates_node) {
          clock_rates_node.RecordChild("metadata", [](inspect::Node& metadata_node) {
            metadata_node.RecordString("name", "clock_state");
            metadata_node.RecordString("type", "enum");

            metadata_node.RecordChild("states", [](inspect::Node& range_node) {
              range_node.RecordInt("disabled", 0);
              range_node.RecordInt("enabled", 1);
            });
          });

          clock_rates_node.RecordChild(
              "history", [enabled_data](inspect::Node& history_node) mutable {
                int curr = 0;
                for (const auto& data : enabled_data) {
                  history_node.RecordChild(
                      std::format("{}", curr++), [timestamp = data.timestamp_ns,
                                                  data = data.value](inspect::Node& entry_node) {
                        entry_node.RecordInt("@time", static_cast<int64_t>(timestamp));
                        entry_node.RecordInt("value", static_cast<int64_t>(data));
                      });
                }
              });
        });
  }

  return fpromise::make_ok_promise(std::move(inspector));
}

uint8_t ClockDriver::GetDataForRate(uint64_t rate) {
  if (auto existing = rate_to_index_table_.find(rate); existing != rate_to_index_table_.end()) {
    return static_cast<uint8_t>(existing->second);
  }

  size_t index = rate_to_index_table_.size();
  if (index >= std::numeric_limits<uint8_t>::max()) {
    fdf::error("Cannot store any more clock rates in index table.");
    return 0;
  }

  uint8_t new_index = static_cast<uint8_t>(index);
  rate_to_index_table_[rate] = new_index;
  return new_index;
}

uint64_t ClockDriver::GetRateForData(uint8_t data) {
  for (const auto& [rate, idx] : rate_to_index_table_) {
    if (idx == data) {
      return rate;
    }
  }

  fdf::error("Cannot find the clock rate index requested.");
  return 0;
}

zx_status_t ClockDriver::CreateClockDevices(
    std::unordered_set<uint32_t>& reported_initial_conditions,
    const std::shared_ptr<fdf::Namespace>& incoming) {
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
  zx::result clock_impl = incoming->Connect<fuchsia_hardware_clockimpl::Service::Device>();
  if (clock_impl.is_error()) {
    fdf::error("Failed to connect to the clock-impl FIDL protocol: {}", clock_impl);
    return clock_impl.error_value();
  }

  fdf::Arena arena('CLCK');
  fdf::WireUnownedResult clock_properties =
      fdf::WireCall(*clock_impl).buffer(arena)->GetClockProperties();
  std::unordered_map<uint32_t, std::string_view> name_mapping;
  // If the impl doesn't support GetImplMetadata, we can just fallback to device tree based names.
  if (clock_properties.ok() && clock_properties->is_ok()) {
    for (const auto& property : clock_properties->value()->clock_properties) {
      if (property.has_clock_id() && property.has_clock_name()) {
        name_mapping[property.clock_id()] = property.clock_name().get();
      }
    }
  }

  std::optional<fuchsia_hardware_clockimpl::ClockIdsMetadata> metadata;
  {
    // Try to get generic metadata first
    zx::result generic_res =
        fdf_metadata::GetMetadataFromFidlServiceIfExists<fuchsia_driver_metadata::Dictionary>(
            incoming->svc_dir(), "fuchsia.hardware.clockimpl.ClockIdsMetadata");
    if (generic_res.is_ok() && generic_res.value().has_value()) {
      auto parsed = clock_metadata::ClockMetadata::Parse(*generic_res.value());
      if (parsed) {
        metadata = ConvertMetadata(std::move(*parsed));
      } else {
        fdf::error("Failed to parse generic Clock metadata");
      }
    }

    if (!metadata.has_value()) {
      // Fall back to old metadata
      zx::result clock_nodes_metadata_res =
          fdf_metadata::GetMetadata<fuchsia_hardware_clockimpl::ClockIdsMetadata>(incoming);
      if (clock_nodes_metadata_res.is_error()) {
        fdf::error("Failed to get clock IDs: {}", clock_nodes_metadata_res);
        return clock_nodes_metadata_res.status_value();
      }
      metadata = std::move(*clock_nodes_metadata_res);
    }
  }

  const auto& clock_nodes = metadata->clock_nodes();
  if (!clock_nodes.has_value()) {
    return ZX_OK;
  }

  for (const auto& node : clock_nodes.value()) {
    if (!node.clock_id().has_value()) {
      fdf::error("Clock ID Metadata has an entry with no clock id");
      return ZX_ERR_INVALID_ARGS;
    }
    const uint32_t clock_id = node.clock_id().value();

    // Prefer to use the name provided by the impl driver, but if one doesn't exist, use the
    // name provided by the device tree. If neither are there, use "<anonymous>".
    std::string_view name;
    if (!name_mapping.empty() && name_mapping.contains(clock_id)) {
      name = name_mapping[clock_id];
      id_to_name_[clock_id] = name;
    } else if (node.name().has_value()) {
      name = node.name().value();
      id_to_name_[clock_id] = name;
    } else {
      name = "<anonymous>";
    }

    // ClockDevice must be dynamically allocated because it has a ServerBindingGroup and compat
    // server property which cannot be moved.
    auto clock_device = std::make_unique<ClockDevice>(
        GetOrCreateRateBuffer(clock_id), GetOrCreateEnableBuffer(clock_id), this, clock_id, name);
    zx_status_t status = clock_device->Init(incoming, outgoing(), node.node_id(), this->node(),
                                            !reported_initial_conditions.contains(clock_id));
    if (status != ZX_OK) {
      fdf::error("Failed to initialize clock device: {}", zx_status_get_string(status));
      return status;
    }

    reported_initial_conditions.insert(clock_id);

    clock_devices_.emplace_back(std::move(clock_device));
  }

  return ZX_OK;
#else
#error "Cannot create clock devices: Clock IDs not available at given Fuchsia API level";
#endif
}

const std::shared_ptr<ByteBuffer>& ClockDriver::GetOrCreateRateBuffer(uint32_t clock_id) {
  if (!inspect_rate_buffers_.contains(clock_id)) {
    inspect_rate_buffers_[clock_id] = std::make_shared<ByteBuffer>(kInspectRateEntries);
  }

  return inspect_rate_buffers_[clock_id];
}

const std::shared_ptr<BitBuffer>& ClockDriver::GetOrCreateEnableBuffer(uint32_t clock_id) {
  if (!inspect_enable_buffers_.contains(clock_id)) {
    inspect_enable_buffers_[clock_id] = std::make_shared<BitBuffer>(kInspectEnableEntries);
  }

  return inspect_enable_buffers_[clock_id];
}

zx_status_t ClockDriver::ConfigureClocks(
    const fuchsia_hardware_clockimpl::InitMetadata& metadata,
    fdf::ClientEnd<fuchsia_hardware_clockimpl::ClockImpl> clock_impl_client,
    std::unordered_set<uint32_t>& reported_initial_conditions) {
  fdf::WireSyncClient<fuchsia_hardware_clockimpl::ClockImpl> clock_impl{
      std::move(clock_impl_client)};
  fdf::Arena arena{'CLOC'};

  // Stop processing the list if any call returns an error so that clocks are not accidentally
  // enabled in an unknown state.
  for (const auto& step : metadata.steps()) {
    auto call = step.call();
    if (!call.has_value()) {
      fdf::error("Clock Metadata init step is missing a call field");
      return ZX_ERR_INVALID_ARGS;
    }
    auto clock_id = step.id();
    auto which = call->Which();

    // Delay doesn't apply to any particular clock ID so we enforce that the ID field is
    // unset. Every other type of init call requires an ID so we enforce that ID is set.
    if (which == fuchsia_hardware_clockimpl::InitCall::Tag::kDelay && clock_id.has_value()) {
      fdf::error("Clock Init Delay calls must not have an ID, id = {}", clock_id.value());
      return ZX_ERR_INVALID_ARGS;
    }
    if (which != fuchsia_hardware_clockimpl::InitCall::Tag::kDelay && !clock_id.has_value()) {
      fdf::error("Clock init calls must have an ID");
      return ZX_ERR_INVALID_ARGS;
    }

    // Make sure we have the initial conditions of the clock.
    if (clock_id.has_value() && !reported_initial_conditions.contains(clock_id.value())) {
      auto state = TryGetClockState(clock_impl.client_end(), clock_id.value());
      GetOrCreateEnableBuffer(clock_id.value())->AddEntry(state.enabled, GetCurrentMsec());
      GetOrCreateRateBuffer(clock_id.value())
          ->AddEntry(GetDataForRate(state.rate_hz), GetCurrentMsec());
      reported_initial_conditions.insert(clock_id.value());
    }

    switch (which) {
      case fuchsia_hardware_clockimpl::InitCall::Tag::kEnable: {
        fdf::WireUnownedResult result = clock_impl.buffer(arena)->Enable(clock_id.value());
        if (!result.ok()) {
          fdf::error("Failed to send Enable request for clock {}: {}", clock_id.value(),
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to enable clock {}: {}", clock_id.value(),
                     zx_status_get_string(result->error_value()));
          return result->error_value();
        }

        GetOrCreateEnableBuffer(clock_id.value())->AddEntry(true, GetCurrentMsec());
        break;
      }
      case fuchsia_hardware_clockimpl::InitCall::Tag::kDisable: {
        fdf::WireUnownedResult result = clock_impl.buffer(arena)->Disable(clock_id.value());
        if (!result.ok()) {
          fdf::error("Failed to send Disable request for clock {}: {}", clock_id.value(),
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to disable clock {}: {}", clock_id.value(),
                     zx_status_get_string(result->error_value()));
          return result->error_value();
        }

        GetOrCreateEnableBuffer(clock_id.value())->AddEntry(false, GetCurrentMsec());
        break;
      }
      case fuchsia_hardware_clockimpl::InitCall::Tag::kRateHz: {
        fdf::WireUnownedResult result =
            clock_impl.buffer(arena)->SetRate(clock_id.value(), call->rate_hz().value());
        if (!result.ok()) {
          fdf::error("Failed to send SetRate request for clock {}: {}", clock_id.value(),
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to set rate for clock {}: {}", clock_id.value(),
                     zx_status_get_string(result->error_value()));
          return result->error_value();
        }

        GetOrCreateRateBuffer(clock_id.value())
            ->AddEntry(GetDataForRate(call->rate_hz().value()), GetCurrentMsec());
        break;
      }
      case fuchsia_hardware_clockimpl::InitCall::Tag::kInputIdx: {
        fdf::WireUnownedResult result =
            clock_impl.buffer(arena)->SetInput(clock_id.value(), call->input_idx().value());
        if (!result.ok()) {
          fdf::error("Failed to send SetInput request for clock {}: {}", clock_id.value(),
                     result.status_string());
          return result.status();
        }
        if (result->is_error()) {
          fdf::error("Failed to set input for clock {}: {}", clock_id.value(),
                     zx_status_get_string(result->error_value()));
          return result->error_value();
        }
        break;
      }
      case fuchsia_hardware_clockimpl::InitCall::Tag::kDelay:
        zx::nanosleep(zx::deadline_after(zx::duration(call->delay().value())));
        break;
      default:
        fdf::warn("Unhandled init call");
        break;
    }
  }

  return ZX_OK;
}

FUCHSIA_DRIVER_EXPORT2(ClockDriver);
