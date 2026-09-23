// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_THERMAL_BIN_TEMPERATURE_CLI_DEVICE_RESOLVER_H_
#define SRC_DEVICES_THERMAL_BIN_TEMPERATURE_CLI_DEVICE_RESOLVER_H_

#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/zx/result.h>

#include <string>
#include <string_view>
#include <vector>

struct DeviceInfo {
  std::string name;
  std::string path;
};

std::vector<DeviceInfo> GetTemperatureDevicesForReading();

// Commands
inline constexpr std::string_view kCmdList = "list";
inline constexpr std::string_view kCmdResolution = "resolution";
inline constexpr std::string_view kCmdRead = "read";
inline constexpr std::string_view kCmdReadNorm = "read-norm";
inline constexpr std::string_view kCmdReadAll = "read-all";
inline constexpr std::string_view kCmdTripPoint = "trippoint";
inline constexpr std::string_view kCmdWait = "wait";
inline constexpr std::string_view kCmdName = "name";
inline constexpr std::string_view kCmdTrigger = "trigger";
inline constexpr std::string_view kCmdHelp = "help";

template <typename Protocol>
zx::result<fidl::WireSyncClient<Protocol>> ConnectToDevice(std::string_view path,
                                                           std::string_view device_type_name = "") {
  auto client_end = component::Connect<Protocol>(path);
  if (client_end.is_error()) {
    if (!device_type_name.empty()) {
      printf("Failed to connect to %.*s device at %.*s: %s\n",
             static_cast<int>(device_type_name.size()), device_type_name.data(),
             static_cast<int>(path.size()), path.data(), client_end.status_string());
    }
    return client_end.take_error();
  }
  return zx::ok(fidl::WireSyncClient<Protocol>(std::move(client_end.value())));
}

enum class DeviceType : uint8_t {
  kTemperature = 0,
  kAdc,
  kTrippoint,
};

// Represents the final, fully-resolved device targeting a specific connection path.
// This is the public configuration passed to the execution layers.
struct ResolvedDevice {
  std::string path;
  DeviceType type;
  std::string friendly_name;
  std::string base_path;
};

// Public API
std::string_view ToString(DeviceType type);
std::string_view GetDeviceTypeName(DeviceType type, std::string_view command);
zx::result<ResolvedDevice> ResolveDevice(std::string_view provided_path_or_name,
                                         std::string_view command);
void do_list();
bool IsInteger(std::string_view s);
bool IsKnownCommand(std::string_view arg);

#endif  // SRC_DEVICES_THERMAL_BIN_TEMPERATURE_CLI_DEVICE_RESOLVER_H_
