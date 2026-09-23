// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_MAC_ADAPTER_H_
#define SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_MAC_ADAPTER_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.net.tun/cpp/wire.h>
#include <fidl/fuchsia.net/cpp/wire.h>

#include <optional>

#include <fbl/mutex.h>

#include "state.h"

namespace network {
namespace tun {

class MacAdapter;

// An abstract MacAdapter parent.
//
// This abstract class allows the owner of a `MacAdapter` to be notified of important events.
class MacAdapterParent {
 public:
  virtual ~MacAdapterParent() = default;

  // Called when there are changes to the internal state of the `adapter`.
  virtual void OnMacStateChanged(MacAdapter* adapter) = 0;
};

// An entity that instantiates a MacAddrDeviceInterface and provides an implementations of
// `fuchsia.hardware.network.device.MacAddr` that grants access to the requested operating state
// of its interface.
//
// `MacAdapter` is used to provide the business logic of virtual MacAddr implementations both for
// `tun.Device` and `tun.DevicePair` device classes.
class MacAdapter : public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  // Creates a new `MacAdapter` with `parent`.
  // `mac` is the device's own MAC address, reported to the MacAddrDeviceInterface.
  // if `promisc_only` is true, the only filtering mode reported to the interface will be
  // `MODE_PROMISCUOUS`.
  // On success, the adapter is stored in `out`.
  static zx::result<std::unique_ptr<MacAdapter>> Create(
      MacAdapterParent* parent, fdf::UnownedUnsynchronizedDispatcher dispatcher,
      fuchsia_net::wire::MacAddress mac, bool promisc_only);
  MacAdapter(MacAdapter&&) = delete;
  ~MacAdapter() override;

  const fuchsia_net::wire::MacAddress& mac() { return mac_; }

  // MacAddr protocol:
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

  MacState GetMacState();

  // Binds a client to this adapter. Should be called at most once.
  fdf::ClientEnd<fuchsia_hardware_network_driver::MacAddr> BindDriver();

 private:
  MacAdapter(MacAdapterParent* parent, fdf::UnownedUnsynchronizedDispatcher dispatcher,
             fuchsia_net::wire::MacAddress mac, bool promisc_only)
      : parent_(parent),
        dispatcher_(std::move(dispatcher)),
        mac_(mac),
        promisc_only_(promisc_only) {}

  fbl::Mutex state_lock_;
  MacAdapterParent* const parent_;  // pointer to parent, not owned.
  fdf::UnownedUnsynchronizedDispatcher dispatcher_;
  fuchsia_net::wire::MacAddress mac_;
  const bool promisc_only_;

  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::MacAddr>> binding_;

  MacState mac_state_ __TA_GUARDED(state_lock_);
};

}  // namespace tun
}  // namespace network

#endif  // SRC_CONNECTIVITY_NETWORK_TUN_NETWORK_TUN_MAC_ADAPTER_H_
