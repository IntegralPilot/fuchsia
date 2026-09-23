// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>

#include <cmath>
#include <string>
#include <vector>

#include "src/lib/fxl/command_line.h"

// Sets the power state.
// Returns the time in nanoseconds that it took to set the power state.
// Returns a negative number if there was an error.
int SetPowerState(fidl::ClientEnd<fuchsia_gpu_magma::DebugUtils>& client_end, int64_t power_state) {
  auto result = fidl::WireCall(client_end)->SetPowerState(power_state);
  if (!result.ok()) {
    fprintf(stderr, "magma SetPowerState transport failed: %d", result.status());
    return -1;
  }
  if (result->is_error()) {
    fprintf(stderr, "magma SetPowerState failed: %d", result->error_value());
    return -1;
  }
  printf("Setting power state to %ld, took %10ld ns\n", power_state, result->value()->time_in_ns);
  return static_cast<int>(result->value()->time_in_ns);
}

// Convert a comma separated list of numbers into a vector of integers.
// Eg: "1,2,34,5" => [1, 2, 34, 5]
std::vector<int> GetPowerStates(const std::string& s) {
  std::vector<int> states;
  std::string current;
  for (size_t i = 0; i < s.size(); i++) {
    if (s[i] == ',') {
      states.push_back(atoi(current.c_str()));
      current = "";
    } else {
      current.push_back(s[i]);
    }
  }
  states.push_back(atoi(current.c_str()));
  return states;
}

void PrintStats(const std::vector<int>& data, const char* units) {
  int max = 0;
  size_t sum = 0;
  for (int value : data) {
    sum += value;
    max = (max > value) ? max : value;
  }
  double mean = static_cast<double>(sum) / static_cast<double>(data.size());

  double sq_sum = 0.0;
  for (int value : data) {
    sq_sum += std::pow(value - mean, 2);
  }
  double variance = sq_sum / static_cast<double>(data.size() - 1);
  double std_dev = std::sqrt(variance);

  printf("Count:   %15ld\n", data.size());
  printf("Max:     %15d %s\n", max, units);
  printf("Mean:    %15.0f %s\n", mean, units);
  printf("Std dev: %15.0f %s\n", std_dev, units);
}

int main(int argc, char** argv) {
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);

  // This CLI runs in `component explore` so we open our component's out dir.
  zx::result<fidl::ClientEnd<fuchsia_io::Directory>> svc = component::OpenServiceRoot("/out/svc");
  if (svc.is_error()) {
    fprintf(stderr, "Failed to open /out/svc: %s\n", svc.status_string());
    return 1;
  }
  fidl::ClientEnd<fuchsia_io::Directory> svc_dir = std::move(svc).value();

  auto watcher =
      component::SyncServiceMemberWatcher<fuchsia_gpu_magma::PowerService::DebugUtils>(svc_dir);
  zx::result client_end = watcher.GetNextInstance(/*stop_at_idle=*/true);
  if (client_end.is_error()) {
    printf("Failed to open magma device: %s\n", client_end.status_string());
    return -1;
  }

  static const char kCountFlag[] = "count";
  size_t count = 1;
  std::string count_string;
  if (command_line.GetOptionValue(kCountFlag, &count_string)) {
    count = atol(count_string.c_str());
  }

  static const char kPowerStateFlag[] = "power-state";
  std::string power_state_string;
  if (command_line.GetOptionValue(kPowerStateFlag, &power_state_string)) {
    std::vector<int> power_states = GetPowerStates(power_state_string);
    auto timings = std::vector<std::vector<int>>(power_states.size());
    for (size_t i = 0; i < count; i++) {
      for (size_t j = 0; j < power_states.size(); j++) {
        int time = SetPowerState(client_end.value(), power_states[j]);
        if (time < 0) {
          return -1;
        }
        timings[j].push_back(time);
      }
    }

    for (size_t i = 0; i < power_states.size(); i++) {
      printf("Power State: %d\n", power_states[i]);
      PrintStats(timings[i], "ns");
      printf("\n");
    }
  } else {
    fprintf(stderr, "No request\n");
    return -1;
  }

  return 0;
}
