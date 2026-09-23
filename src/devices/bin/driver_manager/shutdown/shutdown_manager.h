// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_SHUTDOWN_MANAGER_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_SHUTDOWN_MANAGER_H_

#include <fidl/fuchsia.diagnostics/cpp/wire.h>
#include <fidl/fuchsia.process.lifecycle/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmo.h>

#include <list>
#include <vector>

#include "src/devices/bin/driver_manager/shutdown/node_remover.h"

namespace driver_manager {
using fuchsia_system_state::SystemPowerState;

// Theory of operation of ShutdownManager:
//  There are a number of ways shutdown can be initiated:
//   - The process could be terminated, resulting in a signal from the Lifecycle channel
//   - Any of the fidl connections could be dropped
//  These events can cause one of two stages of the driver shutdown to be triggered:
//  Package Shutdown:  The shutdown manager signals the node_remover to shut down all package
//  drivers; ie: drivers that depend on storage and fshost.
//  Boot/All Shutdown:  The shutdown manager signals the node_remover to shut down all drivers.
//
//  When the node_remover signals it has completed removing the package drivers,
//  The Shutdown Manager will transition to kPackageStopped.  If something has signaled the
//  Shutdown Manager to shutdown the boot drivers in that time, the shutdown manager will
//  transition to shutting down boot drivers immediately after the package drivers are removed.
//  Otherwise, the Shutdown Manager will wait for an invocation of SignalBootShutdown before
//  shutting down boot drivers.
//  Either way, when boot drivers are fully shutdown, the Shutdown Manager will signal the
//  system to stop in some manner, dictated by what is returned by `GetSystemPowerState`.
//  The default state, which is invoked if there is some error, is REBOOT.
//  Any errors in the shutdown process are logged, but ulimately do not stop the shutdown.
//  The ShutdownManager is not thread safe. It assumes that all channels will be dispatched
//  on the same single threaded dispatcher, and that all callbacks will also be called on
//  that same thread.
class ShutdownManager : public fidl::WireServer<fuchsia_process_lifecycle::Lifecycle> {
 public:
  enum class State : uint32_t {
    // The system is running, nothing is being stopped.
    kRunning = 0u,
    // The devices whose's drivers live in storage are stopped or in the middle of being
    // stopped.
    kPackageStopping = 1u,
    // Package drivers have been stopped, but we haven't started shutting down boot drivers yet.
    kPackageStopped = 2u,
    // The entire system is in the middle of being stopped.
    kBootStopping = 3u,
    // The entire system is stopped.
    kStopped = 4u,
  };

  // State of reboot leases required for system shutdown.
  enum class LeaseState {
    // Reboot leases have not been requested yet.
    kNotLeased,
    // Reboot leases have been requested but are not yet acquired.
    kRequested,
    // Reboot leases have been successfully acquired.
    kAcquired,
  };

  ShutdownManager(NodeRemover* node_remover, async_dispatcher_t* dispatcher);

  void Publish(component::OutgoingDirectory& outgoing);

  // Called by the node_remover when it finishes removing drivers in storage.
  // Should only be called when in state: kPackageStopping.
  // This function will transition the state to State::kBootStopping.
  void OnPackageShutdownComplete();

  // Called by the node_remover when it finishes removing boot drivers.
  // Should only be called when in state: kBootStopping.
  // This function will transition the state to State::kStopped.
  void OnBootShutdownComplete();

 private:
  // Signal state for when devfs and fshost are shutdown.
  class Lifecycle : public fidl::WireServer<fuchsia_process_lifecycle::Lifecycle> {
   public:
    explicit Lifecycle(fit::callback<void(fit::callback<void(zx_status_t)>)> on_stop)
        : on_stop_(std::move(on_stop)) {}

    void Stop(StopCompleter::Sync& completer) override {
      on_stop_([completer = completer.ToAsync()](zx_status_t status) mutable {
        completer.Close(status);
      });
    }

