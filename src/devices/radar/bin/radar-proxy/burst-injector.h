// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_RADAR_BIN_RADAR_PROXY_BURST_INJECTOR_H_
#define SRC_DEVICES_RADAR_BIN_RADAR_PROXY_BURST_INJECTOR_H_

#include <fidl/fuchsia.hardware.radar/cpp/fidl.h>
#include <lib/async/dispatcher.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/time.h>

#include <list>
#include <optional>

namespace radar {
namespace FidlRadar = fuchsia_hardware_radar;

class BurstInjectorManager;

class BurstInjector : public fidl::Server<FidlRadar::RadarBurstInjector> {
 public:
  BurstInjector(async_dispatcher_t* dispatcher,
                fidl::ServerEnd<FidlRadar::RadarBurstInjector> server_end,
                BurstInjectorManager* parent);
  ~BurstInjector() override;

 private:
  struct MappedInjectorVmo {
    uint32_t id;
    fzl::VmoMapper mapped_vmo;
    cpp20::span<const uint8_t> bursts;
  };

  // fidl::Server<FidlRadar::RadarBurstInjector>
  void GetBurstProperties(GetBurstPropertiesCompleter::Sync& completer) override;
  void EnqueueBursts(EnqueueBurstsRequest& request,
                     EnqueueBurstsCompleter::Sync& completer) override;
  void StartBurstInjection(StartBurstInjectionCompleter::Sync& completer) override;
  void StopBurstInjection(StopBurstInjectionCompleter::Sync& completer) override;

  void FinishBurstInjection(StopBurstInjectionCompleter::Async completer);
  void OnBurstInjectionTimerExpire();
  void ScheduleNextBurstInjection();

  async_dispatcher_t* const dispatcher_;
  BurstInjectorManager* const parent_;
  zx::time last_burst_timestamp_{};
  bool inject_bursts_ = false;
  std::list<MappedInjectorVmo> injector_vmo_queue_;
  fidl::ServerBindingRef<FidlRadar::RadarBurstInjector> server_;
  bool schedule_next_burst_ = false;
  std::optional<StopBurstInjectionCompleter::Async> stop_burst_injection_completer_;
};

class BurstInjectorManager {
 public:
  // Tells the manager to ignore future bursts from the radar driver. Returns the timestamp of the
  // last burst from the driver.
  virtual zx::time StartBurstInjection() = 0;

  // Tells the manager to resume forwarding bursts from the radar driver.
  virtual void StopBurstInjection() = 0;

  // Sends the injected burst to all radar clients.
  virtual void SendBurst(cpp20::span<const uint8_t> burst, zx::time timestamp) = 0;

  // The injector client disconnected, and if started, injection should be stopped.
  virtual void OnInjectorUnbound(BurstInjector* injector) = 0;

  virtual FidlRadar::RadarBurstReaderGetBurstPropertiesResponse burst_properties() const = 0;
};

}  // namespace radar

#endif  // SRC_DEVICES_RADAR_BIN_RADAR_PROXY_BURST_INJECTOR_H_
