// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>

#include <filesystem>

namespace {

void usage(char* argv[]) {
  printf("Usage: %s [--read|--off|<brightness-val>]\n", argv[0]);
  printf("options:\n    <brightness-val>: 0.0-1.0\n");
}

namespace FidlBacklight = fuchsia_hardware_backlight;

}  // namespace

int main(int argc, char* argv[]) {
  if (argc != 2) {
    usage(argv);
    return -1;
  }

  component::SyncServiceMemberWatcher<FidlBacklight::Service::Backlight> watcher;
  zx::result client_end = watcher.GetNextInstance(true);
  if (client_end.is_error()) {
    printf("Failed to open backlight: %s\n", client_end.status_string());
    return -1;
  }
  fidl::WireSyncClient<FidlBacklight::Device> client(std::move(client_end.value()));
  if (strcmp(argv[1], "--read") == 0) {
    const fidl::WireResult result = client->GetStateNormalized();
    if (!result.ok()) {
      printf("GetStateNormalized transport failed with %s\n", result.status_string());
      return -1;
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      printf("GetStateNormalized call failed with %s\n",
             zx_status_get_string(response.error_value()));
      return -1;
    }
    const FidlBacklight::wire::State& state = response.value()->state;
    printf("Backlight:%s Brightness:%f\n", state.backlight_on ? "on" : "off", state.brightness);
    return 0;
  }

  bool on;
  double brightness;
  if (strcmp(argv[1], "--off") == 0) {
    on = false;
    brightness = 0.0;
  } else {
    char* endptr;
    brightness = strtod(argv[1], &endptr);
    if (endptr == argv[1] || *endptr != '\0') {
      usage(argv);
      return -1;
    }
    if (brightness < 0.0 || brightness > 1.0) {
      printf("Invalid brightness %f\n", brightness);
      return -1;
    }
    on = true;
  }

  const fidl::WireResult result = client->SetStateNormalized({
      .backlight_on = on,
      .brightness = brightness,
  });
  if (!result.ok()) {
    printf("SetStateNormalized transport failed with %s\n", result.status_string());
    return -1;
  }
  const fit::result response = result.value();
  if (response.is_error()) {
    printf("SetStateNormalized call failed with %s\n",
           zx_status_get_string(response.error_value()));
    return -1;
  }

  return 0;
}