   private:
    fit::callback<void(fit::callback<void(zx_status_t)>)> on_stop_;
  };

  // All external shutdown signals ultimately call either `SignalBootShutdown` or
  // `SignalPackageShutdown`. These two functions interact with the `ShutdownManager` state
  // machine and signal the node_remover to remove nodes.
  //  SignalPackageShutdown interacts with the `ShutdownManager` state machine thusly:
  //  State:           |      Action
  //  ---------------------------------------------
  //  kRunning:        |  Transition to kPackageStopping.
  //                   |  Signal the nove_remover to remove package drivers.
  //                   |  Add callback to list to be called when all package drivers are removed
  //  kPackageStopping |  Add callback to list to be called when all package drivers are removed
  //  All other states |  Immediately call callback
  void SignalPackageShutdown(fit::callback<void(zx_status_t)> cb);
  //  When the shutdown manager receives the SignalBootShutdown:
  //  State:           |      Action
  //  ---------------------------------------------
  //  kRunning or      |  Transition to kBootStopping.
  //   kPackageStopped |  Signal the nove_remover to remove all drivers.
  //                   |  Add callback to list to be called when all drivers are removed
  //  kPackageStopping |  Add callback to list to be called when all drivers are removed
  //                   |  Set flag so that when the packages are fully removed, we will
  //                   |  continue to remove the boot drivers
  //  kBootStopping    |  Add callback to list to be called when all drivers are removed
  //  All other states |  Immediately call callback
  void SignalBootShutdown(fit::callback<void(zx_status_t)> cb);
  void AcquireShutdownLeases(fit::callback<void()> callback);

  // fuchsia.process.lifecycle/Lifecycle interface
  // The process must clean up its state in preparation for termination, and
  // must close the channel hosting the `Lifecycle` protocol when it is
  // ready to be terminated. The process should exit after it completes its
  // cleanup. At the discretion of the system the process may be terminated
  // before it closes the `Lifecycle` channel.
  void Stop(StopCompleter::Sync& completer) override;

  // Execute the shutdown strategy set in shutdown_system_state_.
  // This should be done after all attempts at shutting down drivers has been made.
  void SystemExecute();

  // Called when one of our connections is dropped.
  void OnUnbound(const char* connection, fidl::UnbindInfo info);

  // The driver runner should always be valid while the shutdown manager exists.
  // TODO(https://fxbug.dev/42065671): ensure that this pointer is valid
  NodeRemover* node_remover_;

  // Tracks when the devfs component is stopped by component manager. We shutdown all drivers upon
  // receiving this signal.
  Lifecycle devfs_lifecycle_;
  // Tracks when the devfs-with-pkg component is stopped by component manager. We shutdown all
  // packaged drivers upon receiving this signal.
  Lifecycle devfs_with_pkg_lifecycle_;

  fidl::ServerBindingGroup<fuchsia_process_lifecycle::Lifecycle> lifecycle_bindings_;
  std::list<fit::callback<void(zx_status_t)>> package_shutdown_complete_callbacks_;
  std::list<fit::callback<void(zx_status_t)>> boot_shutdown_complete_callbacks_;

  State shutdown_state_ = State::kRunning;
  // After package shutdown completes, wait for separate boot shutdown signal
  bool received_boot_shutdown_signal_ = false;
  LeaseState lease_state_ = LeaseState::kNotLeased;
  std::vector<fit::callback<void()>> shutdown_lease_acquired_callbacks_;

  async_dispatcher_t* dispatcher_;
  zx::resource mexec_resource_, power_resource_;
  fidl::ClientEnd<fuchsia_diagnostics::LogFlusher> log_flush_;
  // Tracks if we received a stop signal from the fuchsia_process_lifecycle::Lifecycle channel.
  bool lifecycle_stop_ = false;
};

}  // namespace driver_manager
#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_SHUTDOWN_SHUTDOWN_MANAGER_H_
