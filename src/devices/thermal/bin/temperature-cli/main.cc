// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.adc/cpp/wire.h>
#include <fidl/fuchsia.hardware.temperature/cpp/wire.h>
#include <fidl/fuchsia.hardware.trippoint/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <stdio.h>

#include <charconv>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

#include "device_resolver.h"

constexpr char kUsageMessage[] =
    R"""(Issue one or more commands to a thermal device.

    Usage: temperature-cli [device_path_or_name] <command> [args...] [<command2> [args2...] ...]
           temperature-cli list
           temperature-cli read-all
           temperature-cli --help

    [device_path_or_name] can be:
    - An absolute path (e.g., /dev/class/temperature/000 or /svc/fuchsia.hardware.temperature.Service/default)
    - A friendly name (e.g., soc-thermal)
    - A service instance name/hash (e.g., a2471f28e36fbe951476bce7910aa396)

    If [device_path_or_name] is omitted:
    - If only one device matches the command type, it is automatically used.
    - If multiple devices match, the user is prompted to select one.

    Command Chaining:
        Multiple commands can be chained together sequentially (e.g. `trippoint ... wait`).
        Commands that target the same device protocol will share the same persistent connection,
        allowing oneshot trippoints to trigger and be handled without connection drop resets.

    Commands:
        list             - List all temperature device paths and their friendly names

        (For temperature class devices)
        name             - Get sensor name
        read             - Read temperature in Celsius
        read-all         - List all devices, then read them all

        (For ADC class devices)
        resolution       - Get ADC resolution
        read             - Read ADC sample
        read-norm        - Read normalized ADC sample [0.0-1.0]

        (For trippoint class devices)
        trippoint        - Get/set trippoint. Follow with multiple sets of index:type,configuration.
                           If no trippoint is specified, list all trippoints for this device.
        wait             - Wait for a trippoint to be triggered
        trigger          - Use debug service to trigger a trippoint (requires index arg)

    If no command is specified, "read" is assumed.

    Examples:
        temperature-cli list
        temperature-cli read-all
        temperature-cli LITTLE             (equivalent to 'temperature-cli LITTLE read')
        temperature-cli soc-thermal read
        temperature-cli /dev/class/temperature/000 name read
        temperature-cli /dev/class/adc/000 read
        temperature-cli LITTLE trippoint 0:above,35.5 wait
        temperature-cli /svc/fuchsia.hardware.trippoint.Service/default/trippoint trippoint 0:below,4.2 1:above,cleared
)""";

namespace FidlTemperature = fuchsia_hardware_temperature;
namespace FidlAdc = fuchsia_hardware_adc;
namespace FidlTrippoint = fuchsia_hardware_trippoint;

// Trippoint Configs
constexpr char kTripTypeAbove[] = "above";
constexpr char kTripTypeBelow[] = "below";
constexpr char kTripConfigCleared[] = "cleared";

