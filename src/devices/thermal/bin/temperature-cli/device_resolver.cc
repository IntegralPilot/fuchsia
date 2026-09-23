// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "device_resolver.h"

#include <fidl/fuchsia.hardware.adc/cpp/wire.h>
#include <fidl/fuchsia.hardware.temperature/cpp/wire.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <stdio.h>

#include <algorithm>
#include <array>
#include <cctype>
#include <charconv>
#include <filesystem>
#include <optional>
#include <string>
#include <vector>

namespace FidlTemperature = fuchsia_hardware_temperature;

namespace {

// Device Paths
constexpr char kTemperatureServiceDir[] = "/svc/fuchsia.hardware.temperature.Service";
constexpr char kTemperatureDevfsDir[] = "/dev/class/temperature";
constexpr char kAdcServiceDir[] = "/svc/fuchsia.hardware.adc.Service";
constexpr char kAdcDevfsDir[] = "/dev/class/adc";
constexpr char kTrippointServiceDir[] = "/svc/fuchsia.hardware.trippoint.Service";
constexpr char kTrippointDebugServiceDir[] = "/svc/fuchsia.hardware.trippoint.DebugService";

// Service Members
constexpr char kServiceMemberDevice[] = "device";
constexpr char kServiceMemberTripPoint[] = "trippoint";
constexpr char kServiceMemberDebug[] = "debug";

// Represents a raw device candidate found during filesystem scanning (discovery phase).
// This is transient internal helper state and is mapped to the public ResolvedDevice
// once a command and resolution path are selected.
struct DiscoveredDevice {
  std::string name;
  std::string path;
  bool is_service = false;
};

bool IsTrippointCommand(std::string_view arg) {
  return arg == kCmdTripPoint || arg == kCmdWait || arg == kCmdTrigger;
}

std::string_view GetMemberForService(std::string_view service_dir, std::string_view command) {
  if (service_dir == kTemperatureServiceDir) {
    return IsTrippointCommand(command) ? kServiceMemberTripPoint : kServiceMemberDevice;
  }
  if (service_dir == kAdcServiceDir) {
    return kServiceMemberDevice;
  }
  if (service_dir == kTrippointServiceDir) {
    return kServiceMemberTripPoint;
  }
  if (service_dir == kTrippointDebugServiceDir) {
    return kServiceMemberDebug;
  }
  return "";
}

bool HasServiceMember(std::string_view path) {
  auto check = [](std::string_view path, std::string_view member) {
    return path.size() >= member.size() + 1 && path[path.size() - member.size() - 1] == '/' &&
           path.ends_with(member);
  };
  return check(path, kServiceMemberDevice) || check(path, kServiceMemberTripPoint) ||
         check(path, kServiceMemberDebug);
}

std::string GetPathForDevice(const DiscoveredDevice& dev, std::string_view command) {
  if (dev.is_service) {
    for (auto service_dir : {
             kTemperatureServiceDir,
             kAdcServiceDir,
             kTrippointServiceDir,
             kTrippointDebugServiceDir,
         }) {
      if (dev.path.starts_with(service_dir)) {
        std::string_view member = GetMemberForService(service_dir, command);
        if (!member.empty()) {
          return (std::filesystem::path(dev.path) / member).string();
        }
      }
    }
  }
  return dev.path;
}

std::optional<std::string> TryGetSensorName(const std::string& device_member_path) {
  auto client_res = ConnectToDevice<FidlTemperature::Device>(device_member_path);
  if (client_res.is_error()) {
    return std::nullopt;
  }
  auto client = std::move(client_res.value());
  auto response = client->GetSensorName();
  if (response.ok()) {
    return std::string(response->name.data(), response->name.size());
  }
  return std::nullopt;
}

zx::result<size_t> PromptUserSelection(const std::vector<std::string>& display_items,
                                       std::string_view device_type_name) {
  printf("Multiple %.*s devices found:\n", static_cast<int>(device_type_name.size()),
         device_type_name.data());
  for (size_t i = 0; i < display_items.size(); ++i) {
    printf("  %zu. %s\n", i + 1, display_items[i].c_str());
  }
  printf("Select a device (1-%zu): ", display_items.size());

  char input_buf[128];
  if (fgets(input_buf, sizeof(input_buf), stdin) == nullptr) {
    printf("Error reading input.\n");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  std::string_view input_sv(input_buf);
  auto first = input_sv.find_first_not_of(" \t\r\n");
  if (first == std::string_view::npos) {
    printf("Invalid selection.\n");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  auto last = input_sv.find_last_not_of(" \t\r\n");
  input_sv = input_sv.substr(first, last - first + 1);

  uint64_t selection;
  auto [p, ec] = std::from_chars(input_sv.data(), input_sv.data() + input_sv.size(), selection);
  if (ec != std::errc() || p != input_sv.data() + input_sv.size() || selection < 1 ||
      selection > display_items.size()) {
    printf("Invalid selection.\n");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(selection - 1);
}

std::vector<std::filesystem::directory_entry> GetEntriesInDirectory(std::string_view dir_path) {
  std::vector<std::filesystem::directory_entry> entries;
  std::error_code ec;
  std::filesystem::path path(dir_path);
  if (std::filesystem::exists(path, ec) && !ec) {
    for (auto it = std::filesystem::directory_iterator(path, ec);
         it != std::filesystem::directory_iterator() && !ec; it.increment(ec)) {
      entries.push_back(*it);
    }
    if (ec) {
      printf("Warning: directory iteration failed for '%.*s': %s\n",
             static_cast<int>(dir_path.size()), dir_path.data(), ec.message().c_str());
    }
  }
  return entries;
}

std::vector<DiscoveredDevice> ScanServiceDir(std::string_view service_dir,
                                             std::string_view member_to_check,
                                             std::string_view name_prefix = "",
                                             std::string_view name_member_to_connect = "") {
  std::vector<DiscoveredDevice> devices;
  for (const auto& entry : GetEntriesInDirectory(service_dir)) {
    std::filesystem::path path = entry.path();
    std::filesystem::path member_path = path / member_to_check;
    std::error_code ec;
    if (std::filesystem::exists(member_path, ec) && !ec) {
      std::string name;
      if (name_prefix.empty()) {
        name = path.filename().string();
      } else {
        name = std::string(name_prefix) + path.filename().string();
      }
      if (!name_member_to_connect.empty()) {
        std::filesystem::path name_member_path = path / name_member_to_connect;
        auto name_opt = TryGetSensorName(name_member_path.string());
        if (name_opt) {
          if (name_prefix.empty()) {
            name = *name_opt;
          } else {
            name = std::string(name_prefix) + *name_opt;
          }
        } else if (member_to_check == kServiceMemberDevice) {
          // Skip if we can't connect to temperature device member (matches original behavior)
          continue;
        }
      }
      devices.push_back({name, path.string(), /*is_service=*/true});
    }
  }
  return devices;
}

std::vector<DiscoveredDevice> ScanDevfsDir(std::string_view devfs_dir) {
  std::vector<DiscoveredDevice> devices;
  for (const auto& entry : GetEntriesInDirectory(devfs_dir)) {
    devices.push_back(
        {entry.path().filename().string(), entry.path().string(), /*is_service=*/false});
  }
  return devices;
}

std::vector<DiscoveredDevice> GetTemperatureDevices() {
  auto devices =
      ScanServiceDir(kTemperatureServiceDir, kServiceMemberDevice, "", kServiceMemberDevice);
  if (devices.empty()) {
    return ScanDevfsDir(kTemperatureDevfsDir);
  }
  return devices;
}

std::vector<DiscoveredDevice> GetTrippointDevices() {
  auto devices =
      ScanServiceDir(kTemperatureServiceDir, kServiceMemberTripPoint, "", kServiceMemberDevice);
  auto standalone = ScanServiceDir(kTrippointServiceDir, kServiceMemberTripPoint, "trippoint-");
  devices.insert(devices.end(), standalone.begin(), standalone.end());
  return devices;
}

std::vector<DiscoveredDevice> GetTrippointDebugDevices() {
  return ScanServiceDir(kTrippointDebugServiceDir, kServiceMemberDebug, "debug-");
}

std::vector<DiscoveredDevice> GetAdcDevices() {
  auto devices = ScanServiceDir(kAdcServiceDir, kServiceMemberDevice);
  if (devices.empty()) {
    return ScanDevfsDir(kAdcDevfsDir);
  }
  return devices;
}

zx::result<ResolvedDevice> ResolveDeviceFromList(const std::vector<DiscoveredDevice>& devices,
                                                 std::string_view provided_path_or_name,
                                                 std::string_view command, DeviceType device_type,
                                                 bool suppress_errors = false) {
  std::string_view device_type_name = GetDeviceTypeName(device_type, command);
  if (!provided_path_or_name.empty()) {
    for (const auto& dev : devices) {
      std::string instance_name = std::filesystem::path(dev.path).filename().string();
      if (dev.name == provided_path_or_name || instance_name == provided_path_or_name) {
        return zx::ok(ResolvedDevice{
            .path = GetPathForDevice(dev, command),
            .type = device_type,
            .friendly_name = dev.name,
            .base_path = dev.path,
        });
      }
    }
    if (!suppress_errors) {
      printf("Failed to resolve %.*s device name '%.*s'\n",
             static_cast<int>(device_type_name.size()), device_type_name.data(),
             static_cast<int>(provided_path_or_name.size()), provided_path_or_name.data());
    }
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  if (devices.empty()) {
    if (!suppress_errors) {
      printf("No %.*s devices found.\n", static_cast<int>(device_type_name.size()),
             device_type_name.data());
    }
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  if (devices.size() == 1) {
    std::string path = GetPathForDevice(devices[0], command);
    printf("Using %.*s device: %s (%s)\n", static_cast<int>(device_type_name.size()),
           device_type_name.data(), devices[0].name.c_str(), path.c_str());
    return zx::ok(ResolvedDevice{
        .path = path,
        .type = device_type,
        .friendly_name = devices[0].name,
        .base_path = devices[0].path,
    });
  }

  std::vector<std::string> display_items;
  display_items.reserve(devices.size());
  for (const auto& dev : devices) {
    display_items.push_back(dev.name + " (" + dev.path + ")");
  }

  auto selection_res = PromptUserSelection(display_items, device_type_name);
  if (selection_res.is_error()) {
    return selection_res.take_error();
  }
  size_t selection = selection_res.value();

  std::string path = GetPathForDevice(devices[selection], command);
  printf("Using %.*s device: %s (%s)\n", static_cast<int>(device_type_name.size()),
         device_type_name.data(), devices[selection].name.c_str(), path.c_str());
  return zx::ok(ResolvedDevice{
      .path = path,
      .type = device_type,
      .friendly_name = devices[selection].name,
      .base_path = devices[selection].path,
  });
}

zx::result<ResolvedDevice> ResolveTemperatureDevice(std::string_view provided_path_or_name,
                                                    std::string_view command,
                                                    bool suppress_errors = false) {
  return ResolveDeviceFromList(GetTemperatureDevices(), provided_path_or_name, command,
                               DeviceType::kTemperature, suppress_errors);
}

zx::result<ResolvedDevice> ResolveTrippointDevice(std::string_view provided_path_or_name,
                                                  std::string_view command) {
  // The 'trigger' command simulates a trippoint event for testing, which requires the
  // trippoint debug service. Other commands (e.g., 'trippoint', 'wait') interact with the
  // production trippoint service.
  bool is_debug = command == kCmdTrigger;
  auto devices = is_debug ? GetTrippointDebugDevices() : GetTrippointDevices();
  return ResolveDeviceFromList(devices, provided_path_or_name, command, DeviceType::kTrippoint);
}

DeviceType DeduceDeviceTypeFromPathAndCommand(std::string_view path, std::string_view command) {
  if (path.starts_with(kAdcServiceDir) || path.starts_with(kAdcDevfsDir)) {
    return DeviceType::kAdc;
  }
  if (path.starts_with(kTrippointServiceDir) || path.starts_with(kTrippointDebugServiceDir)) {
    return DeviceType::kTrippoint;
  }
  if (path.starts_with(kTemperatureServiceDir)) {
    if (IsTrippointCommand(command)) {
      return DeviceType::kTrippoint;
    }
    return DeviceType::kTemperature;
  }
  if (path.starts_with(kTemperatureDevfsDir)) {
    return DeviceType::kTemperature;
  }

  // Fallback for arbitrary/unknown paths
  if (IsTrippointCommand(command)) {
    return DeviceType::kTrippoint;
  }
  if (command == kCmdResolution || command == kCmdReadNorm) {
    return DeviceType::kAdc;
  }
  if (command == kCmdRead) {
    if (path.find("adc") != std::string::npos) {
      return DeviceType::kAdc;
    }
    return DeviceType::kTemperature;
  }
  return DeviceType::kTemperature;
}

zx::result<ResolvedDevice> ResolveAdcDevice(std::string_view provided_path_or_name,
                                            std::string_view command,
                                            bool suppress_errors = false) {
  return ResolveDeviceFromList(GetAdcDevices(), provided_path_or_name, command, DeviceType::kAdc,
                               suppress_errors);
}

}  // namespace

std::string_view ToString(DeviceType type) {
  switch (type) {
    case DeviceType::kTemperature:
      return "temperature";
    case DeviceType::kAdc:
      return "ADC";
    case DeviceType::kTrippoint:
      return "trippoint";
  }
}

std::string_view GetDeviceTypeName(DeviceType type, std::string_view command) {
  if (type == DeviceType::kTrippoint && command == kCmdTrigger) {
    return "trippoint debug";
  }
  return ToString(type);
}

bool IsInteger(std::string_view s) {
  return !s.empty() &&
         std::all_of(s.begin(), s.end(), [](unsigned char c) { return std::isdigit(c); });
}

bool IsKnownCommand(std::string_view arg) {
  static constexpr std::array kKnownCommands = {
      kCmdList, kCmdResolution, kCmdRead,    kCmdReadNorm, kCmdTripPoint,
      kCmdWait, kCmdName,       kCmdTrigger, kCmdReadAll,  kCmdHelp,
  };
  return std::find(kKnownCommands.begin(), kKnownCommands.end(), arg) != kKnownCommands.end();
}

void do_list() {
  auto devices = GetTemperatureDevices();
  if (devices.empty()) {
    printf("No temperature devices found.\n");
    return;
  }
  printf("Found %zu temperature devices:\n", devices.size());
  for (const auto& dev : devices) {
    printf("  %-20s (%s)\n", dev.name.c_str(), dev.path.c_str());
  }
}

zx::result<ResolvedDevice> ResolveDevice(std::string_view provided_path_or_name,
                                         std::string_view command) {
  std::string path(provided_path_or_name);
  if (!path.empty() && path.starts_with('/')) {
    DeviceType dev_type = DeduceDeviceTypeFromPathAndCommand(path, command);
    // It's a path. Check if it needs a service member.
    for (auto service_dir : {
             kTemperatureServiceDir,
             kAdcServiceDir,
             kTrippointServiceDir,
             kTrippointDebugServiceDir,
         }) {
      if (path.starts_with(service_dir)) {
        if (!HasServiceMember(path)) {
          std::string_view member = GetMemberForService(service_dir, command);
          if (!member.empty()) {
            if (!path.ends_with('/')) {
              path += '/';
            }
            path += member;
          }
        }
        break;
      }
    }
    return zx::ok(ResolvedDevice{
        .path = path,
        .type = dev_type,
        .friendly_name = "",
        .base_path = "",
    });
  }

  if (IsTrippointCommand(command)) {
    return ResolveTrippointDevice(path, command);
  }

  if (command == kCmdResolution || command == kCmdReadNorm) {
    return ResolveAdcDevice(path, command);
  }

  if (command == kCmdRead) {
    // Try Temperature first (silently to avoid noisy fallback prints)
    auto temp_res = ResolveTemperatureDevice(path, command, /*suppress_errors=*/true);
    if (temp_res.is_ok()) {
      return temp_res;
    }
    // Fallback to ADC
    auto adc_res = ResolveAdcDevice(path, command, /*suppress_errors=*/true);
    if (adc_res.is_ok()) {
      return adc_res;
    }
    if (!path.empty()) {
      printf("Failed to resolve device name '%.*s' as a temperature or ADC device\n",
             static_cast<int>(path.size()), path.data());
    } else {
      printf("No temperature or ADC devices found.\n");
    }
    return temp_res.take_error();
  }

  // Fallback for any other command (e.g., name or unknown commands)
  return ResolveTemperatureDevice(path, command);
}

std::vector<DeviceInfo> GetTemperatureDevicesForReading() {
  auto devices = GetTemperatureDevices();
  std::vector<DeviceInfo> result;
  result.reserve(devices.size());
  for (const auto& dev : devices) {
    result.push_back({
        .name = dev.name,
        .path = GetPathForDevice(dev, kCmdRead),
    });
  }
  return result;
}
