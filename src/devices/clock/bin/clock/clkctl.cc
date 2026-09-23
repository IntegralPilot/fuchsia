// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <fidl/fuchsia.hardware.clock.measure/cpp/wire.h>
#include <fidl/fuchsia.hardware.clock/cpp/wire.h>
#include <getopt.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <libgen.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

#include <print>

#include "src/devices/clock/bin/clock/clock_debug.h"
#include "src/lib/files/directory.h"

using FrequencyInfo = fuchsia_hardware_clock_measure::wire::FrequencyInfo;

using action_t = enum action {
  UNKNOWN,
  MEASURE,
};

int usage(const char* cmd) {
  fprintf(stderr,
          "\nInteract with clocks on the SOC:\n"
          "   %s list                       List all clocks\n"
          "   %s list -v                    List all clocks verbose\n"
          "   %s show <id>                  Shows information for the clock\n"
          "   %s enable <id>                Enable the clock\n"
          "   %s disable <id>               Disable the clock\n"
          "   %s query-rate <id> <rate>     Query the rate\n"
          "   %s set-rate <id> <rate>       Sets the clock rate\n"
          "   %s set-input <id> <idx>       Sets the clock's input\n"
          "   %s measure                    Measures all clock values\n"
          "   %s measure -idx <idx>         Measure CLK idx\n"
          "   %s help                       Print this message\n",
          cmd, cmd, cmd, cmd, cmd, cmd, cmd, cmd, cmd, cmd, cmd);
  return -1;
}

// Returns "true" if the argument matches the prefix.
// In this case, moves the argument past the prefix.
bool prefix_match(const char** arg, const char* prefix) {
  if (!strncmp(*arg, prefix, strlen(prefix))) {
    *arg += strlen(prefix);
    return true;
  }
  return false;
}

// Gets the value of a particular field passed through
// command line.
const char* getValue(int argc, char** argv, const char* field) {
  int i = 1;
  while (i < argc - 1 && strcmp(argv[i], field) != 0) {
    ++i;
  }
  if (i >= argc - 1) {
    printf("NULL\n");
    return nullptr;
  }
  return argv[i + 1];
}

std::optional<std::string> guess_dev() {
  const static std::string kDeviceDir = "/dev/class/clock-impl";
  std::vector<std::string> files;
  if (!files::ReadDirContents(kDeviceDir, &files)) {
    return std::nullopt;
  }

  for (const auto& file : files) {
    if (file == ".") {
      continue;
    }
    return kDeviceDir + '/' + file;
  }

  return std::nullopt;
}

ssize_t measure_clk_util(
    const fidl::WireSyncClient<fuchsia_hardware_clock_measure::Measurer>& client, uint32_t idx) {
  auto result = client->Measure(idx);

  if (!result.ok()) {
    fprintf(stderr, "ERROR: failed to measure clock: %s\n", zx_status_get_string(result.status()));
    return -1;
  }

  if (result->is_error()) {
    fprintf(stderr, "ERROR: failed to measure clock: %s\n",
            zx_status_get_string(result->error_value()));
    return -1;
  }

  std::string name(std::begin(result.value()->info.name), std::end(result.value()->info.name));

  printf("[%4d][%4ld MHz] %s\n", idx, result.value()->info.frequency, name.c_str());
  return 0;
}

ssize_t measure_clk(const fidl::WireSyncClient<fuchsia_hardware_clock_measure::Measurer>& client,
                    uint32_t idx, bool clk) {
  auto clk_count_result = client->GetCount();
  if (!clk_count_result.ok()) {
    fprintf(stderr, "Failed to get count: %s\n", zx_status_get_string(clk_count_result.status()));
    return -1;
  }

  if (clk) {
    if (idx > clk_count_result->count) {
      fprintf(stderr, "ERROR: Invalid clock index.\n");
      return -1;
    }
    return measure_clk_util(client, idx);
  }
  for (uint32_t i = 0; i < clk_count_result->count; i++) {
    ssize_t rc = measure_clk_util(client, i);
    if (rc < 0) {
      return rc;
    }
  }

  return 0;
}

std::optional<uint32_t> parse_uint32(const char* str) {
  if (str == nullptr) {
    return std::nullopt;
  }
  errno = 0;
  char* end;
  uint64_t val = strtoul(str, &end, 10);

  if (errno == ERANGE || *end != '\0' || str == end || val > UINT32_MAX) {
    return std::nullopt;
  }
  return static_cast<uint32_t>(val);
}

std::optional<uint64_t> parse_uint64(const char* str) {
  if (str == nullptr) {
    return std::nullopt;
  }
  errno = 0;
  char* end;
  uint64_t val = strtoul(str, &end, 10);

  if (errno == ERANGE || *end != '\0' || str == end) {
    return std::nullopt;
  }
  return val;
}

