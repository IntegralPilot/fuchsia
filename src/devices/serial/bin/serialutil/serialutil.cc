// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "serialutil.h"

#include <fidl/fuchsia.hardware.serial/cpp/fidl.h>
#include <getopt.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <libgen.h>

#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <optional>
#include <string_view>

#include "lib/zx/result.h"

// Client type for communicating with a Serial device.
using Client = fidl::ClientEnd<fuchsia_hardware_serial::Device>;

// Default devfs path to use.
constexpr std::string_view kSerialDevFsPath{"/dev/class/serial/"};

namespace {

// Stringify the `Class` enum.
std::string_view ToString(::fuchsia_hardware_serial::Class device_class) {
  switch (device_class) {
    case fuchsia_hardware_serial::Class::kGeneric:
      return "Generic";
    case fuchsia_hardware_serial::Class::kBluetoothHci:
      return "Bluetooth HCI";
    case fuchsia_hardware_serial::Class::kConsole:
      return "Console";
    case fuchsia_hardware_serial::Class::kKernelDebug:
      return "Kernel Debug";
    case fuchsia_hardware_serial::Class::kMcu:
      return "MCU";
  }
}

// Loads the default devfs path and retrieves the first entry found.
std::optional<std::string> GetFirstDeviceEntry() {
  std::filesystem::path path{kSerialDevFsPath};
  for (auto const& entry : std::filesystem::directory_iterator{path}) {
    std::cout << "Located instance: " << entry.path() << std::endl;
    return entry.path();
  }

  std::cerr << "Couldn't find " << kSerialDevFsPath << " instance!" << std::endl;
  return std::nullopt;
}

// Connects the to given devfs endpoint.
//
// @param path Devfs file path to open.
// @param connection Optional: If set this connection is used for connecting to the Serial
// device instead of connecting via a file system path.
//
// @return The client connection on success, error status if there is a failure.
zx::result<Client> ConnectToDevfs(
    const std::string& path, fidl::ClientEnd<fuchsia_hardware_serial::DeviceProxy> connection) {
  std::cout << "Connecting to " << path << "...";

  if (!connection.is_valid()) {
    auto result = component::Connect<fuchsia_hardware_serial::DeviceProxy>(path);
    if (result.is_error()) {
      std::cerr << "Can not open serial device: " << result.status_string() << std::endl;
      return result.take_error();
    }
    connection = std::move(*result);
  }

  auto [client_end, server] = fidl::Endpoints<fuchsia_hardware_serial::Device>::Create();
  if (auto result = fidl::WireCall(connection)->GetChannel(std::move(server)); !result.ok()) {
    std::cerr << "GetChannel failed: " << result.status_string() << std::endl;
    return zx::error(result.status());
  }

  std::cout << "connected!" << std::endl;

  return zx::ok(std::move(client_end));
}

// Queries and outputs the serial device information.
zx::result<> LogDeviceInfo(Client& client_end) {
  std::cerr << "Getting device info..." << std::endl;
  auto info = fidl::WireCall(client_end)->GetClass();
  if (!info.ok()) {
    std::cerr << "Couldn't query device class: " << info.status_string() << std::endl;
    return zx::error(info.status());
  }
  std::cout << "Device class: " << ToString(info->device_class) << std::endl;

  return zx::ok();
}

// Performs a single read from the Serial device and outputs the results to stdout.
zx::result<> ReadFromDevice(Client& client_end) {
  std::cout << "Reading data...";
  auto read_result = fidl::WireCall(client_end)->Read();
  if (!read_result.ok()) {
    std::cerr << "Call to Read() failed: " << read_result.status_string() << std::endl;
    return zx::error(read_result.status());
  }

  if (read_result->is_error()) {
    std::cerr << "Read() resulted in faiure: " << zx_status_get_string(read_result->error_value())
              << std::endl;
    return read_result->take_error();
  }

  std::cout << "read complete!" << std::endl;

  // We could do something smarter here, for now just pretend printable data.
  auto& data = read_result.value()->data;
  std::string_view data_view{reinterpret_cast<char*>(data.data()), data.size()};
  std::cout << data_view << std::endl << std::endl;

  return zx::ok();
}

int Usage(const char* name) {
  std::cerr << "Connect to a serial endpoint and read from it." << std::endl
            << std::endl
            << name << " [--help] [--dev endpoint] [--iter count]" << std::endl
            << "Options:" << std::endl
            << "   --dev endpoint Connect to specified serial <endpoint>." << std::endl
            << "                  Default: Connect to first serial device found under " << std::endl
            << "                  " << kSerialDevFsPath << std::endl
            << "   --iter count   Log <count> number of reads." << std::endl
            << "                  Default: 1." << std::endl
            << "   --help         Print this message." << std::endl;
  return -1;
}
}  // namespace

namespace serial {
int SerialUtil::Execute(int argc, char* argv[],
                        fidl::ClientEnd<fuchsia_hardware_serial::DeviceProxy> connection) {
  const char* cmd = basename(argv[0]);
  std::optional<std::string> path;
  std::optional<unsigned> iterations{1};
  static const struct option opts[] = {
      {.name = "dev", .has_arg = required_argument, .flag = nullptr, .val = 'd'},
      {.name = "iter", .has_arg = required_argument, .flag = nullptr, .val = 'i'},
      {.name = "help", .has_arg = no_argument, .flag = nullptr, .val = 'h'},
      {.name = nullptr, .has_arg = 0, .flag = nullptr, .val = 0},
  };

  for (int opt; (opt = getopt_long(argc, argv, "", opts, nullptr)) != -1;) {
    switch (opt) {
      case 'd':
        path = optarg;
        break;
      case 'i':
        iterations = std::strtoul(optarg, nullptr, 10);
        break;
      case 'h':
        Usage(cmd);
        return 0;
      default:
        return Usage(cmd);
    }
  }

  argv += optind;
  argc -= optind;

  if (argc != 0) {
    return Usage(cmd);
  }

  // Get or validate the path when not running tests.
  if (!connection.is_valid()) {
    if (!path) {
      path = GetFirstDeviceEntry();
    } else if (!std::filesystem::exists(*path)) {
      std::cerr << "Invalid path provided: " << *path << std::endl;
      path.reset();
    }
  } else {
    // Just use a fake path when testing.
    if (!path) {
      path = kSerialDevFsPath;
      *path += "fake_serial";
    }
  }

  if (!path) {
    return Usage(argv[0]);
  }

  auto client_end = ConnectToDevfs(*path, std::move(connection));
  if (LogDeviceInfo(*client_end).is_error()) {
    return -1;
  }

  std::cout << "Starting to read from device..." << std::endl;

  for (unsigned i = 0; i < iterations; i++) {
    if (ReadFromDevice(*client_end).is_error()) {
      return -1;
    }
  }

  std::cout << "Exiting..." << std::endl;
  return 0;
}
}  // namespace serial
