// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_DEVICE_PORT_H_
#define SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_DEVICE_PORT_H_

#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/optional.h>
#include <lib/zx/event.h>

#include <fbl/mutex.h>

#include "src/connectivity/network/drivers/network-device/mac/public/network_mac.h"
#include "status_watcher.h"

namespace network::internal {

class DeviceInterface;

class DevicePort : public fidl::WireServer<netdev::Port> {
 public:
  using TeardownCallback = fit::callback<void(DevicePort&)>;
  using OnCreated = fit::callback<void(zx::result<std::unique_ptr<DevicePort>>)>;

  // Asynchronously create a DevicePort object. This will make calls on the port_client to determine
  // the initial state of the port. Once the port object is ready it will be provided through the
  // on_created callback. If an error occurs it will also be reported through on_created. Note that
  // on an error the callback may be (but is not guaranteed to be) called inline from the Create
  // call. Because of this it's a good idea to avoid acquiring locks in the on_created callback when
  // an error is reported.
  static void Create(
      DeviceInterface* parent, async_dispatcher_t* dispatcher, netdev::wire::PortId id,
      fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>&& port_client,
      fdf_dispatcher_t* mac_dispatcher, TeardownCallback&& on_teardown, OnCreated&& on_created);

  ~DevicePort() override;

  netdev::wire::PortId id() const { return id_; }

  struct Counters {
    std::atomic<uint64_t> rx_frames = 0;
    std::atomic<uint64_t> rx_bytes = 0;
    std::atomic<uint64_t> tx_frames = 0;
    std::atomic<uint64_t> tx_bytes = 0;
  };

  // Notifies port of status changes notifications from the network device implementation.
  void StatusChanged(const netdev::wire::PortStatus& new_status);
  // Starts port teardown process.
  //
  // Once port is torn down and ready to be deleted, the teardown callback passed on construction
  // will be called.
  // Calling teardown while a teardown is already in progress is a no-op.
  void Teardown();
  // Notifies the port a session attached to it.
  //
  // When the session attaches to a port, the port will notify the network port implementation that
  // the port is active.
  void SessionAttached();
  // Notifies the port a session detached from it.
  //
  // When the session is detached from a port, the port will notify the network port
  // implementation that the port is inactive.
  void SessionDetached();

  // Binds a new FIDL request to this port.
  void Bind(fidl::ServerEnd<netdev::Port> req);

  // Returns true if `frame_type` is a valid inbound frame type for subscription to on this port.
  bool IsValidRxFrameType(netdev::wire::FrameType frame_type) const;
  // Returns true if `frame_type` is a valid outbound frame type for this port.
  bool IsValidTxFrameType(netdev::wire::FrameType frame_type) const;

  // FIDL protocol implementation.
  void GetInfo(GetInfoCompleter::Sync& completer) override;
  void GetStatus(GetStatusCompleter::Sync& completer) override;
  void GetStatusWatcher(GetStatusWatcherRequestView request,
                        GetStatusWatcherCompleter::Sync& _completer) override;
  void GetMac(GetMacRequestView request, GetMacCompleter::Sync& _completer) override;
  void GetDevice(GetDeviceRequestView request, GetDeviceCompleter::Sync& _completer) override;
  void Clone(CloneRequestView request, CloneCompleter::Sync& _completer) override;
  void GetCounters(GetCountersCompleter::Sync& completer) override;
  void GetDiagnostics(GetDiagnosticsRequestView request,
                      GetDiagnosticsCompleter::Sync& _completer) override;
  void GetIdentity(GetIdentityCompleter::Sync& completer) override;

  Counters& counters() { return counters_; }

 private:
  // Helper class to keep track of clients bound to DevicePort.
  class Binding : public fbl::DoublyLinkedListable<std::unique_ptr<Binding>> {
   public:
    Binding() = default;
    void Unbind() {
      if (binding_.has_value()) {
        binding_->Unbind();
      }
    }
    void Bind(fidl::ServerBindingRef<netdev::Port> binding) { binding_ = std::move(binding); }

   private:
    std::optional<fidl::ServerBindingRef<netdev::Port>> binding_;
  };

  using BindingList = fbl::DoublyLinkedList<std::unique_ptr<Binding>>;

  DevicePort(DeviceInterface* parent, async_dispatcher_t* dispatcher, netdev::wire::PortId id,
             fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>&& port,
             TeardownCallback&& on_teardown);

  void Init(fdf_dispatcher_t* mac_dispatcher, fit::callback<void(zx_status_t)>&& on_complete);
  void GetMac(fit::callback<void(zx::result<::fdf::ClientEnd<netdriver::MacAddr>>)>&& on_complete);
  void CreateMacInterface(::fdf::ClientEnd<netdriver::MacAddr>&& client_end,
                          fdf_dispatcher_t* mac_dispatcher,
                          fit::callback<void(zx_status_t)>&& on_complete);
  void GetInitialPortInfo(fit::callback<void(zx_status_t)>&& on_complete);
  void GetInitialStatus(fit::callback<void(zx_status_t)>&& on_complete);

  // Concludes an ongoing teardown if it is ongoing and all internal resources are released.
  //
  // Returns true if teardown callback was dispatched.
  // Callers must assume the port is destroyed immediately if this function returns true.
  bool MaybeFinishTeardown() __TA_REQUIRES(lock_);
  // Applies the current port status to the network port implementation.
  void ApplyPortStatus() __TA_REQUIRES(lock_);

  DeviceInterface* const parent_;  // Pointer to parent device. Not owned.
  async_dispatcher_t* const dispatcher_;
  const netdev::wire::PortId id_;
  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort> port_;
  Counters counters_;
  std::shared_ptr<MacAddrDeviceInterface> mac_ __TA_GUARDED(lock_);
  BindingList bindings_ __TA_GUARDED(lock_);
  zx::event id_event_;

  netdev::wire::PortClass port_class_;
  netdev::PortStatus status_;
  std::vector<netdev::wire::FrameType> supported_rx_;
  std::vector<netdev::wire::FrameTypeSupport> supported_tx_;

  fbl::Mutex lock_;
  StatusWatcherList watchers_ __TA_GUARDED(lock_);
  TeardownCallback on_teardown_ __TA_GUARDED(lock_);
  bool teardown_started_ __TA_GUARDED(lock_) = false;
  bool has_session_attached_ __TA_GUARDED(lock_) = false;

  DISALLOW_COPY_ASSIGN_AND_MOVE(DevicePort);
};

}  // namespace network::internal

#endif  // SRC_CONNECTIVITY_NETWORK_DRIVERS_NETWORK_DEVICE_DEVICE_DEVICE_PORT_H_
