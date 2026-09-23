// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_PORT_ADAPTER_H_
#define SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_PORT_ADAPTER_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>

#include <fbl/mutex.h>

#include "config.h"
#include "mac_adapter.h"
#include "state.h"

namespace network {
namespace tun {

class PortAdapter;

// An abstract PortAdapter parent.
//
// This abstract class allows the owner of a `PortAdapter` to change its behavior and be notified
// of important events.
class PortAdapterParent : public MacAdapterParent {
 public:
  ~PortAdapterParent() override = default;

  // Called when the device's `has_session` state changes.
  virtual void OnHasSessionsChanged(PortAdapter& port) = 0;
  // Called when the port's status changes.
  //
  // `new_status` must be reported to the device containing the port.
  virtual void OnPortStatusChanged(PortAdapter& port, const PortStatus& new_status) = 0;
  // Called when the port is destroyed and completely removed from the device.
  // May destroy the `port`.
  virtual void OnPortDestroyed(PortAdapter& port) = 0;
};

// An adapter for `NetworkPort`.
//
// `PortAdapter` is used to provide the business logic of virtual `NetworkPort` implementations
// both for `tun.Device` and `tun.DevicePair` device classes.
class PortAdapter : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort> {
 public:
  PortAdapter(PortAdapterParent* parent, const BasePortConfig& config,
              std::unique_ptr<MacAdapter> mac, fdf::UnownedUnsynchronizedDispatcher dispatcher);
  PortAdapter(PortAdapter&&) = delete;

  // NetworkPort protocol:
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override;
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override;
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override;
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override;

  // Provides a channel to communicate with adapter.
  fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkPort> BindDriver();

  // Sets this port's emulated `online` status.
  //
  // Returns true if the online status changed.
  bool SetOnline(bool online);
  bool online();
  bool has_sessions();
  uint32_t mtu() const { return config_.mtu; }
  const std::unique_ptr<MacAdapter>& mac() const { return mac_; }
  uint8_t id() const { return config_.port_id; }
  bool rx_checksum_offload() const { return config_.rx_checksum_offload; }

 private:
  std::array<fuchsia_hardware_network::wire::FrameType,
             fuchsia_hardware_network::wire::kMaxFrameTypes>
      rx_types_;
  std::array<fuchsia_hardware_network::wire::FrameTypeSupport,
             fuchsia_hardware_network::wire::kMaxFrameTypes>
      tx_types_;
  // Pointer to parent, not owned.
  PortAdapterParent* const parent_;
  fdf::UnownedUnsynchronizedDispatcher dispatcher_;
  const std::unique_ptr<MacAdapter> mac_;
  const BasePortConfig config_;

  fbl::Mutex state_lock_;
  bool has_sessions_ __TA_GUARDED(state_lock_) = false;
  PortStatus port_status_ __TA_GUARDED(state_lock_);
};

}  // namespace tun
}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_PORT_ADAPTER_H_