int main(int argc, char** argv) {
  const char* cmd = basename(argv[0]);
  const char* index = nullptr;
  action_t subcmd = UNKNOWN;
  bool clk = false;
  uint32_t idx = 0;

  // If no arguments passed, bail out after dumping
  // usage information.
  if (argc == 1) {
    return usage(cmd);
  }

  // Parse all args.
  while (argc > 1) {
    const char* arg = argv[1];
    if (strcmp(arg, "list") == 0) {
      bool verbose = false;
      if (argc > 2) {
        const char* arg2 = argv[2];
        if (prefix_match(&arg2, "-v") || prefix_match(&arg2, "--verbose")) {
          verbose = true;
        }
      }

      std::vector<clock_debug::Clock> clocks = clock_debug::ListClocks();
      clock_debug::PrintClocks(clocks, verbose);
      return 0;
    }

    if (strcmp(arg, "show") == 0) {
      auto id = parse_uint32(argv[2]);
      if (!id.has_value()) {
        std::println(stderr, "ERROR: Invalid clock ID: {}", argv[2]);
        return -1;
      }
      clock_debug::Clock clock = clock_debug::GetClock(*id);
      clock_debug::ShowClock(clock);
      return 0;
    }

    if (strcmp(arg, "enable") == 0) {
      auto id = parse_uint32(argv[2]);
      if (!id.has_value()) {
        std::println(stderr, "ERROR: Invalid clock ID: {}", argv[2]);
        return -1;
      }
      clock_debug::Clock clock = clock_debug::GetClock(*id);
      clock_debug::EnableClock(clock);
      return 0;
    }

    if (strcmp(arg, "disable") == 0) {
      auto id = parse_uint32(argv[2]);
      if (!id.has_value()) {
        std::println(stderr, "ERROR: Invalid clock ID: {}", argv[2]);
        return -1;
      }
      clock_debug::Clock clock = clock_debug::GetClock(*id);
      clock_debug::DiableClock(clock);
      return 0;
    }

    if (strcmp(arg, "query-rate") == 0) {
      auto id = parse_uint32(argv[2]);
      if (!id.has_value()) {
        std::println(stderr, "ERROR: Invalid clock ID: {}", argv[2]);
        return -1;
      }
      clock_debug::Clock clock = clock_debug::GetClock(*id);
      auto rate = parse_uint64(argv[3]);
      if (!rate.has_value()) {
        std::println(stderr, "ERROR: Invalid rate: {}", argv[3]);
        return -1;
      }
      clock_debug::QueryRate(clock, *rate);
      return 0;
    }

    if (strcmp(arg, "set-rate") == 0) {
      auto id = parse_uint32(argv[2]);
      if (!id.has_value()) {
        std::println(stderr, "ERROR: Invalid clock ID: {}", argv[2]);
        return -1;
      }
      clock_debug::Clock clock = clock_debug::GetClock(*id);
      auto rate = parse_uint64(argv[3]);
      if (!rate.has_value()) {
        std::println(stderr, "ERROR: Invalid rate: {}", argv[3]);
        return -1;
      }
      clock_debug::ClockSetRate(clock, *rate);
      return 0;
    }

    if (strcmp(arg, "set-input") == 0) {
      auto id = parse_uint32(argv[2]);
      if (!id.has_value()) {
        std::println(stderr, "ERROR: Invalid clock ID: {}", argv[2]);
        return -1;
      }
      clock_debug::Clock clock = clock_debug::GetClock(*id);
      uint32_t index = atoi(argv[3]);
      clock_debug::ClockSetInput(clock, index);
      return 0;
    }

    if (prefix_match(&arg, "measure")) {
      subcmd = MEASURE;
    }

    if (prefix_match(&arg, "-idx")) {
      index = getValue(argc, argv, "-idx");
      clk = true;
      if (index) {
        idx = atoi(index);
      } else {
        fprintf(stderr, "Enter Valid CLK IDX.\n");
      }
    }
    if (prefix_match(&arg, "help")) {
      return usage(cmd);
    }
    argc--;
    argv++;
  }

  // Get the device path.
  std::optional path = guess_dev();
  if (!path.has_value()) {
    fprintf(stderr, "No CLK device found.\n");
    return usage(cmd);
  }
  zx::result client_end =
      component::Connect<fuchsia_hardware_clock_measure::Measurer>(path.value());
  if (client_end.is_error()) {
    fprintf(stderr, "Failed to connect to clock_device: %s\n", client_end.status_string());
    return -1;
  }
  fidl::WireSyncClient client(std::move(client_end.value()));

  ssize_t err;
  switch (subcmd) {
    case MEASURE:
      err = measure_clk(client, idx, clk);
      if (err) {
        printf("Measure CLK failed.\n");
      }
      break;
    default:
      return usage(cmd);
  }

  return 0;
}
