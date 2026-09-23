// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fidl/fuchsia.hardware.powersource/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <filesystem>
#include <iterator>

#include <fbl/algorithm.h>
#include <fbl/string_buffer.h>
#include <fbl/vector.h>

using pwrdev_t = struct {
  fuchsia_hardware_powersource::wire::PowerType type;
  std::string name;
  uint8_t state;
  zx::event events;
  fidl::WireSyncClient<fuchsia_hardware_powersource::Source> fidl_channel;
};

struct arg_data {
  bool debug;
  bool poll_events;
};

constexpr const char* type_to_string[] = {"AC", "battery"};

zx::result<fuchsia_hardware_powersource::wire::SourceInfo> get_source_info(
    const fidl::WireSyncClient<fuchsia_hardware_powersource::Source>& client) {
  const fidl::WireResult result = client->GetPowerInfo();
  if (!result.ok()) {
    fprintf(stderr, "GetPowerInfo failed (transport: %s)\n", result.status_string());
    return zx::error(result.status());
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    fprintf(stderr, "GetPowerInfo failed (operation: %s)\n", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok(response.info);
}

constexpr const char* state_to_string[] = {"online", "discharging", "charging", "critical"};
constexpr const char* state_offline = "offline/not present";

const char* get_state_string(uint32_t state, fbl::StringBuffer<256>* buf) {
  buf->Clear();
  for (size_t i = 0; i < std::size(state_to_string); i++) {
    if (state & (1 << i)) {
      if (buf->length()) {
        buf->Append(", ");
      }
      buf->Append(state_to_string[i]);
    }
  }

  return (buf->length() > 0) ? buf->c_str() : state_offline;
}

zx_status_t get_battery_info(
    const fidl::WireSyncClient<fuchsia_hardware_powersource::Source>& client) {
  const fidl::WireResult result = client->GetBatteryInfo();
  if (!result.ok()) {
    fprintf(stderr, "GetBatteryInfo failed (transport: %s)\n", result.status_string());
    return result.status();
  }
  const fidl::WireResponse response = result.value();
  if (zx_status_t status = response.status; status != ZX_OK) {
    fprintf(stderr, "GetBatteryInfo failed (operation: %s)\n", zx_status_get_string(status));
    return status;
  }
  fuchsia_hardware_powersource::wire::BatteryInfo binfo = response.info;

  const char* unit;
  switch (binfo.unit) {
    case fuchsia_hardware_powersource::wire::BatteryUnit::kMw:
      unit = "mW";
      break;
    case fuchsia_hardware_powersource::wire::BatteryUnit::kMa:
      unit = "mA";
      break;
  }
  printf("             design capacity: %d %s\n", binfo.design_capacity, unit);
  printf("          last full capacity: %d %s\n", binfo.last_full_capacity, unit);
  printf("              design voltage: %d mV\n", binfo.design_voltage);
  printf("            warning capacity: %d %s\n", binfo.capacity_warning, unit);
  printf("                low capacity: %d %s\n", binfo.capacity_low, unit);
  printf("     low/warning granularity: %d %s\n", binfo.capacity_granularity_low_warning, unit);
  printf("    warning/full granularity: %d %s\n", binfo.capacity_granularity_warning_full, unit);
  printf("                present rate: %d %s\n", binfo.present_rate, unit);
  printf("          remaining capacity: %d %s\n", binfo.remaining_capacity, unit);
  printf("             present voltage: %d mV\n", binfo.present_voltage);
  printf("==========================================\n");
  printf("remaining battery percentage: %d %%\n",
         binfo.remaining_capacity * 100 / binfo.last_full_capacity);
  if (binfo.present_rate < 0) {
    printf(
        "      remaining battery life: %.2f h\n",
        static_cast<float>(binfo.remaining_capacity) / static_cast<float>(binfo.present_rate) * -1);
  }
  putchar('\n');
  return ZX_OK;
}

void parse_arguments(int argc, char** argv, struct arg_data* args) {
  int opt;
  while ((opt = getopt(argc, argv, "p")) != -1) {
    switch (opt) {
      case 'p':
        args->poll_events = true;
        break;
      default:
        fprintf(stderr, "Invalid arg: %c\nUsage: %s [-p]\n", opt, argv[0]);
        exit(EXIT_FAILURE);
    }
  }
}

void handle_event(pwrdev_t& interface) {
  zx::result result = get_source_info(interface.fidl_channel);
  if (result.is_error()) {
    exit(EXIT_FAILURE);
  }
  fuchsia_hardware_powersource::wire::SourceInfo info = result.value();

  fbl::StringBuffer<256> old_buf;
  fbl::StringBuffer<256> new_buf;
  printf("%s (%s): state change %s (%#x) -> %s (%#x)\n", interface.name.c_str(),
         type_to_string[fidl::ToUnderlying(interface.type)],
         get_state_string(interface.state, &old_buf), interface.state,
         get_state_string(info.state, &new_buf), info.state);

  if (interface.type == fuchsia_hardware_powersource::wire::PowerType::kBattery &&
      (info.state & fuchsia_hardware_powersource::wire::kPowerStateOnline)) {
    if (get_battery_info(interface.fidl_channel) != ZX_OK) {
      exit(EXIT_FAILURE);
    }
  }

  interface.state = info.state;
}

void poll_events(fbl::Vector<pwrdev_t>& interfaces) {
  zx_wait_item_t* items = new zx_wait_item_t[interfaces.size()];
  for (size_t i = 0; i < interfaces.size(); i++) {
    items[i].handle = interfaces[i].events.get();
    items[i].waitfor = ZX_USER_SIGNAL_0;
    items[i].pending = 0;
  }

  zx_status_t status;
  printf("waiting for events...\n\n");
  for (;;) {
    status = zx_object_wait_many(items, interfaces.size(), ZX_TIME_INFINITE);
    if (status != ZX_OK) {
      printf("zx_object_wait_many() returned %d\n", status);
      exit(EXIT_FAILURE);
    }

    for (size_t i = 0; i < interfaces.size(); i++) {
      if (items[i].pending & ZX_USER_SIGNAL_0) {
        handle_event(interfaces[i]);
      }
    }
  }
}

int main(int argc, char** argv) {
  struct arg_data args = {};
  parse_arguments(argc, argv, &args);

  fbl::StringBuffer<256> state_str;
  fbl::Vector<pwrdev_t> interfaces;
  for (auto const& dir_entry : std::filesystem::directory_iterator{"/dev/class/power"}) {
    zx::result client_end =
        component::Connect<fuchsia_hardware_powersource::Source>(dir_entry.path().c_str());
    if (client_end.is_error()) {
      printf("failed to connect to %s: %s; skipping\n", dir_entry.path().c_str(),
             client_end.status_string());
      continue;
    }
    fidl::WireSyncClient client{std::move(client_end.value())};

    zx::result result = get_source_info(client);
    if (result.is_error()) {
      printf("failed to read source info from %s: %s; skipping\n", dir_entry.path().c_str(),
             result.status_string());
      continue;
    }
    fuchsia_hardware_powersource::wire::SourceInfo pinfo = result.value();

    printf("[%s] type: %s, state: %s (%#x)\n", dir_entry.path().c_str(),
           type_to_string[fidl::ToUnderlying(pinfo.type)],
           get_state_string(pinfo.state, &state_str), pinfo.state);

    if (pinfo.type == fuchsia_hardware_powersource::wire::PowerType::kBattery &&
        (pinfo.state & fuchsia_hardware_powersource::wire::kPowerStateOnline)) {
      if (zx_status_t status = get_battery_info(client); status != ZX_OK) {
        printf("failed to read battery info from %s: %s; skipping\n", dir_entry.path().c_str(),
               zx_status_get_string(status));
        continue;
      }
    }

    if (args.poll_events) {
      fidl::WireResult result = client->GetStateChangeEvent();
      if (!result.ok()) {
        printf("failed to get event from %s: %s; skipping\n", dir_entry.path().c_str(),
               result.status_string());
        continue;
      }
      auto& response = result.value();
      if (zx_status_t status = response.status; status != ZX_OK) {
        printf("failed to get event from %s: %s; skipping\n", dir_entry.path().c_str(),
               zx_status_get_string(status));
        continue;
      }

      interfaces.push_back({
          .type = pinfo.type,
          .name = dir_entry.path().string(),
          .state = pinfo.state,
          .events = std::move(response.handle),
          .fidl_channel = std::move(client),
      });
    }
  }

  if (args.poll_events) {
    poll_events(interfaces);
  }

  return 0;
}
