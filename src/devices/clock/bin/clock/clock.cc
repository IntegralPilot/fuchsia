// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.rtc/cpp/fidl.h>
#include <getopt.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <libgen.h>
#include <stdio.h>
#include <zircon/status.h>

#include "src/lib/files/directory.h"

int usage(const char *cmd) {
  fprintf(stderr,
          "Interact with the real-time or monotonic clocks:\n"
          "   %s                              Print the time\n"
          "   %s --help                       Print this message\n"
          "   %s --set YYYY-mm-ddThh:mm:ss    Set the time\n"
          "   %s --monotonic                  Print nanoseconds since boot\n"
          "   optionally specify an RTC device with --dev PATH_TO_DEVICE_NODE\n",
          cmd, cmd, cmd, cmd);
  return -1;
}

std::optional<std::string> guess_dev() {
  const static std::string kDeviceDir = "/dev/class/rtc";
  std::vector<std::string> files;
  if (!files::ReadDirContents(kDeviceDir, &files)) {
    return std::nullopt;
  }

  for (const auto &file : files) {
    if (file == ".") {
      continue;
    }
    return kDeviceDir + '/' + file;
  }

  return std::nullopt;
}

int print_rtc(const fidl::SyncClient<fuchsia_hardware_rtc::Device> &client) {
  auto result = client->Get();
  if (result.is_error()) {
    return -1;
  }
  fuchsia_hardware_rtc::Time rtc = result->rtc();
  printf("%04d-%02d-%02dT%02d:%02d:%02d\n", rtc.year(), rtc.month(), rtc.day(), rtc.hours(),
         rtc.minutes(), rtc.seconds());
  return 0;
}

int set_rtc(const fidl::SyncClient<fuchsia_hardware_rtc::Device> &client, const std::string &time) {
  uint16_t year;
  uint8_t month, day, hours, minutes, seconds;
  int n = sscanf(time.c_str(), "%04hu-%02hhu-%02hhuT%02hhu:%02hhu:%02hhu", &year, &month, &day,
                 &hours, &minutes, &seconds);
  if (n != 6) {
    printf("Bad time format.\n");
    return -1;
  }

  auto result = client->Set2({{{{
      .seconds = seconds,
      .minutes = minutes,
      .hours = hours,
      .day = day,
      .month = month,
      .year = year,
  }}}});

  if (result.is_ok()) {
    return ZX_OK;
  }

  if (result.error_value().is_framework_error()) {
    fprintf(stderr, "Set RTC failed with framework error: %s\n",
            result.error_value().framework_error().status_string());
    return result.error_value().framework_error().status();
  }

  if (result.error_value().is_domain_error()) {
    fprintf(stderr, "Set RTC failed with domain error: %s\n",
            zx_status_get_string(result.error_value().domain_error()));
    return result.error_value().domain_error();
  }

  return ZX_ERR_INTERNAL;
}

void print_monotonic() { printf("%lu\n", zx_clock_get_monotonic()); }

int main(int argc, char **argv) {
  int err = 0;
  const char *cmd = basename(argv[0]);
  std::optional<std::string> path;
  std::optional<std::string> set;
  static const struct option opts[] = {
      {"set", required_argument, NULL, 's'},
      {"dev", required_argument, NULL, 'd'},
      {"monotonic", no_argument, NULL, 'm'},
      {"help", no_argument, NULL, 'h'},
      {0, 0, 0, 0},
  };
  for (int opt; (opt = getopt_long(argc, argv, "", opts, NULL)) != -1;) {
    switch (opt) {
      case 's':
        set = optarg;
        break;
      case 'd':
        path = optarg;
        break;
      case 'm':
        print_monotonic();
        return 0;
      case 'h':
        usage(cmd);
        return 0;
      default:
        return usage(cmd);
    }
  }

  argv += optind;
  argc -= optind;

  if (argc != 0) {
    return usage(cmd);
  }

  if (!path) {
    path = guess_dev();
    if (!path) {
      fprintf(stderr, "No RTC found.\n");
      return usage(cmd);
    }
  }

  auto client_end = component::Connect<fuchsia_hardware_rtc::Device>(path.value());
  if (client_end.is_error()) {
    fprintf(stderr, "Can not open RTC device\n");
    return usage(cmd);
  }

  fidl::SyncClient client{std::move(*client_end)};

  if (set) {
    err = set_rtc(client, set.value());
    if (err) {
      printf("Set RTC failed.\n");
      usage(cmd);
    }
    return err;
  }

  err = print_rtc(client);
  if (err) {
    usage(cmd);
  }
  return err;
}
