// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.radar/cpp/wire.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>

#include <cstdio>

#include "radarutil.h"

int main(int argc, char** argv) {
  // radarutil is a diagnostic tool designed to talk directly to the hardware
  // driver service, bypassing the radar proxy. We use a watcher to dynamically
  // find the first available device instance.
  component::SyncServiceMemberWatcher<fuchsia_hardware_radar::Service::Device> watcher;
  zx::result client = watcher.GetNextInstance(/*stop_at_idle=*/true);
  if (client.is_error()) {
    fprintf(stderr, "Failed to connect to device: %s\n", client.status_string());
    return 1;
  }

  zx_status_t status = radarutil::RadarUtil::Run(argc, argv, std::move(client.value()));
  return status == ZX_OK ? 0 : 1;
}
