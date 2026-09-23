// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "radar-provider-proxy.h"
#include "radar-proxy.h"
#include "radar-reader-proxy.h"

namespace radar {

class DefaultRadarDeviceConnector : public RadarDeviceConnector {
 public:
  void ConnectToFirstRadarDevice(ConnectDeviceCallback connect_device) override {
    component::SyncServiceMemberWatcher<fuchsia_hardware_radar::Service::Device> watcher;
    while (true) {
      zx::result client_end = watcher.GetNextInstance(/*stop_at_idle = */ true);
      if (client_end.is_error()) {
        break;
      }
      if (connect_device(std::move(client_end.value()))) {
        break;
      }
    }
  }
};

}  // namespace radar

int main(int argc, const char** argv) {
  radar::DefaultRadarDeviceConnector connector;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  std::unique_ptr<radar::RadarProxy> proxy;

  proxy = radar::RadarProxy::Create(loop.dispatcher(), &connector);
  if (!proxy) {
    return 1;
  }

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());

  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return 1;
  }

  result = proxy->AddProtocols(&outgoing);
  if (result.is_error()) {
    return 1;
  }

  return loop.Run();
}
