// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_PROXY_H_
#define SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_PROXY_H_

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>

#include <memory>
#include <string>

namespace radar {

class RadarDeviceConnector {
 public:
  // Called on zero or more eligible radar devices. If true is returned, the device is usable and
  // the callback should not be invoked again. Otherwise, the device is not usable, and the search
  // for a suitable device should continue.
  using ConnectDeviceCallback =
      fit::function<bool(fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider>)>;

  // Calls the connect_device callback on available devices until it returns true.
  virtual void ConnectToFirstRadarDevice(ConnectDeviceCallback connect_device) = 0;
};

class RadarProxy : public fidl::Server<fuchsia_hardware_radar::RadarBurstReaderProvider> {
 public:
  static std::unique_ptr<RadarProxy> Create(async_dispatcher_t* dispatcher,
                                            RadarDeviceConnector* connector);

  RadarProxy() = default;

  zx::result<> Init(async_dispatcher_t* dispatcher) {
    return service_watcher_.Begin(
        dispatcher,
        [this](fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) {
          OnDeviceFound(std::move(client_end));
        });
  }

  // Called by a ServiceMemberWatcher when fuchsia.hardware.radar.Service has a new instance.
  virtual void OnDeviceFound(
      fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) = 0;

  virtual zx::result<> AddProtocols(component::OutgoingDirectory* outgoing) = 0;

 private:
  component::ServiceMemberWatcher<fuchsia_hardware_radar::Service::Device> service_watcher_;
};

}  // namespace radar

#endif  // SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_PROXY_H_
