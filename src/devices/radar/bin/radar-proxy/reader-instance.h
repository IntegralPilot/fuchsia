// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RADAR_BIN_RADAR_PROXY_READER_INSTANCE_H_
#define SRC_DEVICES_RADAR_BIN_RADAR_PROXY_READER_INSTANCE_H_

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/time.h>

#include "src/devices/radar/lib/vmo-manager/vmo-manager.h"

namespace radar {

class ReaderInstance;

class ReaderInstanceManager {
 public:
  // Called by the instance when the number of registered VMOs changes.
  virtual void ResizeVmoPool(size_t vmo_count) = 0;

  // The instance got a request to start receiving bursts.
  virtual void StartBursts() = 0;

  // The instance got a request to stop receiving bursts. If this is the last instance that was
  // receiving them, the instance manager may ask the radar driver to stop as well.
  virtual void RequestStopBursts() = 0;

  // The instance encountered an error and should be deleted.
  virtual void OnInstanceUnbound(ReaderInstance* instance) = 0;

  virtual fuchsia_hardware_radar::RadarBurstReaderGetBurstPropertiesResponse burst_properties()
      const = 0;
};

class ReaderInstance : public fidl::Server<fuchsia_hardware_radar::RadarBurstReader> {
 public:
  ReaderInstance(async_dispatcher_t* dispatcher,
                 fidl::ServerEnd<fuchsia_hardware_radar::RadarBurstReader> server_end,
                 ReaderInstanceManager* parent)
      : parent_(parent),
        server_(fidl::BindServer(dispatcher, std::move(server_end), this,
                                 [](ReaderInstance* instance, auto, auto) {
                                   instance->parent_->OnInstanceUnbound(instance);
                                 })),
        manager_(parent_->burst_properties().size()) {}

  void SendBurst(cpp20::span<const uint8_t> burst, zx::time timestamp);
  void SendError(fuchsia_hardware_radar::StatusCode error);

  // This causes our on_unbound function to be called, which tells the parent to delete us.
  void Close(zx_status_t status) { server_.Close(status); }

  bool bursts_started() const { return bursts_started_; }

 private:
  // fidl::Server<fuchsia_hardware_radar::RadarBurstReader>
  void GetBurstProperties(GetBurstPropertiesCompleter::Sync& completer) override;
  void RegisterVmos(RegisterVmosRequest& request, RegisterVmosCompleter::Sync& completer) override;
  void UnregisterVmos(UnregisterVmosRequest& request,
                      UnregisterVmosCompleter::Sync& completer) override;
  void StartBursts(StartBurstsCompleter::Sync& completer) override;
  void StopBursts(StopBurstsCompleter::Sync& completer) override;
  void UnlockVmo(UnlockVmoRequest& request, UnlockVmoCompleter::Sync& completer) override;

  ReaderInstanceManager* const parent_;
  bool bursts_started_ = false;
  fidl::ServerBindingRef<fuchsia_hardware_radar::RadarBurstReader> server_;
  VmoManager manager_;
  size_t vmo_count_ = 0;
  bool sent_error_ = false;
};

}  // namespace radar

#endif  // SRC_DEVICES_RADAR_BIN_RADAR_PROXY_READER_INSTANCE_H_
