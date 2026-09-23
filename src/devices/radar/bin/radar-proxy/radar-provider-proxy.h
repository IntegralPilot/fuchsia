// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_PROVIDER_PROXY_H_
#define SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_PROVIDER_PROXY_H_

#include <lib/async/dispatcher.h>

#include <tuple>
#include <vector>

#include "radar-proxy.h"

namespace radar {

class RadarProviderProxy
    : public RadarProxy,
      public fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstReaderProvider> {
 public:
  RadarProviderProxy(async_dispatcher_t* dispatcher, RadarDeviceConnector* connector)
      : dispatcher_(dispatcher), connector_(connector) {}
  ~RadarProviderProxy() override;

  void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override;

  void OnDeviceFound(
      fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) override;

  zx::result<> AddProtocols(component::OutgoingDirectory* outgoing) override;

  void on_fidl_error(fidl::UnbindInfo info) override;

 private:
  bool ConnectToRadarDevice(
      fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end);
  void ConnectClient(fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReader> server,
                     ConnectCompleter::Async completer);

  async_dispatcher_t* const dispatcher_;
  RadarDeviceConnector* const connector_;
  fidl::Client<fuchsia_hardware_radar::RadarBurstReaderProvider> radar_client_;
  std::vector<std::tuple<fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReader>,
                         ConnectCompleter::Async>>
      connect_requests_;
};

}  // namespace radar

#endif  // SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_PROVIDER_PROXY_H_
