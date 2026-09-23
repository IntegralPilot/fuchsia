// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_NETWORK_PORT_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_NETWORK_PORT_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.network.driver/cpp/fidl.h>
#include <fidl/fuchsia.hardware.network/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/span.h>
#include <lib/sync/cpp/completion.h>
#include <stdint.h>
#include <zircon/compiler.h>

#include <mutex>
#include <optional>

namespace wlan::drivers::components {

// A network port corresponds to a network interface. A network device can provide multiple network
// ports. Such as a NIC with multiple RJ45 ports or in this case a WLAN device supporting multiple
// interfaces using a single device.
// The user should instantiate an object of this class and provide an implementation of
// NetworkPort::Callbacks to handle the various calls that are made to it. An implementation can
// choose to either inherit from NetworkPort (and optionally NetworkPort::Callbacks at the same
// time) or create a standalone object.
// A network port is created and registered with the network device when Init is called. The port
// is removed when the destructor of this class is called, i.e. when the object is destroyed.
class NetworkPort : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkPort>,
                    public fdf::WireServer<fuchsia_hardware_network_driver::MacAddr> {
 public:
  class Callbacks {
   public:
    virtual ~Callbacks();

    // Called when the device needs to retrieve the port status. This information is already filled
    // out by NetworkPort so implementing this method is optional but could be useful if the driver
    // wants to provide more specific or customized information.
    virtual void PortGetStatus(fuchsia_hardware_network::PortStatus* out_status) {}

    // Called when a port is being removed from the network device. This is an optional method for
    // drivers that want to take extra action when this happens. This will NOT be called when
    // calling NetworkPort::RemovePort. It will only be called on unexpected port removals.
    virtual void PortRemoved() {}

    // Called when the network device needs to know the MTU of the port.
    virtual uint32_t PortGetMtu() = 0;

    // Called when the network device needs to know the MAC address of the port.
    virtual void MacGetAddress(fuchsia_net::MacAddress* out_mac) = 0;

    // Called when the network device needs to know about the features supported by the port. See
    // the Features type for the various options.
    virtual void MacGetFeatures(fuchsia_hardware_network_driver::Features* out_features) = 0;

    // Called when the network device needs to set one of the supported MAC filtering modes from the
    // features call. See features_t for the different modes. When multicast filtering is enabled
    // then the driver should only accept unicast frames and multicast frames from the MAC addresses
    // specified in multicast_macs.
    virtual void MacSetMode(fuchsia_hardware_network::wire::MacFilterMode mode,
                            cpp20::span<const ::fuchsia_net::wire::MacAddress> multicast_macs) = 0;
  };

  enum class Role { Client, Ap };

  NetworkPort(fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>&& netdev_ifc,
              Callbacks& iface, uint8_t port_id);
  virtual ~NetworkPort();

  // Must be called to make the port operational. If this fails the port object is no longer valid
  // and no further operations on the port are valid. Initialization is asynchronous and the result
  // will be reported through |on_complete|. The port is not valid for use until |on_complete| has
  // been called. In case of an error |on_complete| may (but is not guaranteed to) be called inline
  // from this function, be careful about acquiring locks during error handling in |on_complete| if
  // those locks are already held when calling Init. On success |on_complete| is always called
  // asynchronously. The |dispatcher| must stay alive and running until the port is removed, either
  // through |RemovePort|, object destruction, or |Callbacks::PortRemoved| being called. Otherwise
  // removal of the port could deadlock.
  void Init(Role role, fdf_dispatcher_t* dispatcher,
            fit::callback<void(zx_status_t)>&& on_complete);
  // Remove the port. |on_complete| will be used to report the result asynchronously. After calling
  // RemovePort no further operations on the port are valid. If the port was successfully
  // initialized by a call to Init it must be removed by a call to RemovePort. If Init has been
  // called it's not safe to destroy the port until RemovePort has been called and completion has
  // been signaled via a call to |on_complete|. It's safe to destroy the port even if |on_complete|
  // reports an error.
  void RemovePort(fit::callback<void(zx_status_t)>&& on_complete) __TA_EXCLUDES(mutex_);

  void SetPortOnline(bool online) __TA_EXCLUDES(mutex_);
  bool IsOnline() const __TA_EXCLUDES(mutex_);
  uint8_t PortId() const { return port_id_; }

  // NetworkPortProtocol implementation
  void GetInfo(fdf::Arena& arena, GetInfoCompleter::Sync& completer) override;
  void GetStatus(fdf::Arena& arena, GetStatusCompleter::Sync& completer) override;
  void SetActive(fuchsia_hardware_network_driver::wire::NetworkPortSetActiveRequest* request,
                 fdf::Arena& arena, SetActiveCompleter::Sync& completer) override;
  void GetMac(fdf::Arena& arena, GetMacCompleter::Sync& completer) override;
  void Removed(fdf::Arena& arena, RemovedCompleter::Sync& completer) override;

  // MacAddrProtocol implementation
  void GetAddress(fdf::Arena& arena, GetAddressCompleter::Sync& completer) override;
  void GetFeatures(fdf::Arena& arena, GetFeaturesCompleter::Sync& completer) override;
  void SetMode(fuchsia_hardware_network_driver::wire::MacAddrSetModeRequest* request,
               fdf::Arena& arena, SetModeCompleter::Sync& completer) override;

 private:
  void RemovePortLocked(fit::callback<void(zx_status_t)>&& on_complete) __TA_REQUIRES(mutex_)
      __TA_RELEASE(mutex_);
  void GetPortStatusLocked(fuchsia_hardware_network::PortStatus* out_status) __TA_REQUIRES(mutex_);

  Role role_;
  Callbacks& iface_;
  mutable std::mutex mutex_;
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc> netdev_ifc_;
  const uint8_t port_id_;
  bool online_ __TA_GUARDED(mutex_) = false;
  fit::callback<void(zx_status_t)> on_removed_ __TA_GUARDED(mutex_);
  fdf_dispatcher_t* dispatcher_ = nullptr;
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::NetworkPort>> port_binding_
      __TA_GUARDED(mutex_);
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::MacAddr>> mac_binding_
      __TA_GUARDED(mutex_);
};

}  // namespace wlan::drivers::components

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_INCLUDE_WLAN_DRIVERS_COMPONENTS_NETWORK_PORT_H_