namespace {

// Not commands, but additional flags that map to kCmdHelp
constexpr std::string_view kShortFlagHelp = "-h";
constexpr std::string_view kLongFlagHelp = "--help";

struct CommandBlock {
  std::string_view command;
  std::vector<std::string_view> extra_args;
};

struct CmdArgs {
  std::string_view device_path_or_name;
  std::vector<CommandBlock> command_blocks;
};

zx::result<CmdArgs> ParseArgs(int argc, char** argv) {
  CmdArgs args;
  if (argc < 2) {
    return zx::ok(args);
  }

  std::string_view argv1(argv[1]);
  if (argv1 == kCmdHelp || argv1 == kShortFlagHelp || argv1 == kLongFlagHelp) {
    args.command_blocks.push_back({.command = kCmdHelp});
    return zx::ok(args);
  }

  if (argv1 == kCmdList) {
    CommandBlock block{.command = kCmdList};
    for (int i = 2; i < argc; ++i) {
      block.extra_args.push_back(argv[i]);
    }
    args.command_blocks.push_back(block);
    return zx::ok(args);
  }

  if (argv1 == kCmdReadAll) {
    CommandBlock block{.command = kCmdReadAll};
    for (int i = 2; i < argc; ++i) {
      block.extra_args.push_back(argv[i]);
    }
    args.command_blocks.push_back(block);
    return zx::ok(args);
  }

  // If the first argument is not a command, treat it as a device path or friendly name.
  // If no subsequent command is specified, default to a "read" command.
  int command_start_idx = 1;
  if (!IsKnownCommand(argv[1])) {
    args.device_path_or_name = argv[1];
    if (argc < 3) {
      args.command_blocks.push_back({.command = kCmdRead});
      return zx::ok(args);
    }
    command_start_idx = 2;
  }

  int i = command_start_idx;
  while (i < argc) {
    std::string_view cmd = argv[i];
    if (!IsKnownCommand(cmd)) {
      printf("Unknown or misplaced command: %.*s\n", static_cast<int>(cmd.size()), cmd.data());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    if (cmd == kCmdList || cmd == kCmdReadAll || cmd == kCmdHelp) {
      printf("Command '%.*s' is global and cannot be chained with device-specific commands.\n",
             static_cast<int>(cmd.size()), cmd.data());
      return zx::error(ZX_ERR_INVALID_ARGS);
    }
    CommandBlock block{.command = cmd};
    i++;
    while (i < argc && !IsKnownCommand(argv[i])) {
      block.extra_args.push_back(argv[i]);
      i++;
    }
    args.command_blocks.push_back(block);
  }

  return zx::ok(args);
}

// Defines the valid device types associated with each CLI command.
struct CommandCompatibility {
  std::string_view command;
  DeviceType type;
};

constexpr CommandCompatibility kCommandCompatibilities[] = {
    {kCmdRead, DeviceType::kTemperature}, {kCmdRead, DeviceType::kAdc},
    {kCmdName, DeviceType::kTemperature}, {kCmdResolution, DeviceType::kAdc},
    {kCmdReadNorm, DeviceType::kAdc},     {kCmdTripPoint, DeviceType::kTrippoint},
    {kCmdWait, DeviceType::kTrippoint},   {kCmdTrigger, DeviceType::kTrippoint},
};

std::string ExpectedTypeForCommand(std::string_view command) {
  std::string expected;
  for (const auto& compat : kCommandCompatibilities) {
    if (compat.command == command) {
      if (!expected.empty()) {
        expected += " or ";
      }
      expected += ToString(compat.type);
    }
  }
  return expected.empty() ? "unknown" : expected;
}

bool IsCompatible(DeviceType type, std::string_view command) {
  for (const auto& compat : kCommandCompatibilities) {
    if (compat.command == command && compat.type == type) {
      return true;
    }
  }
  return false;
}

enum class CommandType {
  kRead,
  kName,
  kResolution,
  kReadNorm,
  kTripPoint,
  kWait,
  kTrigger,
};

std::optional<CommandType> ParseCommand(std::string_view command) {
  if (command == kCmdRead)
    return CommandType::kRead;
  if (command == kCmdName)
    return CommandType::kName;
  if (command == kCmdResolution)
    return CommandType::kResolution;
  if (command == kCmdReadNorm)
    return CommandType::kReadNorm;
  if (command == kCmdTripPoint)
    return CommandType::kTripPoint;
  if (command == kCmdWait)
    return CommandType::kWait;
  if (command == kCmdTrigger)
    return CommandType::kTrigger;
  return std::nullopt;
}

std::string ToString(const FidlTrippoint::wire::TripPointType& type) {
  switch (type) {
    case FidlTrippoint::TripPointType::kOneshotTempAbove:
      return "OneshotTempAbove";
    case FidlTrippoint::TripPointType::kOneshotTempBelow:
      return "OneshotTempBelow";
    default:
      return "Unknown";
  }
}

std::string ToString(const FidlTrippoint::wire::TripPointValue& value) {
  switch (value.Which()) {
    case FidlTrippoint::wire::TripPointValue::Tag::kClearedTripPoint:
      return "ClearedTripPoint";
    case FidlTrippoint::wire::TripPointValue::Tag::kOneshotTempAboveTripPoint:
      return "OneshotTempAboveTripPoint(" +
             std::to_string(value.oneshot_temp_above_trip_point().critical_temperature_celsius) +
             ")";
    case FidlTrippoint::wire::TripPointValue::Tag::kOneshotTempBelowTripPoint:
      return "OneshotTempBelowTripPoint(" +
             std::to_string(value.oneshot_temp_below_trip_point().critical_temperature_celsius) +
             ")";
    default:
      return "Unknown";
  }
}

void print_trippoint(
    const fidl::VectorView<FidlTrippoint::wire::TripPointDescriptor>& descriptors) {
  for (const auto& trippoint : descriptors) {
    printf("{\n");
    printf("   .index = %d,\n", trippoint.index);
    printf("   .type = %s,\n", ToString(trippoint.type).c_str());
    printf("   .configuration = %s,\n", ToString(trippoint.configuration).c_str());
    printf("},\n");
  }
}

// Parses trippoint override arguments formatted as "index:type,configuration"
// (e.g., "0:above,35.5" or "1:below,cleared").
std::vector<FidlTrippoint::wire::TripPointDescriptor> parse_set_trippoints_args(
    const std::vector<std::string_view>& extra_args) {
  std::vector<FidlTrippoint::wire::TripPointDescriptor> descriptors;
  for (std::string_view trippoint : extra_args) {
    auto delim = trippoint.find(':');
    if (delim == std::string_view::npos) {
      return {};
    }
    std::string_view index = trippoint.substr(0, delim);
    trippoint.remove_prefix(delim + 1);
    delim = trippoint.find(',');
    if (delim == std::string_view::npos) {
      return {};
    }
    std::string_view type = trippoint.substr(0, delim);
    trippoint.remove_prefix(delim + 1);
    std::string_view configuration = trippoint;

    uint32_t index_val;
    auto [p1, ec1] = std::from_chars(index.data(), index.data() + index.size(), index_val);
    if (ec1 != std::errc() || p1 != index.data() + index.size()) {
      return {};
    }

    FidlTrippoint::wire::TripPointDescriptor desc;
    if (type == kTripTypeAbove) {
      if (configuration == kTripConfigCleared) {
        desc = {
            .type = FidlTrippoint::wire::TripPointType::kOneshotTempAbove,
            .index = index_val,
            .configuration = FidlTrippoint::wire::TripPointValue::WithClearedTripPoint(
                FidlTrippoint::wire::ClearedTripPoint()),
        };
      } else {
        float config_val;
        auto [p2, ec2] = std::from_chars(configuration.data(),
                                         configuration.data() + configuration.size(), config_val);
        if (ec2 != std::errc() || p2 != configuration.data() + configuration.size()) {
          return {};
        }
        desc = {
            .type = FidlTrippoint::wire::TripPointType::kOneshotTempAbove,
            .index = index_val,
            .configuration = FidlTrippoint::wire::TripPointValue::WithOneshotTempAboveTripPoint(
                FidlTrippoint::wire::OneshotTempAboveTripPoint(config_val)),
        };
      }
    } else if (type == kTripTypeBelow) {
      if (configuration == kTripConfigCleared) {
        desc = {
            .type = FidlTrippoint::wire::TripPointType::kOneshotTempBelow,
            .index = index_val,
            .configuration = FidlTrippoint::wire::TripPointValue::WithClearedTripPoint(
                FidlTrippoint::wire::ClearedTripPoint()),
        };
      } else {
        float config_val;
        auto [p2, ec2] = std::from_chars(configuration.data(),
                                         configuration.data() + configuration.size(), config_val);
        if (ec2 != std::errc() || p2 != configuration.data() + configuration.size()) {
          return {};
        }
        desc = {
            .type = FidlTrippoint::wire::TripPointType::kOneshotTempBelow,
            .index = index_val,
            .configuration = FidlTrippoint::wire::TripPointValue::WithOneshotTempBelowTripPoint(
                FidlTrippoint::wire::OneshotTempBelowTripPoint(config_val)),
        };
      }
    } else {
      return {};
    }

    descriptors.emplace_back(desc);
  }
  return descriptors;
}

struct SharedClients {
  std::optional<fidl::WireSyncClient<FidlTemperature::Device>> temperature;
  std::optional<fidl::WireSyncClient<FidlAdc::Device>> adc;
  std::optional<fidl::WireSyncClient<FidlTrippoint::TripPoint>> trippoint;
  std::optional<fidl::WireSyncClient<FidlTrippoint::Debug>> debug;
};

int HandleTemperatureRead(std::string_view device_path, SharedClients& clients) {
  if (!clients.temperature) {
    auto client_res =
        ConnectToDevice<FidlTemperature::Device>(device_path, ToString(DeviceType::kTemperature));
    if (client_res.is_error()) {
      return -1;
    }
    clients.temperature = std::move(client_res.value());
  }
  auto& client = *clients.temperature;
  auto response = client->GetTemperatureCelsius();
  if (!response.ok()) {
    printf("GetTemperatureCelsius FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  if (response->status) {
    printf("GetTemperatureCelsius failed: status = %d\n", response->status);
    return -1;
  }
  printf("temperature = %f\n", response->temp);
  return 0;
}

int HandleTemperatureName(std::string_view device_path, SharedClients& clients) {
  if (!clients.temperature) {
    auto client_res =
        ConnectToDevice<FidlTemperature::Device>(device_path, ToString(DeviceType::kTemperature));
    if (client_res.is_error()) {
      return -1;
    }
    clients.temperature = std::move(client_res.value());
  }
  auto& client = *clients.temperature;
  auto response = client->GetSensorName();
  if (!response.ok()) {
    printf("GetSensorName FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  printf("Sensor Name = %.*s\n", static_cast<int>(response->name.size()), response->name.data());
  return 0;
}

int HandleAdcResolution(std::string_view device_path, SharedClients& clients) {
  if (!clients.adc) {
    auto client_res = ConnectToDevice<FidlAdc::Device>(device_path, ToString(DeviceType::kAdc));
    if (client_res.is_error()) {
      return -1;
    }
    clients.adc = std::move(client_res.value());
  }
  auto& client = *clients.adc;
  auto response = client->GetResolution();
  if (!response.ok()) {
    printf("GetResolution FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  if (response->is_error()) {
    printf("GetResolution failed: status = %d\n", response->error_value());
    return -1;
  }
  printf("adc resolution = %u\n", response->value()->resolution);
  return 0;
}

int HandleAdcRead(std::string_view device_path, SharedClients& clients) {
  if (!clients.adc) {
    auto client_res = ConnectToDevice<FidlAdc::Device>(device_path, ToString(DeviceType::kAdc));
    if (client_res.is_error()) {
      return -1;
    }
    clients.adc = std::move(client_res.value());
  }
  auto& client = *clients.adc;
  auto response = client->GetSample();
  if (!response.ok()) {
    printf("GetSample FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  if (response->is_error()) {
    printf("GetSample failed: status = %d\n", response->error_value());
    return -1;
  }
  printf("Value = %u\n", response->value()->value);
  return 0;
}

int HandleAdcReadNorm(std::string_view device_path, SharedClients& clients) {
  if (!clients.adc) {
    auto client_res = ConnectToDevice<FidlAdc::Device>(device_path, ToString(DeviceType::kAdc));
    if (client_res.is_error()) {
      return -1;
    }
    clients.adc = std::move(client_res.value());
  }
  auto& client = *clients.adc;
  auto response = client->GetNormalizedSample();
  if (!response.ok()) {
    printf("GetNormalizedSample FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  if (response->is_error()) {
    printf("GetNormalizedSample failed: status = %d\n", response->error_value());
    return -1;
  }
  printf("Value = %f\n", response->value()->value);
  return 0;
}

int HandleTripPoint(std::string_view device_path, const std::vector<std::string_view>& extra_args,
                    SharedClients& clients) {
  if (!clients.trippoint) {
    auto client_res =
        ConnectToDevice<FidlTrippoint::TripPoint>(device_path, ToString(DeviceType::kTrippoint));
    if (client_res.is_error()) {
      return -1;
    }
    clients.trippoint = std::move(client_res.value());
  }
  auto& client = *clients.trippoint;
  if (extra_args.empty()) {
    auto response = client->GetTripPointDescriptors();
    if (!response.ok()) {
      printf("GetTripPointDescriptors FIDL call failed: %s\n", response.status_string());
      return -1;
    }
    if (response->is_error()) {
      printf("GetTripPointDescriptors failed: status = %d\n", response->error_value());
      return -1;
    }
    print_trippoint(response->value()->descriptors);
  } else {
    std::vector descriptors = parse_set_trippoints_args(extra_args);
    if (descriptors.empty()) {
      printf("Invalid trippoints list\n");
      return -1;
    }
    auto fidl_descriptors =
        fidl::VectorView<FidlTrippoint::wire::TripPointDescriptor>::FromExternal(descriptors);
    printf("Setting trippoints:\n");
    print_trippoint(fidl_descriptors);
    auto response = client->SetTripPoints(fidl_descriptors);
    if (!response.ok()) {
      printf("SetTripPoints FIDL call failed: %s\n", response.status_string());
      return -1;
    }
    if (response->is_error()) {
      printf("SetTripPoints failed: status = %d\n", response->error_value());
      return -1;
    }
  }
  return 0;
}

int HandleWait(std::string_view device_path, SharedClients& clients) {
  if (!clients.trippoint) {
    auto client_res =
        ConnectToDevice<FidlTrippoint::TripPoint>(device_path, ToString(DeviceType::kTrippoint));
    if (client_res.is_error()) {
      return -1;
    }
    clients.trippoint = std::move(client_res.value());
  }
  auto& client = *clients.trippoint;
  auto response = client->WaitForAnyTripPoint();
  if (!response.ok()) {
    printf("WaitForAnyTripPoint FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  if (response->is_error()) {
    printf("WaitForAnyTripPoint failed: status = %d\n", response->error_value());
    return -1;
  }
  printf("TripPoint indexed %u was tripped. Measured temperature was %f C\n",
         response->value()->result.index, response->value()->result.measured_temperature_celsius);
  return 0;
}

int HandleTrigger(std::string_view device_path, const std::vector<std::string_view>& extra_args,
                  SharedClients& clients) {
  if (!clients.debug) {
    auto client_res = ConnectToDevice<FidlTrippoint::Debug>(
        device_path, GetDeviceTypeName(DeviceType::kTrippoint, kCmdTrigger));
    if (client_res.is_error()) {
      return -1;
    }
    clients.debug = std::move(client_res.value());
  }
  auto& client = *clients.debug;
  if (extra_args.empty()) {
    printf("%.*s command requires an index argument\n", static_cast<int>(kCmdTrigger.size()),
           kCmdTrigger.data());
    return -1;
  }
  uint32_t index_val;
  std::string_view index_str = extra_args[0];
  auto [p, ec] = std::from_chars(index_str.data(), index_str.data() + index_str.size(), index_val);
  if (ec != std::errc() || p != index_str.data() + index_str.size()) {
    printf("Invalid index '%.*s'. Index must be a non-negative integer.\n",
           static_cast<int>(extra_args[0].size()), extra_args[0].data());
    return -1;
  }
  auto response = client->Trip(index_val);
  if (!response.ok()) {
    printf("Trip FIDL call failed: %s\n", response.status_string());
    return -1;
  }
  return 0;
}

}  // namespace

int main(int argc, char** argv) {
  auto args_res = ParseArgs(argc, argv);
  if (args_res.is_error()) {
    return -1;
  }
  const auto& args = args_res.value();

  if (args.command_blocks.empty()) {
    printf("%s", kUsageMessage);
    return -1;
  }

  if (args.command_blocks[0].command == kCmdHelp) {
    printf("%s", kUsageMessage);
    return 0;
  }

  if (args.command_blocks[0].command == kCmdList) {
    if (!args.command_blocks[0].extra_args.empty()) {
      printf("Command '%.*s' is global; additional arguments will be ignored.\n",
             static_cast<int>(kCmdList.size()), kCmdList.data());
    }
    do_list();
    return 0;
  }

  if (args.command_blocks[0].command == kCmdReadAll) {
    if (!args.command_blocks[0].extra_args.empty()) {
      printf("Command '%.*s' is global; additional arguments will be ignored.\n",
             static_cast<int>(kCmdReadAll.size()), kCmdReadAll.data());
    }
    auto devices = GetTemperatureDevicesForReading();
    if (devices.empty()) {
      printf("No temperature devices found.\n");
      return 0;
    }
    printf("Found %zu temperature devices:\n", devices.size());
    for (const auto& dev : devices) {
      printf("  %-20s (%s)\n", dev.name.c_str(), dev.path.c_str());
    }
    printf("\n");
    int ret = 0;
    for (const auto& dev : devices) {
      printf("Reading %s ...\n", dev.name.c_str());
      SharedClients local_clients;
      if (HandleTemperatureRead(dev.path, local_clients) != 0) {
        ret = -1;
      }
    }
    return ret;
  }

  SharedClients clients;
  std::string locked_device(args.device_path_or_name);

  for (const auto& block : args.command_blocks) {
    auto resolved_res = ResolveDevice(locked_device, block.command);
    if (resolved_res.is_error()) {
      return -1;
    }
    auto resolved = resolved_res.value();

    // For chained commands, lock onto the first resolved device path/name
    // so subsequent commands target the same physical/logical device.
    if (locked_device.empty()) {
      if (!resolved.friendly_name.empty()) {
        locked_device = resolved.friendly_name;
      } else {
        locked_device = resolved.base_path;
      }
    }

    if (!IsCompatible(resolved.type, block.command)) {
      printf("Incompatible device type for command '%.*s'. Expected %s, detected %.*s.\n",
             static_cast<int>(block.command.size()), block.command.data(),
             ExpectedTypeForCommand(block.command).c_str(),
             static_cast<int>(ToString(resolved.type).size()), ToString(resolved.type).data());
      return -1;
    }

    auto cmd_type_opt = ParseCommand(block.command);
    if (!cmd_type_opt) {
      printf("Unknown command: '%.*s'\n", static_cast<int>(block.command.size()),
             block.command.data());
      return -1;
    }

    int res = 0;
    switch (*cmd_type_opt) {
      case CommandType::kRead:
        if (resolved.type == DeviceType::kTemperature) {
          res = HandleTemperatureRead(resolved.path, clients);
        } else {
          res = HandleAdcRead(resolved.path, clients);
        }
        break;
      case CommandType::kName:
        res = HandleTemperatureName(resolved.path, clients);
        break;
      case CommandType::kResolution:
        res = HandleAdcResolution(resolved.path, clients);
        break;
      case CommandType::kReadNorm:
        res = HandleAdcReadNorm(resolved.path, clients);
        break;
      case CommandType::kTripPoint:
        res = HandleTripPoint(resolved.path, block.extra_args, clients);
        break;
      case CommandType::kWait:
        res = HandleWait(resolved.path, clients);
        break;
      case CommandType::kTrigger:
        res = HandleTrigger(resolved.path, block.extra_args, clients);
        break;
    }

    if (res != 0) {
      return res;
    }
  }

  return 0;
}
