// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_READER_PROXY_H_
#define SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_READER_PROXY_H_

#include <lib/fzl/vmo-mapper.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>

#include <memory>
#include <optional>
#include <tuple>
#include <vector>

#include "burst-injector.h"
#include "radar-proxy.h"
#include "reader-instance.h"

namespace radar {

class RadarReaderProxy : public RadarProxy,
                         public ReaderInstanceManager,
                         public BurstInjectorManager,
                         public fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstReader> {
 public:
  RadarReaderProxy(async_dispatcher_t* dispatcher, RadarDeviceConnector* connector)
      : dispatcher_(dispatcher), connector_(connector) {}
  ~RadarReaderProxy() override;

  // RadarProxy
  void Connect(ConnectRequest& request, ConnectCompleter::Sync& completer) override;
  void OnDeviceFound(
      fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end) override;
  zx::result<> AddProtocols(component::OutgoingDirectory* outgoing) override;

  // ReaderInstanceManager
  void ResizeVmoPool(size_t vmo_count) override;
  void StartBursts() override;
  void RequestStopBursts() override;
  void OnInstanceUnbound(ReaderInstance* instance) override;

  // ReaderInstanceManager/BurstInjectorManager
  fuchsia_hardware_radar::RadarBurstReaderGetBurstPropertiesResponse burst_properties()
      const override;

  // BurstInjectorManager
  zx::time StartBurstInjection() override;
  void StopBurstInjection() override;
  void SendBurst(cpp20::span<const uint8_t> burst, zx::time timestamp) override;
  void OnInjectorUnbound(BurstInjector* injector) override;

  // fidl::AsyncEventHandler<fuchsia_hardware_radar::RadarBurstReader>
  void on_fidl_error(fidl::UnbindInfo info) override;

  void OnBurst(fidl::Event<fuchsia_hardware_radar::RadarBurstReader::OnBurst>& event) override;

 private:
  struct MappedVmo {
    fzl::VmoMapper mapped_vmo;
    zx::vmo vmo;

    const uint8_t* start() const { return reinterpret_cast<uint8_t*>(mapped_vmo.start()); }
  };

  void BindInjector(fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstInjector> server_end);

  bool ValidateDevice(fidl::ClientEnd<fuchsia_hardware_radar::RadarBurstReaderProvider> client_end);

  // Handles an unexpected fatal error that may have gotten us out of sync with the driver. Closes
  // our driver client end, sends an epitaph to all connected clients, and attempts to connect to
  // any other drivers that may be present.
  void HandleFatalError(zx_status_t status);

  void RequestStartBursts();
  void StopBursts();

  async_dispatcher_t* const dispatcher_;
  RadarDeviceConnector* const connector_;
  fidl::Client<fuchsia_hardware_radar::RadarBurstReader> radar_client_;
  std::optional<fuchsia_hardware_radar::RadarBurstReaderGetBurstPropertiesResponse>
      burst_properties_;
  std::vector<MappedVmo> vmo_pool_;
  std::vector<std::unique_ptr<ReaderInstance>> instances_;
  std::vector<std::tuple<fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReader>,
                         ConnectCompleter::Async>>
      connect_requests_;
  zx::time last_burst_timestamp_{};
  bool inject_bursts_ = false;
  std::optional<BurstInjector> injector_;
  fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstInjector> pending_injector_client_;
};

}  // namespace radar

#endif  // SRC_DEVICES_RADAR_BIN_RADAR_PROXY_RADAR_READER_PROXY_H_
