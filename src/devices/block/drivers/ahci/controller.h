// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_AHCI_CONTROLLER_H_
#define SRC_DEVICES_BLOCK_DRIVERS_AHCI_CONTROLLER_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zx/time.h>
#include <zircon/types.h>

#include <memory>
#include <mutex>

#include <fbl/condition_variable.h>

#include "ahci.h"
#include "bus.h"
#include "port.h"

namespace ahci {

class Controller : public fdf::DriverBase2 {
 public:
  static constexpr char kDriverName[] = "ahci";

  Controller() : fdf::DriverBase2(kDriverName) {}

  ~Controller() = default;

  DISALLOW_COPY_ASSIGN_AND_MOVE(Controller);

  zx::result<> Start(fdf::DriverContext context) override;

  void Stop(fdf::StopCompleter completer) __TA_EXCLUDES(lock_) override;

  virtual zx::result<std::unique_ptr<Bus>> CreateBus();

  // Read or write a 32-bit AHCI controller reg. Endinaness is corrected.
  uint32_t RegRead(size_t offset);
  zx_status_t RegWrite(size_t offset, uint32_t val);

  // Create irq and worker dispatchers.
  zx_status_t LaunchIrqAndWorkerDispatchers();

  // Release all resources.
  void Shutdown() __TA_EXCLUDES(lock_);

  zx_status_t HbaReset();
  void AhciEnable();

  zx_status_t SetDevInfo(uint32_t portnr, SataDeviceInfo* devinfo);
  void Queue(uint32_t portnr, SataTransaction* txn);

  void SignalWorker() { worker_event_completion_.Signal(); }

  inspect::Inspector& inspect() { return component_inspector_->inspector(); }
  inspect::Node& inspect_node() { return inspect_node_; }

  Bus* bus() { return bus_.get(); }
  Port* port(uint32_t portnr) { return &ports_[portnr]; }
  std::vector<std::unique_ptr<SataDevice>>& sata_devices() { return sata_devices_; }

  // Called by children device of this controller for invoking AddChild() or instantiating
  // compat::DeviceServer.
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& root_node() { return root_node_; }
  std::string_view driver_name() const { return name(); }
  const std::shared_ptr<fdf::Namespace>& driver_incoming() const { return incoming_; }
  std::shared_ptr<fdf::OutgoingDirectory>& driver_outgoing() { return outgoing(); }
  const std::optional<std::string>& driver_node_name() const { return node_name_; }

  zx::event node_token() const {
    zx::event copy;
    if (node_token_.is_valid()) {
      node_token_.duplicate(ZX_RIGHT_SAME_RIGHTS, &copy);
    }
    return copy;
  }

 private:
  void WorkerLoop();
  void IrqLoop();

  // Initialize controller and detect devices.
  zx_status_t Init();

  bool ShouldExit() __TA_EXCLUDES(lock_);

  std::shared_ptr<fdf::Namespace> incoming_;
  std::optional<inspect::ComponentInspector> component_inspector_;
  std::optional<std::string> node_name_;
  zx::event node_token_;
  inspect::Node inspect_node_;

  std::mutex lock_;
  bool shutdown_ __TA_GUARDED(lock_) = false;

  // Dispatcher for handling interrupt requests.
  fdf::Dispatcher irq_dispatcher_;
  // Signaled when irq_dispatcher_ is shut down.
  libsync::Completion irq_shutdown_completion_;

  // Dispatcher for processing queued block requests.
  fdf::Dispatcher worker_dispatcher_;
  // True when worker_dispatcher_ is shut down.
  std::atomic_bool worker_shutdown_;
  // Signaled when there is work to be done in the worker loop.
  libsync::Completion worker_event_completion_;

  std::unique_ptr<Bus> bus_;
  Port ports_[AHCI_MAX_PORTS];
  std::vector<std::unique_ptr<SataDevice>> sata_devices_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> parent_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::Node> root_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> node_controller_;
};

}  // namespace ahci

#endif  // SRC_DEVICES_BLOCK_DRIVERS_AHCI_CONTROLLER_H_
