// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/clock/bin/clock/clock_debug.h"

#include <lib/component/incoming/cpp/service_member_watcher.h>

#include <algorithm>
#include <print>
#include <unordered_set>

namespace clock_debug {

std::vector<Clock> ListClocks() {
  component::SyncServiceMemberWatcher<fuchsia_hardware_clock::DebugService::Device> watcher;
  std::vector<Clock> clocks;
  std::unordered_set<uint32_t> ids;
  while (true) {
    zx::result<fidl::ClientEnd<fuchsia_hardware_clock::Clock>> result =
        watcher.GetNextInstance(true);
    if (result.is_error()) {
      if (result.error_value() != ZX_ERR_STOP) {
        std::println("Failed to list all clocks: {}", zx_status_get_string(result.error_value()));
      }
      break;
    }

    fidl::Result props = fidl::Call(*result)->GetProperties();
    if (props.is_error()) {
      continue;
    }

    // We only want one entry per clock id since we can have multiple nodes for each one. Doesn't
    // matter which node we go through.
    auto [_, inserted] = ids.insert(props->id());
    if (inserted) {
      clocks.emplace_back(props->id(), props->name(), std::move(*result));
    }
  }

  std::ranges::sort(clocks, {}, &Clock::id);

  return clocks;
}

Clock GetClock(uint32_t id) {
  component::SyncServiceMemberWatcher<fuchsia_hardware_clock::DebugService::Device> watcher;
  while (true) {
    zx::result<fidl::ClientEnd<fuchsia_hardware_clock::Clock>> result =
        watcher.GetNextInstance(true);
    if (result.is_error()) {
      break;
    }

    fidl::Result props = fidl::Call(*result)->GetProperties();
    if (props.is_error()) {
      continue;
    }

    if (props->id() == id) {
      return Clock{
          .id = props->id(),
          .name = props->name(),
          .clock_client = std::move(*result),
      };
    }
  }

  return {};
}

void PrintClocks(const std::vector<Clock>& clocks, bool verbose) {
  if (verbose) {
    for (auto& clock : clocks) {
      ShowClock(clock);
      auto id_length = std::format("{}", clock.id).length();

      // 17 is 15 (length of left column in ShowClock) + 2 (size of "| ")
      size_t separator_width = 17 + std::max(clock.name.length(), id_length);
      std::println("{0:-<{1}}", "", separator_width);
    }

    return;
  }

  size_t id_width = std::string("Clock ID").length();
  size_t name_width = std::string("Name").length();
  for (const auto& [id, name, _] : clocks) {
    auto id_length = std::format("{}", id).length();
    id_width = std::max(id_length, id_width);
    name_width = std::max(name.length(), name_width);
  }

  std::string header = std::format("{:<{}} | {:<{}}", "Clock ID", id_width, "Name", name_width);
  std::string divider(header.length(), '-');

  std::println("{}", header);
  std::println("{}", divider);

  for (const auto& [id, name, _] : clocks) {
    std::println("{:<{}} | {:<{}}", id, id_width, name, name_width);
  }
}

void ShowClock(const Clock& clock) {
  auto& client = clock.clock_client;
  std::optional<bool> enabled;
  auto is_enabled_result = fidl::Call(client)->IsEnabled();
  if (is_enabled_result.is_ok()) {
    enabled = is_enabled_result.value().enabled();
  }

  std::optional<uint32_t> rate;
  auto get_rate_result = fidl::Call(client)->GetRate();
  if (get_rate_result.is_ok()) {
    rate = get_rate_result.value().hz();
  }

  std::optional<uint32_t> input;
  auto get_input_result = fidl::Call(client)->GetInput();
  if (get_input_result.is_ok()) {
    input = get_input_result.value().index();
  }

  std::optional<uint32_t> num_inputs;
  auto num_inputs_result = fidl::Call(client)->GetNumInputs();
  if (num_inputs_result.is_ok()) {
    num_inputs = num_inputs_result.value().n();
  }

  std::println("{:<15}| {}", "Clock ID", clock.id);
  std::println("{:<15}| {}", "Name", clock.name);

  if (enabled) {
    std::println("{:<15}| {}", "Enabled", enabled.value());
  }

  if (rate) {
    std::println("{:<15}| {}hz", "Rate", rate.value());
  }

  if (input) {
    std::println("{:<15}| {}", "Current Input", input.value());
  }

  if (num_inputs) {
    std::println("{:<15}| {}", "Total Inputs", num_inputs.value());
  }
}

void QueryRate(const Clock& clock, uint64_t rate) {
  auto& client = clock.clock_client;

  auto query_result = fidl::Call(client)->QuerySupportedRate({rate});

  if (query_result.is_ok()) {
    std::println("{:<15}| {}hz", "Output Rate", query_result.value().hz_out());
  } else {
    std::println("QueryRate failed: {}", query_result.error_value().FormatDescription());
  }
}

void EnableClock(const Clock& clock) {
  auto& client = clock.clock_client;

  auto result = fidl::Call(client)->Enable();
  if (result.is_ok()) {
    std::println("Enabled successfully.");
  } else {
    std::println("Failed to enable: {}", result.error_value().FormatDescription());
  }
}

void DiableClock(const Clock& clock) {
  auto& client = clock.clock_client;

  auto result = fidl::Call(client)->Disable();
  if (result.is_ok()) {
    std::println("Disabled successfully.");
  } else {
    std::println("Failed to disable: {}", result.error_value().FormatDescription());
  }
}

void ClockSetRate(const Clock& clock, uint64_t rate) {
  auto& client = clock.clock_client;

  auto result = fidl::Call(client)->SetRate({rate});
  if (result.is_ok()) {
    std::println("Set rate successfully.");
  } else {
    std::println("Failed to set rate: {}", result.error_value().FormatDescription());
  }
}

void ClockSetInput(const Clock& clock, uint32_t index) {
  auto& client = clock.clock_client;

  auto result = fidl::Call(client)->SetInput({index});
  if (result.is_ok()) {
    std::println("Set input successfully.");
  } else {
    std::println("Failed to set input: {}", result.error_value().FormatDescription());
  }
}

}  // namespace clock_debug
